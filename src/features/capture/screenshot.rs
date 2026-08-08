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
        Bounds {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        }
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
        None
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
fn grab(_area: Bounds) -> Result<RawCapture, AppError> {
    Err(AppError::Other(
        "захват экрана на этой платформе не реализован".into(),
    ))
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
