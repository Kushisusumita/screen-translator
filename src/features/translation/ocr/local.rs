//! Reading text on the machine itself.
//!
//! Windows ships a recogniser of its own. It answers in hundredths of a
//! second where the online one takes most of a second, and the picture never
//! leaves the computer — which is what makes it possible to read an area of
//! the screen again and again while a game is running.
//!
//! It was made for pages, though, and has to be helped with a frame of a
//! game. So it is asked three times at once: about the picture as it is, and
//! about the two versions of it that [`super::strokes`] makes, where only
//! thin strokes are left and the scene they stood on is gone. The three
//! answers are then put together by [`super::layout`], which keeps for every
//! place the reading that makes the most sense and throws out what makes
//! none. Type set large is asked about once more, at half the size, and
//! kept only if it reads the same: a logo does not.
//!
//! One recogniser reads one language, and only the languages installed in
//! Windows have one; [`language_for`] says which would be used, and the
//! caller falls back to the online recogniser when the answer is none.

use std::borrow::Cow;
use std::time::Instant;

use once_cell::sync::Lazy;
use tracing::{debug, warn};

use super::layout::{self, WordLine};
use super::strokes::{self, Gray};
use super::Recognised;
use crate::entities::language::Language;
use crate::features::capture::Frame;
use crate::shared::error::AppError;
use crate::shared::i18n::t;
use crate::shared::logging::redact;

/// The recognisers on this machine: each language, and the name Windows
/// knows its recogniser by.
static INSTALLED: Lazy<Vec<(Language, String)>> = Lazy::new(|| {
    let mut found: Vec<(Language, String)> = Vec::new();
    for tag in engine::tags() {
        if let Some(language) = Language::from_code(&tag) {
            if !found.iter().any(|(known, _)| *known == language) {
                found.push((language, tag));
            }
        }
    }
    debug!(
        languages = ?found.iter().map(|(_, tag)| tag.as_str()).collect::<Vec<_>>(),
        "Recognisers on this computer"
    );
    found
});

/// The languages that can be read on this machine.
pub fn languages() -> Vec<Language> {
    INSTALLED.iter().map(|(language, _)| *language).collect()
}

/// The language text would be read as on this machine, or `None` when it
/// has to be read online.
pub fn language_for(source: Language, target: Language) -> Option<Language> {
    choose(&languages(), source, target)
}

/// A source the user named is read as that, or not here at all. Left on
/// "detect", there is nothing to detect with — a recogniser has to be told
/// its language before it reads — so the choice falls on a language that is
/// not the one being translated into: text already in that needs no
/// translating. English first, as what a game not yet translated is most
/// often in.
fn choose(installed: &[Language], source: Language, target: Language) -> Option<Language> {
    if source != Language::Auto {
        return installed.contains(&source).then_some(source);
    }
    if target != Language::En && installed.contains(&Language::En) {
        return Some(Language::En);
    }
    installed.iter().copied().find(|language| *language != target)
}

/// What to say when text was to be read on this machine only and it has no
/// recogniser for the language.
pub fn missing(source: Language) -> AppError {
    let message = if source == Language::Auto {
        t("Windows has no text recogniser for another language than the one you translate into. Add a language in Windows settings, or let text be read online.").to_string()
    } else {
        t("Windows has no text recogniser for {language}. Add that language in Windows settings, or let text be read online.")
            .replace("{language}", source.name_ru())
    };
    AppError::Other(message)
}

/// A capture made ready for the recogniser: enlarged if its text is likely
/// to be small, and with a margin, because lettering that touches the edge
/// of a picture is lettering a recogniser tends not to see.
struct Prepared<'a> {
    rgba: Cow<'a, [u8]>,
    w: usize,
    h: usize,
    scale: f32,
    margin: f32,
}

fn prepared(frame: &Frame) -> Prepared<'_> {
    let (w, h) = (frame.width(), frame.height());
    let scale = enlargement(w, h);
    // A whole display is left as it is: nothing on it is hard against the
    // edge, and a copy of it is fifteen megabytes.
    let margin = if w * h * scale * scale <= 2_000_000 { 16 } else { 0 };
    if scale == 1 && margin == 0 {
        return Prepared {
            rgba: Cow::Borrowed(frame.rgba()),
            w,
            h,
            scale: 1.0,
            margin: 0.0,
        };
    }

    let (sw, sh) = (w * scale, h * scale);
    let scaled: Cow<'_, [u8]> = if scale == 1 {
        Cow::Borrowed(frame.rgba())
    } else {
        let image = image::RgbaImage::from_raw(w as u32, h as u32, frame.rgba().to_vec());
        match image {
            Some(image) => Cow::Owned(
                image::imageops::resize(
                    &image,
                    sw as u32,
                    sh as u32,
                    image::imageops::FilterType::CatmullRom,
                )
                .into_raw(),
            ),
            None => Cow::Borrowed(frame.rgba()),
        }
    };
    let (sw, sh, scale) = if scaled.len() == sw * sh * 4 {
        (sw, sh, scale)
    } else {
        (w, h, 1)
    };

    // The margin carries on whatever was at the edge, so that it adds room
    // and no edge of its own.
    let (pw, ph) = (sw + margin * 2, sh + margin * 2);
    let mut rgba = vec![0u8; pw * ph * 4];
    for y in 0..ph {
        let from_y = y.saturating_sub(margin).min(sh - 1);
        let row = &scaled[from_y * sw * 4..(from_y + 1) * sw * 4];
        let out = &mut rgba[y * pw * 4..(y + 1) * pw * 4];
        out[margin * 4..(margin + sw) * 4].copy_from_slice(row);
        for x in 0..margin {
            out[x * 4..x * 4 + 4].copy_from_slice(&row[..4]);
            let at = (margin + sw + x) * 4;
            out[at..at + 4].copy_from_slice(&row[(sw - 1) * 4..]);
        }
    }
    Prepared {
        rgba: Cow::Owned(rgba),
        w: pw,
        h: ph,
        scale: scale as f32,
        margin: margin as f32,
    }
}

/// A picture at half the size: every pixel the average of four.
struct Halved {
    rgba: Vec<u8>,
    w: usize,
    h: usize,
}

fn halved(rgba: &[u8], w: usize, h: usize) -> Halved {
    let (hw, hh) = (w / 2, h / 2);
    let mut out = vec![0u8; hw * hh * 4];
    for y in 0..hh {
        let (upper, lower) = (&rgba[y * 2 * w * 4..], &rgba[(y * 2 + 1) * w * 4..]);
        let row = &mut out[y * hw * 4..(y + 1) * hw * 4];
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let at = x * 8;
            for channel in 0..4 {
                let sum = upper[at + channel] as u16
                    + upper[at + 4 + channel] as u16
                    + lower[at + channel] as u16
                    + lower[at + 4 + channel] as u16;
                px[channel] = (sum / 4) as u8;
            }
        }
    }
    Halved {
        rgba: out,
        w: hw,
        h: hh,
    }
}

/// How much to enlarge a capture before reading it. One line of interface
/// text is fourteen or sixteen pixels tall, and reads far better at three
/// times that.
fn enlargement(w: usize, h: usize) -> usize {
    const MOST_AFTER: usize = 4_500_000;
    let mut scale = if h < 40 {
        3
    } else if h < 120 {
        2
    } else {
        1
    };
    while scale > 1 && w * h * scale * scale > MOST_AFTER {
        scale -= 1;
    }
    scale
}

/// Reads a capture with the recogniser for `language`. Blocks for as long
/// as that takes — some hundredths of a second — so it belongs on a worker
/// thread.
///
/// An empty answer is an answer: nothing on the picture read as writing in
/// that language.
pub fn read(frame: &Frame, language: Language) -> Result<Recognised, AppError> {
    let Some((_, tag)) = INSTALLED.iter().find(|(known, _)| *known == language) else {
        return Err(missing(language));
    };
    let (width, height) = (frame.width(), frame.height());
    if width < 2 || height < 2 {
        return Ok(Recognised::default());
    }

    let started = Instant::now();
    let picture = prepared(frame);
    let (w, h) = (picture.w, picture.h);
    if w.max(h) > engine::largest() {
        return Err(AppError::Other(
            t("The area is too large to be read on this computer").to_string(),
        ));
    }
    let gray = Gray::of_rgba(&picture.rgba, w, h);
    let stroke = (strokes::STROKE * picture.scale as usize) | 1;
    let made_ready = started.elapsed();

    // Three readings at once: the recogniser spends its time thinking, not
    // waiting, and three threads of it take hardly longer than one.
    let timed = |what: &'static str, picture: engine::Picture<'_>| {
        let asked = Instant::now();
        let lines = engine::read(tag, picture, w, h);
        let ms = asked.elapsed().as_millis() as u64;
        match lines {
            Ok(lines) => {
                let said: Vec<String> = lines.iter().map(WordLine::text).collect();
                debug!(
                    reading = what,
                    ms,
                    lines = lines.len(),
                    text = %redact(&said.join(" / ")),
                    "Read on this computer"
                );
                Ok(lines)
            }
            Err(error) => {
                warn!(reading = what, error = %error, "The recogniser on this computer failed");
                Err(error)
            }
        }
    };
    let (plain, light, dark) = std::thread::scope(|scope| {
        let light = scope.spawn(|| {
            let page = strokes::light(&gray, stroke);
            timed("light strokes", engine::Picture::Grey(&page.px))
        });
        let dark = scope.spawn(|| {
            let page = strokes::dark(&gray, stroke);
            timed("dark strokes", engine::Picture::Grey(&page.px))
        });
        let plain = timed("as it is", engine::Picture::Colour(&picture.rgba));
        (plain, light.join(), dark.join())
    });

    // The picture as it is has to have been read; the other two only add to
    // it, and a reading that failed adds nothing. Why one failed has gone to
    // the log; the screen only needs to know that it did.
    let plain = plain.map_err(|_| {
        AppError::Other(t("Text recognition is unavailable right now").to_string())
    })?;
    let extra = |reading: std::thread::Result<Result<Vec<WordLine>, String>>| {
        reading.ok().and_then(Result::ok).unwrap_or_default()
    };
    let readings = vec![plain, extra(light), extra(dark)];

    // Back from the enlarged, margined picture to the capture.
    let to_capture = |mut line: WordLine| {
        for word in &mut line.words {
            word.rect.x = (word.rect.x - picture.margin) / picture.scale;
            word.rect.y = (word.rect.y - picture.margin) / picture.scale;
            word.rect.w /= picture.scale;
            word.rect.h /= picture.scale;
        }
        line
    };
    let readings = readings
        .into_iter()
        .map(|lines| lines.into_iter().map(to_capture).collect())
        .collect();

    let mut kept = layout::merged(readings);
    // Whatever is large enough to be in doubt is large enough to be read at
    // half the size, where it is type of an ordinary size and everything
    // else is too small to matter. Unless the picture itself is too small
    // to be halved, and then there is no asking.
    if kept.iter().any(layout::Kept::is_doubtful) && w.min(h) >= 80 {
        let asked = Instant::now();
        let small = halved(&picture.rgba, w, h);
        match engine::read(tag, engine::Picture::Colour(&small.rgba), small.w, small.h) {
            Ok(lines) => {
                let again: Vec<WordLine> = lines
                    .into_iter()
                    .map(|mut line| {
                        for word in &mut line.words {
                            word.rect.x *= 2.0;
                            word.rect.y *= 2.0;
                            word.rect.w *= 2.0;
                            word.rect.h *= 2.0;
                        }
                        to_capture(line)
                    })
                    .collect();
                let before = kept.len();
                kept.retain(|line| !line.is_doubtful() || line.is_borne_out_by(&again));
                debug!(
                    ms = asked.elapsed().as_millis() as u64,
                    dropped = before - kept.len(),
                    "Large type read again at half the size"
                );
            }
            // Not having been able to ask is no reason to throw text away.
            Err(error) => warn!(error = %error, "Could not read large type a second time"),
        }
    }
    let lines: Vec<WordLine> = kept.into_iter().map(|kept| kept.line).collect();
    let blocks = layout::blocks(&lines, width as f32);
    debug!(
        ms = started.elapsed().as_millis() as u64,
        ready_ms = made_ready.as_millis() as u64,
        lines = lines.len(),
        blocks = blocks.len(),
        scale = picture.scale,
        "Recognised on this computer"
    );
    Ok(Recognised {
        blocks,
        detected: None,
    })
}

#[cfg(windows)]
mod engine {
    //! The recogniser itself: `Windows.Media.Ocr`.

    use std::collections::HashMap;
    use std::sync::Mutex;

    use once_cell::sync::Lazy;
    use windows::core::{Interface, HSTRING};
    use windows::Globalization::Language as Tongue;
    use windows::Graphics::Imaging::{
        BitmapAlphaMode, BitmapBufferAccessMode, BitmapPixelFormat, SoftwareBitmap,
    };
    use windows::Media::Ocr::{OcrEngine, OcrResult};
    use windows::Win32::System::WinRT::IMemoryBufferByteAccess;

    use super::super::layout::{Word, WordLine};
    use super::super::PxRect;

    pub enum Picture<'a> {
        /// RGBA, four bytes a pixel.
        Colour(&'a [u8]),
        /// One byte a pixel.
        Grey(&'a [u8]),
    }

    /// The names of the recognisers Windows has installed: `en-US`, `ru`.
    pub fn tags() -> Vec<String> {
        let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else {
            return Vec::new();
        };
        list.into_iter()
            .filter_map(|language| language.LanguageTag().ok())
            .map(|tag| tag.to_string_lossy())
            .collect()
    }

    /// The longest side of a picture the recogniser accepts.
    pub fn largest() -> usize {
        OcrEngine::MaxImageDimension().map_or(2600, |side| side as usize)
    }

    /// Recognisers already made, by language. Making one loads its model,
    /// which is worth doing once; and one recogniser is not asked two things
    /// at a time, so there are as many as have ever been wanted at once.
    static IDLE: Lazy<Mutex<HashMap<String, Vec<OcrEngine>>>> = Lazy::new(Default::default);

    fn borrow(tag: &str) -> windows::core::Result<OcrEngine> {
        let waiting = IDLE.lock().ok().and_then(|mut idle| idle.get_mut(tag)?.pop());
        match waiting {
            Some(engine) => Ok(engine),
            None => OcrEngine::TryCreateFromLanguage(&Tongue::CreateLanguage(&HSTRING::from(tag))?),
        }
    }

    fn give_back(tag: &str, engine: OcrEngine) {
        if let Ok(mut idle) = IDLE.lock() {
            idle.entry(tag.to_string()).or_default().push(engine);
        }
    }

    pub fn read(tag: &str, picture: Picture<'_>, w: usize, h: usize) -> Result<Vec<WordLine>, String> {
        let engine = borrow(tag).map_err(|e| e.to_string())?;
        let outcome = (|| {
            let bitmap = bitmap(&picture, w, h)?;
            let result = engine.RecognizeAsync(&bitmap)?.get()?;
            lines(&result, w as f32, h as f32)
        })();
        give_back(tag, engine);
        outcome.map_err(|e: windows::core::Error| e.to_string())
    }

    /// Copies the pixels into a bitmap of the kind the recogniser takes.
    fn bitmap(picture: &Picture<'_>, w: usize, h: usize) -> windows::core::Result<SoftwareBitmap> {
        let (format, each) = match picture {
            Picture::Colour(_) => (BitmapPixelFormat::Bgra8, 4),
            Picture::Grey(_) => (BitmapPixelFormat::Gray8, 1),
        };
        let bitmap = SoftwareBitmap::CreateWithAlpha(format, w as i32, h as i32, BitmapAlphaMode::Ignore)?;
        {
            let buffer = bitmap.LockBuffer(BitmapBufferAccessMode::Write)?;
            let plane = buffer.GetPlaneDescription(0)?;
            let reference = buffer.CreateReference()?;
            let access: IMemoryBufferByteAccess = reference.cast()?;
            let (mut data, mut capacity) = (std::ptr::null_mut::<u8>(), 0u32);
            // SAFETY: the reference is alive for as long as the slice made
            // from it is used, and the bitmap is locked for writing.
            unsafe { access.GetBuffer(&mut data, &mut capacity)? };
            let (start, stride) = (plane.StartIndex.max(0) as usize, plane.Stride.max(0) as usize);
            let needed = start + stride * h.saturating_sub(1) + w * each;
            if data.is_null() || stride < w * each || (capacity as usize) < needed {
                return Err(windows::core::Error::from(windows::Win32::Foundation::E_UNEXPECTED));
            }
            // SAFETY: `capacity` bytes at `data` are the bitmap's own, and
            // every row written below was checked to lie inside them.
            let bytes = unsafe { std::slice::from_raw_parts_mut(data, capacity as usize) };
            for y in 0..h {
                let row = &mut bytes[start + y * stride..start + y * stride + w * each];
                match picture {
                    Picture::Grey(px) => row.copy_from_slice(&px[y * w..(y + 1) * w]),
                    Picture::Colour(rgba) => {
                        let source = &rgba[y * w * 4..(y + 1) * w * 4];
                        for (out, px) in row.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
                            out[0] = px[2];
                            out[1] = px[1];
                            out[2] = px[0];
                            out[3] = 255;
                        }
                    }
                }
            }
            let _ = reference.Close();
            let _ = buffer.Close();
        }
        Ok(bitmap)
    }

    /// What was read, word by word, where it stands on the picture.
    ///
    /// The recogniser straightens a picture it takes to be tilted before it
    /// reads it, and answers in the coordinates of the straightened one. A
    /// frame of a game is never tilted, so a tilt is a misjudgement — one
    /// that every box then carries. Turning each box back by the angle the
    /// recogniser reports puts them where the words are.
    fn lines(result: &OcrResult, w: f32, h: f32) -> windows::core::Result<Vec<WordLine>> {
        let angle = result
            .TextAngle()
            .and_then(|angle| angle.Value())
            .unwrap_or(0.0)
            .to_radians();
        let (sin, cos) = (angle.sin() as f32, angle.cos() as f32);
        let (cx, cy) = (w / 2.0, h / 2.0);

        let mut out = Vec::new();
        for line in result.Lines()? {
            let mut words = Vec::new();
            for word in line.Words()? {
                let rect = word.BoundingRect()?;
                let text = word.Text()?.to_string_lossy();
                if text.trim().is_empty() || rect.Width <= 0.0 || rect.Height <= 0.0 {
                    continue;
                }
                let (dx, dy) = (rect.X + rect.Width / 2.0 - cx, rect.Y + rect.Height / 2.0 - cy);
                let (x, y) = (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos);
                words.push(Word {
                    rect: PxRect::new(x - rect.Width / 2.0, y - rect.Height / 2.0, rect.Width, rect.Height),
                    text,
                });
            }
            if !words.is_empty() {
                out.push(WordLine { words });
            }
        }
        Ok(out)
    }
}

#[cfg(not(windows))]
mod engine {
    //! No recogniser of the system's own is used on this platform yet, so
    //! there is no language it reads and nothing is ever asked of it.

    use super::super::layout::WordLine;

    pub enum Picture<'a> {
        Colour(&'a [u8]),
        Grey(&'a [u8]),
    }

    pub fn tags() -> Vec<String> {
        Vec::new()
    }

    pub fn largest() -> usize {
        0
    }

    pub fn read(_tag: &str, picture: Picture<'_>, _w: usize, _h: usize) -> Result<Vec<WordLine>, String> {
        let (Picture::Colour(px) | Picture::Grey(px)) = picture;
        let _ = px;
        Err("no recogniser on this platform".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::capture::Bounds;

    use Language::{Auto, De, En, Ja, Ru};

    #[test]
    fn a_source_that_was_named_is_read_as_that_or_not_here_at_all() {
        assert_eq!(choose(&[En, Ru], De, Ru), None);
        assert_eq!(choose(&[En, Ru, De], De, Ru), Some(De));
        assert_eq!(choose(&[En, Ru], Ru, En), Some(Ru));
    }

    #[test]
    fn a_source_left_to_be_detected_is_read_as_what_is_not_the_target() {
        // The usual case: a Russian Windows, an English game.
        assert_eq!(choose(&[Ru, En], Auto, Ru), Some(En));
        // Translating into English, it is the other one.
        assert_eq!(choose(&[En, Ru], Auto, En), Some(Ru));
        // English first among several.
        assert_eq!(choose(&[Ja, De, En], Auto, Ru), Some(En));
        assert_eq!(choose(&[Ja, De], Auto, Ru), Some(Ja));
    }

    #[test]
    fn with_nothing_but_the_target_language_there_is_nothing_to_read_with() {
        assert_eq!(choose(&[Ru], Auto, Ru), None);
        assert_eq!(choose(&[], Auto, Ru), None);
        assert_eq!(choose(&[], En, Ru), None);
    }

    #[test]
    fn one_line_of_text_is_enlarged_and_a_whole_display_is_not() {
        assert_eq!(enlargement(400, 18), 3);
        assert_eq!(enlargement(600, 90), 2);
        assert_eq!(enlargement(2560, 1440), 1);
        // And not into something enormous.
        assert_eq!(enlargement(20_000, 100), 1);
    }

    fn frame(w: i32, h: i32) -> Frame {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 7, 255]);
            }
        }
        Frame::from_rgba(Bounds { x: 0, y: 0, w, h }, rgba)
    }

    #[test]
    fn a_small_capture_is_given_a_margin_that_carries_on_its_edges() {
        let shot = frame(300, 200);
        let picture = prepared(&shot);
        assert_eq!((picture.w, picture.h), (332, 232));
        assert_eq!((picture.scale, picture.margin), (1.0, 16.0));
        let at = |x: usize, y: usize| &picture.rgba[(y * picture.w + x) * 4..(y * picture.w + x) * 4 + 4];
        // The capture itself, moved in by the margin.
        assert_eq!(at(16, 16), [0, 0, 7, 255]);
        assert_eq!(at(16 + 299, 16 + 199), [43, 199, 7, 255]);
        // The margin repeats the nearest pixel of the capture.
        assert_eq!(at(0, 0), [0, 0, 7, 255]);
        assert_eq!(at(331, 100), [43, 84, 7, 255]);
        assert_eq!(at(100, 231), [84, 199, 7, 255]);
    }

    #[test]
    fn a_line_of_text_is_enlarged_as_well() {
        let shot = frame(200, 30);
        let picture = prepared(&shot);
        assert_eq!(picture.scale, 3.0);
        assert_eq!((picture.w, picture.h), (200 * 3 + 32, 30 * 3 + 32));
    }

    #[test]
    fn a_picture_at_half_the_size_averages_its_pixels_four_by_four() {
        // Two by two of one colour, then two by two of another; and a row
        // and a column left over, which are dropped.
        let mut rgba = Vec::new();
        for y in 0..3 {
            for x in 0..5 {
                let value = if x < 2 { 40 } else { 200 };
                rgba.extend_from_slice(&[value, (y * 10) as u8, 0, 255]);
            }
        }
        let small = halved(&rgba, 5, 3);
        assert_eq!((small.w, small.h), (2, 1));
        assert_eq!(small.rgba, [40, 5, 0, 255, 200, 5, 0, 255]);
    }

    #[test]
    fn a_whole_display_is_read_as_it_is() {
        let shot = frame(2000, 1200);
        let picture = prepared(&shot);
        assert!(matches!(picture.rgba, Cow::Borrowed(_)));
        assert_eq!((picture.w, picture.h, picture.scale, picture.margin), (2000, 1200, 1.0, 0.0));
    }

    /// Reads pictures from disk with the recogniser of the machine the test
    /// is run on, and prints what it made of them: for working on how its
    /// lines are put together, on real pictures.
    ///
    /// `SAKURA_OCR_PROBE` holds the paths, separated by `;`, and
    /// `SAKURA_OCR_LANG` the language, `en` unless said otherwise.
    #[test]
    #[ignore = "needs pictures, and a recogniser installed in Windows"]
    fn probe() {
        let Ok(paths) = std::env::var("SAKURA_OCR_PROBE") else {
            return;
        };
        let language = std::env::var("SAKURA_OCR_LANG")
            .ok()
            .and_then(|code| Language::from_code(&code))
            .unwrap_or(En);
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();
        crate::shared::logging::set_verbose(true);
        println!("recognisers: {:?}", languages());
        for path in paths.split(';').filter(|path| !path.is_empty()) {
            let image = image::open(path).expect("picture").to_rgba8();
            let (w, h) = image.dimensions();
            let shot = Frame::from_rgba(
                Bounds { x: 0, y: 0, w: w as i32, h: h as i32 },
                image.into_raw(),
            );
            // Twice: the first read of a language also loads its model.
            let _ = read(&shot, language);
            let asked = Instant::now();
            let found = read(&shot, language).expect("read");
            println!("== {path}  {w}x{h}  {} ms", asked.elapsed().as_millis());
            for block in &found.blocks {
                println!(
                    "   [{:>5.0},{:>5.0} {:>4.0}x{:<3.0}] glyph {:>4.1} lines {} {:?}  {}",
                    block.rect.x,
                    block.rect.y,
                    block.rect.w,
                    block.rect.h,
                    block.glyph_height,
                    block.lines,
                    block.align,
                    block.text
                );
            }
        }
    }
}
