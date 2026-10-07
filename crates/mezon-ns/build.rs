use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    let native = root.join("src/native");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .opt_level(3)
        .warnings(false)
        .define("MEZON_NS_STATIC", None)
        .include(native.join("include"))
        .include(&native)
        .include(native.join("vendor/onnxruntime"))
        .file(native.join("mezon_ns_c_api.cpp"))
        .file(native.join("mezon_ns_engine.cpp"));
    if env::var("TARGET").is_ok_and(|target| target.contains("msvc")) {
        build.flag("/EHsc");
    }
    build.compile("mezon_ns");
    println!("cargo:rerun-if-changed={}", native.display());
}
