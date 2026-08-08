//! Putting the capture overlay where macOS does not want to put it.
//!
//! Two things fight the overlay on this platform.
//!
//! **The menu bar and the Dock.** An ordinary window cannot cover either:
//! AppKit clamps it to the screen's *visible* frame and keeps both above it. The
//! overlay is meant to be the whole desktop, frozen, so the backdrop ended up
//! shifted down by the height of the menu bar and the strips underneath could
//! not be selected at all.
//!
//! **Spaces.** A regular application belongs to a Space. Raising it while the
//! user is watching something full-screen in another one does not bring the
//! overlay to them — it takes them to the overlay, which is not what "translate
//! what is on my screen" means. An accessory application has no Space of its
//! own, and a window marked `canJoinAllSpaces` appears on whichever one is in
//! front, over a full-screen app included.
//!
//! There is no winit or eframe API for any of this, and the overlay is a child
//! viewport, so its window handle is not reachable from `eframe::Frame`. What is
//! reachable is `NSApp.windows`, so the window is found by its title and
//! adjusted directly.

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSScreen, NSScreenSaverWindowLevel, NSWindow,
    NSWindowCollectionBehavior,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tracing::info;

/// Title the overlay viewport is created with, and the only way to tell its
/// window apart from the settings and result windows.
pub const OVERLAY_TITLE: &str = "Sakura capture overlay";

/// Shown on every Space, and left alone by Mission Control.
const FOLLOWS_THE_USER: NSWindowCollectionBehavior =
    NSWindowCollectionBehavior::CanJoinAllSpaces.union(NSWindowCollectionBehavior::Stationary);

/// Turns the process into a menu-bar agent: no Dock icon, no application menu,
/// and — the point here — no Space of its own to drag the user back to.
///
/// This is what the app already is on Windows, where it lives in the tray and
/// has no taskbar button.
pub fn become_menu_bar_agent() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    if app.activationPolicy() == NSApplicationActivationPolicy::Accessory {
        return;
    }
    if app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
        info!("Running as a menu-bar agent");
    }
}

/// Brings the application forward without changing Space.
///
/// Needed for keyboard input: a window of an inactive application never becomes
/// key, and then Esc and Tab do nothing over the overlay.
pub fn activate() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

/// Places the overlay over whatever the user is looking at, full-screen apps
/// included, and gives it the keyboard.
///
/// Idempotent, and cheap enough to call every frame: an app of this shape has a
/// handful of windows, and nothing is written unless it differs.
pub fn present_overlay(title: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        // AppKit is main-thread only. Every caller is on it; this is the guard
        // that says so rather than a condition that is expected to happen.
        return;
    };

    let Some(desktop) = union_of_screens(mtm) else {
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    for window in app.windows().iter() {
        let window_title = window.title().to_string();

        if window_title != title {
            // The 1×1 host window parked off screen, and the settings window.
            // Left where they are, but not allowed to anchor the application to
            // one Space — activating the overlay would otherwise switch to
            // whichever Space they happen to be on.
            if !window.collectionBehavior().contains(FOLLOWS_THE_USER) {
                window.setCollectionBehavior(window.collectionBehavior() | FOLLOWS_THE_USER);
            }
            continue;
        }

        raise(&window, desktop);
    }
}

fn raise(window: &NSWindow, desktop: NSRect) {
    // Above the menu bar (24) and the Dock (20). The screen saver's level is the
    // conventional choice for a full-desktop overlay.
    if window.level() != NSScreenSaverWindowLevel {
        info!(
            was = window.level(),
            now = NSScreenSaverWindowLevel,
            "Overlay raised above the menu bar"
        );
        window.setLevel(NSScreenSaverWindowLevel);
    }

    let behaviour = FOLLOWS_THE_USER
        // Lets the overlay sit over another app's full-screen Space instead of
        // being pushed to a Space of its own.
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        // It is not a document window; ⌘` should not cycle to it.
        | NSWindowCollectionBehavior::IgnoresCycle;
    if window.collectionBehavior() != behaviour {
        window.setCollectionBehavior(behaviour);
    }

    let frame = window.frame();
    if !same_rect(frame, desktop) {
        info!(
            was = format!(
                "{}×{} at ({}, {})",
                frame.size.width, frame.size.height, frame.origin.x, frame.origin.y
            ),
            now = format!(
                "{}×{} at ({}, {})",
                desktop.size.width, desktop.size.height, desktop.origin.x, desktop.origin.y
            ),
            "Overlay frame corrected"
        );
        window.setFrame_display(desktop, true);
    }

    // Ordered front *after* the behaviour is set, so it appears on the Space the
    // user is on rather than pulling them to this application's own.
    window.orderFrontRegardless();

    if !window.isKeyWindow() {
        activate();
        if window.canBecomeKeyWindow() {
            window.makeKeyWindow();
        }
    }
}

/// Every screen's frame — not `visibleFrame`, which is the whole point — as one
/// rectangle, in AppKit's bottom-left origin coordinates.
fn union_of_screens(mtm: MainThreadMarker) -> Option<NSRect> {
    let screens = NSScreen::screens(mtm);
    let mut union: Option<NSRect> = None;

    for screen in screens.iter() {
        let frame = screen.frame();
        union = Some(match union {
            None => frame,
            Some(u) => {
                let min_x = u.origin.x.min(frame.origin.x);
                let min_y = u.origin.y.min(frame.origin.y);
                let max_x = (u.origin.x + u.size.width).max(frame.origin.x + frame.size.width);
                let max_y = (u.origin.y + u.size.height).max(frame.origin.y + frame.size.height);
                NSRect::new(
                    NSPoint::new(min_x, min_y),
                    NSSize::new(max_x - min_x, max_y - min_y),
                )
            }
        });
    }

    union
}

/// AppKit hands back floats it computed itself, so an exact comparison would
/// re-set the frame every frame on any fractional scale factor.
fn same_rect(a: NSRect, b: NSRect) -> bool {
    const EPSILON: f64 = 0.5;
    (a.origin.x - b.origin.x).abs() < EPSILON
        && (a.origin.y - b.origin.y).abs() < EPSILON
        && (a.size.width - b.size.width).abs() < EPSILON
        && (a.size.height - b.size.height).abs() < EPSILON
}
