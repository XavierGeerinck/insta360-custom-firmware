fn main() {
    println!("cargo:rerun-if-changed=vendor/speexdsp");
    println!("cargo:rerun-if-changed=build.rs");

    let mut b = cc::Build::new();
    b.include("vendor/speexdsp/include");
    b.include("vendor/speexdsp/libspeexdsp");
    b.define("FLOATING_POINT", None);
    b.define("USE_KISS_FFT", None);
    b.define("EXPORT", Some(""));
    b.files([
        "vendor/speexdsp/libspeexdsp/mdf.c",
        "vendor/speexdsp/libspeexdsp/preprocess.c",
        "vendor/speexdsp/libspeexdsp/fftwrap.c",
        "vendor/speexdsp/libspeexdsp/kiss_fft.c",
        "vendor/speexdsp/libspeexdsp/kiss_fftr.c",
        "vendor/speexdsp/libspeexdsp/filterbank.c",
    ]);
    b.warnings(false);
    b.compile("speexdsp");
}
