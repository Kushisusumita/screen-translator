use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    TTTOOLINFOW, TOOLTIPS_CLASSW, TTF_ABSOLUTE, TTF_TRACK, TTS_ALWAYSTIP, TTS_NOPREFIX,
    TTM_ADDTOOLW, TTM_SETTIPBKCOLOR, TTM_SETTIPTEXTCOLOR, TTM_SETMAXTIPWIDTH,
    TTM_TRACKACTIVATE, TTM_TRACKPOSITION,
};
use windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SendMessageW, SetWindowPos,
    CW_USEDEFAULT, HWND_TOPMOST, SWP_NOMOVE, SWP_NOSIZE,
    WS_EX_TOPMOST, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::PCWSTR;

/// A native Win32 tracking tooltip shown near the selected region.
/// The tooltip stays visible until dropped.
pub struct NativeTooltip {
    hwnd: HWND,
    hwnd_parent: HWND,
    // Keeps the UTF-16 text buffer alive while the tooltip control references it.
    _text: Vec<u16>,
}

// HWND is a raw pointer but NativeTooltip is created/destroyed on the same thread.
unsafe impl Send for NativeTooltip {}

impl NativeTooltip {
    /// Show a native Win32 tooltip at screen position (`x`, `y`) with `text`.
    /// Returns `None` if the Win32 API calls fail.
    pub fn show(x: i32, y: i32, text: &str) -> Option<Self> {
        unsafe {
            let hinstance = GetModuleHandleW(PCWSTR::null()).ok()?;

            // Minimal invisible parent window — TOOLTIPS_CLASS32 requires a parent.
            let static_cls: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let hwnd_parent = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(static_cls.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                -2, -2, 1, 1,
                None, None, hinstance, None,
            )
            .unwrap_or(HWND(std::ptr::null_mut()));

            if hwnd_parent.0.is_null() {
                return None;
            }

            // Create the TOOLTIPS_CLASS32 window.
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                PCWSTR::null(),
                WS_POPUP | WINDOW_STYLE(TTS_ALWAYSTIP) | WINDOW_STYLE(TTS_NOPREFIX),
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                hwnd_parent,
                None,
                hinstance,
                None,
            )
            .unwrap_or(HWND(std::ptr::null_mut()));

            if hwnd.0.is_null() {
                let _ = DestroyWindow(hwnd_parent);
                return None;
            }

            // Allow multi-line text by setting a maximum width (480 logical pixels).
            SendMessageW(hwnd, TTM_SETMAXTIPWIDTH, WPARAM(0), LPARAM(480));

            // Dark theme: rgb(18,18,28) background, white text.
            // COLORREF format is 0x00BBGGRR.
            // rgb(18,18,28) → R=0x12, G=0x12, B=0x1c → COLORREF = 0x001c1212
            SendMessageW(hwnd, TTM_SETTIPBKCOLOR, WPARAM(0x001c1212_usize), LPARAM(0));
            SendMessageW(hwnd, TTM_SETTIPTEXTCOLOR, WPARAM(0x00FFFFFF_usize), LPARAM(0));

            // Build null-terminated UTF-16 text.
            let mut text_wide: Vec<u16> = text.encode_utf16().collect();
            text_wide.push(0u16);

            let mut ti = TTTOOLINFOW {
                cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_TRACK | TTF_ABSOLUTE,
                hwnd: hwnd_parent,
                uId: 1,
                rect: RECT::default(),
                lpszText: windows::core::PWSTR(text_wide.as_mut_ptr()),
                ..Default::default()
            };

            // Register the tool.
            SendMessageW(
                hwnd,
                TTM_ADDTOOLW,
                WPARAM(0),
                LPARAM(&mut ti as *mut _ as isize),
            );

            // Position: MAKELPARAM(x, y) — x in low word, y in high word.
            let pos = (x as u16 as u32) | ((y as u16 as u32) << 16);
            SendMessageW(hwnd, TTM_TRACKPOSITION, WPARAM(0), LPARAM(pos as isize));

            // Activate the tracking tooltip.
            SendMessageW(
                hwnd,
                TTM_TRACKACTIVATE,
                WPARAM(1),
                LPARAM(&mut ti as *mut _ as isize),
            );

            // Ensure it stays on top of all other windows.
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);

            Some(NativeTooltip {
                hwnd,
                hwnd_parent,
                _text: text_wide,
            })
        }
    }
}

impl Drop for NativeTooltip {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.0.is_null() {
                let _ = DestroyWindow(self.hwnd);
            }
            if !self.hwnd_parent.0.is_null() {
                let _ = DestroyWindow(self.hwnd_parent);
            }
        }
    }
}
