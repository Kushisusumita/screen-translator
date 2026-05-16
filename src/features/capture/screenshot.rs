use crate::shared::error::AppError;
use image::{ImageBuffer, RgbImage};
use std::io::Cursor;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

pub unsafe fn capture_region(x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>, AppError> {
    if w < 8 || h < 8 {
        return Err(AppError::Other(format!(
            "Selection too small: {}x{} (minimum 8×8 pixels)",
            w, h
        )));
    }

    let screen_dc = GetDC(HWND(std::ptr::null_mut()));
    if screen_dc.0.is_null() {
        return Err(AppError::Other("GetDC failed".to_string()));
    }

    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.0.is_null() {
        ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
        return Err(AppError::Other("CreateCompatibleDC failed".to_string()));
    }

    let bmp = CreateCompatibleBitmap(screen_dc, w, h);
    if bmp.0.is_null() {
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
        return Err(AppError::Other("CreateCompatibleBitmap failed".to_string()));
    }

    let old_obj = SelectObject(mem_dc, bmp);

    let blt_result = BitBlt(mem_dc, 0, 0, w, h, screen_dc, x, y, SRCCOPY);
    if blt_result.is_err() {
        SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
        return Err(AppError::Other("BitBlt failed".to_string()));
    }

    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // negative = top-down (prevents vertical flip)
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default(); 1],
    };

    let mut bits = vec![0u8; (w * h * 4) as usize];

    let scan_lines = GetDIBits(
        mem_dc,
        bmp,
        0,
        h as u32,
        Some(bits.as_mut_ptr() as *mut _),
        &mut bi,
        DIB_RGB_COLORS,
    );

    SelectObject(mem_dc, old_obj);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem_dc);
    ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);

    if scan_lines == 0 {
        return Err(AppError::Other("GetDIBits returned 0 scan lines".to_string()));
    }

    // Convert BGRA to RGB (GDI gives us BGRA)
    let mut rgb_data = Vec::with_capacity((w * h * 3) as usize);
    for chunk in bits.chunks_exact(4) {
        rgb_data.push(chunk[2]); // R (BGRA: B=0, G=1, R=2, A=3)
        rgb_data.push(chunk[1]); // G
        rgb_data.push(chunk[0]); // B
    }

    let img: RgbImage =
        ImageBuffer::from_raw(w as u32, h as u32, rgb_data)
            .ok_or_else(|| AppError::Other("Failed to create RGB image buffer".to_string()))?;

    let mut jpeg_data: Vec<u8> = Vec::new();
    let mut cursor = Cursor::new(&mut jpeg_data);
    img.write_to(&mut cursor, image::ImageFormat::Jpeg)?;

    Ok(jpeg_data)
}

/// Capture the full primary screen and return as egui::ColorImage for use as overlay background.
pub unsafe fn capture_full_screen_image() -> Result<egui::ColorImage, AppError> {
    let w = GetSystemMetrics(SM_CXSCREEN);
    let h = GetSystemMetrics(SM_CYSCREEN);

    if w <= 0 || h <= 0 {
        return Err(AppError::Other("Invalid screen dimensions".to_string()));
    }

    let screen_dc = GetDC(HWND(std::ptr::null_mut()));
    if screen_dc.0.is_null() {
        return Err(AppError::Other("GetDC failed".to_string()));
    }

    let mem_dc = CreateCompatibleDC(screen_dc);
    if mem_dc.0.is_null() {
        ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
        return Err(AppError::Other("CreateCompatibleDC failed".to_string()));
    }

    let bmp = CreateCompatibleBitmap(screen_dc, w, h);
    if bmp.0.is_null() {
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);
        return Err(AppError::Other("CreateCompatibleBitmap failed".to_string()));
    }

    let old_obj = SelectObject(mem_dc, bmp);
    let _ = BitBlt(mem_dc, 0, 0, w, h, screen_dc, 0, 0, SRCCOPY);

    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default(); 1],
    };

    let mut bits = vec![0u8; (w * h * 4) as usize];
    GetDIBits(
        mem_dc,
        bmp,
        0,
        h as u32,
        Some(bits.as_mut_ptr() as *mut _),
        &mut bi,
        DIB_RGB_COLORS,
    );

    SelectObject(mem_dc, old_obj);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem_dc);
    ReleaseDC(HWND(std::ptr::null_mut()), screen_dc);

    // BGRA -> egui::Color32 (RGBA)
    let pixels: Vec<egui::Color32> = bits
        .chunks_exact(4)
        .map(|c| egui::Color32::from_rgb(c[2], c[1], c[0]))
        .collect();

    Ok(egui::ColorImage {
        size: [w as usize, h as usize],
        pixels,
    })
}
