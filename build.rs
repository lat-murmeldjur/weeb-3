use std::{
    env, fs,
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

fn collect_all_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_all_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn version_for_files(mut files: Vec<PathBuf>) -> String {
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

fn source_build_version() -> String {
    let mut files = Vec::new();
    collect_files(Path::new("src"), &mut files);
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "static/404.html",
        "static/example.html",
        "static/hls_loader.js",
        "static/hls-stream-example.html",
        "static/index.html",
        "static/issue-1-json-sync-example.html",
        "static/service.js",
    ] {
        let path = PathBuf::from(path);
        if path.exists() {
            files.push(path);
        }
    }
    version_for_files(files)
}

fn asset_build_version() -> String {
    let mut files = Vec::new();
    for path in [
        "static/404.html",
        "static/example.html",
        "static/hls-stream-example.html",
        "static/index.html",
        "static/issue-1-json-sync-example.html",
        "static/service.js",
        "static/weeb_3.js",
        "static/weeb_3_bg.wasm",
    ] {
        let path = PathBuf::from(path);
        if path.exists() {
            files.push(path);
        }
    }
    println!("cargo:rerun-if-changed=static/snippets");
    collect_all_files(Path::new("static/snippets"), &mut files);
    version_for_files(files)
}

fn main() {
    let source_version = source_build_version();
    println!("cargo:rustc-env=WEEB3_BUILD_VERSION={source_version}");
    let asset_version = if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        source_version
    } else {
        asset_build_version()
    };
    println!("cargo:rustc-env=WEEB3_ASSET_VERSION={asset_version}");
    prost_build::compile_protos(&["src/etiquette_0.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_1.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_2.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_4.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_5.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_6.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_7.proto"], &["src/"]).unwrap();
    prost_build::compile_protos(&["src/etiquette_8.proto"], &["src/"]).unwrap();
}
