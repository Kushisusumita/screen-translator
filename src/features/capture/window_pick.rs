//! Enumerates top-level windows so the "Окно" capture mode can highlight the
//! one under the cursor.
//!
//! The list has to be taken *before* the overlay appears: once it is up, our own
//! full-screen always-on-top window is the only thing `WindowFromPoint` would
//! ever return.

use super::screenshot::Bounds;

#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub bounds: Bounds,
    pub title: String,
}

/// Visible top-level windows, front-most first.
pub fn enumerate() -> Vec<WindowInfo> {
    #[cfg(windows)]
    {
        win::enumerate()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// Front-most window containing the point, in desktop physical pixels.
///
/// The list is already in z-order, so the first hit is the visible one — which
/// is what the user is pointing at even when windows overlap.
pub fn hit_test(windows: &[WindowInfo], x: i32, y: i32) -> Option<&WindowInfo> {
    windows.iter().find(|w| {
        x >= w.bounds.x && x < w.bounds.right() && y >= w.bounds.y && y < w.bounds.bottom()
    })
}

#[cfg(windows)]
mod win {
    use super::{Bounds, WindowInfo};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsIconic,
        IsWindowVisible, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
    };

    pub fn enumerate() -> Vec<WindowInfo> {
        let mut out: Vec<WindowInfo> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(callback),
                LPARAM(&mut out as *mut Vec<WindowInfo> as isize),
            );
        }
        out
    }

    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<WindowInfo>);

        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return TRUE;
        }

        // Tool windows are palettes and tooltips, never a capture target.
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
            return TRUE;
        }

        // UWP apps keep invisible "cloaked" host windows around that still
        // report themselves as visible and cover the whole screen.
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return TRUE;
        }

        if GetWindowTextLengthW(hwnd) == 0 {
            return TRUE;
        }

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return TRUE;
        }
        let bounds = Bounds {
            x: rect.left,
            y: rect.top,
            w: rect.right - rect.left,
            h: rect.bottom - rect.top,
        };
        if bounds.w < 64 || bounds.h < 48 {
            return TRUE;
        }

        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf) as usize;
        let title = String::from_utf16_lossy(&buf[..n.min(buf.len())]);

        out.push(WindowInfo { bounds, title });
        TRUE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(x: i32, y: i32, w: i32, h: i32, title: &str) -> WindowInfo {
        WindowInfo {
            bounds: Bounds { x, y, w, h },
            title: title.into(),
        }
    }

    #[test]
    fn returns_the_front_most_window_when_they_overlap() {
        let list = vec![
            win(100, 100, 200, 200, "front"),
            win(0, 0, 800, 600, "behind"),
        ];
        assert_eq!(hit_test(&list, 150, 150).unwrap().title, "front");
        assert_eq!(hit_test(&list, 50, 50).unwrap().title, "behind");
    }

    #[test]
    fn a_point_outside_everything_hits_nothing() {
        let list = vec![win(0, 0, 100, 100, "only")];
        assert!(hit_test(&list, 500, 500).is_none());
    }

    #[test]
    fn the_right_and_bottom_edges_are_exclusive() {
        let list = vec![win(0, 0, 100, 100, "only")];
        assert!(hit_test(&list, 99, 99).is_some());
        assert!(hit_test(&list, 100, 100).is_none());
    }

    #[test]
    fn negative_coordinates_work_for_a_monitor_left_of_the_primary() {
        let list = vec![win(-1920, 0, 400, 300, "left screen")];
        assert_eq!(hit_test(&list, -1800, 100).unwrap().title, "left screen");
    }
}
