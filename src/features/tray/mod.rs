use std::sync::{Arc, Mutex};
use std::thread;
use tracing::{error, info};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, LoadIconW, LoadImageW, MSG, PostQuitMessage, RegisterClassExW,
    SetForegroundWindow, ShowWindow, TrackPopupMenu, TranslateMessage, HICON, HMENU,
    IDI_APPLICATION, IMAGE_ICON, LR_LOADFROMFILE, MF_SEPARATOR, MF_STRING, SW_HIDE,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, WM_DESTROY, WNDCLASSEXW, WS_EX_TOOLWINDOW,
    WS_POPUP,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

const WM_TRAY_ICON: u32 = 0x8001;
const WM_APP_BASE: u32 = 0x8000;
const ID_TRAY_SETTINGS: usize = (WM_APP_BASE + 2) as usize;
const ID_TRAY_EXIT: usize = (WM_APP_BASE + 3) as usize;
const TRAY_UID: u32 = 1;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_LBUTTONDBLCLK: u32 = 0x0203;

#[derive(Debug, Clone, PartialEq)]
pub enum TrayEvent {
    ShowSettings,
    Exit,
}

pub struct TrayManager {
    _thread: thread::JoinHandle<()>,
}

impl TrayManager {
    pub fn start(event_sender: Arc<Mutex<Option<TrayEvent>>>, ctx: egui::Context) -> Self {
        let handle = thread::Builder::new()
            .name("tray-thread".to_string())
            .spawn(move || run_tray_loop(event_sender, ctx))
            .expect("Failed to spawn tray thread");
        TrayManager { _thread: handle }
    }
}

struct TrayThreadData {
    event_sender: Arc<Mutex<Option<TrayEvent>>>,
    ctx: egui::Context,
}

unsafe fn load_custom_icon() -> Option<HICON> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets").join("icon.ico"));
        }
    }
    candidates.push(std::path::PathBuf::from("assets/icon.ico"));

    for path in candidates {
        if !path.exists() {
            continue;
        }
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0u16))
            .collect();
        if let Ok(h) = LoadImageW(None, PCWSTR(wide.as_ptr()), IMAGE_ICON, 32, 32, LR_LOADFROMFILE)
        {
            if !h.0.is_null() {
                return Some(HICON(h.0));
            }
        }
    }
    None
}

fn run_tray_loop(event_sender: Arc<Mutex<Option<TrayEvent>>>, ctx: egui::Context) {
    unsafe {
        let class_name: Vec<u16> = "TrayMsgWnd\0".encode_utf16().collect();
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(tray_wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            -10, -10, 1, 1,
            None, None, hinstance, None,
        )
        .unwrap_or(HWND(std::ptr::null_mut()));

        if hwnd.0.is_null() {
            error!("Failed to create tray window");
            return;
        }
        let _ = ShowWindow(hwnd, SW_HIDE);

        TRAY_DATA.with(|cell| {
            *cell.borrow_mut() = Some(TrayThreadData { event_sender, ctx });
        });

        let icon = load_custom_icon().unwrap_or_else(|| {
            LoadIconW(None, IDI_APPLICATION).unwrap_or(HICON(std::ptr::null_mut()))
        });

        let tip_text = "Screen Translator\0";
        let mut tip_arr = [0u16; 128];
        for (i, c) in tip_text.encode_utf16().enumerate().take(127) {
            tip_arr[i] = c;
        }

        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_UID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY_ICON,
            hIcon: icon,
            szTip: tip_arr,
            ..Default::default()
        };

        if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
            info!("Tray icon added");
        } else {
            error!("Shell_NotifyIconW NIM_ADD failed");
        }

        let mut msg = MSG::default();
        loop {
            let result = GetMessageW(&mut msg, None, 0, 0);
            if result.0 <= 0 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

thread_local! {
    static TRAY_DATA: std::cell::RefCell<Option<TrayThreadData>> =
        std::cell::RefCell::new(None);
}

unsafe fn send_tray_event(event: TrayEvent) {
    TRAY_DATA.with(|cell| {
        if let Some(ref data) = *cell.borrow() {
            if let Ok(mut guard) = data.event_sender.lock() {
                *guard = Some(event);
            }
            data.ctx.request_repaint();
        }
    });
}

unsafe fn show_context_menu(hwnd: HWND) {
    let hmenu: HMENU = match CreatePopupMenu() {
        Ok(h) => h,
        Err(e) => {
            error!("CreatePopupMenu failed: {}", e);
            return;
        }
    };

    let settings_text: Vec<u16> = "Settings\0".encode_utf16().collect();
    let exit_text: Vec<u16> = "Exit\0".encode_utf16().collect();

    let _ = AppendMenuW(hmenu, MF_STRING, ID_TRAY_SETTINGS, PCWSTR(settings_text.as_ptr()));
    let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(hmenu, MF_STRING, ID_TRAY_EXIT, PCWSTR(exit_text.as_ptr()));

    let mut pt = POINT { x: 0, y: 0 };
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);

    let cmd = TrackPopupMenu(
        hmenu,
        TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RETURNCMD,
        pt.x, pt.y, 0, hwnd, None,
    );
    let _ = DestroyMenu(hmenu);

    match cmd.0 as usize {
        ID_TRAY_SETTINGS => send_tray_event(TrayEvent::ShowSettings),
        ID_TRAY_EXIT => send_tray_event(TrayEvent::Exit),
        _ => {}
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAY_ICON => {
            let event_id = (lparam.0 as u32) & 0xFFFF;
            match event_id {
                WM_RBUTTONUP => show_context_menu(hwnd),
                WM_LBUTTONDBLCLK => send_tray_event(TrayEvent::ShowSettings),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}
