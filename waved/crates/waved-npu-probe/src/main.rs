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
//! No model loading, no graph construction — that's a follow-up probe once
//! we know step 4 passes.

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::os::raw::c_int;
use std::process::ExitCode;

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
const LIB_CANDIDATES: &[&str] = &[
    "libOpenVX.so",
    "libovxlib.so",
    "libOpenCL.so",
];

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
    match OpenOptions::new().read(true).write(true).open("/dev/galcore") {
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
    step(3, &format!("resolve vxCreateContext / vxReleaseContext in {libname}"));

    let create: libloading::Symbol<VxCreateContextFn> = match unsafe {
        lib.get(b"vxCreateContext\0")
    } {
        Ok(s) => {
            ok("vxCreateContext symbol found");
            s
        }
        Err(e) => {
            fail(&format!("vxCreateContext missing: {e}"));
            return false;
        }
    };

    let release: libloading::Symbol<VxReleaseContextFn> = match unsafe {
        lib.get(b"vxReleaseContext\0")
    } {
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
    step(n, &format!("check for vsi_nn_* (VeriSilicon NN SDK) in {libname}"));
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

fn main() -> ExitCode {
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

    println!();
    if ctx_ok {
        println!("RESULT: OK — NPU reachable from waved's process.");
        ExitCode::SUCCESS
    } else {
        println!("RESULT: FAIL — library loaded but context creation failed.");
        ExitCode::from(3)
    }
}

