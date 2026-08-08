//! Screen capture and the coordinate system everything else works in.
//!
//! The original code took `GetSystemMetrics(SM_CXSCREEN)` as "the screen" and
//! fed egui's logical points straight into `BitBlt` as physical pixels. Both
//! assumptions break on ordinary hardware:
//!
//! * on a 150 % display every captured rectangle was two-thirds the size the
//!   user drew, cropping the right and bottom off the text;
//! * on a second monitor — especially one placed left of or above the primary,
//!   where desktop coordinates go negative — the capture came from the wrong
//!   place entirely, or came back black.
//!
//! So: one coordinate system, stated once. **Desktop physical pixels**, origin
//! at the top-left of the virtual desktop, which may be negative. egui points
//! are converted at the boundary and nowhere else.

use std::io::Cursor;

use image::{ImageBuffer, RgbImage};
use tracing::{debug, warn};

use crate::shared::error::AppError;

#[cfg(windows)]
use windows::Win32::Foundation::{HWND, RECT};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HDC, SRCCOPY,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowRect, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// A rectangle in desktop physical pixels. `x`/`y` may be negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Bounds {
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Intersection, or `None` when the two do not overlap at all.
    pub fn clamp_to(&self, outer: Bounds) -> Option<Bounds> {
        let x = self.x.max(outer.x);
        let y = self.y.max(outer.y);
        let right = self.right().min(outer.right());
        let bottom = self.bottom().min(outer.bottom());
        if right <= x || bottom <= y {
            return None;
        }
        Some(Bounds {
            x,
            y,
            w: right - x,
            h: bottom - y,
        })
    }
}

/// The full virtual desktop: every monitor, as one rectangle.
pub fn virtual_desktop() -> Bounds {
    #[cfg(windows)]
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w > 0 && h > 0 {
            return Bounds { x, y, w, h };
        }
        warn!("SM_C*VIRTUALSCREEN returned nothing usable; falling back to 1920×1080");
        Bounds {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        }
    }
    #[cfg(not(windows))]
    {
        portable::virtual_desktop().unwrap_or_else(|| {
            warn!("No monitor could be enumerated; falling back to 1920×1080");
            Bounds {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
            }
        })
    }
}

/// Where a normal window may sit: the desktop minus the taskbar, the menu bar
/// and the Dock, in desktop physical pixels.
///
/// The capture overlay covers the whole desktop on purpose. Everything else —
/// the result popup, the floating window — belongs inside this, or it ends up
/// half under the Dock with its last line unreadable.
pub fn work_area() -> Bounds {
    let desktop = virtual_desktop();

    #[cfg(windows)]
    {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{
            SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
        };

        let mut rect = RECT::default();
        let ok = unsafe {
            SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut rect as *mut RECT as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        };
        if ok.is_ok() {
            let area = Bounds {
                x: rect.left,
                y: rect.top,
                w: rect.right - rect.left,
                h: rect.bottom - rect.top,
            };
            // Only covers the primary monitor, so it is an intersection rather
            // than a replacement: a second screen keeps its full height.
            if area.w > 0 && area.h > 0 {
                return area.clamp_to(desktop).unwrap_or(desktop);
            }
        }
        debug!("SPI_GETWORKAREA gave nothing usable; using the whole desktop");
        desktop
    }

    #[cfg(target_os = "macos")]
    {
        let Some((x, y, w, h)) = super::mac_window::work_area_points() else {
            return desktop;
        };
        // AppKit answers in points; everything here is in desktop physical
        // pixels. The menu bar and the Dock are on the primary screen, so its
        // scale is the one that applies.
        let scale = portable::primary_scale() as f64;
        let area = Bounds {
            x: desktop.x + (x * scale).round() as i32,
            y: desktop.y + (y * scale).round() as i32,
            w: (w * scale).round() as i32,
            h: (h * scale).round() as i32,
        };
        area.clamp_to(desktop).unwrap_or(desktop)
    }

    // No portable way to ask X11 or Wayland for the panel geometry, and guessing
    // is worse than the status quo.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        desktop
    }
}

/// Frame of the window that currently has focus, in desktop physical pixels.
///
/// Captured *before* the overlay appears — once the overlay is up it is itself
/// the foreground window.
pub fn foreground_window_bounds() -> Option<Bounds> {
    #[cfg(windows)]
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        let b = Bounds {
            x: r.left,
            y: r.top,
            w: r.right - r.left,
            h: r.bottom - r.top,
        };
        if b.w < 16 || b.h < 16 {
            return None;
        }
        b.clamp_to(virtual_desktop())
    }
    #[cfg(not(windows))]
    {
        portable::focused_window_bounds().and_then(|b| b.clamp_to(virtual_desktop()))
    }
}

// ── Raw capture ──────────────────────────────────────────────────────────────

/// BGRA pixels straight from GDI, plus the size they came back at.
struct RawCapture {
    bgra: Vec<u8>,
    w: i32,
    h: i32,
}

#[cfg(windows)]
/// Owns a device context and a bitmap so that every early return still releases
/// them. The original code repeated the cleanup at each failure point and missed
/// it on one, leaking a GDI bitmap per failed capture.
struct GdiCapture {
    screen_dc: HDC,
    mem_dc: HDC,
    bmp: HBITMAP,
}

#[cfg(windows)]
impl Drop for GdiCapture {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.mem_dc);
            ReleaseDC(HWND(std::ptr::null_mut()), self.screen_dc);
        }
    }
}

#[cfg(windows)]
fn grab(area: Bounds) -> Result<RawCapture, AppError> {
    unsafe {
        let screen_dc = GetDC(HWND(std::ptr::null_mut()));
        if screen_dc.0.is_null() {
            return Err(AppError::Other("GetDC не выдал контекст экрана".into()));
        }

        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.0.is_null() {
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return Err(AppError::Other("CreateCompatibleDC не сработал".into()));
        }

        let bmp = CreateCompatibleBitmap(screen_dc, area.w, area.h);
        if bmp.0.is_null() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
            return Err(AppError::Other("CreateCompatibleBitmap не сработал".into()));
        }

        let _guard = GdiCapture {
            screen_dc,
            mem_dc,
            bmp,
        };

        let old = SelectObject(mem_dc, bmp);
        let blit = BitBlt(
            mem_dc, 0, 0, area.w, area.h, screen_dc, area.x, area.y, SRCCOPY,
        );
        // GetDIBits documents that the bitmap must *not* be selected into a DC
        // when it is called, so the selection is undone the moment BitBlt is
        // done with it rather than at scope exit.
        SelectObject(mem_dc, old);
        blit.map_err(|e| AppError::Other(format!("BitBlt не сработал: {e}")))?;

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: area.w,
                // Negative height = top-down rows, which saves a vertical flip.
                biHeight: -area.h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            bmiColors: [Default::default(); 1],
        };

        // i32 multiplication would overflow on an absurdly large virtual
        // desktop; do the arithmetic in usize.
        let len = (area.w as usize)
            .checked_mul(area.h as usize)
            .and_then(|p| p.checked_mul(4))
            .ok_or_else(|| AppError::Other("область слишком велика".into()))?;
        let mut bgra = vec![0u8; len];

        let scan_lines = GetDIBits(
            mem_dc,
            bmp,
            0,
            area.h as u32,
            Some(bgra.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );

        if scan_lines == 0 {
            return Err(AppError::Other("GetDIBits вернул 0 строк".into()));
        }

        Ok(RawCapture {
            bgra,
            w: area.w,
            h: area.h,
        })
    }
}

#[cfg(not(windows))]
fn grab(area: Bounds) -> Result<RawCapture, AppError> {
    portable::grab(area)
}

/// Monitor scale factors, for converting `xcap`'s logical geometry. Only the
/// platforms that go through `xcap` have one.
#[cfg(not(windows))]
pub use portable::ScaleMap;

/// macOS and Linux capture, on top of `xcap` — ScreenCaptureKit / CoreGraphics
/// on macOS, X11 or the Wayland portal on Linux.
///
/// `xcap` speaks logical points for geometry but returns physical pixels, so
/// every rectangle is multiplied by the monitor's scale factor on the way in.
/// Past this module the rest of the app only ever sees desktop physical pixels,
/// exactly as it does on Windows.
#[cfg(not(windows))]
mod portable {
    use super::{Bounds, RawCapture};
    use crate::shared::error::AppError;
    use tracing::{debug, warn};
    use xcap::image::RgbaImage;
    use xcap::Monitor;

    /// A monitor with its frame already converted to desktop physical pixels.
    struct Screen {
        bounds: Bounds,
        scale: f32,
        monitor: Monitor,
    }

    fn screens() -> Vec<Screen> {
        let monitors = match Monitor::all() {
            Ok(m) => m,
            Err(e) => {
                warn!(error = %e, "Could not enumerate monitors");
                return Vec::new();
            }
        };

        monitors
            .into_iter()
            .filter_map(|monitor| {
                let scale = match monitor.scale_factor() {
                    Ok(s) if s > 0.0 => s,
                    _ => 1.0,
                };
                let x = monitor.x().ok()?;
                let y = monitor.y().ok()?;
                let w = monitor.width().ok()? as i32;
                let h = monitor.height().ok()? as i32;
                if w <= 0 || h <= 0 {
                    return None;
                }
                Some(Screen {
                    bounds: Bounds {
                        x: scaled(x, scale),
                        y: scaled(y, scale),
                        w: scaled(w, scale),
                        h: scaled(h, scale),
                    },
                    scale,
                    monitor,
                })
            })
            .collect()
    }

    fn scaled(v: i32, scale: f32) -> i32 {
        (v as f32 * scale).round() as i32
    }

    /// Scale factor of the primary monitor, for the platform geometry that is
    /// only ever reported for that one — the menu bar and the Dock.
    pub fn primary_scale() -> f32 {
        let screens = screens();
        screens
            .iter()
            .find(|s| s.monitor.is_primary().unwrap_or(false))
            .or_else(|| screens.first())
            .map(|s| s.scale)
            .unwrap_or(1.0)
    }

    /// Union of every monitor, in desktop physical pixels.
    pub fn virtual_desktop() -> Option<Bounds> {
        let screens = screens();
        let (first, rest) = screens.split_first()?;
        let mut union = first.bounds;
        for s in rest {
            let x = union.x.min(s.bounds.x);
            let y = union.y.min(s.bounds.y);
            let right = union.right().max(s.bounds.right());
            let bottom = union.bottom().max(s.bounds.bottom());
            union = Bounds {
                x,
                y,
                w: right - x,
                h: bottom - y,
            };
        }
        Some(union)
    }

    /// Frame of the focused window, if the platform will say which one it is.
    pub fn focused_window_bounds() -> Option<Bounds> {
        let windows = xcap::Window::all().ok()?;
        // Monitors are needed for the scale factor: window geometry, like
        // monitor geometry, comes back in logical points.
        let screens = screens();

        for w in windows {
            if !w.is_focused().unwrap_or(false) || w.is_minimized().unwrap_or(false) {
                continue;
            }
            let (x, y) = (w.x().ok()?, w.y().ok()?);
            let (width, height) = (w.width().ok()? as i32, w.height().ok()? as i32);
            let scale = scale_at(&screens, x, y);
            let bounds = Bounds {
                x: scaled(x, scale),
                y: scaled(y, scale),
                w: scaled(width, scale),
                h: scaled(height, scale),
            };
            if bounds.w < 16 || bounds.h < 16 {
                continue;
            }
            return Some(bounds);
        }
        None
    }

    /// The monitor layout, taken once, for callers that convert a whole list of
    /// rectangles — enumerating monitors per window would be a syscall each.
    pub struct ScaleMap {
        /// Logical frame of each monitor, with its scale factor.
        monitors: Vec<(Bounds, f32)>,
    }

    impl ScaleMap {
        pub fn new() -> Self {
            let monitors = screens()
                .iter()
                .map(|s| {
                    let logical = Bounds {
                        x: (s.bounds.x as f32 / s.scale).round() as i32,
                        y: (s.bounds.y as f32 / s.scale).round() as i32,
                        w: (s.bounds.w as f32 / s.scale).round() as i32,
                        h: (s.bounds.h as f32 / s.scale).round() as i32,
                    };
                    (logical, s.scale)
                })
                .collect();
            ScaleMap { monitors }
        }

        /// Scale factor of whichever monitor holds this logical point, so a
        /// mixed-DPI desktop converts each window with its own factor.
        pub fn at(&self, logical_x: i32, logical_y: i32) -> f32 {
            for (frame, scale) in &self.monitors {
                if logical_x >= frame.x
                    && logical_x < frame.right()
                    && logical_y >= frame.y
                    && logical_y < frame.bottom()
                {
                    return *scale;
                }
            }
            self.monitors.first().map(|(_, s)| *s).unwrap_or(1.0)
        }
    }

    /// Scale factor of whichever monitor holds this logical point, so that a
    /// mixed-DPI desktop converts each window with its own factor.
    fn scale_at(screens: &[Screen], logical_x: i32, logical_y: i32) -> f32 {
        for s in screens {
            let lx = (s.bounds.x as f32 / s.scale).round() as i32;
            let ly = (s.bounds.y as f32 / s.scale).round() as i32;
            let lw = (s.bounds.w as f32 / s.scale).round() as i32;
            let lh = (s.bounds.h as f32 / s.scale).round() as i32;
            if logical_x >= lx && logical_x < lx + lw && logical_y >= ly && logical_y < ly + lh {
                return s.scale;
            }
        }
        screens.first().map(|s| s.scale).unwrap_or(1.0)
    }

    /// Same contract as the GDI path: BGRA at exactly `area`'s size, stitched
    /// from every monitor the rectangle touches.
    pub fn grab(area: Bounds) -> Result<RawCapture, AppError> {
        if area.w <= 0 || area.h <= 0 {
            return Err(AppError::Other("пустая область захвата".into()));
        }

        let screens = screens();
        if screens.is_empty() {
            return Err(AppError::Other("не найден ни один монитор".into()));
        }

        let mut bgra = vec![0u8; area.w as usize * area.h as usize * 4];
        let mut captured_any = false;
        let mut last_error = None;

        for s in &screens {
            let Some(hit) = area.clamp_to(s.bounds) else {
                continue;
            };
            // Monitor-relative, and back in logical points for xcap.
            let lx = ((hit.x - s.bounds.x) as f32 / s.scale).round().max(0.0) as u32;
            let ly = ((hit.y - s.bounds.y) as f32 / s.scale).round().max(0.0) as u32;
            let lw = ((hit.w as f32 / s.scale).round() as u32).max(1);
            let lh = ((hit.h as f32 / s.scale).round() as u32).max(1);

            match s.monitor.capture_region(lx, ly, lw, lh) {
                Ok(image) => {
                    blit(&image, hit, area, &mut bgra);
                    captured_any = true;
                }
                Err(e) => {
                    warn!(error = %e, "Capture from one monitor failed");
                    last_error = Some(e.to_string());
                }
            }
        }

        if !captured_any {
            return Err(AppError::Other(last_error.unwrap_or_else(|| {
                "область захвата не попала ни на один монитор".into()
            })));
        }

        debug!(w = area.w, h = area.h, "Captured via xcap");
        Ok(RawCapture {
            bgra,
            w: area.w,
            h: area.h,
        })
    }

    /// Copies one monitor's slice into the output buffer, converting RGBA to
    /// BGRA. The source is sampled rather than copied row-for-row because
    /// rounding a scaled rectangle can leave it a pixel off the requested size.
    fn blit(image: &RgbaImage, hit: Bounds, area: Bounds, out: &mut [u8]) {
        let (iw, ih) = (image.width() as i32, image.height() as i32);
        if iw <= 0 || ih <= 0 {
            return;
        }
        let src = image.as_raw();

        for row in 0..hit.h {
            let dy = hit.y - area.y + row;
            if dy < 0 || dy >= area.h {
                continue;
            }
            let sy = (row * ih / hit.h).clamp(0, ih - 1);
            for col in 0..hit.w {
                let dx = hit.x - area.x + col;
                if dx < 0 || dx >= area.w {
                    continue;
                }
                let sx = (col * iw / hit.w).clamp(0, iw - 1);
                let si = ((sy * iw + sx) * 4) as usize;
                let di = ((dy * area.w + dx) * 4) as usize;
                out[di] = src[si + 2];
                out[di + 1] = src[si + 1];
                out[di + 2] = src[si];
                out[di + 3] = 255;
            }
        }
    }
}

// ── Public capture API ───────────────────────────────────────────────────────

/// The whole virtual desktop as an egui image, for the frozen backdrop.
pub fn capture_desktop_image() -> Result<(egui::ColorImage, Bounds), AppError> {
    let area = virtual_desktop();
    let raw = grab(area)?;

    let pixels: Vec<egui::Color32> = raw
        .bgra
        .chunks_exact(4)
        .map(|c| egui::Color32::from_rgb(c[2], c[1], c[0]))
        .collect();

    Ok((
        egui::ColorImage {
            size: [raw.w as usize, raw.h as usize],
            pixels,
        },
        area,
    ))
}

/// Captures `area` and encodes it for OCR.
pub fn capture_region_for_ocr(area: Bounds) -> Result<Vec<u8>, AppError> {
    let Some(area) = area.clamp_to(virtual_desktop()) else {
        return Err(AppError::Other(
            "выделенная область целиком за пределами экрана".into(),
        ));
    };
    if area.w < MIN_SIDE || area.h < MIN_SIDE {
        return Err(AppError::Other(format!(
            "Слишком маленькая область: {}×{} (минимум {MIN_SIDE}×{MIN_SIDE})",
            area.w, area.h
        )));
    }

    let raw = grab(area)?;

    let mut rgb = Vec::with_capacity((raw.w as usize) * (raw.h as usize) * 3);
    for c in raw.bgra.chunks_exact(4) {
        rgb.push(c[2]);
        rgb.push(c[1]);
        rgb.push(c[0]);
    }

    let img: RgbImage = ImageBuffer::from_raw(raw.w as u32, raw.h as u32, rgb)
        .ok_or_else(|| AppError::Other("не удалось собрать изображение".into()))?;

    encode_for_ocr(img)
}

pub const MIN_SIDE: i32 = 8;

/// Upscales small captures before encoding.
///
/// OCR accuracy falls off a cliff below roughly 20 px of glyph height, and a
/// selection around a single line of UI text is often 14–16 px tall. Resampling
/// up costs a millisecond and turns "no text found" into a clean read. Quality
/// is raised to 92 for the same reason: JPEG ringing around small glyphs is
/// exactly the artefact that confuses a recogniser.
fn encode_for_ocr(img: RgbImage) -> Result<Vec<u8>, AppError> {
    use image::codecs::jpeg::JpegEncoder;
    use image::imageops::FilterType;

    let (w, h) = img.dimensions();
    let scale = upscale_factor(w, h);

    let img = if scale > 1 {
        debug!(from = ?(w, h), scale, "Upscaling capture for OCR");
        image::imageops::resize(&img, w * scale, h * scale, FilterType::Lanczos3)
    } else {
        img
    };

    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);
    JpegEncoder::new_with_quality(&mut cursor, 92)
        .encode_image(&img)
        .map_err(AppError::Image)?;
    Ok(out)
}

/// Small captures get scaled up; large ones are left alone so a full-screen
/// grab does not turn into a 40 MB upload.
fn upscale_factor(w: u32, h: u32) -> u32 {
    const MAX_PIXELS_AFTER: u64 = 4_500_000;
    let pixels = (w as u64) * (h as u64);

    let wanted = if h < 40 {
        3
    } else if h < 120 {
        2
    } else {
        1
    };

    let mut scale = wanted;
    while scale > 1 && pixels * (scale as u64) * (scale as u64) > MAX_PIXELS_AFTER {
        scale -= 1;
    }
    scale
}

/// Live capture, checked against the machine it runs on.
///
/// Ignored by default: a CI runner has no display, and on macOS the first call
/// raises the Screen Recording prompt. Run it with
/// `cargo test -- --ignored capture` after granting the permission.
#[cfg(all(test, not(windows)))]
mod live_tests {
    use super::*;

    #[test]
    #[ignore = "needs a real display and, on macOS, Screen Recording permission"]
    fn the_desktop_capture_comes_back_at_the_size_that_was_asked_for() {
        let desktop = virtual_desktop();
        assert!(desktop.w > 0 && desktop.h > 0, "no usable desktop bounds");

        let (image, area) = capture_desktop_image().expect("desktop capture");
        assert_eq!(area, desktop);
        assert_eq!(image.size, [desktop.w as usize, desktop.h as usize]);
        assert_eq!(image.pixels.len(), (desktop.w * desktop.h) as usize);
    }

    #[test]
    #[ignore = "needs a real display and, on macOS, Screen Recording permission"]
    fn a_region_in_the_middle_of_the_desktop_captures_that_region() {
        let desktop = virtual_desktop();
        let region = Bounds {
            x: desktop.x + desktop.w / 4,
            y: desktop.y + desktop.h / 4,
            w: 320,
            h: 200,
        };
        // The OCR path re-encodes as JPEG, so a non-empty result means the
        // whole chain — stitch, convert, encode — held together.
        let jpeg = capture_region_for_ocr(region).expect("region capture");
        assert!(jpeg.len() > 1024, "suspiciously small JPEG: {}", jpeg.len());
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "not a JPEG");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: Bounds = Bounds {
        x: -1920,
        y: 0,
        w: 3840,
        h: 1080,
    };

    #[test]
    fn a_selection_inside_the_desktop_is_unchanged() {
        let sel = Bounds {
            x: 10,
            y: 10,
            w: 100,
            h: 50,
        };
        assert_eq!(sel.clamp_to(DESKTOP), Some(sel));
    }

    #[test]
    fn negative_coordinates_are_valid_on_a_left_hand_monitor() {
        let sel = Bounds {
            x: -1900,
            y: 5,
            w: 200,
            h: 100,
        };
        assert_eq!(sel.clamp_to(DESKTOP), Some(sel));
    }

    #[test]
    fn a_selection_running_off_the_edge_is_trimmed_not_rejected() {
        let sel = Bounds {
            x: 1800,
            y: 1000,
            w: 500,
            h: 500,
        };
        assert_eq!(
            sel.clamp_to(DESKTOP),
            Some(Bounds {
                x: 1800,
                y: 1000,
                w: 120,
                h: 80
            })
        );
    }

    #[test]
    fn a_selection_entirely_off_screen_is_rejected() {
        let sel = Bounds {
            x: 5000,
            y: 0,
            w: 100,
            h: 100,
        };
        assert_eq!(sel.clamp_to(DESKTOP), None);
    }

    #[test]
    fn a_zero_width_overlap_is_rejected_rather_than_producing_an_empty_bitmap() {
        let sel = Bounds {
            x: 1920,
            y: 0,
            w: 100,
            h: 100,
        };
        // Touches the right edge exactly; nothing to capture.
        let outer = Bounds {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        assert_eq!(sel.clamp_to(outer), None);
    }

    #[test]
    fn a_single_line_of_text_gets_scaled_up() {
        assert_eq!(upscale_factor(400, 18), 3);
        assert_eq!(upscale_factor(600, 90), 2);
    }

    #[test]
    fn a_full_screen_grab_is_not_scaled() {
        assert_eq!(upscale_factor(3840, 2160), 1);
    }

    #[test]
    fn scaling_backs_off_before_producing_an_enormous_upload() {
        // 2000×100 at ×2 would be 800k pixels — fine. At a wider input it drops.
        assert_eq!(upscale_factor(20000, 100), 1);
    }
}
