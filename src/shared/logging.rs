//! Application logging: one file per day, pruned automatically.
//!
//! Design goals, in order:
//!
//! 1. **Bounded.** A tray app runs for weeks. Logs must never grow without limit,
//!    so each day gets its own file, files older than `retention_days` are
//!    deleted, and a single day is capped in bytes.
//! 2. **Fresh.** The current day's file is what you look at when something
//!    breaks; yesterday's noise is out of the way in its own file.
//! 3. **Private.** OCR text and translations are the contents of the user's
//!    screen. They are never written at INFO — `redact()` yields a length
//!    instead, and the real text only appears under verbose logging.
//!
//! The rotation is keyed to the **local** date, matching the timestamps written
//! inside the file. `tracing-appender`'s own daily roller uses UTC, which for
//! anyone east or west of Greenwich produces a file called `…-08-08.log` whose
//! first entries are stamped the 9th — confusing exactly when you are trying to
//! find out what happened and when.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use time::format_description::FormatItem;
use time::macros::format_description;
use time::{Date, OffsetDateTime, UtcOffset};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

const FILENAME_PREFIX: &str = "screen-translator";
const FILENAME_SUFFIX: &str = "log";

const DATE_IN_NAME: &[FormatItem<'static>] = format_description!("[year]-[month]-[day]");

static VERBOSE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub dir: PathBuf,
    /// Files older than this many days are deleted. 1 = keep only today.
    pub retention_days: u16,
    /// Hard ceiling for a single day's file. Writing stops once reached and
    /// resumes at the next date change.
    pub max_bytes_per_day: u64,
    /// Log the actual recognised and translated text instead of just its length.
    pub verbose: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            dir: default_log_dir(),
            retention_days: 3,
            max_bytes_per_day: 8 * 1024 * 1024,
            verbose: false,
        }
    }
}

pub fn default_log_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("screen-translator")
        .join("logs")
}

/// Installs the global subscriber. The returned guard flushes the background
/// writer thread on drop — keep it alive for the whole process.
#[must_use = "dropping the guard stops log writes"]
pub fn init(cfg: &LogConfig) -> Option<WorkerGuard> {
    VERBOSE.store(cfg.verbose, Ordering::Relaxed);

    let _ = std::fs::create_dir_all(&cfg.dir);

    // Resolved once, here, before any other thread exists: the `time` crate
    // refuses to read the local offset from a multi-threaded process.
    let offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

    // Versions before rotation wrote a single ever-growing file one level up,
    // next to config.toml. Nothing reads it any more.
    if let Some(parent) = cfg.dir.parent() {
        let legacy = parent.join(format!("{FILENAME_PREFIX}.{FILENAME_SUFFIX}"));
        if legacy.exists() {
            let _ = std::fs::remove_file(&legacy);
        }
    }

    let writer = DailyFile::new(
        cfg.dir.clone(),
        cfg.retention_days,
        cfg.max_bytes_per_day,
        offset,
    );
    let pruned = writer.pruned_at_start;

    let (writer, guard) = tracing_appender::non_blocking(writer);

    let default_level = if cfg.verbose { "debug" } else { "info" };
    let filter = || {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            // Silence the dependency tree; only our own spans are interesting.
            EnvFilter::new(format!(
                "{app}={lvl},warn",
                app = env!("CARGO_PKG_NAME").replace('-', "_"),
                lvl = default_level
            ))
        })
    };

    let timer = tracing_subscriber::fmt::time::OffsetTime::new(
        offset,
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]"),
    );

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_timer(timer.clone())
        .with_filter(filter());

    let registry = tracing_subscriber::registry().with(file_layer);

    // In debug builds the console is attached, so mirror everything there too.
    #[cfg(debug_assertions)]
    let registry = registry.with(
        tracing_subscriber::fmt::layer()
            .with_timer(timer)
            .with_filter(filter()),
    );

    registry.init();

    tracing::info!(
        dir = %cfg.dir.display(),
        retention_days = cfg.retention_days,
        max_bytes_per_day = cfg.max_bytes_per_day,
        pruned_files = pruned,
        verbose = cfg.verbose,
        utc_offset_hours = offset.whole_hours(),
        "Logging initialised"
    );

    Some(guard)
}

// ── The rolling writer ───────────────────────────────────────────────────────

/// Appends to `screen-translator.<local-date>.log`, switching files when the
/// local date changes and deleting anything older than the retention window.
struct DailyFile {
    dir: PathBuf,
    retention_days: u16,
    cap: u64,
    offset: UtcOffset,

    file: Option<File>,
    day: Date,
    written: u64,
    capped: bool,
    last_day_check: Instant,

    pruned_at_start: usize,
}

impl DailyFile {
    fn new(dir: PathBuf, retention_days: u16, cap: u64, offset: UtcOffset) -> Self {
        let today = OffsetDateTime::now_utc().to_offset(offset).date();
        let pruned = prune(&dir, retention_days, today);

        let mut me = Self {
            dir,
            retention_days,
            cap,
            offset,
            file: None,
            day: today,
            written: 0,
            capped: false,
            last_day_check: Instant::now(),
            pruned_at_start: pruned,
        };
        me.open_today();
        me
    }

    fn today(&self) -> Date {
        OffsetDateTime::now_utc().to_offset(self.offset).date()
    }

    fn open_today(&mut self) {
        let path = self.dir.join(file_name(self.day));
        // Appending rather than truncating: restarting the app several times in
        // a day must not throw away the earlier runs.
        self.written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.capped = self.written >= self.cap;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
    }

    /// Checking the wall clock on every line would be wasteful; once a minute is
    /// plenty to notice a date rollover.
    fn roll_if_new_day(&mut self) {
        if self.last_day_check.elapsed() < Duration::from_secs(60) {
            return;
        }
        self.last_day_check = Instant::now();

        let today = self.today();
        if today == self.day {
            return;
        }
        self.day = today;
        self.file = None;
        self.open_today();
        prune(&self.dir, self.retention_days, today);
    }
}

impl Write for DailyFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.roll_if_new_day();

        // Report success in every giving-up case: returning an error here makes
        // tracing spin retrying, which is worse than dropping the line.
        if self.capped {
            return Ok(buf.len());
        }
        let Some(file) = self.file.as_mut() else {
            return Ok(buf.len());
        };

        if self.written + buf.len() as u64 > self.cap {
            self.capped = true;
            let notice = format!(
                "--- лимит журнала на сегодня ({} байт) исчерпан, дальнейшие записи пропущены ---\n",
                self.cap
            );
            let _ = file.write_all(notice.as_bytes());
            let _ = file.flush();
            return Ok(buf.len());
        }

        let n = file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

fn file_name(date: Date) -> String {
    let stamp = date
        .format(DATE_IN_NAME)
        .unwrap_or_else(|_| "0000-00-00".to_string());
    format!("{FILENAME_PREFIX}.{stamp}.{FILENAME_SUFFIX}")
}

/// Parses a rotated log file name back into its date.
fn parse_file_name(name: &str) -> Option<Date> {
    let rest = name.strip_prefix(FILENAME_PREFIX)?;
    let stamp = rest
        .strip_prefix('.')?
        .strip_suffix(&format!(".{FILENAME_SUFFIX}"))?;
    Date::parse(stamp, DATE_IN_NAME).ok()
}

/// Deletes rotated files older than the retention window, plus the single
/// unrotated `screen-translator.log` written by earlier versions.
///
/// Returns how many files went.
fn prune(dir: &Path, retention_days: u16, today: Date) -> usize {
    let keep_from = today - time::Duration::days(retention_days.max(1) as i64 - 1);

    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };

        let stale = if name == format!("{FILENAME_PREFIX}.{FILENAME_SUFFIX}") {
            true // legacy single-file log from before rotation existed
        } else {
            match parse_file_name(name) {
                Some(date) => date < keep_from,
                None => false,
            }
        };

        if stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

// ── Privacy helpers ──────────────────────────────────────────────────────────

/// Whether the user opted into logging recognised text verbatim.
pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

/// Renders user text for a log line. Screen contents are private, so the default
/// is a character count; the real text appears only under verbose logging.
pub fn redact(text: &str) -> String {
    if verbose() {
        format!("{:?}", clip(text, 2000))
    } else {
        format!("<{} chars>", text.chars().count())
    }
}

/// Truncates to at most `max_bytes` **without splitting a UTF-8 code point**.
///
/// `&s[..n]` panics when `n` lands inside a multi-byte character, which is the
/// normal case for Cyrillic or CJK API responses — exactly the responses this
/// app deals with.
pub fn clip(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn clip_never_splits_a_code_point() {
        let s = "Привет мир"; // every char is 2 bytes
        for n in 0..=s.len() + 4 {
            let out = clip(s, n);
            assert!(s.starts_with(out));
            assert!(out.len() <= n.min(s.len()));
        }
    }

    #[test]
    fn clip_passes_short_strings_through() {
        assert_eq!(clip("abc", 10), "abc");
        assert_eq!(clip("", 0), "");
    }

    #[test]
    fn clip_handles_four_byte_chars() {
        let s = "🌸🌸";
        assert_eq!(clip(s, 5), "🌸");
        assert_eq!(clip(s, 3), "");
    }

    #[test]
    fn file_names_round_trip() {
        let d = date!(2026 - 08 - 09);
        assert_eq!(file_name(d), "screen-translator.2026-08-09.log");
        assert_eq!(parse_file_name(&file_name(d)), Some(d));
    }

    #[test]
    fn unrelated_files_are_not_parsed_as_logs() {
        assert_eq!(parse_file_name("config.toml"), None);
        assert_eq!(parse_file_name("screen-translator.log"), None);
        assert_eq!(parse_file_name("screen-translator.notadate.log"), None);
        assert_eq!(parse_file_name("other-app.2026-08-09.log"), None);
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sakura-log-test-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prune_keeps_the_retention_window_and_drops_the_rest() {
        let dir = temp_dir("window");
        let today = date!(2026 - 08 - 09);
        for d in [
            date!(2026 - 08 - 09),
            date!(2026 - 08 - 08),
            date!(2026 - 08 - 07),
            date!(2026 - 08 - 01),
        ] {
            std::fs::write(dir.join(file_name(d)), b"x").unwrap();
        }

        let removed = prune(&dir, 3, today);
        assert_eq!(removed, 1, "only 08-01 falls outside a 3-day window");
        assert!(dir.join(file_name(date!(2026 - 08 - 07))).exists());
        assert!(!dir.join(file_name(date!(2026 - 08 - 01))).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_of_one_keeps_only_today() {
        let dir = temp_dir("one");
        let today = date!(2026 - 08 - 09);
        std::fs::write(dir.join(file_name(today)), b"x").unwrap();
        std::fs::write(dir.join(file_name(date!(2026 - 08 - 08))), b"x").unwrap();

        prune(&dir, 1, today);
        assert!(dir.join(file_name(today)).exists());
        assert!(!dir.join(file_name(date!(2026 - 08 - 08))).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_removes_the_legacy_unrotated_file() {
        let dir = temp_dir("legacy");
        std::fs::write(dir.join("screen-translator.log"), b"old").unwrap();
        let removed = prune(&dir, 3, date!(2026 - 08 - 09));
        assert_eq!(removed, 1);
        assert!(!dir.join("screen-translator.log").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_leaves_other_files_alone() {
        let dir = temp_dir("others");
        std::fs::write(dir.join("notes.txt"), b"keep me").unwrap();
        prune(&dir, 1, date!(2026 - 08 - 09));
        assert!(dir.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zero_retention_is_treated_as_one_day() {
        let dir = temp_dir("zero");
        let today = date!(2026 - 08 - 09);
        std::fs::write(dir.join(file_name(today)), b"x").unwrap();
        prune(&dir, 0, today);
        assert!(
            dir.join(file_name(today)).exists(),
            "today's log must survive whatever the retention setting says"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_byte_cap_stops_writing_and_says_so() {
        let dir = temp_dir("cap");
        let mut w = DailyFile::new(dir.clone(), 3, 200, UtcOffset::UTC);
        for _ in 0..50 {
            w.write_all(b"0123456789\n").unwrap();
        }
        w.flush().unwrap();

        let path = dir.join(file_name(OffsetDateTime::now_utc().date()));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("лимит журнала"), "cap notice missing");
        assert!(
            content.len() < 400,
            "cap did not hold: {} bytes",
            content.len()
        );

        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restarting_appends_rather_than_truncating() {
        let dir = temp_dir("append");
        {
            let mut w = DailyFile::new(dir.clone(), 3, 10_000, UtcOffset::UTC);
            w.write_all(b"first run\n").unwrap();
            w.flush().unwrap();
        }
        {
            let mut w = DailyFile::new(dir.clone(), 3, 10_000, UtcOffset::UTC);
            w.write_all(b"second run\n").unwrap();
            w.flush().unwrap();
        }
        let path = dir.join(file_name(OffsetDateTime::now_utc().date()));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("first run"));
        assert!(content.contains("second run"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
