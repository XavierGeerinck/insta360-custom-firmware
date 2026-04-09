use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let lvgl_root = manifest.join("lvgl-src/lvgl");
    let lvgl_src = lvgl_root.join("src");
    let conf_dir = manifest.clone();

    // Collect all LVGL .c files recursively
    let c_files = collect_c_files(&lvgl_src);
    println!("cargo:warning=Compiling {} LVGL C files", c_files.len());

    cc::Build::new()
        .files(&c_files)
        .include(&conf_dir)       // lv_conf.h
        .include(&manifest.join("lvgl-src"))  // #include "lvgl/lvgl.h"
        .include(&lvgl_root)      // internal lvgl includes
        .include(&lvgl_src)
        .define("LV_CONF_INCLUDE_SIMPLE", None)
        .flag("-w")
        .opt_level(2)
        .compile("lvgl");

    let bindings = bindgen::Builder::default()
        .header(lvgl_root.join("lvgl.h").to_str().unwrap())
        .clang_arg(format!("-I{}", conf_dir.display()))
        .clang_arg(format!("-I{}", manifest.join("lvgl-src").display()))
        .clang_arg(format!("-I{}", lvgl_root.display()))
        .clang_arg(format!("-I{}", lvgl_src.display()))
        .clang_arg("-DLV_CONF_INCLUDE_SIMPLE")
        .allowlist_function("lv_.*")
        .allowlist_type("lv_.*")
        .allowlist_var("LV_.*")
        .derive_debug(true)
        .derive_default(true)
        .generate()
        .expect("bindgen failed");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings.write_to_file(out.join("lvgl_bindings.rs")).unwrap();

    println!("cargo:rerun-if-changed=lv_conf.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=lvgl-src/lvgl/src");
}

fn collect_c_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                files.extend(collect_c_files(&p));
            } else if p.extension().map_or(false, |e| e == "c") {
                files.push(p);
            }
        }
    }
    files
}
