use crate::shared::error::AppError;

pub fn copy_text_to_clipboard(text: &str) -> Result<(), AppError> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}
