//! A file's bytes, kept for as long as the process runs.
//!
//! This exists for fonts. No single typeface covers Japanese, Chinese and
//! Korean, so the app loads every fallback it can find — eighty megabytes of
//! font files on Windows — and egui, given a font it *owns*, copies it: once
//! into its definitions and again each time the font stack is rebuilt. A
//! program that lives in the tray was holding close to three hundred megabytes
//! for the sake of a few dozen glyphs.
//!
//! Handed out as `&'static [u8]` instead, a font is borrowed and never copied.
//! And where the platform can map a file into memory rather than read it, the
//! bytes are not even loaded until something looks at them: of an eighteen
//! megabyte Chinese font, the pages that hold the characters actually drawn.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The contents of the file at `path`, or `None` if it cannot be read.
///
/// The same file asked for twice is the same memory: nothing is read or
/// mapped a second time. The memory is never given back, which is the point —
/// only ask for files that are wanted until the app exits.
pub fn forever(path: &str) -> Option<&'static [u8]> {
    /// Every path asked for so far, and what came of it — a failure included,
    /// so that a missing font is not looked for again on every rebuild.
    type Loaded = HashMap<String, Option<&'static [u8]>>;
    static LOADED: OnceLock<Mutex<Loaded>> = OnceLock::new();
    let mut loaded = LOADED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(known) = loaded.get(path) {
        return *known;
    }
    let bytes = map(path).or_else(|| read(path));
    loaded.insert(path.to_owned(), bytes);
    bytes
}

fn read(path: &str) -> Option<&'static [u8]> {
    let bytes = std::fs::read(path).ok()?;
    Some(Box::leak(bytes.into_boxed_slice()))
}

/// Maps the file read-only. The view is never unmapped.
#[cfg(windows)]
fn map(path: &str) -> Option<&'static [u8]> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, GetFileSizeEx, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, OPEN_EXISTING,
    };
    use windows::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, FILE_MAP_READ, PAGE_READONLY,
    };

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let file = CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            // Others may read it; nobody may write to it while it is mapped.
            FILE_SHARE_READ,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()?;

        let mut size = 0i64;
        let mapping = if GetFileSizeEx(file, &mut size).is_ok() && size > 0 {
            CreateFileMappingW(file, None, PAGE_READONLY, 0, 0, PCWSTR::null()).ok()
        } else {
            None
        };
        // The mapping holds the file open from here on, and the view will
        // hold the mapping: neither handle is needed past its next use.
        let _ = CloseHandle(file);
        let mapping = mapping?;
        let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
        let _ = CloseHandle(mapping);

        if view.Value.is_null() {
            return None;
        }
        // Valid for the life of the process: the view is never unmapped, and
        // a mapped file can be neither truncated nor replaced underneath it.
        Some(std::slice::from_raw_parts(
            view.Value as *const u8,
            size as usize,
        ))
    }
}

#[cfg(not(windows))]
fn map(_path: &str) -> Option<&'static [u8]> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str, contents: &[u8]) -> String {
        let path = std::env::temp_dir().join(format!(
            "sakura-file-bytes-{}-{name}",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("temp dir is writable");
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_file_comes_back_byte_for_byte() {
        let contents: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
        let path = scratch("whole", &contents);
        assert_eq!(forever(&path), Some(contents.as_slice()));
    }

    #[test]
    fn asking_twice_gives_the_same_memory() {
        let path = scratch("twice", b"the same bytes both times");
        let first = forever(&path).expect("readable");
        let second = forever(&path).expect("readable");
        assert_eq!(first.as_ptr(), second.as_ptr());
    }

    #[test]
    fn a_missing_file_is_none_and_stays_none() {
        let path = scratch("gone", b"x");
        std::fs::remove_file(&path).expect("removable");
        assert_eq!(forever(&path), None);
        assert_eq!(forever(&path), None);
    }

    #[test]
    fn an_empty_file_is_empty_rather_than_an_error() {
        // Nothing to map in a file with nothing in it; reading still works.
        let path = scratch("empty", b"");
        assert_eq!(forever(&path), Some(&[][..]));
    }
}
