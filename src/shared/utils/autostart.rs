use crate::shared::error::AppError;
use windows::core::PCWSTR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::Foundation::ERROR_SUCCESS;

const AUTOSTART_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
const AUTOSTART_VALUE: &str = "ScreenTranslator";

pub fn set_autostart(enabled: bool, exe_path: &str) -> Result<(), AppError> {
    let key_wide: Vec<u16> = AUTOSTART_KEY
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .collect();
    let value_wide: Vec<u16> = AUTOSTART_VALUE
        .encode_utf16()
        .chain(std::iter::once(0u16))
        .collect();

    unsafe {
        let mut hkey = HKEY::default();
        let result = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_wide.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        );

        if result != ERROR_SUCCESS {
            return Err(AppError::Other(format!(
                "RegCreateKeyExW failed with: {:?}",
                result
            )));
        }

        if enabled {
            let path_wide: Vec<u16> = exe_path
                .encode_utf16()
                .chain(std::iter::once(0u16))
                .collect();
            // REG_SZ stores UTF-16, so we pass the raw bytes
            let path_bytes = std::slice::from_raw_parts(
                path_wide.as_ptr() as *const u8,
                path_wide.len() * 2,
            );
            let set_result = RegSetValueExW(
                hkey,
                PCWSTR(value_wide.as_ptr()),
                0,
                REG_SZ,
                Some(path_bytes),
            );
            if set_result != ERROR_SUCCESS {
                let _ = RegCloseKey(hkey);
                return Err(AppError::Other(format!(
                    "RegSetValueExW failed with: {:?}",
                    set_result
                )));
            }
        } else {
            // Ignore error if value doesn't exist
            let _ = RegDeleteValueW(hkey, PCWSTR(value_wide.as_ptr()));
        }

        let _ = RegCloseKey(hkey);
    }

    Ok(())
}

pub fn get_current_exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}
