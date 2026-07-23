//! NPU proof-of-life probe for the Insta360 Wave (Amlogic A113X2 + Vivante VIP).
//!
//! Answers a single question: can waved's own process reach the NPU, or is
//! it locked by a stock service / kernel policy / ABI mismatch?
//!
//! Probe order (each step gated on the previous):
//!   1. open("/dev/galcore", O_RDWR) — kernel device accessible to our uid.
//!   2. dlopen("libOpenVX.so") — Khronos/Vivante OpenVX runtime loadable.
//!   3. dlsym "vxCreateContext" — symbol present (rules out a stub .so).
//!   4. vxCreateContext() — call actually touches the driver via ioctl.
//!   5. vxReleaseContext() — clean release.
//!
//! A run that makes it to step 4 returning a non-null handle means our
//! process has a fully working NPU path. Step 5 confirms the release side.
//!
//! With no arguments the probe retains this original context check.
//! `--run-add` additionally loads a deterministic NBG through the stock
//! `libnnsdk.so`, executes it ten times, and verifies the result.
//! `--run-stock-ctc` executes an unmodified firmware graph as a control when
//! validating whether a newly compiled NBG targets the correct NPU product.

mod nnsdk;

use std::ffi::c_void;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::os::raw::c_int;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;

// Opaque OpenVX types — we never dereference these, so a void* is fine.
type VxContext = *mut c_void;

// Minimal FFI surface. OpenVX exposes dozens of entrypoints; we only need
// two for proof-of-life.
type VxCreateContextFn = unsafe extern "C" fn() -> VxContext;
type VxReleaseContextFn = unsafe extern "C" fn(*mut VxContext) -> c_int;

// Candidates to dlopen, in priority order. libOpenVX.so is the Khronos
// standard entry point and what we'd link against for real. libovxlib.so is
// VeriSilicon's higher-level wrapper (used by stock libnnsdk.so); probing
// it too is cheap and tells us whether that path is also available from our
// uid. libOpenCL.so is probed last as a sanity check — Vivante's OpenCL
// runtime lives in the same driver, so if all three fail the issue is
// kernel/permissions, not a single missing file.
const LIB_CANDIDATES: &[&str] = &["libOpenVX.so", "libovxlib.so", "libOpenCL.so"];

fn step(n: u32, msg: &str) {
    println!("[{n}] {msg}");
}

fn ok(msg: &str) {
    println!("    OK: {msg}");
}

fn fail(msg: &str) {
    println!("    FAIL: {msg}");
}

fn probe_galcore() -> bool {
    step(1, "open /dev/galcore O_RDWR");
    match OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/galcore")
    {
        Ok(_f) => {
            ok("/dev/galcore opened — kernel driver reachable from our uid");
            true
        }
        Err(e) => {
            fail(&format!("/dev/galcore open failed: {e}"));
            false
        }
    }
}

/// Try to dlopen each candidate; return the first that loads plus its name.
fn probe_dlopen() -> Option<(libloading::Library, &'static str)> {
    step(2, "dlopen candidate libraries");
    for name in LIB_CANDIDATES {
        // SAFETY: dlopen of a system library is defined; the returned Library
        // has no destructors we run on this path.
        match unsafe { libloading::Library::new(name) } {
            Ok(lib) => {
                ok(&format!("dlopen {name}"));
                return Some((lib, name));
            }
            Err(e) => {
                println!("    skip {name}: {e}");
            }
        }
    }
    fail("none of the candidate libraries loaded");
    None
}

fn probe_vx_context(lib: &libloading::Library, libname: &str) -> bool {
    step(
        3,
        &format!("resolve vxCreateContext / vxReleaseContext in {libname}"),
    );

    let create: libloading::Symbol<VxCreateContextFn> =
        match unsafe { lib.get(b"vxCreateContext\0") } {
            Ok(s) => {
                ok("vxCreateContext symbol found");
                s
            }
            Err(e) => {
                fail(&format!("vxCreateContext missing: {e}"));
                return false;
            }
        };

    let release: libloading::Symbol<VxReleaseContextFn> =
        match unsafe { lib.get(b"vxReleaseContext\0") } {
            Ok(s) => {
                ok("vxReleaseContext symbol found");
                s
            }
            Err(e) => {
                fail(&format!("vxReleaseContext missing: {e}"));
                return false;
            }
        };

    step(4, "vxCreateContext()");
    // SAFETY: we just resolved the symbol from the library and the OpenVX
    // contract is that vxCreateContext takes no args and returns a handle
    // (or NULL on failure). The returned pointer, if non-null, stays valid
    // until vxReleaseContext.
    let ctx = unsafe { create() };
    if ctx.is_null() {
        fail("vxCreateContext returned NULL — driver rejected the request");
        return false;
    }
    ok(&format!("context handle = {ctx:p}"));

    step(5, "vxReleaseContext()");
    let mut ctx_mut = ctx;
    // SAFETY: &mut to a valid non-null context; contract is that the pointer
    // is zeroed on success.
    let status = unsafe { release(&mut ctx_mut) };
    // OpenVX: VX_SUCCESS == 0.
    if status == 0 {
        ok(&format!("released cleanly (status={status})"));
        true
    } else {
        fail(&format!("vxReleaseContext returned {status}"));
        // Not a hard failure for "can we use the NPU" — we got a handle.
        true
    }
}

/// Best-effort probe of VeriSilicon's vsi_nn_* API (what libnnsdk.so and
/// TIM-VX use). Independent of the OpenVX probe; skipped if the symbols
/// aren't exported by whichever library loaded.
fn probe_vsi_nn(n: u32, lib: &libloading::Library, libname: &str) {
    step(
        n,
        &format!("check for vsi_nn_* (VeriSilicon NN SDK) in {libname}"),
    );
    type VsiNnCreateContextFn = unsafe extern "C" fn() -> *mut c_void;
    match unsafe { lib.get::<VsiNnCreateContextFn>(b"vsi_nn_CreateContext\0") } {
        Ok(f) => {
            // SAFETY: resolved symbol, documented no-arg constructor.
            let c = unsafe { f() };
            if c.is_null() {
                fail("vsi_nn_CreateContext returned NULL");
            } else {
                ok(&format!("vsi_nn_CreateContext() = {c:p}"));
                // No release — this probe's purpose is to confirm reachability.
                // The process exits right after, so leaking the context is fine.
            }
        }
        Err(_) => {
            println!("    note: vsi_nn_CreateContext not exported by {libname} (expected for libOpenVX.so)");
        }
    }
}

fn run_context_probe() -> ExitCode {
    println!("waved-npu-probe — proof-of-life for Vivante NPU from our process");
    println!();

    let galcore_ok = probe_galcore();
    if !galcore_ok {
        println!();
        println!("RESULT: FAIL — /dev/galcore not accessible. NPU not reachable.");
        return ExitCode::from(1);
    }

    let (lib, libname) = match probe_dlopen() {
        Some(pair) => pair,
        None => {
            println!();
            println!("RESULT: FAIL — no Vivante userspace library loadable.");
            return ExitCode::from(2);
        }
    };

    let ctx_ok = probe_vx_context(&lib, libname);
    probe_vsi_nn(6, &lib, libname);

    // libovxlib.so is the path stock libnnsdk.so and TIM-VX both use for
    // real model inference, so check it explicitly even if step 2 already
    // succeeded with a different library.
    if libname != "libovxlib.so" {
        step(7, "also probe libovxlib.so (VeriSilicon NN API)");
        match unsafe { libloading::Library::new("libovxlib.so") } {
            Ok(ovxlib) => {
                ok("dlopen libovxlib.so");
                probe_vsi_nn(8, &ovxlib, "libovxlib.so");
            }
            Err(e) => {
                fail(&format!("libovxlib.so dlopen failed: {e}"));
            }
        }
    }

    // TIM-VX smoke: our own cross-built libtim-vx.so (SP#4 Task 2). Success
    // here means TIM-VX links cleanly against the device's Vivante 6.4.19
    // libs despite being built against the 6.4.15-era prebuilt SDK headers.
    // Expected at /data/waved/lib/libtim-vx.so; skipped if not present so
    // the probe still passes before Task 2 is deployed.
    step(9, "dlopen libtim-vx.so (our cross-built TIM-VX)");
    match unsafe { libloading::Library::new("libtim-vx.so") } {
        Ok(_tim) => {
            ok("libtim-vx.so loaded — no missing symbols against device libs");
        }
        Err(e) => {
            // Distinguish "not deployed yet" from "ABI mismatch": Task 2
            // builds land the .so at /data/waved/lib/; a bare 'file not
            // found' before deploy is OK.
            let s = format!("{e}");
            if s.contains("not found") || s.contains("No such file") {
                println!("    skip: libtim-vx.so not deployed yet (run scripts/build-tim-vx.sh + adb push)");
            } else {
                fail(&format!(
                    "libtim-vx.so dlopen failed (likely ABI issue): {s}"
                ));
            }
        }
    }

    println!();
    if ctx_ok {
        println!("RESULT: OK — NPU reachable from waved's process.");
        ExitCode::SUCCESS
    } else {
        println!("RESULT: FAIL — library loaded but context creation failed.");
        ExitCode::from(3)
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Mode {
    ContextProbe,
    RunAdd,
    RunStockCtc,
}

fn parse_mode(args: &[OsString]) -> Result<Mode, String> {
    match args {
        [] => Ok(Mode::ContextProbe),
        [argument] if argument == "--run-add" => Ok(Mode::RunAdd),
        [argument] if argument == "--run-stock-ctc" => Ok(Mode::RunStockCtc),
        _ => Err(format!(
            "usage: waved-npu-probe [--run-add|--run-stock-ctc]\nreceived: {args:?}"
        )),
    }
}

fn run_add_mode() -> ExitCode {
    let (sender, receiver) = mpsc::channel();
    let _worker = thread::spawn(move || {
        let _ = sender.send(nnsdk::run_add_poc());
    });

    match receiver.recv_timeout(nnsdk::TIMEOUT) {
        Ok(Ok(report)) => {
            println!();
            println!(
                "RESULT: PASS — NPU executed {} compiled-graph iterations; \
                 output={:?}, average={:.3} ms, cleanup={}.",
                report.latencies.len(),
                report.final_output,
                report.average_latency().as_secs_f64() * 1000.0,
                report.cleanup_status
            );
            ExitCode::SUCCESS
        }
        Ok(Err(error)) => {
            eprintln!();
            eprintln!("RESULT: FAIL — {error}");
            nnsdk::print_galcore_holders();
            ExitCode::from(error.exit_code())
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            eprintln!();
            eprintln!(
                "RESULT: FAIL — graph execution exceeded {} seconds; \
                 driver recovery was not changed.",
                nnsdk::TIMEOUT.as_secs()
            );
            nnsdk::print_galcore_holders();
            ExitCode::from(nnsdk::EXIT_TIMEOUT)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            eprintln!();
            eprintln!("RESULT: FAIL — NPU worker exited without a result.");
            nnsdk::print_galcore_holders();
            ExitCode::from(nnsdk::EXIT_WORKER_DISCONNECTED)
        }
    }
}

fn run_stock_ctc_mode() -> ExitCode {
    let (sender, receiver) = mpsc::channel();
    let _worker = thread::spawn(move || {
        let _ = sender.send(nnsdk::run_stock_ctc_poc());
    });

    match receiver.recv_timeout(nnsdk::TIMEOUT) {
        Ok(Ok(report)) => {
            println!();
            println!(
                "RESULT: PASS — NPU executed {} firmware-graph iterations; \
                 output_bytes={}, output_sha256={}, average={:.3} ms, cleanup={}.",
                report.latencies.len(),
                report.output_bytes,
                report.output_sha256,
                report.average_latency().as_secs_f64() * 1000.0,
                report.cleanup_status
            );
            ExitCode::SUCCESS
        }
        Ok(Err(error)) => {
            eprintln!();
            eprintln!("RESULT: FAIL — {error}");
            nnsdk::print_galcore_holders();
            ExitCode::from(error.exit_code())
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            eprintln!();
            eprintln!(
                "RESULT: FAIL — firmware graph execution exceeded {} seconds; \
                 driver recovery was not changed.",
                nnsdk::TIMEOUT.as_secs()
            );
            nnsdk::print_galcore_holders();
            ExitCode::from(nnsdk::EXIT_TIMEOUT)
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            eprintln!();
            eprintln!("RESULT: FAIL — NPU worker exited without a result.");
            nnsdk::print_galcore_holders();
            ExitCode::from(nnsdk::EXIT_WORKER_DISCONNECTED)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match parse_mode(&args) {
        Ok(Mode::ContextProbe) => run_context_probe(),
        Ok(Mode::RunAdd) => run_add_mode(),
        Ok(Mode::RunStockCtc) => run_stock_ctc_mode(),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_selects_context_probe() {
        assert_eq!(parse_mode(&[]).unwrap(), Mode::ContextProbe);
    }

    #[test]
    fn run_add_argument_selects_inference_probe() {
        assert_eq!(
            parse_mode(&[OsString::from("--run-add")]).unwrap(),
            Mode::RunAdd
        );
    }

    #[test]
    fn run_stock_ctc_argument_selects_control_probe() {
        assert_eq!(
            parse_mode(&[OsString::from("--run-stock-ctc")]).unwrap(),
            Mode::RunStockCtc
        );
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        let error = parse_mode(&[OsString::from("--unexpected")]).unwrap_err();
        assert!(error.contains("usage:"));
    }
}
