//! Minimal, device-matched wrapper around the Wave's stock `libnnsdk.so`.
//!
//! The ABI layouts in this module are intentionally narrow. They are based on
//! the public Amlogic headers and cross-checked against the exact firmware
//! binaries shipped on the connected Wave. See the implementation plan for
//! the offsets recovered from the stock `libasr_reverb.so` caller.

use libloading::Library;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_void};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::mem;
use std::path::Path;
use std::ptr;
use std::slice;
use std::time::{Duration, Instant};

pub const MODEL_PATH: &str = "/data/waved/lib/add_graph.nb";
pub const STOCK_MODEL_PATH: &str = "/model/asr_de/de_online/ctc_online.nb";
pub const ITERATIONS: usize = 10;
pub const STOCK_ITERATIONS: usize = 3;
pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const EXIT_TIMEOUT: u8 = 21;
pub const EXIT_WORKER_DISCONNECTED: u8 = 22;

const LIBNNSDK_PATH: &str = "/usr/lib/libnnsdk.so";
const MODEL_SHA256: &str = "9bef0c0f2271ad534cb975b386be8428e973dc7ee4c71c04854c95eeac1fea5c";
const STOCK_MODEL_SHA256: &str = "5d2795be781af80593321cf0ae627f56f4f6f64e71e95d1d8b0a27f6192a9f66";
const AML_CONFIG_SIZE: usize = 2272;
const AML_OUTPUT_CONFIG_SIZE: usize = 56;
const MAX_TENSOR_DIMENSIONS: usize = 6;

const MODEL_TYPE_ONNX: c_int = 4;
const NBG_TYPE_MEMORY: c_int = 1;
const INPUT_TYPE_BINARY_RAW: c_int = 3;
const OUTPUT_FORMAT_RAW: c_int = 1;
const PERF_MODE_RUN: c_int = 1;
const PERF_MODE_OUTPUT_GET: c_int = 2;

#[derive(Clone, Copy, Debug)]
enum ErrorStage {
    Driver,
    ModelRead,
    Checksum,
    RuntimeLoad,
    Symbol,
    Create,
    Input,
    Execution,
    Output,
    Verification,
    Cleanup,
}

impl ErrorStage {
    fn exit_code(self) -> u8 {
        match self {
            Self::Driver => 10,
            Self::ModelRead => 11,
            Self::Checksum => 12,
            Self::RuntimeLoad => 13,
            Self::Symbol => 14,
            Self::Create => 15,
            Self::Input => 16,
            Self::Execution => 17,
            Self::Output => 18,
            Self::Verification => 19,
            Self::Cleanup => 20,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Driver => "driver",
            Self::ModelRead => "model-read",
            Self::Checksum => "checksum",
            Self::RuntimeLoad => "runtime-load",
            Self::Symbol => "symbol-resolution",
            Self::Create => "graph-create",
            Self::Input => "input-submit",
            Self::Execution => "graph-execute",
            Self::Output => "output-retrieve",
            Self::Verification => "output-verify",
            Self::Cleanup => "cleanup",
        }
    }
}

#[derive(Debug)]
pub struct ProbeError {
    stage: ErrorStage,
    detail: String,
}

impl ProbeError {
    fn new(stage: ErrorStage, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
        }
    }

    pub fn exit_code(&self) -> u8 {
        self.stage.exit_code()
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stage.label(), self.detail)
    }
}

#[derive(Debug)]
pub struct RunReport {
    pub latencies: Vec<Duration>,
    pub final_output: [u8; 2],
    pub cleanup_status: c_int,
}

#[derive(Debug)]
pub struct StockRunReport {
    pub latencies: Vec<Duration>,
    pub output_sha256: String,
    pub output_bytes: usize,
    pub cleanup_status: c_int,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NbgHeader {
    version: u32,
    product: u32,
}

impl StockRunReport {
    pub fn average_latency(&self) -> Duration {
        let total: Duration = self.latencies.iter().copied().sum();
        total / self.latencies.len() as u32
    }
}

impl RunReport {
    pub fn average_latency(&self) -> Duration {
        let total: Duration = self.latencies.iter().copied().sum();
        total / self.latencies.len() as u32
    }
}

#[repr(C)]
struct AmlConfig {
    type_size: c_int,
    length: c_int,
    path: *const c_char,
    data: *const c_char,
    model_type: c_int,
    nbg_type: c_int,
    reserved: [u8; AML_CONFIG_SIZE - 32],
}

impl AmlConfig {
    fn nbg_memory(model: &[u8]) -> Result<Self, ProbeError> {
        let length = c_int::try_from(model.len()).map_err(|_| {
            ProbeError::new(
                ErrorStage::Create,
                format!(
                    "model is too large for NN SDK length field: {}",
                    model.len()
                ),
            )
        })?;

        Ok(Self {
            type_size: AML_CONFIG_SIZE as c_int,
            length,
            path: ptr::null(),
            data: model.as_ptr().cast(),
            model_type: MODEL_TYPE_ONNX,
            nbg_type: NBG_TYPE_MEMORY,
            reserved: [0; AML_CONFIG_SIZE - 32],
        })
    }
}

#[repr(C)]
#[derive(Default)]
struct InputInfo {
    valid: c_int,
    int16_type: c_int,
    preprocess_debug: c_int,
    mean: [f32; 3],
    scale: f32,
    input_format: c_int,
}

#[repr(C)]
struct NnInput {
    type_size: c_int,
    input_index: c_int,
    size: c_int,
    padding: c_int,
    input: *mut u8,
    input_type: c_int,
    info: InputInfo,
}

impl NnInput {
    fn binary(index: usize, data: &mut [u8]) -> Result<Self, ProbeError> {
        let input_index = c_int::try_from(index).map_err(|_| {
            ProbeError::new(
                ErrorStage::Input,
                format!("input index does not fit C int: {index}"),
            )
        })?;
        let size = c_int::try_from(data.len()).map_err(|_| {
            ProbeError::new(
                ErrorStage::Input,
                format!("input size does not fit C int: {}", data.len()),
            )
        })?;

        Ok(Self {
            // The firmware's own caller leaves this compatibility field zero.
            type_size: 0,
            input_index,
            size,
            padding: 0,
            input: data.as_mut_ptr(),
            input_type: INPUT_TYPE_BINARY_RAW,
            info: InputInfo::default(),
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AmlOutputConfig {
    type_size: c_int,
    module_type: c_int,
    perf_mode: c_int,
    format: c_int,
    order: c_int,
    reserved: [u8; AML_OUTPUT_CONFIG_SIZE - 20],
}

impl AmlOutputConfig {
    fn raw(perf_mode: c_int) -> Self {
        Self {
            type_size: AML_OUTPUT_CONFIG_SIZE as c_int,
            // The stock ASR caller also leaves this field at zero.
            module_type: 0,
            perf_mode,
            format: OUTPUT_FORMAT_RAW,
            order: 0,
            reserved: [0; AML_OUTPUT_CONFIG_SIZE - 20],
        }
    }
}

#[repr(C)]
struct NnBufferParams {
    num_of_dims: u32,
    sizes: [u32; 4],
    data_format: c_int,
    quant_format: c_int,
    quant_data: [u8; 8],
}

#[repr(C)]
struct OutBuffer {
    size: u32,
    name: [c_char; 64],
    buffer: *mut u8,
    params: *mut NnBufferParams,
    output_format: c_int,
}

#[repr(C)]
struct NnOutput {
    type_size: c_int,
    num: u32,
    outputs: [OutBuffer; 32],
}

#[repr(C)]
struct TensorInfoEntry {
    dimension_count: u32,
    dimensions: [u32; MAX_TENSOR_DIMENSIONS],
    data_format: u32,
    data_type: u32,
    quantization_format: u32,
    fixed_point_position: c_int,
    tf_scale: f32,
    tf_zero_point: c_int,
    name: [c_char; 64],
}

#[repr(C)]
struct TensorInfoList {
    valid: u32,
    count: u32,
    entries: *mut TensorInfoEntry,
}

type GetTensorInfoFn = unsafe extern "C" fn(*const c_char) -> *mut TensorInfoList;
type FreeTensorInfoFn = unsafe extern "C" fn(*mut TensorInfoList) -> c_int;
type ModuleCreateFn = unsafe extern "C" fn(*mut AmlConfig) -> *mut c_void;
type ModuleInputSetFn = unsafe extern "C" fn(*mut c_void, *mut NnInput) -> c_int;
type ModuleOutputGetFn = unsafe extern "C" fn(*mut c_void, AmlOutputConfig) -> *mut NnOutput;
type ModuleDestroyFn = unsafe extern "C" fn(*mut c_void) -> c_int;

struct Runtime {
    _library: Library,
    create: ModuleCreateFn,
    input_set: ModuleInputSetFn,
    output_get: ModuleOutputGetFn,
    destroy: ModuleDestroyFn,
    get_input_tensor_info: GetTensorInfoFn,
    get_output_tensor_info: GetTensorInfoFn,
    free_tensor_info: FreeTensorInfoFn,
}

impl Runtime {
    fn load() -> Result<Self, ProbeError> {
        // SAFETY: loading the device's fixed, absolute system-library path.
        let library = unsafe { Library::new(LIBNNSDK_PATH) }.map_err(|error| {
            ProbeError::new(
                ErrorStage::RuntimeLoad,
                format!("dlopen {LIBNNSDK_PATH}: {error}"),
            )
        })?;
        println!("    OK: dlopen {LIBNNSDK_PATH}");

        // SAFETY: each type is verified against both the headers and the exact
        // firmware-matched call sites documented in the implementation plan.
        let create = unsafe { resolve(&library, b"aml_module_create\0") }?;
        let input_set = unsafe { resolve(&library, b"aml_module_input_set\0") }?;
        let output_get = unsafe { resolve(&library, b"aml_module_output_get\0") }?;
        let destroy = unsafe { resolve(&library, b"aml_module_destroy\0") }?;
        let get_input_tensor_info = unsafe { resolve(&library, b"aml_util_getInputTensorInfo\0") }?;
        let get_output_tensor_info =
            unsafe { resolve(&library, b"aml_util_getOutputTensorInfo\0") }?;
        let free_tensor_info = unsafe { resolve(&library, b"aml_util_freeTensorInfo\0") }?;

        Ok(Self {
            _library: library,
            create,
            input_set,
            output_get,
            destroy,
            get_input_tensor_info,
            get_output_tensor_info,
            free_tensor_info,
        })
    }

    fn input_metadata(&self, model: &[u8]) -> Result<Vec<ModelTensor>, ProbeError> {
        self.tensor_metadata(model, self.get_input_tensor_info, "input")
    }

    fn output_metadata(&self, model: &[u8]) -> Result<Vec<ModelTensor>, ProbeError> {
        self.tensor_metadata(model, self.get_output_tensor_info, "output")
    }

    fn tensor_metadata(
        &self,
        model: &[u8],
        getter: GetTensorInfoFn,
        kind: &str,
    ) -> Result<Vec<ModelTensor>, ProbeError> {
        // SAFETY: the model starts with a checked NBG header and remains live
        // while the synchronous metadata parser reads it.
        let list = unsafe { getter(model.as_ptr().cast()) };
        let result = unsafe { copy_tensor_metadata(list, kind) };
        if !list.is_null() {
            // SAFETY: this pointer was allocated by the paired libnnsdk
            // metadata getter and has not previously been released.
            let status = unsafe { (self.free_tensor_info)(list) };
            if status != 0 {
                return Err(ProbeError::new(
                    ErrorStage::Cleanup,
                    format!("free {kind} tensor metadata returned {status}"),
                ));
            }
        }
        result
    }
}

unsafe fn resolve<T: Copy>(library: &Library, name: &'static [u8]) -> Result<T, ProbeError> {
    let printable = String::from_utf8_lossy(&name[..name.len().saturating_sub(1)]);
    // SAFETY: the caller supplies the ABI-verified function-pointer type.
    let symbol = unsafe { library.get::<T>(name) }
        .map_err(|error| ProbeError::new(ErrorStage::Symbol, format!("{printable}: {error}")))?;
    println!("    OK: resolved {printable}");
    Ok(*symbol)
}

struct Graph {
    runtime: Runtime,
    context: *mut c_void,
    _model: Vec<u8>,
}

impl Graph {
    fn create(runtime: Runtime, model: Vec<u8>) -> Result<Self, ProbeError> {
        let mut config = AmlConfig::nbg_memory(&model)?;
        // SAFETY: the model remains alive in the returned Graph, the config
        // layout is firmware-verified, and create is an exact resolved symbol.
        let context = unsafe { (runtime.create)(&mut config) };
        if context.is_null() {
            return Err(ProbeError::new(
                ErrorStage::Create,
                "aml_module_create returned NULL",
            ));
        }

        println!("    OK: graph context = {context:p}");
        Ok(Self {
            runtime,
            context,
            _model: model,
        })
    }

    fn set_input(&mut self, index: usize, data: &mut [u8]) -> Result<(), ProbeError> {
        let mut input = NnInput::binary(index, data)?;
        // SAFETY: context is live; input and its backing slice remain valid
        // for the duration of the synchronous input_set call.
        let status = unsafe { (self.runtime.input_set)(self.context, &mut input) };
        if status != 0 {
            return Err(ProbeError::new(
                ErrorStage::Input,
                format!("input {index}: aml_module_input_set returned {status}"),
            ));
        }
        let preview_len = data.len().min(8);
        println!(
            "    OK: input {index} submitted ({} bytes, prefix {:?})",
            data.len(),
            &data[..preview_len]
        );
        Ok(())
    }

    fn execute(&self) -> Result<Duration, ProbeError> {
        if self.context.is_null() {
            return Err(ProbeError::new(
                ErrorStage::Execution,
                "graph context is NULL",
            ));
        }

        let config = AmlOutputConfig::raw(PERF_MODE_RUN);
        let started = Instant::now();
        // SAFETY: context is live and the 56-byte aggregate layout and
        // by-value AAPCS64 calling convention are firmware-verified.
        let _ignored = unsafe { (self.runtime.output_get)(self.context, config) };
        Ok(started.elapsed())
    }

    fn output(&self) -> Result<OutputObservation, ProbeError> {
        let config = AmlOutputConfig::raw(PERF_MODE_OUTPUT_GET);
        // SAFETY: context is live and config has the verified layout.
        let output = unsafe { (self.runtime.output_get)(self.context, config) };
        // SAFETY: the returned pointer is owned by libnnsdk and documented to
        // remain valid until the graph is destroyed or the next output call.
        unsafe { read_output(output) }
    }

    fn raw_output(&self) -> Result<RawOutputObservation, ProbeError> {
        let config = AmlOutputConfig::raw(PERF_MODE_OUTPUT_GET);
        // SAFETY: context is live and config has the verified layout.
        let output = unsafe { (self.runtime.output_get)(self.context, config) };
        // SAFETY: the returned descriptor remains valid until the next output
        // operation or graph destruction and is copied before either occurs.
        unsafe { read_raw_output(output) }
    }

    fn destroy(mut self) -> Result<c_int, ProbeError> {
        let context = mem::replace(&mut self.context, ptr::null_mut());
        // SAFETY: context was created by the paired runtime and has not yet
        // been destroyed.
        let status = unsafe { (self.runtime.destroy)(context) };
        if status != 0 {
            return Err(ProbeError::new(
                ErrorStage::Cleanup,
                format!("aml_module_destroy returned {status}"),
            ));
        }
        Ok(status)
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        if self.context.is_null() {
            return;
        }

        // SAFETY: best-effort cleanup for an otherwise-live paired context.
        let status = unsafe { (self.runtime.destroy)(self.context) };
        self.context = ptr::null_mut();
        if status != 0 {
            eprintln!("    cleanup fallback: aml_module_destroy returned {status}");
        }
    }
}

#[derive(Debug)]
struct TensorMetadata {
    dimensions: Vec<u32>,
    data_format: c_int,
    quant_format: c_int,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModelTensor {
    name: String,
    dimensions: Vec<u32>,
    data_format: u32,
    data_type: u32,
    quantization_format: u32,
}

impl ModelTensor {
    fn byte_len(&self) -> Result<usize, ProbeError> {
        tensor_byte_len(&self.dimensions, self.data_format).map_err(|detail| {
            ProbeError::new(
                ErrorStage::Input,
                format!("input tensor {:?}: {detail}", self.name),
            )
        })
    }
}

#[derive(Debug)]
struct OutputObservation {
    values: [u8; 2],
    output_count: u32,
    output_size: usize,
    output_format: c_int,
    metadata: Option<TensorMetadata>,
}

#[derive(Debug)]
struct RawOutputObservation {
    output_count: u32,
    total_bytes: usize,
    sha256: String,
}

unsafe fn read_output(output: *mut NnOutput) -> Result<OutputObservation, ProbeError> {
    // SAFETY: caller passes the direct return value from libnnsdk.
    let output = unsafe { output.as_ref() }.ok_or_else(|| {
        ProbeError::new(ErrorStage::Output, "aml_module_output_get returned NULL")
    })?;

    if output.num == 0 {
        return Err(ProbeError::new(
            ErrorStage::Output,
            "runtime reported zero output tensors",
        ));
    }

    let first = &output.outputs[0];
    let size = first.size as usize;
    let data = if first.buffer.is_null() {
        None
    } else {
        // SAFETY: libnnsdk reports `size` bytes for this non-null buffer.
        Some(unsafe { slice::from_raw_parts(first.buffer.cast_const(), size) })
    };
    let values = verify_add_output(output.num, size, data)
        .map_err(|detail| ProbeError::new(ErrorStage::Verification, detail))?;

    let metadata = if first.params.is_null() {
        None
    } else {
        // SAFETY: params belongs to the same live output descriptor.
        let params = unsafe { &*first.params };
        let dimension_count = usize::try_from(params.num_of_dims)
            .unwrap_or(usize::MAX)
            .min(params.sizes.len());
        Some(TensorMetadata {
            dimensions: params.sizes[..dimension_count].to_vec(),
            data_format: params.data_format,
            quant_format: params.quant_format,
        })
    };

    Ok(OutputObservation {
        values,
        output_count: output.num,
        output_size: size,
        output_format: first.output_format,
        metadata,
    })
}

unsafe fn read_raw_output(output: *mut NnOutput) -> Result<RawOutputObservation, ProbeError> {
    // SAFETY: caller passes the direct return value from libnnsdk.
    let output = unsafe { output.as_ref() }.ok_or_else(|| {
        ProbeError::new(ErrorStage::Output, "aml_module_output_get returned NULL")
    })?;
    if output.num == 0 || output.num > output.outputs.len() as u32 {
        return Err(ProbeError::new(
            ErrorStage::Output,
            format!("runtime reported invalid output count {}", output.num),
        ));
    }

    let mut digest = Sha256::new();
    let mut total_bytes = 0_usize;
    let mut any_nonzero = false;
    for (index, descriptor) in output.outputs[..output.num as usize].iter().enumerate() {
        let size = descriptor.size as usize;
        if size == 0 {
            return Err(ProbeError::new(
                ErrorStage::Output,
                format!("output {index} has zero bytes"),
            ));
        }
        if descriptor.buffer.is_null() {
            return Err(ProbeError::new(
                ErrorStage::Output,
                format!("output {index} buffer is NULL"),
            ));
        }
        // SAFETY: libnnsdk reports `size` bytes for this non-null buffer.
        let bytes = unsafe { slice::from_raw_parts(descriptor.buffer.cast_const(), size) };
        any_nonzero |= bytes.iter().any(|byte| *byte != 0);
        digest.update(bytes);
        total_bytes = total_bytes.checked_add(size).ok_or_else(|| {
            ProbeError::new(ErrorStage::Output, "total output size overflowed usize")
        })?;
    }
    if !any_nonzero {
        return Err(ProbeError::new(
            ErrorStage::Verification,
            "all retrieved output bytes are zero",
        ));
    }

    Ok(RawOutputObservation {
        output_count: output.num,
        total_bytes,
        sha256: format!("{:x}", digest.finalize()),
    })
}

unsafe fn copy_tensor_metadata(
    list: *mut TensorInfoList,
    kind: &str,
) -> Result<Vec<ModelTensor>, ProbeError> {
    // SAFETY: caller passes the direct return value from the corresponding
    // libnnsdk metadata getter.
    let list = unsafe { list.as_ref() }.ok_or_else(|| {
        ProbeError::new(
            ErrorStage::ModelRead,
            format!("{kind} tensor metadata getter returned NULL"),
        )
    })?;
    if list.valid == 0 {
        return Err(ProbeError::new(
            ErrorStage::ModelRead,
            format!("{kind} tensor metadata is marked invalid"),
        ));
    }
    if list.count == 0 || list.count > 16 {
        return Err(ProbeError::new(
            ErrorStage::ModelRead,
            format!("invalid {kind} tensor count {}", list.count),
        ));
    }
    if list.entries.is_null() {
        return Err(ProbeError::new(
            ErrorStage::ModelRead,
            format!("{kind} tensor metadata entries are NULL"),
        ));
    }

    // SAFETY: the validated list reports `count` contiguous entries.
    let entries = unsafe { slice::from_raw_parts(list.entries, list.count as usize) };
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let dimension_count = usize::try_from(entry.dimension_count).unwrap_or(usize::MAX);
            if dimension_count == 0 || dimension_count > entry.dimensions.len() {
                return Err(ProbeError::new(
                    ErrorStage::ModelRead,
                    format!(
                        "{kind} tensor {index} has invalid dimension count {}",
                        entry.dimension_count
                    ),
                ));
            }
            let name_bytes: Vec<u8> = entry
                .name
                .iter()
                .copied()
                .take_while(|byte| *byte != 0)
                .map(|byte| byte as u8)
                .collect();
            Ok(ModelTensor {
                name: String::from_utf8_lossy(&name_bytes).into_owned(),
                dimensions: entry.dimensions[..dimension_count].to_vec(),
                data_format: entry.data_format,
                data_type: entry.data_type,
                quantization_format: entry.quantization_format,
            })
        })
        .collect()
}

fn tensor_byte_len(dimensions: &[u32], data_format: u32) -> Result<usize, String> {
    let element_bytes: usize = match data_format {
        0 => 4,     // FP32
        1 => 2,     // FP16
        2 | 3 => 1, // UINT8 / INT8
        4 | 5 => 2, // UINT16 / INT16
        other => return Err(format!("unsupported data format {other}")),
    };
    dimensions
        .iter()
        .try_fold(element_bytes, |bytes, dimension| {
            let dimension = usize::try_from(*dimension)
                .map_err(|_| format!("dimension {dimension} does not fit usize"))?;
            if dimension == 0 {
                return Err("zero-sized dimension".to_owned());
            }
            bytes
                .checked_mul(dimension)
                .ok_or_else(|| "tensor byte size overflowed usize".to_owned())
        })
}

fn parse_nbg_header(model: &[u8]) -> Result<NbgHeader, ProbeError> {
    if model.len() < 12 {
        return Err(ProbeError::new(
            ErrorStage::Checksum,
            format!("NBG header is truncated: {} bytes", model.len()),
        ));
    }
    if !model.starts_with(b"VPMN") {
        return Err(ProbeError::new(
            ErrorStage::Checksum,
            "model does not start with VPMN",
        ));
    }

    Ok(NbgHeader {
        version: u32::from_le_bytes(model[4..8].try_into().expect("four-byte version slice")),
        product: u32::from_le_bytes(model[8..12].try_into().expect("four-byte product slice")),
    })
}

fn verify_add_output(
    output_count: u32,
    output_size: usize,
    data: Option<&[u8]>,
) -> Result<[u8; 2], String> {
    if output_count != 1 {
        return Err(format!(
            "expected exactly one output tensor, runtime reported {output_count}"
        ));
    }
    if output_size < 2 {
        return Err(format!(
            "expected at least two output bytes, runtime reported {output_size}"
        ));
    }
    let data = data.ok_or_else(|| "output buffer is NULL".to_owned())?;
    if data.len() < 2 {
        return Err(format!(
            "output slice is shorter than two bytes: {}",
            data.len()
        ));
    }

    // A local sentinel ensures a missing copy cannot accidentally look valid.
    let mut observed = [0xa5, 0xa5];
    observed.copy_from_slice(&data[..2]);
    if observed != [4, 6] {
        return Err(format!(
            "wrong add output: got {observed:?}, expected [4, 6]"
        ));
    }
    Ok(observed)
}

pub fn run_add_poc() -> Result<RunReport, ProbeError> {
    println!("waved-npu-probe --run-add — execute a compiled graph on the Wave NPU");
    println!();

    boundary(1, "open /dev/galcore O_RDWR");
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/galcore")
        .map_err(|error| {
            ProbeError::new(
                ErrorStage::Driver,
                format!("/dev/galcore open failed: {error}"),
            )
        })?;
    println!("    OK: kernel driver reachable");

    boundary(2, "read NPU status (read-only)");
    print_sysfs_value("/sys/module/galcore/parameters/nanoqFreq", "nanoqFreq");
    print_sysfs_value("/sys/module/galcore/parameters/enableNN", "enableNN");

    boundary(3, &format!("read and validate {MODEL_PATH}"));
    let model = fs::read(MODEL_PATH).map_err(|error| {
        ProbeError::new(ErrorStage::ModelRead, format!("read {MODEL_PATH}: {error}"))
    })?;
    let header = parse_nbg_header(&model)?;
    let digest = format!("{:x}", Sha256::digest(&model));
    if digest != MODEL_SHA256 {
        return Err(ProbeError::new(
            ErrorStage::Checksum,
            format!("got {digest}, expected {MODEL_SHA256}"),
        ));
    }
    println!(
        "    OK: {} bytes, nbg_version=0x{:08x}, target=0x{:08x}, sha256 {}",
        model.len(),
        header.version,
        header.product,
        MODEL_SHA256
    );

    boundary(4, "load libnnsdk.so and resolve ABI-verified symbols");
    let runtime = Runtime::load()?;

    boundary(5, "create graph from NBG memory");
    let mut graph = Graph::create(runtime, model)?;

    boundary(6, "submit two UINT8[2] inputs");
    let mut left = [1_u8, 2];
    let mut right = [3_u8, 4];
    graph.set_input(0, &mut left)?;
    graph.set_input(1, &mut right)?;

    boundary(7, &format!("execute and verify {ITERATIONS} iterations"));
    let mut latencies = Vec::with_capacity(ITERATIONS);
    let mut final_output = [0xa5, 0xa5];
    for iteration in 1..=ITERATIONS {
        let elapsed = graph.execute()?;
        let observation = graph.output()?;
        if iteration == 1 {
            println!(
                "    metadata: outputs={}, bytes={}, format={}, tensor={:?}",
                observation.output_count,
                observation.output_size,
                observation.output_format,
                observation.metadata
            );
            if let Some(metadata) = &observation.metadata {
                println!(
                    "    tensor detail: dims={:?}, data_format={}, quant_format={}",
                    metadata.dimensions, metadata.data_format, metadata.quant_format
                );
            }
        }
        final_output = observation.values;
        println!(
            "    OK: iteration {iteration:02}/{ITERATIONS}: {:?} in {:.3} ms",
            observation.values,
            elapsed.as_secs_f64() * 1000.0
        );
        latencies.push(elapsed);
    }

    boundary(8, "destroy graph and release runtime resources");
    let cleanup_status = graph.destroy()?;
    println!("    OK: aml_module_destroy status={cleanup_status}");

    Ok(RunReport {
        latencies,
        final_output,
        cleanup_status,
    })
}

pub fn run_stock_ctc_poc() -> Result<StockRunReport, ProbeError> {
    println!("waved-npu-probe --run-stock-ctc — execute a firmware-matched graph on the Wave NPU");
    println!();

    boundary(1, "open /dev/galcore O_RDWR");
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/galcore")
        .map_err(|error| {
            ProbeError::new(
                ErrorStage::Driver,
                format!("/dev/galcore open failed: {error}"),
            )
        })?;
    println!("    OK: kernel driver reachable");

    boundary(2, "read NPU status (read-only)");
    print_sysfs_value("/sys/module/galcore/parameters/nanoqFreq", "nanoqFreq");
    print_sysfs_value("/sys/module/galcore/parameters/enableNN", "enableNN");

    boundary(3, &format!("read and validate {STOCK_MODEL_PATH}"));
    let model = fs::read(STOCK_MODEL_PATH).map_err(|error| {
        ProbeError::new(
            ErrorStage::ModelRead,
            format!("read {STOCK_MODEL_PATH}: {error}"),
        )
    })?;
    let header = parse_nbg_header(&model)?;
    if header.product != 0x1000000e {
        return Err(ProbeError::new(
            ErrorStage::Checksum,
            format!("unexpected stock model target 0x{:08x}", header.product),
        ));
    }
    let digest = format!("{:x}", Sha256::digest(&model));
    if digest != STOCK_MODEL_SHA256 {
        return Err(ProbeError::new(
            ErrorStage::Checksum,
            format!("got {digest}, expected {STOCK_MODEL_SHA256}"),
        ));
    }
    println!(
        "    OK: {} bytes, nbg_version=0x{:08x}, target=0x{:08x}, sha256 {}",
        model.len(),
        header.version,
        header.product,
        STOCK_MODEL_SHA256
    );

    boundary(4, "load libnnsdk.so and query NBG tensor metadata");
    let runtime = Runtime::load()?;
    let inputs = runtime.input_metadata(&model)?;
    let outputs = runtime.output_metadata(&model)?;
    println!("    inputs: {inputs:?}");
    println!("    outputs: {outputs:?}");
    let expected_output_bytes = outputs.iter().try_fold(0_usize, |total, tensor| {
        let tensor_bytes =
            tensor_byte_len(&tensor.dimensions, tensor.data_format).map_err(|detail| {
                ProbeError::new(
                    ErrorStage::ModelRead,
                    format!("output tensor {:?}: {detail}", tensor.name),
                )
            })?;
        total.checked_add(tensor_bytes).ok_or_else(|| {
            ProbeError::new(
                ErrorStage::ModelRead,
                "expected output byte size overflowed usize",
            )
        })
    })?;

    boundary(5, "create graph from firmware NBG memory");
    let mut graph = Graph::create(runtime, model)?;

    boundary(6, "submit deterministic zero-filled inputs");
    let mut input_buffers = inputs
        .iter()
        .map(|tensor| {
            let byte_len = tensor.byte_len()?;
            if byte_len > 64 * 1024 * 1024 {
                return Err(ProbeError::new(
                    ErrorStage::Input,
                    format!("refusing unexpectedly large input buffer of {byte_len} bytes"),
                ));
            }
            Ok(vec![0_u8; byte_len])
        })
        .collect::<Result<Vec<_>, ProbeError>>()?;
    for (index, buffer) in input_buffers.iter_mut().enumerate() {
        graph.set_input(index, buffer)?;
    }

    boundary(
        7,
        &format!("execute and hash {STOCK_ITERATIONS} deterministic iterations"),
    );
    let mut latencies = Vec::with_capacity(STOCK_ITERATIONS);
    let mut expected_digest: Option<String> = None;
    let mut output_bytes = 0_usize;
    for iteration in 1..=STOCK_ITERATIONS {
        let elapsed = graph.execute()?;
        let observation = graph.raw_output()?;
        if observation.output_count as usize != outputs.len()
            || observation.total_bytes != expected_output_bytes
        {
            return Err(ProbeError::new(
                ErrorStage::Verification,
                format!(
                    "output shape mismatch: runtime count/bytes={}/{}, metadata count/bytes={}/{}",
                    observation.output_count,
                    observation.total_bytes,
                    outputs.len(),
                    expected_output_bytes
                ),
            ));
        }
        if let Some(expected) = &expected_digest {
            if observation.sha256 != *expected {
                return Err(ProbeError::new(
                    ErrorStage::Verification,
                    format!(
                        "output digest changed: got {}, expected {}",
                        observation.sha256, expected
                    ),
                ));
            }
        } else {
            expected_digest = Some(observation.sha256.clone());
            output_bytes = observation.total_bytes;
        }
        println!(
            "    OK: iteration {iteration:02}/{STOCK_ITERATIONS}: outputs={}, bytes={}, sha256={}, {:.3} ms",
            observation.output_count,
            observation.total_bytes,
            observation.sha256,
            elapsed.as_secs_f64() * 1000.0
        );
        latencies.push(elapsed);
    }

    boundary(8, "destroy graph and release runtime resources");
    let cleanup_status = graph.destroy()?;
    println!("    OK: aml_module_destroy status={cleanup_status}");

    Ok(StockRunReport {
        latencies,
        output_sha256: expected_digest.unwrap_or_default(),
        output_bytes,
        cleanup_status,
    })
}

fn boundary(number: u32, message: &str) {
    println!("[npu:{number}] {message}");
}

fn print_sysfs_value(path: &str, label: &str) {
    match fs::read_to_string(path) {
        Ok(value) => println!("    {label}={}", value.trim()),
        Err(error) => println!("    note: {label} unavailable: {error}"),
    }
}

pub fn print_galcore_holders() {
    let holders = galcore_holders();
    if holders.is_empty() {
        eprintln!("    galcore holders: none discoverable");
        return;
    }

    eprintln!("    galcore holders:");
    for (pid, command) in holders {
        eprintln!("      pid={pid} comm={command}");
    }
}

fn galcore_holders() -> BTreeMap<u32, String> {
    let mut holders = BTreeMap::new();
    let Ok(processes) = fs::read_dir("/proc") else {
        return holders;
    };

    for process in processes.flatten() {
        let name = process.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };

        let fd_dir = process.path().join("fd");
        let Ok(fds) = fs::read_dir(fd_dir) else {
            continue;
        };
        let holds_galcore = fds.flatten().any(|fd| {
            fs::read_link(fd.path())
                .map(|target| target == Path::new("/dev/galcore"))
                .unwrap_or(false)
        });
        if !holds_galcore {
            continue;
        }

        let command = fs::read_to_string(process.path().join("comm"))
            .map(|value| value.trim().to_owned())
            .unwrap_or_else(|_| "<unknown>".to_owned());
        holders.insert(pid, command);
    }
    holders
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn aml_config_layout_matches_firmware_caller() {
        assert_eq!(size_of::<AmlConfig>(), 2272);
        assert_eq!(offset_of!(AmlConfig, type_size), 0);
        assert_eq!(offset_of!(AmlConfig, length), 4);
        assert_eq!(offset_of!(AmlConfig, path), 8);
        assert_eq!(offset_of!(AmlConfig, data), 16);
        assert_eq!(offset_of!(AmlConfig, model_type), 24);
        assert_eq!(offset_of!(AmlConfig, nbg_type), 28);
    }

    #[test]
    fn nn_input_layout_matches_firmware_caller() {
        assert_eq!(size_of::<InputInfo>(), 32);
        assert_eq!(size_of::<NnInput>(), 64);
        assert_eq!(offset_of!(NnInput, input_index), 4);
        assert_eq!(offset_of!(NnInput, size), 8);
        assert_eq!(offset_of!(NnInput, input), 16);
        assert_eq!(offset_of!(NnInput, input_type), 24);
        assert_eq!(offset_of!(NnInput, info), 28);
    }

    #[test]
    fn output_layout_matches_firmware_caller() {
        assert_eq!(size_of::<AmlOutputConfig>(), 56);
        assert_eq!(offset_of!(AmlOutputConfig, perf_mode), 8);
        assert_eq!(offset_of!(AmlOutputConfig, format), 12);
        assert_eq!(size_of::<OutBuffer>(), 96);
        assert_eq!(offset_of!(OutBuffer, size), 0);
        assert_eq!(offset_of!(OutBuffer, name), 4);
        assert_eq!(offset_of!(OutBuffer, buffer), 72);
        assert_eq!(offset_of!(OutBuffer, params), 80);
        assert_eq!(offset_of!(NnOutput, num), 4);
        assert_eq!(offset_of!(NnOutput, outputs), 8);
        assert_eq!(8 + offset_of!(OutBuffer, buffer), 0x50);
    }

    #[test]
    fn tensor_info_layout_matches_public_sdk_header() {
        assert_eq!(size_of::<TensorInfoEntry>(), 116);
        assert_eq!(offset_of!(TensorInfoEntry, dimension_count), 0);
        assert_eq!(offset_of!(TensorInfoEntry, dimensions), 4);
        assert_eq!(offset_of!(TensorInfoEntry, data_format), 28);
        assert_eq!(offset_of!(TensorInfoEntry, data_type), 32);
        assert_eq!(offset_of!(TensorInfoEntry, quantization_format), 36);
        assert_eq!(offset_of!(TensorInfoEntry, fixed_point_position), 40);
        assert_eq!(offset_of!(TensorInfoEntry, tf_scale), 44);
        assert_eq!(offset_of!(TensorInfoEntry, tf_zero_point), 48);
        assert_eq!(offset_of!(TensorInfoEntry, name), 52);
        assert_eq!(size_of::<TensorInfoList>(), 16);
        assert_eq!(offset_of!(TensorInfoList, entries), 8);
    }

    #[test]
    fn computes_tensor_storage_size() {
        assert_eq!(tensor_byte_len(&[1, 2, 3], 0).unwrap(), 24);
        assert_eq!(tensor_byte_len(&[1, 2, 3], 3).unwrap(), 6);
        assert!(tensor_byte_len(&[0, 2], 3).is_err());
        assert!(tensor_byte_len(&[1], 99).is_err());
    }

    #[test]
    fn parses_nbg_version_and_product() {
        let header = parse_nbg_header(&[
            b'V', b'P', b'M', b'N', 0x00, 0x00, 0x02, 0x00, 0x0e, 0x00, 0x00, 0x10,
        ])
        .unwrap();
        assert_eq!(
            header,
            NbgHeader {
                version: 0x00020000,
                product: 0x1000000e,
            }
        );
        assert!(parse_nbg_header(b"VPMN").is_err());
        assert!(parse_nbg_header(b"NOPE00000000").is_err());
    }

    #[test]
    fn verifies_expected_add_output() {
        assert_eq!(verify_add_output(1, 2, Some(&[4, 6])).unwrap(), [4, 6]);
    }

    #[test]
    fn rejects_wrong_add_output() {
        let error = verify_add_output(1, 2, Some(&[4, 7])).unwrap_err();
        assert!(error.contains("wrong add output"));
    }

    #[test]
    fn rejects_null_output_buffer() {
        let error = verify_add_output(1, 2, None).unwrap_err();
        assert!(error.contains("NULL"));
    }

    #[test]
    fn rejects_wrong_output_count() {
        let error = verify_add_output(2, 2, Some(&[4, 6])).unwrap_err();
        assert!(error.contains("exactly one"));
    }

    #[test]
    fn rejects_short_output_buffer() {
        let error = verify_add_output(1, 1, Some(&[4])).unwrap_err();
        assert!(error.contains("at least two"));
    }
}
