use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let sdk_path = manifest_dir.join("..").join("vst3sdk");
    println!("cargo:rustc-env=VST3_SDK_PATH={}", sdk_path.display());
}
