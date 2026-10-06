//! Hand-rolled bindings for the speexdsp FFI functions we call.
//!
//! Bindgen would be overkill — speexdsp's API is tiny. We mirror the upstream
//! header declarations exactly; the types are opaque on the Rust side.

use std::ffi::c_int;
use std::os::raw::c_void;

#[repr(C)]
pub struct SpeexEchoState {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SpeexPreprocessState {
    _private: [u8; 0],
}

// speex_echo_ctl request codes (from speex_echo.h).
pub const SPEEX_ECHO_SET_SAMPLING_RATE: c_int = 24;

// speex_preprocess_ctl request codes (from speex_preprocess.h). Only the ones
// we use are named.
pub const SPEEX_PREPROCESS_SET_ECHO_STATE: c_int = 24;
pub const SPEEX_PREPROCESS_SET_DENOISE: c_int = 0;
pub const SPEEX_PREPROCESS_SET_NOISE_SUPPRESS: c_int = 18;
pub const SPEEX_PREPROCESS_SET_ECHO_SUPPRESS: c_int = 20;
pub const SPEEX_PREPROCESS_SET_ECHO_SUPPRESS_ACTIVE: c_int = 22;

extern "C" {
    pub fn speex_echo_state_init(frame_size: c_int, filter_length: c_int) -> *mut SpeexEchoState;
    pub fn speex_echo_state_destroy(st: *mut SpeexEchoState);
    pub fn speex_echo_ctl(st: *mut SpeexEchoState, request: c_int, ptr: *mut c_void) -> c_int;
    pub fn speex_echo_cancellation(
        st: *mut SpeexEchoState,
        rec: *const i16,
        play: *const i16,
        out: *mut i16,
    );

    pub fn speex_preprocess_state_init(
        frame_size: c_int,
        sampling_rate: c_int,
    ) -> *mut SpeexPreprocessState;
    pub fn speex_preprocess_state_destroy(st: *mut SpeexPreprocessState);
    pub fn speex_preprocess_run(st: *mut SpeexPreprocessState, x: *mut i16) -> c_int;
    pub fn speex_preprocess_ctl(
        st: *mut SpeexPreprocessState,
        request: c_int,
        ptr: *mut c_void,
    ) -> c_int;
}
