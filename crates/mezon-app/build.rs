use std::path::{Path, PathBuf};
use std::{env, fs};

fn decompress_woff2(src: &Path, dest: &Path) {
    let data = fs::read(src).expect("Failed to read WOFF2 file");
    let mut cursor = data.as_slice();
    let ttf = woff2_patched::convert_woff2_to_ttf(&mut cursor).expect("Failed to decompress WOFF2");
    fs::write(dest, ttf).expect("Failed to write TTF file");
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let in_dir = manifest_dir.join("../../assets/fonts/gg-sans");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let variants = ["Normal", "Medium", "Semibold", "Bold", "ExtraBold"];
    for variant in &variants {
        let src = in_dir.join(format!("ggsans-{variant}.woff2"));
        let dest = out_dir.join(format!("ggsans-{variant}.ttf"));
        println!("cargo:rerun-if-changed={}", src.display());
        if !dest.exists() {
            decompress_woff2(&src, &dest);
        }
    }
}
