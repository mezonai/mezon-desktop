use std::{collections::BTreeSet, env, fs, path::Path, process::Command};

pub fn isolate_dependencies(webrtc: &Path, out_dir: &Path) {
    println!("cargo:rerun-if-env-changed=NM");
    println!("cargo:rerun-if-env-changed=OBJCOPY");
    println!("cargo:rerun-if-changed={}", webrtc.display());
    let nm = env::var_os("NM").unwrap_or_else(|| "nm".into());
    let objcopy = env::var_os("OBJCOPY").unwrap_or_else(|| "objcopy".into());
    let wrapper = out_dir.join("libwebrtcsys-cxx.a");
    let mut symbols = BTreeSet::new();

    // Keep WebRTC's Protobuf/Abseil definitions and calls separate from ONNX Runtime.
    for archive in [webrtc, wrapper.as_path()] {
        let output = Command::new(&nm)
            .args(["-g", "-P"])
            .arg(archive)
            .output()
            .expect("Failed to run nm; install binutils or set NM");
        assert!(
            output.status.success(),
            "nm failed for {}: {}",
            archive.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        for line in String::from_utf8(output.stdout)
            .expect("Invalid nm output")
            .lines()
        {
            let mut fields = line.split_whitespace();
            if let (Some(symbol), Some(kind)) = (fields.next(), fields.next()) {
                if kind.len() == 1
                    && (symbol.contains("6google8protobuf")
                        || symbol.contains("4absl")
                        || symbol.starts_with("Absl"))
                {
                    symbols.insert(symbol.to_owned());
                }
            }
        }
    }

    let map = out_dir.join("webrtc-private-symbols.txt");
    let mapping: String = symbols
        .iter()
        .map(|symbol| format!("{symbol} mezon_webrtc_{symbol}\n"))
        .collect();
    fs::write(&map, mapping).expect("Failed to write WebRTC symbol map");

    let isolated = out_dir.join("libmezon_webrtc.a");
    let patched_wrapper = out_dir.join("libwebrtcsys-cxx-isolated.a");
    for (input, output) in [(webrtc, &isolated), (wrapper.as_path(), &patched_wrapper)] {
        let result = Command::new(&objcopy)
            .arg("--redefine-syms")
            .arg(&map)
            .arg(input)
            .arg(output)
            .output()
            .expect("Failed to run objcopy; install binutils or set OBJCOPY");
        assert!(
            result.status.success(),
            "objcopy failed for {}: {}",
            input.display(),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fs::rename(patched_wrapper, wrapper).expect("Failed to replace WebRTC wrapper archive");
}
