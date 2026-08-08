//! Global hotkeys.
//!
//! Four bindings now instead of one, so the tray menu's Область / Окно / Весь
//! экран entries each have a shortcut. Two behavioural fixes over the original:
//!
//! * events go through a channel rather than a shared `Option`, which used to
//!   drop a press if two arrived between UI frames;
//! * rebinding validates the new combination **before** giving up the old one.
//!   Previously, binding a combination already owned by another application
//!   unregistered the working hotkey and then failed, leaving the user with no
//!   way to invoke the app at all.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use tracing::{error, info, warn};

use crate::entities::settings::{HotkeyAction, Hotkeys};

/// Which bindings the OS actually accepted. The settings window turns a `false`
/// here into a visible warning next to the row.
pub type Accepted = Arc<Mutex<Vec<(HotkeyAction, bool)>>>;

pub struct HotkeyManager {
    events: Receiver<HotkeyAction>,
    accepted: Accepted,
    #[cfg(windows)]
    control: win::Control,
    _thread: Option<thread::JoinHandle<()>>,
}

impl HotkeyManager {
    pub fn start(hotkeys: Hotkeys, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let accepted: Accepted = Arc::new(Mutex::new(Vec::new()));

        #[cfg(windows)]
        {
            let (control, handle) = win::spawn(hotkeys, tx, ctx, Arc::clone(&accepted));
            HotkeyManager {
                events: rx,
                accepted,
                control,
                _thread: Some(handle),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (hotkeys, tx, ctx);
            warn!("Global hotkeys are not implemented on this platform yet");
            HotkeyManager {
                events: rx,
                accepted,
                _thread: None,
            }
        }
    }

    /// Drains everything queued since the last frame.
    pub fn poll(&self) -> Vec<HotkeyAction> {
        self.events.try_iter().collect()
    }

    pub fn update(&self, hotkeys: Hotkeys) {
        #[cfg(windows)]
        self.control.update(hotkeys);
        #[cfg(not(windows))]
        let _ = hotkeys;
    }

    pub fn rejected(&self) -> Vec<HotkeyAction> {
        self.accepted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, ok)| !ok)
            .map(|(a, _)| *a)
            .collect()
    }

    pub fn shutdown(&self) {
        #[cfg(windows)]
        self.control.shutdown();
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(windows)]
mod win {
    use super::*;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PostMessageW, PostQuitMessage, RegisterClassExW, TranslateMessage, HWND_MESSAGE, MSG,
        WM_APP, WM_CLOSE, WM_DESTROY, WM_HOTKEY, WNDCLASSEXW,
    };

    const WM_UPDATE_HOTKEYS: u32 = WM_APP + 1;
    /// Scratch id used to test-drive a binding before committing to it.
    const PROBE_ID: i32 = 900;

    pub struct Control {
        hwnd: Arc<Mutex<Option<usize>>>,
        pending: Arc<Mutex<Option<Hotkeys>>>,
    }

    impl Control {
        pub fn update(&self, hotkeys: Hotkeys) {
            *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(hotkeys);
            self.post(WM_UPDATE_HOTKEYS);
        }

        pub fn shutdown(&self) {
            self.post(WM_CLOSE);
        }

        fn post(&self, msg: u32) {
            let guard = self.hwnd.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(raw) = *guard {
                unsafe {
                    let _ = PostMessageW(HWND(raw as *mut _), msg, WPARAM(0), LPARAM(0));
                }
            }
        }
    }

    struct ThreadState {
        tx: Sender<HotkeyAction>,
        ctx: egui::Context,
        pending: Arc<Mutex<Option<Hotkeys>>>,
        accepted: Accepted,
        current: Hotkeys,
    }

    thread_local! {
        static STATE: std::cell::RefCell<Option<ThreadState>> =
            const { std::cell::RefCell::new(None) };
    }

    pub fn spawn(
        hotkeys: Hotkeys,
        tx: Sender<HotkeyAction>,
        ctx: egui::Context,
        accepted: Accepted,
    ) -> (Control, thread::JoinHandle<()>) {
        let hwnd_slot: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
        let pending: Arc<Mutex<Option<Hotkeys>>> = Arc::new(Mutex::new(None));

        let control = Control {
            hwnd: Arc::clone(&hwnd_slot),
            pending: Arc::clone(&pending),
        };

        let handle = thread::Builder::new()
            .name("hotkey".to_string())
            .spawn(move || run(hotkeys, tx, ctx, hwnd_slot, pending, accepted))
            .expect("Failed to spawn hotkey thread");

        (control, handle)
    }

    fn run(
        hotkeys: Hotkeys,
        tx: Sender<HotkeyAction>,
        ctx: egui::Context,
        hwnd_slot: Arc<Mutex<Option<usize>>>,
        pending: Arc<Mutex<Option<Hotkeys>>>,
        accepted: Accepted,
    ) {
        unsafe {
            let class_name: Vec<u16> = "SakuraHotkeyWnd\0".encode_utf16().collect();
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
                Default::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                Default::default(),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                None,
                hinstance,
                None,
            )
            .unwrap_or(HWND(std::ptr::null_mut()));

            if hwnd.0.is_null() {
                error!("Could not create the message window for hotkeys");
                return;
            }

            *hwnd_slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(hwnd.0 as usize);

            STATE.with(|cell| {
                *cell.borrow_mut() = Some(ThreadState {
                    tx,
                    ctx,
                    pending,
                    accepted: Arc::clone(&accepted),
                    current: Hotkeys {
                        region: crate::entities::settings::Hotkey::unbound(),
                        window: crate::entities::settings::Hotkey::unbound(),
                        fullscreen: crate::entities::settings::Hotkey::unbound(),
                        repeat: crate::entities::settings::Hotkey::unbound(),
                    },
                });
            });

            apply(hwnd, hotkeys);

            let mut msg = MSG::default();
            loop {
                let r = GetMessageW(&mut msg, hwnd, 0, 0);
                if r.0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // The original never got here: nothing ever posted WM_QUIT, so the
            // hotkeys stayed registered until the process died.
            unregister_all(hwnd);
            let _ = DestroyWindow(hwnd);
            *hwnd_slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
            info!("Hotkey thread stopped");
        }
    }

    fn slot_id(action: HotkeyAction) -> i32 {
        match action {
            HotkeyAction::Region => 1,
            HotkeyAction::Window => 2,
            HotkeyAction::FullScreen => 3,
            HotkeyAction::Repeat => 4,
        }
    }

    fn action_for(id: i32) -> Option<HotkeyAction> {
        HotkeyAction::all().into_iter().find(|a| slot_id(*a) == id)
    }

    unsafe fn unregister_all(hwnd: HWND) {
        for action in HotkeyAction::all() {
            let _ = UnregisterHotKey(hwnd, slot_id(action));
        }
    }

    /// Applies a new binding set, keeping any previous binding whose replacement
    /// the OS refuses.
    unsafe fn apply(hwnd: HWND, wanted: Hotkeys) {
        STATE.with(|cell| {
            let mut guard = cell.borrow_mut();
            let Some(state) = guard.as_mut() else { return };

            let mut results = Vec::new();
            let mut committed = state.current;

            for (action, want) in wanted.all() {
                let id = slot_id(action);
                let have = *committed.slot_mut(action);

                if have == want {
                    // Nothing to do. Note this counts as success even for an
                    // unbound action: "no shortcut wanted, no shortcut set" is
                    // the desired state, not a failure to register one.
                    results.push((action, true));
                    continue;
                }

                if !want.is_bound() {
                    let _ = UnregisterHotKey(hwnd, id);
                    *committed.slot_mut(action) = want;
                    results.push((action, true));
                    continue;
                }

                // Probe first: if this combination belongs to another app, the
                // probe fails and the working binding is left untouched.
                let probe =
                    RegisterHotKey(hwnd, PROBE_ID, HOT_KEY_MODIFIERS(want.modifiers), want.key);
                if probe.is_err() {
                    warn!(
                        action = ?action,
                        combo = %want.display(),
                        "Combination is taken by another application; keeping the previous one"
                    );
                    results.push((action, false));
                    continue;
                }
                let _ = UnregisterHotKey(hwnd, PROBE_ID);

                let _ = UnregisterHotKey(hwnd, id);
                match RegisterHotKey(hwnd, id, HOT_KEY_MODIFIERS(want.modifiers), want.key) {
                    Ok(()) => {
                        info!(action = ?action, combo = %want.display(), "Hotkey registered");
                        *committed.slot_mut(action) = want;
                        results.push((action, true));
                    }
                    Err(e) => {
                        error!(action = ?action, error = ?e, "RegisterHotKey failed");
                        // Put the old one back rather than leaving nothing.
                        if have.is_bound() {
                            let _ = RegisterHotKey(
                                hwnd,
                                id,
                                HOT_KEY_MODIFIERS(have.modifiers),
                                have.key,
                            );
                        }
                        results.push((action, false));
                    }
                }
            }

            state.current = committed;
            *state.accepted.lock().unwrap_or_else(|e| e.into_inner()) = results;
        });
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_HOTKEY => {
                if let Some(action) = action_for(wparam.0 as i32) {
                    STATE.with(|cell| {
                        if let Some(state) = cell.borrow().as_ref() {
                            info!(action = ?action, "Hotkey fired");
                            // A closed receiver means the app is shutting down.
                            let _ = state.tx.send(action);
                            state.ctx.request_repaint();
                        }
                    });
                }
                LRESULT(0)
            }
            WM_UPDATE_HOTKEYS => {
                let wanted = STATE.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .and_then(|s| s.pending.lock().unwrap_or_else(|e| e.into_inner()).take())
                });
                if let Some(wanted) = wanted {
                    apply(hwnd, wanted);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
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
