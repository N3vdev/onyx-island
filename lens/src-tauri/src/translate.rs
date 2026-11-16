//! On-device "Translate in place" using Mozilla's Firefox Translations models
//! via `fxtranslate`. Free forever at any user count: nothing leaves the
//! machine except a one-time download of each language's model.
//!
//! Memory: a loaded model costs ~100–300 MB (two when pivoting through
//! English), so it's loaded on first use in an overlay session and dropped
//! when the overlay is dismissed — no idle timer, so the idle-RAM budget holds.
//! Models are memory-mapped, so dropping them unmaps the weights rather than
//! leaving freed heap the OS may not reclaim.

use std::path::PathBuf;
use std::sync::Mutex;

use fxtranslate::cache::{Cache, ModelFiles};
use fxtranslate::engine::{Engine, Translation};
use fxtranslate::fetch::NetworkFetch;
use tauri::{AppHandle, Emitter, Manager};

/// Every Firefox Translations model goes to or from English; any other pair
/// is served by chaining two models through it.
const HUB: &str = "en";

struct Loaded {
    src: String,
    trg: String,
    translation: Translation,
}

#[derive(Default)]
pub struct Translator {
    loaded: Mutex<Option<Loaded>>,
}

impl Translator {
    /// Loads (or reuses) the `src`→`trg` model and translates each paragraph.
    /// Blocking: run off the async runtime.
    fn translate(&self, cache: &Cache, src: &str, trg: &str, paragraphs: &[String]) -> Result<Vec<String>, String> {
        let mut loaded = self.loaded.lock().unwrap();
        if !matches!(loaded.as_ref(), Some(l) if l.src == src && l.trg == trg) {
            // Drop the previous pair before loading the next, so two models
            // are never resident at once.
            *loaded = None;
            *loaded = Some(Loaded { src: src.into(), trg: trg.into(), translation: load(cache, src, trg)? });
        }
        let translation = &loaded.as_ref().unwrap().translation;
        Ok(paragraphs
            .iter()
            .map(|paragraph| match_leading_case(paragraph, translation.translate_long(paragraph)))
            .collect())
    }

    /// Drops any loaded model. Called when the overlay is dismissed.
    pub fn release(&self) {
        if self.loaded.lock().unwrap().take().is_some() {
            tracing::info!("translation model released");
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationResult {
    /// ISO 639-1 source language, as detected.
    source_lang: String,
    target_lang: String,
    /// One per input paragraph, same order. Equal to the input when the text
    /// is already in the target language.
    translations: Vec<String>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslationStatus {
    /// "downloading" the first time a language is used, then "translating".
    phase: &'static str,
    source_lang: String,
}

fn cache_root(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_local_data_dir()
        .map_err(|err| format!("No local data folder: {err}"))?
        .join("translation-models");
    std::fs::create_dir_all(&dir).map_err(|err| format!("Couldn't create the model folder: {err}"))?;
    Ok(dir)
}

/// Maps whatlang's ISO 639-3 codes to the ISO 639-1 tags Firefox models use.
/// Only languages both sides support are listed; anything else is reported
/// as undetectable rather than guessed.
fn to_model_tag(code: &str) -> Option<&'static str> {
    Some(match code {
        "eng" => "en",
        "deu" => "de",
        "fra" => "fr",
        "spa" => "es",
        "ita" => "it",
        "por" => "pt",
        "nld" => "nl",
        "pol" => "pl",
        "rus" => "ru",
        "ukr" => "uk",
        "jpn" => "ja",
        "cmn" => "zh",
        "kor" => "ko",
        "ara" => "ar",
        "tur" => "tr",
        "ces" => "cs",
        "slk" => "sk",
        "slv" => "sl",
        "hun" => "hu",
        "ron" => "ro",
        "bul" => "bg",
        "ell" => "el",
        "heb" => "he",
        "hin" => "hi",
        "ben" => "bn",
        "fin" => "fi",
        "swe" => "sv",
        "dan" => "da",
        "nob" => "nb",
        "est" => "et",
        "lav" => "lv",
        "lit" => "lt",
        "hrv" => "hr",
        "srp" => "sr",
        "bos" => "bs",
        "cat" => "ca",
        "ind" => "id",
        "vie" => "vi",
        "tha" => "th",
        "pes" => "fa",
        "urd" => "ur",
        "tam" => "ta",
        "tel" => "te",
        "mal" => "ml",
        "kan" => "kn",
        "mar" => "mr",
        "guj" => "gu",
        "aze" => "az",
        "afr" => "af",
        _ => return None,
    })
}

fn detect_language(text: &str) -> Option<&'static str> {
    let detection = whatlang::detect(text)?;
    // A short OCR snippet can produce a language guess with almost no
    // evidence ("Small details. Better flow." scored Norwegian at 0.09).
    // Don't download a model and replace the original with a bad translation.
    detection.is_reliable().then(|| to_model_tag(detection.lang().code())).flatten()
}

/// The model legs needed for `src`→`trg`: one when English is on either side,
/// otherwise two through English (the only shape Firefox's models come in).
fn legs(src: &str, trg: &str) -> Vec<(String, String)> {
    if src == HUB || trg == HUB {
        vec![(src.to_string(), trg.to_string())]
    } else {
        vec![(src.to_string(), HUB.to_string()), (HUB.to_string(), trg.to_string())]
    }
}

/// Cache first, so a language used before translates offline and without a
/// network round-trip; downloads (hash-verified by fxtranslate) only on a miss.
fn leg_files(cache: &Cache, src: &str, trg: &str) -> Result<ModelFiles, String> {
    if let Some(files) = cache.cached_model(src, trg) {
        return Ok(files);
    }
    fxtranslate::loader::ensure_files(&NetworkFetch::new(), cache, src, trg).map_err(|err| {
        tracing::warn!(%err, src, trg, "model download failed");
        format!(
            "Couldn't download the {} → {} translation model. Check your connection and try again.",
            fxtranslate::lang::display_name(src),
            fxtranslate::lang::display_name(trg),
        )
    })
}

fn load(cache: &Cache, src: &str, trg: &str) -> Result<Translation, String> {
    let mut engines = legs(src, trg)
        .into_iter()
        .map(|(a, b)| {
            let files = leg_files(cache, &a, &b)?;
            Engine::load_mmapped(&files.model, &files.src_vocab, &files.trg_vocab)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(match engines.len() {
        1 => Translation::Direct(engines.remove(0)),
        _ => {
            let second = engines.remove(1);
            Translation::Pivot { pivot: HUB.to_string(), first: engines.remove(0), second }
        }
    })
}

fn needs_download(cache: &Cache, src: &str, trg: &str) -> bool {
    legs(src, trg).iter().any(|(a, b)| cache.cached_model(a, b).is_none())
}

/// The models sometimes lower-case a sentence-initial capital ("the weather
/// is nice…"); restore it when the source paragraph started with one.
fn match_leading_case(source: &str, translated: String) -> String {
    let source_starts_upper = source.chars().find(|c| c.is_alphabetic()).is_some_and(char::is_uppercase);
    let mut chars = translated.chars();
    match chars.next() {
        Some(first) if source_starts_upper && first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => translated,
    }
}

/// Translates each paragraph into `target` (an ISO 639-1 tag), detecting the
/// source language from the text itself.
#[tauri::command]
pub async fn translate_paragraphs(
    app: AppHandle,
    paragraphs: Vec<String>,
    target: String,
) -> Result<TranslationResult, String> {
    let target = target.trim().to_ascii_lowercase();
    let joined = paragraphs.join("\n");
    let source = detect_language(&joined)
        .ok_or_else(|| "Couldn't reliably detect the language. Select a longer phrase or paragraph.".to_string())?
        .to_string();

    if source == target {
        return Ok(TranslationResult { source_lang: source, target_lang: target, translations: paragraphs });
    }

    // `cache_root` (mkdir) and `needs_download` (stats each model's cache
    // files) are blocking filesystem calls; on a slow/loaded disk they could
    // otherwise stall whichever tokio worker thread happens to be running
    // this async command — the same pool the hotkey/capture path uses. Both
    // now run inside the same spawn_blocking as the translation itself.
    let started = std::time::Instant::now();
    let (src, trg) = (source.clone(), target.clone());
    let app_for_worker = app.clone();
    let translations = tokio::task::spawn_blocking(move || -> Result<Vec<String>, String> {
        let cache = Cache::with_root(cache_root(&app_for_worker)?);
        let status = |phase| TranslationStatus { phase, source_lang: src.clone() };
        let _ = app_for_worker.emit(
            "translation-status",
            status(if needs_download(&cache, &src, &trg) { "downloading" } else { "translating" }),
        );
        app_for_worker.state::<Translator>().translate(&cache, &src, &trg, &paragraphs)
    })
    .await
    .map_err(|err| format!("Translation stopped unexpectedly: {err}"))??;

    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        source = %source,
        target = %target,
        paragraphs = translations.len(),
        "translation complete"
    );
    Ok(TranslationResult { source_lang: source, target_lang: target, translations })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_pairs_are_direct_and_others_pivot_through_english() {
        assert_eq!(legs("de", "en"), vec![("de".into(), "en".into())]);
        assert_eq!(legs("en", "ja"), vec![("en".into(), "ja".into())]);
        assert_eq!(legs("de", "fr"), vec![("de".into(), "en".into()), ("en".into(), "fr".into())]);
    }

    #[test]
    fn detects_common_languages() {
        assert_eq!(detect_language("Das Wetter ist heute sehr schön und ich gehe spazieren."), Some("de"));
        assert_eq!(detect_language("Je voudrais une tasse de café, s'il vous plaît."), Some("fr"));
        assert_eq!(detect_language("The weather is lovely today and I am going for a walk."), Some("en"));
        assert_eq!(detect_language("Small details. Better flow."), None);
    }

    /// End-to-end against the real models: downloads German→English (~36 MB)
    /// on first run, so it's opt-in. Point CIRCLE_SEARCH_MODEL_CACHE at an
    /// existing cache to reuse downloads:
    /// `cargo test --release -- --ignored translates_german_end_to_end`
    #[test]
    #[ignore = "downloads a translation model over the network"]
    fn translates_german_end_to_end() {
        let root = std::env::var_os("CIRCLE_SEARCH_MODEL_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("circle-search-model-test"));
        let cache = Cache::with_root(root);
        let paragraphs = vec!["Das Wetter ist heute schön.".to_string(), "Guten Morgen!".to_string()];
        let source = detect_language(&paragraphs.join("\n")).unwrap();
        assert_eq!(source, "de");

        let translator = Translator::default();
        let out = translator.translate(&cache, source, "en", &paragraphs).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].to_lowercase().contains("weather"), "{out:?}");
        assert!(out[0].starts_with(char::is_uppercase), "leading capital restored: {out:?}");
        assert!(out[1].to_lowercase().contains("morning"), "{out:?}");

        translator.release();
        assert!(translator.loaded.lock().unwrap().is_none());
    }

    #[test]
    fn restores_a_dropped_leading_capital() {
        assert_eq!(match_leading_case("Guten Morgen", "bonjour".into()), "Bonjour");
        assert_eq!(match_leading_case("guten morgen", "bonjour".into()), "bonjour");
        assert_eq!(match_leading_case("今日は", "It's nice".into()), "It's nice");
    }
}
