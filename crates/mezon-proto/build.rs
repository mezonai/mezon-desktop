use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if env::var("PROTOC").is_err()
        && let Ok(path) = protoc_bin_vendored::protoc_bin_path()
    {
        unsafe { env::set_var("PROTOC", path) };
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let src_dir = manifest_dir.join("src");
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let api_import_dir = out_dir.join("Api");

    fs::create_dir_all(&api_import_dir)?;
    fs::copy(src_dir.join("api.proto"), api_import_dir.join("api.proto"))?;

    prost_build::Config::new()
        .out_dir(&out_dir)
        .compile_protos(
            &[
                api_import_dir.join("api.proto"),
                src_dir.join("realtime.proto"),
            ],
            &[out_dir, src_dir],
        )?;

    println!("cargo:rerun-if-changed=src/api.proto");
    println!("cargo:rerun-if-changed=src/realtime.proto");

    Ok(())
}
