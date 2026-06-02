use std::{
    fs,
    path::{Path, PathBuf},
};

use prost_build;

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
            continue;
        }

        let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
            continue;
        };

        if matches!(extension, "rs" | "proto" | "html" | "js" | "toml") {
            files.push(path);
        }
    }
}

fn build_version() -> String {
    let mut files = Vec::new();
    collect_files(Path::new("src"), &mut files);
    for path in [
        "Cargo.toml",
        "build.rs",
        "static/index.html",
        "static/service.js",
        "docs/index.html",
        "docs/service.js",
    ] {
        let path = PathBuf::from(path);
        if path.exists() {
            files.push(path);
        }
    }

    files.sort();
    files.dedup();

    let mut hash = 14695981039346656037u64;
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        hash = hash_bytes(hash, path.to_string_lossy().as_bytes());
        if let Ok(bytes) = fs::read(&path) {
            hash = hash_bytes(hash, &bytes);
        }
    }

    format!("{:016x}", hash)
}

fn main() {
    println!("cargo:rustc-env=WEEB3_BUILD_VERSION={}", build_version());
    prost_build::compile_protos(&["src/etiquette_0.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_1.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_2.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_3.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_4.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_5.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_6.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_7.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_8.proto"], &["src/"]).unwrap();
}
