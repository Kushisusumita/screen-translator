// The release build hides the console; the debug build keeps it so log lines
// are visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod entities;
mod features;
mod shared;
mod ui;

use eframe::NativeOptions;
use egui::ViewportBuilder;

use app::{App, Startup};
use features::capture::Bounds;
use features::settings::ui::Section;
use shared::i18n::t;
use shared::logging::{self, LogConfig};

/// Command line. Deliberately tiny — this is a tray app, not a CLI.
struct Args {
    /// Open the settings window straight away, optionally on a given page.
    /// Handy for a desktop shortcut, and for looking at the UI without
    /// triggering a screen capture.
    open_settings: Option<Section>,
    /// Something to do as soon as the app is up.
    startup: Option<Startup>,
}

fn parse_args() -> Args {
    let mut args = Args {
        open_settings: None,
        startup: None,
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--settings" | "-s" => {
                let section = it
                    .peek()
                    .and_then(|next| Section::from_name(next))
                    .inspect(|_| {
                        it.next();
                    });
                args.open_settings = Some(section.unwrap_or(Section::General));
            }
            "--region" | "-r" => match it.next().as_deref().and_then(parse_area) {
                Some(area) => args.startup = Some(Startup::Area(area)),
                None => eprintln!(
                    "{}",
                    t("--region takes four numbers: left,top,width,height")
                ),
            },
            "--help" | "-h" => {
                println!("Sakura Screen Translator {}", env!("CARGO_PKG_VERSION"));
                println!();
                println!("{}", t("Runs in the system tray."));
                println!();
                println!(
                    "  --settings, -s [page]     {}",
                    t("open settings on a given page")
                );
                println!("                            (general, keys, languages, engine, appearance, history, logs, about)");
                println!(
                    "  --region, -r x,y,w,h      {}",
                    t("translate this rectangle of the screen right away")
                );
                println!("  --help, -h                {}", t("show this help"));
                std::process::exit(0);
            }
            other => eprintln!(
                "{}",
                t("Unknown argument: {argument}").replace("{argument}", other)
            ),
        }
    }
    args
}

/// `left,top,width,height` in desktop pixels. The left and top may be
/// negative — a monitor to the left of the primary one starts below zero.
fn parse_area(text: &str) -> Option<Bounds> {
    let numbers: Vec<i32> = text
        .split(',')
        .map(|part| part.trim().parse::<i32>())
        .collect::<Result<_, _>>()
        .ok()?;
    match numbers[..] {
        [x, y, w, h] if w > 0 && h > 0 => Some(Bounds { x, y, w, h }),
        _ => None,
    }
}

fn main() -> eframe::Result<()> {
    let args = parse_args();

    // Must happen before any window exists, otherwise Windows lies about screen
    // dimensions for the rest of the process and every capture is mis-scaled.
    enable_dpi_awareness();

    // Logging comes up first, reading only the section it needs, so that the
    // full settings load below can report a migration or a corrupt file.
    let log_settings = features::settings::load_log_settings();
    let log_cfg = LogConfig {
        dir: logging::default_log_dir(),
        retention_days: log_settings.retention_days,
        max_bytes_per_day: log_settings.max_mb_per_day.saturating_mul(1024 * 1024),
        verbose: log_settings.verbose,
    };
    // Held for the whole process - dropping it stops the background log writer.
    let _log_guard = logging::init(&log_cfg);

    let settings = features::settings::load_settings();

    // Before anything is drawn, and before the first error message can be
    // built: the language decides what every string in the process reads as.
    // No preference means follow the OS, and an OS we have no translation for
    // means English.
    let language = settings
        .ui_language
        .or_else(shared::i18n::detect_system)
        .unwrap_or(shared::i18n::Lang::En);
    shared::i18n::set(language);
    tracing::info!(language = language.code(), "Interface language");

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        settings_page = ?args.open_settings,
        startup = ?args.startup,
        "Sakura Screen Translator starting"
    );

    // A second copy would fight the first over the tray icon and the hotkeys.
    let _instance = match SingleInstance::acquire() {
        Some(guard) => guard,
        None => {
            tracing::info!("Another copy is already running; exiting");
            return Ok(());
        }
    };

    features::updater::cleanup_previous_version();

    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Sakura Screen Translator")
            // The real UI lives in child viewports; this host window only needs
            // to exist, so it is 1×1 and parked far off screen.
            .with_inner_size([1.0, 1.0])
            .with_position(egui::pos2(-32000.0, -32000.0))
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            // A placeholder has no use for the keyboard, and an app that
            // starts with the session should not take it from whatever the
            // user was typing into.
            .with_active(false)
            // The GL configuration is chosen once, for this window, and every
            // other window shares it. Where the toolkit handles transparency,
            // asking for it here is what gives the floating views a surface
            // they can be see-through on; without it they come up as opaque
            // rectangles. Windows goes about it another way.
            .with_transparent(features::capture::native::TOOLKIT_TRANSPARENCY)
            .with_visible(true),
        ..Default::default()
    };

    eframe::run_native(
        "Sakura Screen Translator",
        options,
        Box::new(move |cc| {
            Ok(Box::new(App::new(
                cc,
                settings,
                args.open_settings,
                args.startup,
            )))
        }),
    )
}

/// Per-monitor DPI awareness.
///
/// Without it Windows hands the process virtualised, pre-scaled coordinates,
/// and a capture drawn on a 150 % display lands somewhere else entirely.
fn enable_dpi_awareness() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        // Fails harmlessly when the manifest already set it.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// What the one-copy-at-a-time lock is called.
///
/// One copy per profile, not one per machine: a copy pointed at its own
/// folder with `SAKURA_CONFIG_DIR` is a second profile, and is allowed to run
/// beside the ordinary one. Without this the second profile the README
/// promises could only ever be started once the first had been closed.
#[cfg(windows)]
fn instance_name() -> String {
    use std::hash::{Hash, Hasher};

    const NAME: &str = "Local\\SakuraScreenTranslator";
    match std::env::var_os(shared::paths::DATA_DIR_VAR).filter(|dir| !dir.is_empty()) {
        Some(dir) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            dir.hash(&mut hasher);
            format!("{NAME}-{:016x}", hasher.finish())
        }
        None => NAME.to_string(),
    }
}

/// Holds a named mutex for the lifetime of the process.
struct SingleInstance {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

impl SingleInstance {
    fn acquire() -> Option<Self> {
        #[cfg(windows)]
        unsafe {
            use windows::core::PCWSTR;
            use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
            use windows::Win32::System::Threading::CreateMutexW;

            let name: Vec<u16> = instance_name().encode_utf16().chain(Some(0)).collect();
            let handle = CreateMutexW(None, true, PCWSTR(name.as_ptr())).ok()?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return None;
            }
            Some(SingleInstance { handle })
        }
        #[cfg(not(windows))]
        {
            Some(SingleInstance {})
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::Foundation::CloseHandle;
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_area_is_four_comma_separated_numbers() {
        assert_eq!(
            parse_area("100,200,640,120"),
            Some(Bounds {
                x: 100,
                y: 200,
                w: 640,
                h: 120
            })
        );
        assert_eq!(
            parse_area(" 10 , 20 , 30 , 40 "),
            Some(Bounds {
                x: 10,
                y: 20,
                w: 30,
                h: 40
            })
        );
    }

    #[test]
    fn an_area_on_a_monitor_left_of_the_primary_starts_below_zero() {
        assert_eq!(
            parse_area("-1920,0,800,600").map(|b| b.x),
            Some(-1920)
        );
    }

    #[test]
    fn a_malformed_area_is_refused_rather_than_guessed() {
        for bad in ["", "1,2,3", "1,2,3,4,5", "a,b,c,d", "0,0,0,10", "0,0,10,-5"] {
            assert_eq!(parse_area(bad), None, "{bad:?} was accepted");
        }
    }
}
