//! Build script for `mezon-codec`.
//!
//! Links the **system** libvpx (never builds it from source — the CI/dev boxes
//! have no nasm/cmake), generates Rust FFI bindings for it with `bindgen`, and
//! compiles a tiny C shim (`vpx_shim.c`) exposing the ABI-version/variadic
//! libvpx macros as plain functions.

use std::env;
use std::path::PathBuf;

fn main() {
    // Emits the `cargo:rustc-link-*` directives for libvpx and hands back the
    // header search paths so bindgen and the shim compiler can find <vpx/*.h>.
    let lib = pkg_config::Config::new()
        .probe("vpx")
        .expect("system libvpx not found (pkg-config `vpx`); install libvpx");

    // Compile the C shim with the same include paths libvpx advertised.
    let mut shim = cc::Build::new();
    shim.file("vpx_shim.c");
    for path in &lib.include_paths {
        shim.include(path);
    }
    shim.compile("vpx_shim");

    // Generate bindings for the libvpx public API.
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .allowlist_function("vpx_.*")
        .allowlist_type("vpx_.*")
        .allowlist_type("vp8.*")
        .allowlist_type("vp9.*")
        .allowlist_var("VPX_.*")
        .allowlist_var("VP8.*")
        .allowlist_var("VP9.*")
        .default_enum_style(bindgen::EnumVariation::Consts)
        .prepend_enum_name(false)
        .layout_tests(false)
        .generate_comments(false);

    for path in &lib.include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR set by cargo"));
    builder
        .generate()
        .expect("bindgen failed to generate libvpx bindings")
        .write_to_file(out_dir.join("vpx_bindings.rs"))
        .expect("failed to write vpx_bindings.rs");

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=vpx_shim.c");
}
