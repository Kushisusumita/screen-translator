//! What Windows lets a window be, past what winit and eframe expose.
//!
//! The app's windows are child viewports, and their handles are not reachable
//! through eframe. Each one is found by its title instead — the same route the
//! macOS side takes through `NSApp.windows` — and adjusted directly.
//!
//! Four things here matter far more in a game than on a desktop:
//!
//! * **Not taking focus.** A translation that activates its own window pulls
//!   the keyboard away from the game: borderless titles pause or mute, and
//!   exclusive full-screen ones minimise. `WS_EX_NOACTIVATE` keeps the window
//!   clickable without it ever becoming the foreground one.
//! * **Appearing at once.** DWM animates windows in and out. For a window that
//!   shows a frozen copy of the screen, that animation *is* the glitch — the
//!   desktop appears to zoom for a fifth of a second on every capture.
//! * **Staying out of captures.** While a region is being watched, the
//!   translation sits on top of the very pixels that have to be read again.
//!   Windows can leave a window out of screen captures, so the next grab sees
//!   the game rather than the previous answer.
//! * **Showing the game through.** A subtitle bar is a strip of glass, not a
//!   black box. See [`compose_by_alpha`] for why that has to be asked for
//!   here rather than of the toolkit.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HANDLE, HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmEnableBlurBehindWindow, DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWM_BB_BLURREGION, DWM_BB_ENABLE,
    DWM_BLURBEHIND, DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, GetPropW, GetWindowDisplayAffinity, GetWindowLongW,
    GetWindowRect, SetPropW, SetWindowDisplayAffinity, SetWindowLongW, SetWindowPos,
    GWL_EXSTYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOOWNERZORDER, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WS_EX_NOACTIVATE,
};

use super::screenshot::Bounds;

/// How a window should behave. Applied every frame: each setting is checked
/// first and only written when it differs, and winit rewrites the extended
/// style whenever it changes one of its own flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Behaviour {
    /// Never becomes the foreground window, even when clicked.
    pub no_activate: bool,
    /// Skips the open and close animations.
    pub instant: bool,
    /// Asks Windows 11 to round the corners. Fails harmlessly on Windows 10,
    /// where a square window is the platform's own look.
    pub rounded: bool,
    /// Left out of screenshots and screen captures, including the app's own.
    pub hidden_from_capture: bool,
    /// Blended with what is behind it, pixel by pixel, by its own alpha.
    pub translucent: bool,
}

fn find(title: &str) -> Option<HWND> {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) }.ok()?;
    (!hwnd.0.is_null()).then_some(hwnd)
}

/// Applies `behaviour` to the window with this title. Returns false when no
/// such window exists yet — on the frame that creates it, typically.
pub fn apply(title: &str, behaviour: Behaviour) -> bool {
    let Some(hwnd) = find(title) else {
        return false;
    };

    unsafe {
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let wanted = if behaviour.no_activate {
            style | WS_EX_NOACTIVATE.0
        } else {
            style & !WS_EX_NOACTIVATE.0
        };
        if wanted != style {
            SetWindowLongW(hwnd, GWL_EXSTYLE, wanted as i32);
        }

        // Cheap, idempotent, and has no getter worth the round trip.
        let disabled = BOOL::from(behaviour.instant);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &disabled as *const BOOL as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );

        if behaviour.rounded {
            let preference = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &preference as *const DWM_WINDOW_CORNER_PREFERENCE as *const _,
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
        }

        if behaviour.translucent {
            compose_by_alpha(hwnd);
        }

        let mut affinity = 0u32;
        let _ = GetWindowDisplayAffinity(hwnd, &mut affinity);
        let hidden = affinity == WDA_EXCLUDEFROMCAPTURE.0;
        if hidden != behaviour.hidden_from_capture {
            // Refused before Windows 10 2004. Nothing to report: the caller
            // asks `hides_from_capture` and works around it.
            let _ = SetWindowDisplayAffinity(
                hwnd,
                if behaviour.hidden_from_capture {
                    WDA_EXCLUDEFROMCAPTURE
                } else {
                    WDA_NONE
                },
            );
        }
    }
    true
}

/// Has the compositor blend the window by the alpha it was drawn with.
///
/// winit does exactly this for a window created transparent. But the GL
/// integration first asks the driver whether its pixel format "supports
/// transparency" — a question about colour-key transparency that most
/// drivers answer with a no — and on a no it quietly creates every window
/// opaque: the subtitle bar came out a solid black box, and its opacity
/// setting did nothing. Blending by alpha needs nothing of the pixel format
/// but an alpha channel, which it has, so the request is made here.
///
/// Once per window: the window carries a mark saying it has been done.
unsafe fn compose_by_alpha(hwnd: HWND) {
    let mark = w!("SakuraComposedByAlpha");
    if !GetPropW(hwnd, mark).is_invalid() {
        return;
    }
    // An empty region: nothing is blurred, everything is blended.
    let region = CreateRectRgn(0, 0, -1, -1);
    let blur = DWM_BLURBEHIND {
        dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
        fEnable: true.into(),
        hRgnBlur: region,
        fTransitionOnMaximized: false.into(),
    };
    let enabled = DwmEnableBlurBehindWindow(hwnd, &blur).is_ok();
    let _ = DeleteObject(region);
    if enabled {
        let _ = SetPropW(hwnd, mark, HANDLE(1 as _));
    }
}

/// Whether the window really is being left out of captures — the request is
/// refused on older systems, and a grab that trusts it there would read back
/// the translation it is trying to replace.
pub fn hidden_from_capture(title: &str) -> bool {
    let Some(hwnd) = find(title) else {
        return false;
    };
    let mut affinity = 0u32;
    unsafe {
        GetWindowDisplayAffinity(hwnd, &mut affinity).is_ok()
            && affinity == WDA_EXCLUDEFROMCAPTURE.0
    }
}

/// Puts the window on exactly these desktop pixels, on top, without giving it
/// the focus.
///
/// The builder's position and size are in points, and winit turns them back
/// into pixels with a scale factor it has to guess before the window is on any
/// monitor. Close is not good enough for a window that has to line up with the
/// pixels it was captured from, so the frame is set here in the unit the
/// capture is in.
pub fn place(title: &str, bounds: Bounds) {
    let Some(hwnd) = find(title) else { return };
    if frame_of(hwnd) == Some(bounds) {
        return;
    }
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            bounds.x,
            bounds.y,
            bounds.w,
            bounds.h,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

fn frame_of(hwnd: HWND) -> Option<Bounds> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(Bounds {
        x: rect.left,
        y: rect.top,
        w: rect.right - rect.left,
        h: rect.bottom - rect.top,
    })
}

/// True when a mouse button went down somewhere other than on this window.
///
/// A window that never takes the focus never loses it either, so "close when I
/// click away" has nothing to hook into. The button state is read instead —
/// both "down now" and "went down since the last call", so a click that starts
/// and ends between two frames still counts.
pub fn clicked_elsewhere(title: &str) -> bool {
    let pressed = [VK_LBUTTON, VK_RBUTTON].into_iter().any(|key| {
        let state = unsafe { GetAsyncKeyState(key.0 as i32) } as u16;
        state & 0x8000 != 0 || state & 1 != 0
    });
    if !pressed {
        return false;
    }

    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        return false;
    }
    match find(title).and_then(frame_of) {
        Some(frame) => !frame.contains(point.x, point.y),
        None => false,
    }
}
