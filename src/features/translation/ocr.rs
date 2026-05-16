use crate::features::translation::client::HTTP;
use crate::shared::error::AppError;
use serde_json::Value;
use tracing::{info, warn};

pub struct OcrResult {
    pub texts: Vec<String>,
    pub detected_lang: String,
}

pub async fn recognize(
    jpeg_data: &[u8],
    sid: &str,
    yu: &str,
    yum: &str,
) -> Result<OcrResult, AppError> {
    let url = format!(
        "https://translate.yandex.net/ocr/v1.1/recognize\
         ?srv=tr-image&sid={sid}&lang=*&rotate=auto&yu={yu}&yum={yum}"
    );

    let part = reqwest::multipart::Part::bytes(jpeg_data.to_vec())
        .file_name("blob")
        .mime_str("image/jpeg")?;
    let form = reqwest::multipart::Form::new().part("file", part);

    let resp = HTTP
        .post(&url)
        .header("Accept", "*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Referer", "https://translate.yandex.com/")
        .header("Origin", "https://translate.yandex.com")
        .multipart(form)
        .send()
        .await?;

    let status = resp.status();
    let body = resp.text().await?;
    info!("OCR status={} body={}", status, &body[..body.len().min(600)]);

    if !status.is_success() {
        return Err(AppError::Other(format!("OCR API {}: {}", status, body)));
    }

    let json: Value = serde_json::from_str(&body)?;

    let detected_lang = json
        .get("data")
        .and_then(|d| d.get("detected_lang"))
        .and_then(|v| v.as_str())
        .unwrap_or("en")
        .to_string();

    let mut texts = Vec::new();
    if let Some(blocks) = json
        .get("data")
        .and_then(|d| d.get("blocks"))
        .and_then(|v| v.as_array())
    {
        for block in blocks {
            if let Some(boxes) = block.get("boxes").and_then(|v| v.as_array()) {
                for b in boxes {
                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                        let t = t.trim();
                        if !t.is_empty() {
                            texts.push(t.to_string());
                        }
                    }
                }
            }
        }
    }

    if texts.is_empty() {
        warn!("OCR returned no text blocks (lang={})", detected_lang);
    }

    Ok(OcrResult { texts, detected_lang })
}
