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
    #[cfg(not(windows))]
    control: portable::Control,
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
            let (control, handle) = portable::spawn(hotkeys, tx, ctx, Arc::clone(&accepted));
            HotkeyManager {
                events: rx,
                accepted,
                control,
                _thread: handle,
            }
        }
    }

    /// Drains everything queued since the last frame.
    pub fn poll(&self) -> Vec<HotkeyAction> {
        self.events.try_iter().collect()
    }

    pub fn update(&self, hotkeys: Hotkeys) {
        self.control.update(hotkeys);
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
        self.control.shutdown();
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// macOS and Linux, on `global-hotkey` — Carbon `RegisterEventHotKey` on macOS,
/// `XGrabKey` on X11.
///
/// Unlike the Windows path there is no message-loop thread to own: the manager
/// has to be created on the thread running the platform's event loop, which is
/// the one calling this. A thread is still spawned, but only to turn the
/// library's blocking receiver into the same channel-plus-repaint shape the
/// rest of the app already expects.
#[cfg(not(windows))]
mod portable {
    use super::*;

    use std::collections::HashMap;

    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    use crate::entities::settings::{Hotkey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

    /// Registered id → the action it fires. Shared with the listener thread so a
    /// rebind takes effect without restarting it.
    type Bindings = Arc<Mutex<HashMap<u32, HotkeyAction>>>;

    pub struct Control {
        /// `None` when the platform refused a manager at all — an X11-less
        /// session, say. The app stays usable through the tray menu.
        manager: Option<GlobalHotKeyManager>,
        /// What is currently registered, so it can be handed back on a rebind.
        live: Mutex<Vec<HotKey>>,
        bindings: Bindings,
        accepted: Accepted,
    }

    impl Control {
        pub fn update(&self, hotkeys: Hotkeys) {
            let Some(manager) = self.manager.as_ref() else {
                return;
            };

            // Old bindings go first. Registering the new set while the old one
            // is still held would fail with `AlreadyRegistered` for every key
            // the user did not actually change.
            let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            for hotkey in live.drain(..) {
                if let Err(e) = manager.unregister(hotkey) {
                    warn!(error = %e, "Could not release a hotkey");
                }
            }

            let mut bindings = self.bindings.lock().unwrap_or_else(|e| e.into_inner());
            bindings.clear();

            let mut accepted = self.accepted.lock().unwrap_or_else(|e| e.into_inner());
            accepted.clear();

            for (action, binding) in hotkeys.all() {
                let Some(hotkey) = to_hotkey(binding) else {
                    // Unbound on purpose is not a rejection; an unmappable key is.
                    if binding.is_bound() {
                        warn!(?action, key = binding.key, "No portable code for this key");
                        accepted.push((action, false));
                    }
                    continue;
                };

                match manager.register(hotkey) {
                    Ok(()) => {
                        bindings.insert(hotkey.id(), action);
                        live.push(hotkey);
                        accepted.push((action, true));
                        info!(?action, "Hotkey registered");
                    }
                    Err(e) => {
                        // Almost always another application holding the same
                        // combination. The settings window shows this.
                        warn!(?action, error = %e, "Hotkey rejected");
                        accepted.push((action, false));
                    }
                }
            }
        }

        pub fn shutdown(&self) {
            let Some(manager) = self.manager.as_ref() else {
                return;
            };
            let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
            for hotkey in live.drain(..) {
                let _ = manager.unregister(hotkey);
            }
        }
    }

    pub fn spawn(
        hotkeys: Hotkeys,
        tx: Sender<HotkeyAction>,
        ctx: egui::Context,
        accepted: Accepted,
    ) -> (Control, Option<thread::JoinHandle<()>>) {
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => Some(m),
            Err(e) => {
                error!(error = %e, "No global hotkeys on this system");
                None
            }
        };

        let bindings: Bindings = Arc::new(Mutex::new(HashMap::new()));
        let control = Control {
            manager,
            live: Mutex::new(Vec::new()),
            bindings: Arc::clone(&bindings),
            accepted,
        };
        control.update(hotkeys);

        if control.manager.is_none() {
            return (control, None);
        }

        // The app only polls during a frame, and a frame only happens when
        // something asks for one — so the repaint request is what makes a
        // hotkey work while the window is in the background.
        let handle = thread::spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            while let Ok(event) = receiver.recv() {
                if event.state() != HotKeyState::Pressed {
                    continue;
                }
                let action = bindings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&event.id())
                    .copied();
                let Some(action) = action else { continue };
                if tx.send(action).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });

        (control, Some(handle))
    }

    /// Windows virtual-key codes are what the settings file stores on every
    /// platform, so they are translated here rather than at rest.
    fn to_hotkey(binding: Hotkey) -> Option<HotKey> {
        if !binding.is_bound() {
            return None;
        }

        let mut modifiers = Modifiers::empty();
        if binding.modifiers & MOD_CONTROL != 0 {
            modifiers |= Modifiers::CONTROL;
        }
        if binding.modifiers & MOD_ALT != 0 {
            modifiers |= Modifiers::ALT;
        }
        if binding.modifiers & MOD_SHIFT != 0 {
            modifiers |= Modifiers::SHIFT;
        }
        if binding.modifiers & MOD_WIN != 0 {
            // One slot, three names: the Windows key, ⌘ and Super. `HotKey::new`
            // normalises META to SUPER, so it is spelled that way here.
            modifiers |= Modifiers::SUPER;
        }

        Some(HotKey::new(Some(modifiers), vk_to_code(binding.key)?))
    }

    fn vk_to_code(vk: u32) -> Option<Code> {
        let name = match vk {
            0x30..=0x39 => return format!("Digit{}", vk - 0x30).parse().ok(),
            0x41..=0x5A => {
                let letter = (b'A' + (vk - 0x41) as u8) as char;
                return format!("Key{letter}").parse().ok();
            }
            0x70..=0x7B => return format!("F{}", vk - 0x70 + 1).parse().ok(),
            0x20 => "Space",
            0x0D => "Enter",
            0xC0 => "Backquote",
            0xBD => "Minus",
            0xBB => "Equal",
            0xDB => "BracketLeft",
            0xDD => "BracketRight",
            0xBA => "Semicolon",
            0xDE => "Quote",
            0xBC => "Comma",
            0xBE => "Period",
            0xBF => "Slash",
            _ => return None,
        };
        name.parse().ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::entities::settings::{VK_S, VK_T};

        #[test]
        fn the_default_bindings_all_map_to_a_portable_code() {
            for (_, binding) in Hotkeys::default().all() {
                if binding.is_bound() {
                    assert!(to_hotkey(binding).is_some());
                }
            }
        }

        #[test]
        fn letters_digits_and_function_keys_all_translate() {
            assert_eq!(vk_to_code(VK_T), Some(Code::KeyT));
            assert_eq!(vk_to_code(VK_S), Some(Code::KeyS));
            assert_eq!(vk_to_code(0x31), Some(Code::Digit1));
            assert_eq!(vk_to_code(0x70), Some(Code::F1));
            assert_eq!(vk_to_code(0x7B), Some(Code::F12));
            assert_eq!(vk_to_code(0xBF), Some(Code::Slash));
        }

        #[test]
        fn an_unmappable_key_is_reported_rather_than_guessed() {
            // 0x07 is not a key any of the pickers can produce.
            assert_eq!(vk_to_code(0x07), None);
        }

        #[test]
        fn the_windows_key_becomes_the_command_key() {
            let binding = Hotkey::new(MOD_WIN, VK_T);
            let hotkey = to_hotkey(binding).expect("mappable");
            assert!(hotkey.mods.contains(Modifiers::SUPER));
        }
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
