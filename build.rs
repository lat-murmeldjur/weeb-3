use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
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

fn collect_existing(paths: &[&str], files: &mut Vec<PathBuf>) {
    files.extend(paths.iter().map(PathBuf::from).filter(|path| path.exists()));
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
    collect_all_files(Path::new("src"), &mut files);
    collect_existing(
        &[
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
            "static/worker.js",
        ],
        &mut files,
    );
    version_for_files(files)
}

fn asset_build_version() -> String {
    let mut files = Vec::new();
    collect_existing(
        &[
            "static/404.html",
            "static/example.html",
            "static/hls-stream-example.html",
            "static/index.html",
            "static/service.js",
            "static/worker.js",
            "static/weeb_3.js",
            "static/weeb_3_bg.wasm",
        ],
        &mut files,
    );
    println!("cargo:rerun-if-changed=static/snippets");
    collect_all_files(Path::new("static/snippets"), &mut files);
    version_for_files(files)
}

fn main() {
    let version = if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        source_build_version()
    } else {
        asset_build_version()
    };
    println!("cargo:rustc-env=WEEB3_BUILD_VERSION={version}");
    println!("cargo:rustc-env=WEEB3_ASSET_VERSION={version}");
    prost_build::compile_protos(
        &[
            "src/etiquette_0.proto",
            "src/etiquette_1.proto",
            "src/etiquette_2.proto",
            "src/etiquette_4.proto",
            "src/etiquette_5.proto",
            "src/etiquette_6.proto",
            "src/etiquette_7.proto",
            "src/etiquette_8.proto",
        ],
        &["src/"],
    )
    .unwrap();
}
