use crate::features::translation::client::{generate_sid, generate_yu, generate_yum};
use crate::features::translation::ocr::recognize;
use crate::features::translation::translator::translate;
use crate::shared::error::AppError;
use tracing::info;

#[derive(Debug, Clone)]
pub struct PipelineResult {
    #[allow(dead_code)]
    pub original: String,
    pub translated: String,
}

/// Runs the full OCR → translate pipeline from a JPEG image.
/// Session tokens are generated internally — callers never touch Yandex session details.
pub async fn run_pipeline(
    jpeg: &[u8],
    src: &str,
    tgt: &str,
    use_yandex: bool,
    use_google: bool,
) -> Result<PipelineResult, AppError> {
    let sid = generate_sid();
    let yu = generate_yu();
    let yum = generate_yum();

    let ocr = recognize(jpeg, &sid, &yu, &yum).await?;

    if ocr.texts.is_empty() {
        return Err(AppError::Other("No text found in the selected region.".to_string()));
    }

    let original = ocr.texts.join(" ");
    info!("OCR result (lang={}): {}", ocr.detected_lang, original);

    let effective_src = if ocr.detected_lang != "*" && !ocr.detected_lang.is_empty() {
        ocr.detected_lang.clone()
    } else {
        src.to_string()
    };

    let translated =
        translate(&original, &effective_src, tgt, use_yandex, use_google).await?;
    info!("Translation: {}", translated);

    Ok(PipelineResult { original, translated })
}
