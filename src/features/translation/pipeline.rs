//! OCR → repair → translate, as one call.

use std::time::{Duration, Instant};

use tracing::info;

use crate::entities::language::Language;
use crate::entities::settings::{EngineKind, EngineSettings, Settings};
use crate::features::translation::cache;
use crate::features::translation::ocr;
use crate::features::translation::providers::{self, TranslateRequest};
use crate::shared::error::AppError;

/// Everything the pipeline needs, copied out of `Settings` so no lock is held
/// across an await point.
#[derive(Debug, Clone)]
pub struct PipelineParams {
    pub source: Language,
    pub target: Language,
    pub engines: EngineSettings,
}

impl From<&Settings> for PipelineParams {
    fn from(s: &Settings) -> Self {
        Self {
            source: s.source_lang,
            target: s.target_lang,
            engines: s.engines.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub original: String,
    pub translated: String,
    pub engine: EngineKind,
    /// What the text actually turned out to be, once OCR had a look.
    pub source: Language,
    pub target: Language,
    /// Wall clock for the whole capture: OCR plus translation.
    pub elapsed: Duration,
    /// Just the translation call — this is the number the result header shows,
    /// because it is the one that differs between engines.
    pub engine_elapsed: Duration,
    pub from_cache: bool,
}

pub async fn run(jpeg: &[u8], params: &PipelineParams) -> Result<PipelineResult, AppError> {
    let started = Instant::now();

    let recognised = ocr::recognize(jpeg).await?;
    let original = providers::normalize_ocr_text(&recognised.text);

    if original.trim().is_empty() {
        return Err(AppError::NoText);
    }

    // OCR's own guess beats the configured source, which is usually left on
    // "detect". A wrong explicit source is the single most common reason a
    // translation comes back identical to the input.
    let source = match (recognised.detected, params.source) {
        (Some(detected), _) => detected,
        (None, configured) => configured,
    };

    // Translating a language into itself wastes a round trip and often returns
    // a mangled paraphrase.
    if source == params.target {
        info!(
            lang = source.code(),
            "Text is already in the target language"
        );
        return Ok(PipelineResult {
            translated: original.clone(),
            original,
            engine: EngineKind::Yandex,
            source,
            target: params.target,
            elapsed: started.elapsed(),
            engine_elapsed: Duration::ZERO,
            from_cache: false,
        });
    }

    if let Some(hit) = cache::get(&original, source, params.target) {
        info!(engine = hit.engine.label(), "Cache hit");
        return Ok(PipelineResult {
            original,
            translated: hit.text,
            engine: hit.engine,
            source,
            target: params.target,
            elapsed: started.elapsed(),
            engine_elapsed: Duration::ZERO,
            from_cache: true,
        });
    }

    let request = TranslateRequest {
        text: original.clone(),
        source,
        target: params.target,
    };
    let translated = providers::translate(&request, &params.engines).await?;

    cache::put(
        &original,
        source,
        params.target,
        cache::Hit {
            text: translated.text.clone(),
            engine: translated.engine,
        },
    );

    Ok(PipelineResult {
        original,
        translated: translated.text,
        engine: translated.engine,
        source,
        target: params.target,
        elapsed: started.elapsed(),
        engine_elapsed: translated.elapsed,
        from_cache: false,
    })
}
