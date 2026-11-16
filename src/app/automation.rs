//! Driving the app from a script, for checking it end to end.
//!
//! **Debug builds only.** Everything the app does starts with a global shortcut
//! or a drag of the mouse, and neither can be produced from a test without
//! taking the keyboard and the pointer away from whoever is sitting at the
//! machine. So a development build can be pointed at a text file instead:
//!
//! ```text
//! SAKURA_AUTOMATION=C:\tmp\steps.txt
//! ```
//!
//! Lines appended to that file are carried out as they arrive:
//!
//! | line                 | does                                               |
//! |----------------------|----------------------------------------------------|
//! | `capture region`     | what the region shortcut does (also `window`, `screen`) |
//! | `watch`              | what the watch shortcut does                       |
//! | `select 60,60,900,360` | completes the selection with this rectangle      |
//! | `area 60,60,900,360` | translates this rectangle, with no selecting       |
//! | `watch 60,60,900,360` | watches this rectangle, with no selecting         |
//! | `area whole 0,0,1920,1080` | the same, read as a whole window or screen   |
//! | `cancel`             | abandons the selection                             |
//! | `repeat`             | what the repeat shortcut does                      |
//! | `hide`               | what Esc does to a translation                     |
//! | `settings keys`      | opens the settings window on a page                |
//! | `sample`             | puts the sample subtitle up                        |
//! | `quit`               | exits                                              |
//!
//! `capture` and a bare `watch` put the selection overlay up across every
//! display, as the shortcuts do. `area` and `watch` with a rectangle do not:
//! they touch that rectangle and nothing else, which is what to use while
//! somebody is using the rest of the screen.
//!
//! None of this is compiled into a release build.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use egui::Context;
use tracing::{info, warn};

use super::App;
use crate::entities::settings::CaptureMode;
use crate::features::capture::overlay::Purpose;
use crate::features::capture::Bounds;
use crate::features::settings::ui::Section;
use crate::features::translation::CaptureKind;

pub(super) const VARIABLE: &str = "SAKURA_AUTOMATION";

/// Where the script is, and how much of it has been carried out.
pub(super) struct Script {
    path: PathBuf,
    consumed: u64,
}

impl Script {
    pub(super) fn from_env() -> Option<Self> {
        let path = PathBuf::from(std::env::var_os(VARIABLE)?);
        info!(path = %path.display(), "Taking steps from a script");
        Some(Self { path, consumed: 0 })
    }

    /// Whole lines added since the last look.
    fn new_lines(&mut self) -> Vec<String> {
        let Ok(mut file) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        if file.seek(SeekFrom::Start(self.consumed)).is_err() {
            return Vec::new();
        }
        let mut text = String::new();
        if file.read_to_string(&mut text).is_err() {
            return Vec::new();
        }
        // A line still being written has no newline yet; leave it for later.
        let Some(end) = text.rfind('\n') else {
            return Vec::new();
        };
        self.consumed += (end + 1) as u64;
        text[..end]
            .lines()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect()
    }
}

fn parse_bounds(text: &str) -> Option<Bounds> {
    let numbers: Vec<i32> = text
        .split(',')
        .map(|part| part.trim().parse::<i32>())
        .collect::<Result<_, _>>()
        .ok()?;
    match numbers[..] {
        [x, y, w, h] => Some(Bounds { x, y, w, h }),
        _ => None,
    }
}

/// A rectangle, read as a selection around some text — or, with `whole` in
/// front of it, as a window or a screen of many separate pieces.
fn parse_area(text: &str) -> Option<(CaptureKind, Bounds)> {
    match text.strip_prefix("whole") {
        Some(rest) => Some((CaptureKind::Whole, parse_bounds(rest)?)),
        None => Some((CaptureKind::Region, parse_bounds(text)?)),
    }
}

impl App {
    pub(super) fn run_script(&mut self, ctx: &Context) {
        let Some(script) = self.script.as_mut() else {
            return;
        };
        let lines = script.new_lines();
        // The file is polled, so the app has to keep waking up to look at it.
        ctx.request_repaint_after(Duration::from_millis(80));

        for line in lines {
            let (verb, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            let rest = rest.trim();
            info!(step = %line, "Script");

            match verb {
                "capture" => {
                    let mode = match rest {
                        "window" => CaptureMode::Window,
                        "screen" => CaptureMode::FullScreen,
                        _ => CaptureMode::Region,
                    };
                    self.begin_capture(ctx, mode, Purpose::Translate);
                }
                "area" | "watch" if !rest.is_empty() => {
                    let purpose = if verb == "watch" {
                        Purpose::Watch
                    } else {
                        Purpose::Translate
                    };
                    match parse_area(rest) {
                        Some((kind, bounds)) => self.capture_known(ctx, bounds, kind, purpose),
                        None => warn!(step = %line, "Not a rectangle"),
                    }
                }
                "watch" => self.watch_or_stop(ctx),
                "select" => match (parse_bounds(rest), self.overlay.as_mut()) {
                    (Some(bounds), Some(overlay)) => overlay.completed = Some(bounds),
                    (None, _) => warn!(step = %line, "Not a rectangle"),
                    (_, None) => warn!("Nothing is being selected"),
                },
                "cancel" => {
                    if let Some(overlay) = self.overlay.as_mut() {
                        overlay.cancelled = true;
                    }
                }
                "repeat" => self.repeat_last_capture(ctx),
                "hide" => self.dismiss(),
                "settings" => {
                    self.settings_ui.on_open(&self.settings);
                    if let Some(section) = Section::from_name(rest) {
                        self.settings_ui.section = section;
                    }
                    self.settings_open = true;
                    self.settings_wants_focus = true;
                }
                "sample" => self.show_sample(ctx),
                "quit" => self.shutdown(ctx),
                _ => warn!(step = %line, "Unknown step"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn script_at(name: &str) -> (Script, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "sakura-script-{}-{name}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        (
            Script {
                path: path.clone(),
                consumed: 0,
            },
            path,
        )
    }

    fn append(path: &PathBuf, text: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("temp file");
        file.write_all(text.as_bytes()).expect("write");
    }

    #[test]
    fn each_line_is_carried_out_once() {
        let (mut script, path) = script_at("once");
        append(&path, "capture region\nselect 1,2,3,4\n");
        assert_eq!(script.new_lines(), vec!["capture region", "select 1,2,3,4"]);
        assert!(script.new_lines().is_empty(), "the same lines came back twice");

        append(&path, "hide\n");
        assert_eq!(script.new_lines(), vec!["hide"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_line_still_being_written_waits_for_its_newline() {
        let (mut script, path) = script_at("partial");
        append(&path, "capture reg");
        assert!(script.new_lines().is_empty());
        append(&path, "ion\n");
        assert_eq!(script.new_lines(), vec!["capture region"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn blank_lines_and_comments_are_skipped() {
        let (mut script, path) = script_at("comments");
        append(&path, "\n# a note\n   \nwatch\r\n");
        assert_eq!(script.new_lines(), vec!["watch"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_is_simply_no_steps() {
        let (mut script, _) = script_at("missing");
        assert!(script.new_lines().is_empty());
    }

    #[test]
    fn a_rectangle_is_four_numbers() {
        assert_eq!(
            parse_bounds("60,60,900,360"),
            Some(Bounds {
                x: 60,
                y: 60,
                w: 900,
                h: 360
            })
        );
        assert_eq!(parse_bounds("1,2,3"), None);
        assert_eq!(parse_bounds("a,b,c,d"), None);
    }

    #[test]
    fn an_area_is_a_selection_unless_it_says_whole() {
        let bounds = Bounds {
            x: 2620,
            y: 60,
            w: 600,
            h: 670,
        };
        assert_eq!(
            parse_area("2620,60,600,670"),
            Some((CaptureKind::Region, bounds))
        );
        assert_eq!(
            parse_area("whole 2620,60,600,670"),
            Some((CaptureKind::Whole, bounds))
        );
        assert_eq!(parse_area("whole"), None);
        assert_eq!(parse_area("somewhere"), None);
    }
}
