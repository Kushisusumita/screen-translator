use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tracing::{error, info};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG,
    PostMessageW, PostQuitMessage, RegisterClassExW, TranslateMessage, HWND_MESSAGE,
    WM_DESTROY, WM_HOTKEY, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::PCWSTR;

const HOTKEY_ID: i32 = 1;
// Application-specific message to update the registered hotkey at runtime
const WM_UPDATE_HOTKEY: u32 = 0x8001;

pub struct HotkeyManager {
    _thread: thread::JoinHandle<()>,
    shared_hwnd: Arc<Mutex<Option<usize>>>,     // HWND cast to usize (Send-safe handle)
    pending_hotkey: Arc<Mutex<Option<(u32, u32)>>>, // (modifiers, vk) awaiting registration
}

impl HotkeyManager {
    pub fn start(
        modifiers: u32,
        vk: u32,
        fired: Arc<AtomicBool>,
        ctx: egui::Context,
    ) -> Self {
        let shared_hwnd: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
        let pending_hotkey: Arc<Mutex<Option<(u32, u32)>>> = Arc::new(Mutex::new(None));

        let hwnd_clone    = Arc::clone(&shared_hwnd);
        let pending_clone = Arc::clone(&pending_hotkey);

        let handle = thread::Builder::new()
            .name("hotkey-thread".to_string())
            .spawn(move || {
                run_hotkey_loop(modifiers, vk, fired, ctx, hwnd_clone, pending_clone);
            })
            .expect("Failed to spawn hotkey thread");

        HotkeyManager { _thread: handle, shared_hwnd, pending_hotkey }
    }

    /// Re-register the hotkey with new modifiers/key without restarting the thread.
    pub fn update_hotkey(&self, modifiers: u32, vk: u32) {
        if let Ok(mut g) = self.pending_hotkey.lock() {
            *g = Some((modifiers, vk));
        }
        if let Ok(g) = self.shared_hwnd.lock() {
            if let Some(raw) = *g {
                unsafe {
                    let hwnd = HWND(raw as *mut _);
                    let _ = PostMessageW(hwnd, WM_UPDATE_HOTKEY, WPARAM(0), LPARAM(0));
                }
            }
        }
    }
}

// ── Thread-local state for the hotkey window proc ────────────────────────────

struct HotkeyThreadData {
    fired:          Arc<AtomicBool>,
    ctx:            egui::Context,
    pending_hotkey: Arc<Mutex<Option<(u32, u32)>>>,
}

thread_local! {
    static HOTKEY_DATA: std::cell::RefCell<Option<HotkeyThreadData>> =
        std::cell::RefCell::new(None);
}

// ── Thread entry point ────────────────────────────────────────────────────────

fn run_hotkey_loop(
    modifiers: u32,
    vk: u32,
    fired: Arc<AtomicBool>,
    ctx: egui::Context,
    shared_hwnd: Arc<Mutex<Option<usize>>>,
    pending_hotkey: Arc<Mutex<Option<(u32, u32)>>>,
) {
    unsafe {
        let class_name: Vec<u16> = "HotkeyMsgWnd\0".encode_utf16().collect();
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(hotkey_wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPEDWINDOW,
            0, 0, 0, 0,
            HWND_MESSAGE,
            None,
            hinstance,
            None,
        ).unwrap_or(HWND(std::ptr::null_mut()));

        if hwnd.0.is_null() {
            error!("Failed to create message-only window for hotkey");
            return;
        }

        // Publish the HWND so the main thread can post WM_UPDATE_HOTKEY to us.
        if let Ok(mut g) = shared_hwnd.lock() {
            *g = Some(hwnd.0 as usize);
        }

        match RegisterHotKey(hwnd, HOTKEY_ID, HOT_KEY_MODIFIERS(modifiers), vk) {
            Ok(_)  => info!("Hotkey registered (mods={:#x} vk={:#x})", modifiers, vk),
            Err(e) => error!("RegisterHotKey failed: {:?}", e),
        }

        HOTKEY_DATA.with(|cell| {
            *cell.borrow_mut() = Some(HotkeyThreadData { fired, ctx, pending_hotkey });
        });

        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, hwnd, 0, 0);
            if r.0 <= 0 { break; }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ── Window procedure ──────────────────────────────────────────────────────────

unsafe extern "system" fn hotkey_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            if wparam.0 as i32 == HOTKEY_ID {
                HOTKEY_DATA.with(|cell| {
                    if let Some(ref d) = *cell.borrow() {
                        d.fired.store(true, Ordering::SeqCst);
                        d.ctx.request_repaint();
                        info!("Hotkey fired!");
                    }
                });
            }
            LRESULT(0)
        }

        WM_UPDATE_HOTKEY => {
            HOTKEY_DATA.with(|cell| {
                if let Some(ref d) = *cell.borrow() {
                    if let Ok(mut g) = d.pending_hotkey.lock() {
                        if let Some((new_mods, new_vk)) = g.take() {
                            let _ = UnregisterHotKey(hwnd, HOTKEY_ID);
                            match RegisterHotKey(hwnd, HOTKEY_ID, HOT_KEY_MODIFIERS(new_mods), new_vk) {
                                Ok(_)  => info!("Hotkey updated (mods={:#x} vk={:#x})", new_mods, new_vk),
                                Err(e) => error!("Re-register hotkey failed: {:?}", e),
                            }
                        }
                    }
                }
            });
            LRESULT(0)
        }

        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
