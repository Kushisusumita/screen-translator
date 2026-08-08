//! Generates the application icon from the same geometry the UI draws, then
//! embeds it in the executable.
//!
//! Doing it here rather than committing an `.ico` means the taskbar icon, the
//! tray icon and the mark beside "Sakura" in the settings window cannot drift
//! apart — they are all `shared::mark`.

use std::io::BufWriter;
use std::path::PathBuf;

// Pure geometry, no dependencies, so it can be pulled into the build script.
include!("src/shared/mark.rs");

fn main() {
    println!("cargo:rerun-if-changed=src/shared/mark.rs");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is always set"));
    let ico_path = out_dir.join("sakura.ico");

    match write_ico(&ico_path) {
        Ok(()) => {
            let mut res = winres::WindowsResource::new();
            res.set_icon(&ico_path.to_string_lossy());
            if let Err(e) = res.compile() {
                // Not fatal: the app runs fine, it just has no icon in Explorer.
                println!("cargo:warning=could not embed the icon resource: {e}");
            }
        }
        Err(e) => println!("cargo:warning=could not generate the icon: {e}"),
    }
}

fn write_ico(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    use image::ExtendedColorType;

    let frames: Vec<IcoFrame<'static>> = ICON_SIZES
        .iter()
        .map(|&size| {
            let rgba = rasterise(size);
            IcoFrame::as_png(&rgba, size, size, ExtendedColorType::Rgba8)
        })
        .collect::<Result<_, _>>()?;

    let file = std::fs::File::create(path)?;
    IcoEncoder::new(BufWriter::new(file)).encode_images(&frames)?;
    Ok(())
}
