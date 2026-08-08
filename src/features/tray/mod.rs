//! System tray icon and its menu.
//!
//! Fixes over the original, all of them things users actually hit:
//!
//! * the icon is now removed on exit. `Shell_NotifyIconW(NIM_DELETE)` sat after
//!   a message loop that nothing ever asked to quit, so it never ran and every
//!   exit left a ghost icon in the tray until you hovered over it;
//! * `TaskbarCreated` is handled, so the icon comes back when Explorer restarts
//!   instead of disappearing for the rest of the session;
//! * events go through a channel — the old shared `Option` meant a click could
//!   overwrite an unread one, and "Exit" was the one most likely to be lost;
//! * the menu carries the capture modes from the design, with their shortcuts.

use std::sync::mpsc::{channel, Receiver};
use std::thread;

use tracing::info;

use crate::entities::settings::{CaptureMode, Hotkeys};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    Capture(CaptureMode),
    ShowSettings,
    Exit,
}

pub struct TrayManager {
    events: Receiver<TrayEvent>,
    #[cfg(windows)]
    control: win::Control,
    _thread: Option<thread::JoinHandle<()>>,
}

impl TrayManager {
    pub fn start(hotkeys: Hotkeys, ctx: egui::Context, visible: bool) -> Self {
        let (tx, rx) = channel();

        #[cfg(windows)]
        {
            let (control, handle) = win::spawn(hotkeys, tx, ctx, visible);
            TrayManager {
                events: rx,
                control,
                _thread: Some(handle),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (hotkeys, tx, ctx, visible);
            tracing::warn!("Tray icon is not implemented on this platform yet");
            TrayManager {
                events: rx,
                _thread: None,
            }
        }
    }

    pub fn poll(&self) -> Vec<TrayEvent> {
        self.events.try_iter().collect()
    }

    /// Keeps the shortcut labels in the menu in step with the settings.
    pub fn update_hotkeys(&self, hotkeys: Hotkeys) {
        #[cfg(windows)]
        self.control.update_hotkeys(hotkeys);
        #[cfg(not(windows))]
        let _ = hotkeys;
    }

    /// Spins the tray icon while a translation is in flight, and lets it settle
    /// back when the work is done.
    pub fn set_busy(&self, busy: bool) {
        #[cfg(windows)]
        self.control.set_busy(busy);
        #[cfg(not(windows))]
        let _ = busy;
    }

    /// Adds or removes the icon without restarting the thread. The hotkeys keep
    /// working either way.
    pub fn set_visible(&self, visible: bool) {
        #[cfg(windows)]
        self.control.set_visible(visible);
        #[cfg(not(windows))]
        let _ = visible;
    }

    pub fn shutdown(&self) {
        #[cfg(windows)]
        self.control.shutdown();
    }
}

impl Drop for TrayManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The window icon, drawn from the brand mark.
///
/// Previously this came from `assets/icon.ico` next to the executable, which
/// meant a missing file left the app with the generic Windows icon — and under
/// autostart, whose working directory is `C:\Windows\system32`, the relative
/// fallback never resolved at all. Drawing it removes the failure mode.
pub fn app_icon(size: u32) -> egui::IconData {
    egui::IconData {
        rgba: crate::shared::mark::rasterise(size),
        width: size,
        height: size,
    }
}

#[cfg(windows)]
mod win {
    use super::*;

    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};

    use tracing::{error, warn};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, TRUE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICONDATAW_0, NOTIFYICON_VERSION_4,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
        GetSystemMetrics, KillTimer, LoadIconW, PostMessageW, PostQuitMessage, RegisterClassExW,
        RegisterWindowMessageW, SetForegroundWindow, SetTimer, TrackPopupMenu, TranslateMessage,
        HICON, HMENU, ICONINFO, IDI_APPLICATION, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON,
        TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE,
        WM_DESTROY, WM_LBUTTONDBLCLK, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW,
        WS_EX_TOOLWINDOW, WS_POPUP,
    };

    const WM_TRAY_ICON: u32 = WM_APP + 10;
    const WM_UPDATE_HOTKEYS: u32 = WM_APP + 11;
    const WM_SET_VISIBLE: u32 = WM_APP + 12;
    const WM_SET_BUSY: u32 = WM_APP + 13;

    /// Animation timer. ~16 fps is smooth enough for a 16 px mark and costs
    /// almost nothing — each frame is a few thousand coverage samples.
    const ANIM_TIMER: usize = 1;
    const ANIM_INTERVAL_MS: u32 = 60;
    const ANIM_DT: f32 = ANIM_INTERVAL_MS as f32 / 1000.0;
    const TRAY_UID: u32 = 1;

    const ID_REGION: usize = 101;
    const ID_WINDOW: usize = 102;
    const ID_FULLSCREEN: usize = 103;
    const ID_SETTINGS: usize = 110;
    const ID_EXIT: usize = 111;

    pub struct Control {
        hwnd: Arc<Mutex<Option<usize>>>,
        pending: Arc<Mutex<Option<Hotkeys>>>,
    }

    impl Control {
        pub fn update_hotkeys(&self, hotkeys: Hotkeys) {
            *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(hotkeys);
            self.post(WM_UPDATE_HOTKEYS);
        }

        pub fn set_visible(&self, visible: bool) {
            self.post_with(WM_SET_VISIBLE, WPARAM(usize::from(visible)));
        }

        pub fn set_busy(&self, busy: bool) {
            self.post_with(WM_SET_BUSY, WPARAM(usize::from(busy)));
        }

        pub fn shutdown(&self) {
            self.post(WM_CLOSE);
        }

        fn post(&self, msg: u32) {
            self.post_with(msg, WPARAM(0));
        }

        fn post_with(&self, msg: u32, wparam: WPARAM) {
            let guard = self.hwnd.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(raw) = *guard {
                unsafe {
                    let _ = PostMessageW(HWND(raw as *mut _), msg, wparam, LPARAM(0));
                }
            }
        }
    }

    struct ThreadState {
        tx: Sender<TrayEvent>,
        ctx: egui::Context,
        hotkeys: Hotkeys,
        pending: Arc<Mutex<Option<Hotkeys>>>,
        /// The icon currently handed to the shell. Replaced, and the old one
        /// destroyed, on every animation frame.
        icon: HICON,
        taskbar_created: u32,
        /// Set once NIM_ADD succeeds so shutdown knows there is something to remove.
        added: bool,
        /// The working animation, shared with the settings window.
        spin: crate::ui::spin::Spin,
        busy: bool,
    }

    thread_local! {
        static STATE: std::cell::RefCell<Option<ThreadState>> =
            const { std::cell::RefCell::new(None) };
    }

    pub fn spawn(
        hotkeys: Hotkeys,
        tx: Sender<TrayEvent>,
        ctx: egui::Context,
        visible: bool,
    ) -> (Control, thread::JoinHandle<()>) {
        let hwnd_slot: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
        let pending: Arc<Mutex<Option<Hotkeys>>> = Arc::new(Mutex::new(None));

        let control = Control {
            hwnd: Arc::clone(&hwnd_slot),
            pending: Arc::clone(&pending),
        };

        let handle = thread::Builder::new()
            .name("tray".to_string())
            .spawn(move || run(hotkeys, tx, ctx, hwnd_slot, pending, visible))
            .expect("Failed to spawn tray thread");

        (control, handle)
    }

    /// Builds the tray icon at the size the shell actually asks for, rather
    /// than handing Windows one large bitmap to squash down to 16 px.
    unsafe fn load_icon() -> HICON {
        build_icon(0.0, 1.0)
    }

    unsafe fn build_icon(turns: f32, scale: f32) -> HICON {
        let size = GetSystemMetrics(SM_CXSMICON).max(16) as u32;
        match icon_from_mark(size, turns, scale) {
            Some(icon) => icon,
            None => {
                warn!("Could not build the tray icon; falling back to the system one");
                LoadIconW(None, IDI_APPLICATION).unwrap_or(HICON(std::ptr::null_mut()))
            }
        }
    }

    /// Advances the animation one tick and pushes the new frame to the shell.
    ///
    /// When the work is over the spin is not cut off: it carries on to the next
    /// fifth of a turn, where the mark's five-fold symmetry makes it identical
    /// to rest, and only then stops.
    unsafe fn tick_animation(hwnd: HWND) {
        let frame = STATE.with(|cell| {
            let mut guard = cell.borrow_mut();
            let state = guard.as_mut()?;
            let moving = state.spin.advance(ANIM_DT, state.busy);
            Some((state.spin.turns(), state.spin.scale(), !moving))
        });

        let Some((turns, scale, finished)) = frame else {
            return;
        };

        set_icon(hwnd, build_icon(turns, scale));

        if finished {
            let _ = KillTimer(hwnd, ANIM_TIMER);
        }
    }

    /// Swaps in a new icon and releases the one it replaces.
    unsafe fn set_icon(hwnd: HWND, icon: HICON) {
        STATE.with(|cell| {
            let mut guard = cell.borrow_mut();
            let Some(state) = guard.as_mut() else { return };
            let old = state.icon;
            state.icon = icon;
            if state.added {
                let nid = notify_data(hwnd, icon);
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
            if !old.0.is_null() && old != icon {
                let _ = DestroyIcon(old);
            }
        });
    }

    /// Turns straight-alpha RGBA into an `HICON`.
    ///
    /// An icon is a colour bitmap plus a mask. With a 32-bit colour bitmap the
    /// alpha channel does the work, so the mask is left empty.
    unsafe fn icon_from_mark(size: u32, turns: f32, scale: f32) -> Option<HICON> {
        let rgba = crate::shared::mark::rasterise_with(size, turns, scale);
        let (w, h) = (size as i32, size as i32);

        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative: top-down, matching the rasteriser's row order.
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            bmiColors: [Default::default(); 1],
        };

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let colour = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if colour.0.is_null() || bits.is_null() {
            return None;
        }

        // GDI wants BGRA.
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, rgba.len());
        for (out, px) in dst.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            out[0] = px[2];
            out[1] = px[1];
            out[2] = px[0];
            out[3] = px[3];
        }

        // 1 bpp, rows padded to a WORD, all zero.
        let mask_stride = (size as usize).div_ceil(16) * 2;
        let mask_bits = vec![0u8; mask_stride * size as usize];
        let mask = CreateBitmap(w, h, 1, 1, Some(mask_bits.as_ptr() as *const _));
        if mask.0.is_null() {
            let _ = DeleteObject(colour);
            return None;
        }

        let info = ICONINFO {
            fIcon: TRUE,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: colour,
        };
        let icon = CreateIconIndirect(&info).ok();

        // CreateIconIndirect copies the bitmaps; ours are ours to clean up.
        let _ = DeleteObject(colour);
        let _ = DeleteObject(mask);

        icon
    }

    fn notify_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
        let mut tip = [0u16; 128];
        for (i, c) in "Sakura Screen Translator"
            .encode_utf16()
            .take(127)
            .enumerate()
        {
            tip[i] = c;
        }
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_UID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY_ICON,
            hIcon: icon,
            szTip: tip,
            Anonymous: NOTIFYICONDATAW_0 {
                uVersion: NOTIFYICON_VERSION_4,
            },
            ..Default::default()
        }
    }

    fn run(
        hotkeys: Hotkeys,
        tx: Sender<TrayEvent>,
        ctx: egui::Context,
        hwnd_slot: Arc<Mutex<Option<usize>>>,
        pending: Arc<Mutex<Option<Hotkeys>>>,
        visible: bool,
    ) {
        unsafe {
            let class_name: Vec<u16> = "SakuraTrayWnd\0".encode_utf16().collect();
            let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&wc);

            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                -10,
                -10,
                1,
                1,
                None,
                None,
                hinstance,
                None,
            )
            .unwrap_or(HWND(std::ptr::null_mut()));

            if hwnd.0.is_null() {
                error!("Could not create the tray window");
                return;
            }
            *hwnd_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(hwnd.0 as usize);

            // Explorer broadcasts this when the taskbar is recreated after a
            // crash or a restart; without it the icon is gone for good.
            let taskbar_created = RegisterWindowMessageW(PCWSTR(
                "TaskbarCreated\0"
                    .encode_utf16()
                    .collect::<Vec<_>>()
                    .as_ptr(),
            ));

            let icon = load_icon();

            STATE.with(|cell| {
                *cell.borrow_mut() = Some(ThreadState {
                    tx,
                    ctx,
                    hotkeys,
                    pending,
                    icon,
                    taskbar_created,
                    added: false,
                    spin: crate::ui::spin::Spin::new(),
                    busy: false,
                });
            });

            if visible {
                add_icon(hwnd);
            } else {
                info!("Starting without a tray icon, as configured");
            }

            let mut msg = MSG::default();
            loop {
                let r = GetMessageW(&mut msg, None, 0, 0);
                if r.0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            remove_icon(hwnd);
            let _ = KillTimer(hwnd, ANIM_TIMER);
            STATE.with(|cell| {
                if let Some(state) = cell.borrow().as_ref() {
                    if !state.icon.0.is_null() {
                        let _ = DestroyIcon(state.icon);
                    }
                }
            });
            let _ = DestroyWindow(hwnd);
            *hwnd_slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
            info!("Tray thread stopped");
        }
    }

    unsafe fn add_icon(hwnd: HWND) {
        STATE.with(|cell| {
            let mut guard = cell.borrow_mut();
            let Some(state) = guard.as_mut() else { return };
            let nid = notify_data(hwnd, state.icon);
            if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
                state.added = true;
                info!("Tray icon added");
            } else {
                // Not fatal — hotkeys still work — but the user needs to know
                // why there is no icon to right-click.
                error!("Shell_NotifyIconW(NIM_ADD) failed; the app is running without a tray icon");
            }
        });
    }

    unsafe fn remove_icon(hwnd: HWND) {
        STATE.with(|cell| {
            let mut guard = cell.borrow_mut();
            let Some(state) = guard.as_mut() else { return };
            if !state.added {
                return;
            }
            let nid = notify_data(hwnd, state.icon);
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            state.added = false;
            info!("Tray icon removed");
        });
    }

    unsafe fn send(event: TrayEvent) {
        STATE.with(|cell| {
            if let Some(state) = cell.borrow().as_ref() {
                let _ = state.tx.send(event);
                state.ctx.request_repaint();
            }
        });
    }

    unsafe fn show_menu(hwnd: HWND) {
        let hotkeys = STATE.with(|cell| cell.borrow().as_ref().map(|s| s.hotkeys));
        let Some(hotkeys) = hotkeys else { return };

        let menu: HMENU = match CreatePopupMenu() {
            Ok(h) => h,
            Err(e) => {
                error!(error = %e, "CreatePopupMenu failed");
                return;
            }
        };

        // A tab before the shortcut makes Windows right-align it, the way every
        // native menu does.
        let item = |label: &str, hk: &crate::entities::settings::Hotkey| -> Vec<u16> {
            let text = if hk.is_bound() {
                format!("{label}\t{}", hk.display())
            } else {
                label.to_string()
            };
            text.encode_utf16().chain(std::iter::once(0)).collect()
        };

        let region = item(CaptureMode::Region.label_menu(), &hotkeys.region);
        let window = item(CaptureMode::Window.label_menu(), &hotkeys.window);
        let full = item(CaptureMode::FullScreen.label_menu(), &hotkeys.fullscreen);
        let settings: Vec<u16> = "Параметры\0".encode_utf16().collect();
        let exit: Vec<u16> = "Выход\0".encode_utf16().collect();

        let _ = AppendMenuW(menu, MF_STRING, ID_REGION, PCWSTR(region.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, ID_WINDOW, PCWSTR(window.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, ID_FULLSCREEN, PCWSTR(full.as_ptr()));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, PCWSTR(settings.as_ptr()));
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, PCWSTR(exit.as_ptr()));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);

        let cmd = TrackPopupMenu(
            menu,
            TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        );

        // Documented requirement: without this the menu can refuse to dismiss
        // when the next click lands outside it.
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);

        match cmd.0 as usize {
            ID_REGION => send(TrayEvent::Capture(CaptureMode::Region)),
            ID_WINDOW => send(TrayEvent::Capture(CaptureMode::Window)),
            ID_FULLSCREEN => send(TrayEvent::Capture(CaptureMode::FullScreen)),
            ID_SETTINGS => send(TrayEvent::ShowSettings),
            ID_EXIT => send(TrayEvent::Exit),
            _ => {}
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let taskbar_created = STATE.with(|cell| cell.borrow().as_ref().map(|s| s.taskbar_created));

        if Some(msg) == taskbar_created && msg != 0 {
            info!("Explorer restarted; re-adding the tray icon");
            STATE.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    state.added = false;
                }
            });
            add_icon(hwnd);
            return LRESULT(0);
        }

        match msg {
            WM_TRAY_ICON => {
                match (lparam.0 as u32) & 0xFFFF {
                    WM_RBUTTONUP => show_menu(hwnd),
                    WM_LBUTTONDBLCLK => send(TrayEvent::ShowSettings),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_SET_BUSY => {
                let busy = wparam.0 != 0;
                let changed = STATE.with(|cell| {
                    let mut guard = cell.borrow_mut();
                    let Some(state) = guard.as_mut() else {
                        return false;
                    };
                    let changed = state.busy != busy;
                    state.busy = busy;
                    changed
                });
                if changed {
                    // One timer covers both directions: it keeps running after
                    // "not busy" until the mark has coasted to a resting angle.
                    SetTimer(hwnd, ANIM_TIMER, ANIM_INTERVAL_MS, None);
                }
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == ANIM_TIMER {
                    tick_animation(hwnd);
                }
                LRESULT(0)
            }
            WM_SET_VISIBLE => {
                if wparam.0 == 0 {
                    remove_icon(hwnd);
                } else {
                    add_icon(hwnd);
                }
                LRESULT(0)
            }
            WM_UPDATE_HOTKEYS => {
                STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        if let Some(next) = state
                            .pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .take()
                        {
                            state.hotkeys = next;
                        }
                    }
                });
                LRESULT(0)
            }
            WM_CLOSE => {
                remove_icon(hwnd);
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
