use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let payload = manifest_dir
        .parent()
        .unwrap()
        .join("src-tauri")
        .join("target")
        .join("release")
        .join("gemma4-chat.exe");

    if !payload.exists() {
        panic!(
            "missing app payload at {}. build the main Tauri app before building the custom installer.",
            payload.display()
        );
    }

    println!("cargo:rustc-env=GEMMA_PAYLOAD_EXE={}", payload.display());
    println!("cargo:rerun-if-changed={}", payload.display());
    tauri_build::build();
}
