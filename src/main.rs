// In release builds hide the console window; keep it in debug so errors are visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod entities;
mod features;
mod shared;

use app::App;
use eframe::{NativeOptions, egui};
use egui::ViewportBuilder;

fn main() -> eframe::Result<()> {
    init_logging();

    tracing::info!("Screen Translator starting");

    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Screen Translator")
            .with_inner_size([1.0, 1.0])
            .with_position(egui::pos2(-32000.0, -32000.0))
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_visible(true)
            .with_icon(load_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Screen Translator",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

// -- Logging ------------------------------------------------------------------

#[cfg(debug_assertions)]
fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();
}

#[cfg(not(debug_assertions))]
fn init_logging() {
    use std::fs::OpenOptions;

    // Release: WARN+ written to %AppData%\screen-translator\screen-translator.log
    let log_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("screen-translator")
        .join("screen-translator.log");

    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env()
                    .add_directive(tracing::Level::WARN.into()),
            )
            .with_ansi(false)
            .init();
        return;
    }

    // Fallback: no file available, drop to stderr (won't show without console).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();
}

// -- Icon ---------------------------------------------------------------------

fn load_icon() -> egui::IconData {
    if let Ok(bytes) = std::fs::read("assets/icon.ico") {
        if let Ok(img) = image::load_from_memory(&bytes) {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            return egui::IconData { rgba: rgba.into_raw(), width: w, height: h };
        }
    }

    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for i in 0..(size * size) as usize {
        rgba[i * 4] = 40;
        rgba[i * 4 + 1] = 120;
        rgba[i * 4 + 2] = 220;
        rgba[i * 4 + 3] = 255;
    }
    egui::IconData { rgba, width: size, height: size }
}
