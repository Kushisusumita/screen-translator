//! Rounding the corners of the frameless windows on Windows.
//!
//! Same problem as on macOS, from the other end: egui draws a rounded card, but
//! the window under it is a rectangle. When the transparent window cannot be
//! created — an old driver, a disabled compositor — the four corners outside the
//! rounding stay filled and the card sits on a square.
//!
//! Windows 11 has an attribute for exactly this and applies it to the real
//! window, so nothing has to be painted over. On Windows 10 the call fails
//! harmlessly and the window stays square-cornered, which is what every other
//! Windows 10 window looks like anyway.

use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

/// Asks the window manager to round this window's corners.
///
/// Found by title, like its macOS counterpart, because a child viewport's handle
/// is not reachable through eframe.
pub fn round_corners(title: &str) {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

    let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) };
    let Ok(hwnd) = hwnd else { return };
    if hwnd.0.is_null() {
        return;
    }
    apply(hwnd);
}

fn apply(hwnd: HWND) {
    let preference = DWMWCP_ROUND;
    // Fails on Windows 10, where the attribute does not exist. Nothing to
    // report: a square window there is the platform's own look.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const DWM_WINDOW_CORNER_PREFERENCE as *const _,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    };
}
