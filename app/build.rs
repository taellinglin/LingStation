use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let sdk_path = manifest_dir.join("..").join("vst3sdk");
    println!("cargo:rustc-env=VST3_SDK_PATH={}", sdk_path.display());

    #[cfg(windows)]
    {
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let icon_png = manifest_dir.join("..").join("icon.png");
        if icon_png.exists() {
            let icon_ico = out_dir.join("icon.ico");
            if let Ok(img) = image::open(&icon_png) {
                let rgba = img.to_rgba8();
                let (width, height) = rgba.dimensions();
                let icon_image = ico::IconImage::from_rgba_data(width, height, rgba.into_raw());
                if let Ok(entry) = ico::IconDirEntry::encode(&icon_image) {
                    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
                    icon_dir.add_entry(entry);
                    if let Ok(mut file) = std::fs::File::create(&icon_ico) {
                        let _ = icon_dir.write(&mut file);
                        let mut res = winres::WindowsResource::new();
                        res.set_icon(icon_ico.to_string_lossy().as_ref());
                        let _ = res.compile();
                    }
                }
            }
        }
    }
}
