//! Window behaviour the toolkit cannot express, behind one portable door.
//!
//! The views describe what they want — "do not take the focus", "appear at
//! once", "sit on exactly these pixels" — and this module turns that into
//! whatever the platform offers. Where a platform offers nothing the call is a
//! no-op and the view falls back to what a plain window does, so nothing above
//! this line needs a `cfg`.

use super::screenshot::Bounds;

/// What a floating window asks of the platform.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Behaviour {
    /// Stay clickable without ever becoming the active window.
    pub no_activate: bool,
    /// Skip the window manager's open and close animations.
    pub instant: bool,
    /// Round the corners of the window itself, to this radius in points.
    pub rounded: Option<f32>,
    /// Leave the window out of screen captures.
    pub hidden_from_capture: bool,
    /// Let what is behind the window show through wherever it was drawn
    /// see-through. The toolkit's own "transparent" is not always honoured.
    pub translucent: bool,
}

/// Whether this platform can show a window without activating it.
///
/// Where it cannot, a result window takes the focus the ordinary way, and
/// "close when I click away" is answered by the focus leaving it.
pub const QUIET_WINDOWS: bool = cfg!(windows);

/// Whether the toolkit is the one to ask for a see-through window.
///
/// Not on Windows: there its answer hangs on what the graphics driver says
/// about its pixel format, is usually "no", and comes with an error in the
/// log for every window. The compositor is asked directly instead — that is
/// [`Behaviour::translucent`] — and the toolkit is left out of it.
pub const TOOLKIT_TRANSPARENCY: bool = !cfg!(windows);

/// Shows a window that was created hidden, once it has had a frame in which
/// to be set up — given the word not to animate, most of all.
///
/// `frames_so_far` is how many frames the window has already been through.
/// Call it from inside the window's own viewport, every frame.
///
/// The builder's `with_visible(true)` on a later frame ought to do this and
/// does not. egui 0.29 decides whether to pass the change on by comparing the
/// new value with the window's *active* flag rather than its visible one: a
/// window created hidden and inactive is shown by accident, and one created
/// hidden and active — the capture overlay, which needs the keyboard — is
/// never shown at all. Every shortcut then did nothing, while an invisible
/// overlay sat waiting for a selection nobody could make. So it is said
/// outright, as a command, and twice in case the first arrives too early.
pub fn reveal(ctx: &egui::Context, frames_so_far: u32) {
    if QUIET_WINDOWS && (1..=2).contains(&frames_so_far) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    }
}

/// Applies `behaviour` to the window with this title. Idempotent, and cheap
/// enough to call from every frame — which is how it has to be called, since
/// the window does not exist yet on the frame that asks for it.
pub fn apply(title: &str, behaviour: Behaviour) {
    #[cfg(windows)]
    {
        super::win_window::apply(
            title,
            super::win_window::Behaviour {
                no_activate: behaviour.no_activate,
                instant: behaviour.instant,
                rounded: behaviour.rounded.is_some(),
                hidden_from_capture: behaviour.hidden_from_capture,
                translucent: behaviour.translucent,
            },
        );
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(radius) = behaviour.rounded {
            super::mac_window::round_corners(title, radius as f64);
        }
        if behaviour.no_activate {
            super::mac_window::float_over(title);
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = (title, behaviour);
    }
}

/// Puts the window on exactly these desktop pixels. A no-op where the builder's
/// own placement is all there is.
pub fn place(title: &str, bounds: Bounds) {
    #[cfg(windows)]
    super::win_window::place(title, bounds);
    #[cfg(not(windows))]
    {
        let _ = (title, bounds);
    }
}

/// True when the user pressed a mouse button somewhere other than on this
/// window. Always false where there is no way to ask.
pub fn clicked_elsewhere(title: &str) -> bool {
    #[cfg(windows)]
    {
        super::win_window::clicked_elsewhere(title)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}

/// Whether the window is really being left out of screen captures.
pub fn hidden_from_capture(title: &str) -> bool {
    #[cfg(windows)]
    {
        super::win_window::hidden_from_capture(title)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}
