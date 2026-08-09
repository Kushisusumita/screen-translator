/// Only macOS needs the overlay window pushed past what the window server
/// would otherwise allow.
#[cfg(target_os = "macos")]
pub mod mac_window;
/// Windows 11 rounds a window's corners itself, given the word.
#[cfg(windows)]
pub mod win_window;
pub mod overlay;
pub mod screenshot;
pub mod window_pick;

pub use overlay::{Geometry, OverlayState};
pub use screenshot::{
    capture_desktop_image, capture_region_for_ocr, foreground_window_bounds, virtual_desktop,
    work_area, Bounds,
};
