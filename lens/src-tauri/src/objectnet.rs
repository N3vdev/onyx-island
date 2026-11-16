//! The tiny network behind Edit mode's "what did you circle?" step.
//!
//! U²-Net-P (4.6 MB, Apache-2.0) marks the main object in a picture. It is
//! only a *prior*: its 320×320 mask is soft and sometimes unsure (a white
//! shoe on a white backdrop comes out half-transparent), so `snap.rs` uses it
//! to decide what the object is and then snaps the edge to real pixels
//! itself. Without the model — offline, unsupported Windows, a failed run —
//! Edit mode still works, just on colour alone.
//!
//! Nothing here is bundled:
//! - The runtime is Windows' own ONNX Runtime/DirectML, reached through the
//!   Windows ML API, so the installer doesn't grow.
//! - The model downloads on first use into the app's local data folder (like
//!   the translation models), is checked against a pinned SHA-256, and is
//!   only loaded while a snap is running, so idle memory is untouched.
//!
//! Runs on the CPU deliberately: ~0.3 s per snap including setup. The GPU
//! path evaluates in ~12 ms but spends ~1.9 s building its session first,
//! which only pays off if that session is kept alive between snaps.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use image::{imageops::FilterType, RgbImage};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use windows::core::{Interface, HSTRING};
use windows::AI::MachineLearning::{
    LearningModel, LearningModelBinding, LearningModelDevice, LearningModelDeviceKind, LearningModelSession, TensorFloat,
};

/// Third-party release asset: the project's own hosting should replace this
/// before shipping widely. The pinned hash means a changed file is refused,
/// not trusted.
const MODEL_URL: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx";
const MODEL_SHA256: &str = "309c8469258dda742793dce0ebea8e6dd393174f89934733ecc8b14c76f4ddd8";
const MODEL_BYTES: usize = 4_574_861;
const MODEL_FILE: &str = "u2netp.onnx";

/// The network's fixed input (and output) side length.
pub const INPUT_SIZE: u32 = 320;

/// After a failed download, don't try again for this long — otherwise every
/// snap while offline would sit through a timeout first.
const RETRY_AFTER: Duration = Duration::from_secs(300);
static LAST_DOWNLOAD_FAILURE: Mutex<Option<Instant>> = Mutex::new(None);

pub fn model_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_local_data_dir().ok().map(|dir| dir.join("models").join(MODEL_FILE))
}

/// The model's path once it is on disk, downloading it first if needed.
/// `about_to_download` runs just before a download starts so the UI can say
/// so. `None` means "carry on without the network" — offline, a bad
/// download, or a recent failure we're not retrying yet.
pub fn ensure_model(app: &AppHandle, about_to_download: impl FnOnce()) -> Option<PathBuf> {
    let path = model_path(app)?;
    if std::fs::metadata(&path).is_ok_and(|m| m.len() == MODEL_BYTES as u64) {
        return Some(path);
    }
    if LAST_DOWNLOAD_FAILURE.lock().unwrap().is_some_and(|at| at.elapsed() < RETRY_AFTER) {
        return None;
    }
    about_to_download();
    match download(&path) {
        Ok(()) => Some(path),
        Err(err) => {
            tracing::warn!(%err, "could not download the Edit-mode model; using colour-only snapping");
            *LAST_DOWNLOAD_FAILURE.lock().unwrap() = Some(Instant::now());
            None
        }
    }
}

fn download(path: &Path) -> Result<(), String> {
    let response = ureq::get(MODEL_URL)
        .timeout(Duration::from_secs(90))
        .call()
        .map_err(|err| format!("request failed: {err}"))?;
    // One byte over the expected size is enough to prove it's the wrong file.
    let mut bytes = Vec::with_capacity(MODEL_BYTES);
    response
        .into_reader()
        .take(MODEL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("download interrupted: {err}"))?;
    verify(&bytes)?;

    let dir = path.parent().ok_or("no model folder")?;
    std::fs::create_dir_all(dir).map_err(|err| format!("could not create the model folder: {err}"))?;
    // Written under a temporary name and renamed, so a crash mid-write can't
    // leave a half-file that looks complete.
    let partial = path.with_extension("onnx.part");
    std::fs::write(&partial, &bytes).map_err(|err| format!("could not save the model: {err}"))?;
    std::fs::rename(&partial, path).map_err(|err| format!("could not finish saving the model: {err}"))
}

fn verify(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() != MODEL_BYTES {
        return Err(format!("downloaded {} bytes, expected {MODEL_BYTES}", bytes.len()));
    }
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != MODEL_SHA256 {
        return Err(format!("checksum mismatch: {hex}"));
    }
    Ok(())
}

/// Runs the network on `picture` (any size) and returns a 320×320 map, 0..1,
/// of how much each pixel belongs to the main object.
pub fn predict(model_path: &Path, picture: &RgbImage) -> Result<Vec<f32>, String> {
    let input = preprocess(picture);

    let path = std::fs::canonicalize(model_path).map_err(|err| format!("model missing: {err}"))?;
    let path = HSTRING::from(path.to_string_lossy().trim_start_matches(r"\\?\"));
    let model = LearningModel::LoadFromFilePath(&path).map_err(|err| format!("could not load the model: {err}"))?;
    let input_name = model
        .InputFeatures()
        .and_then(|f| f.GetAt(0))
        .and_then(|f| f.Name())
        .map_err(|err| format!("model has no input: {err}"))?;
    // The network has seven outputs; the first is the fused, final map.
    let output_name = model
        .OutputFeatures()
        .and_then(|f| f.GetAt(0))
        .and_then(|f| f.Name())
        .map_err(|err| format!("model has no output: {err}"))?;

    let device = LearningModelDevice::Create(LearningModelDeviceKind::Cpu).map_err(|err| format!("no ML device: {err}"))?;
    let session = LearningModelSession::CreateFromModelOnDevice(&model, &device).map_err(|err| format!("could not start the model: {err}"))?;
    let binding = LearningModelBinding::CreateFromSession(&session).map_err(|err| format!("could not bind the model: {err}"))?;
    let side = INPUT_SIZE as i64;
    let tensor = TensorFloat::CreateFromShapeArrayAndDataArray(&[1, 3, side, side], &input).map_err(|err| format!("could not build the input: {err}"))?;
    binding.Bind(&input_name, &tensor).map_err(|err| format!("could not bind the input: {err}"))?;

    let result = session.Evaluate(&binding, &HSTRING::from("circle-search")).map_err(|err| format!("the model failed: {err}"))?;
    let output: TensorFloat = result
        .Outputs()
        .and_then(|o| o.Lookup(&output_name))
        .and_then(|o| o.cast())
        .map_err(|err| format!("could not read the result: {err}"))?;
    let values: Vec<f32> = output.GetAsVectorView().map_err(|err| format!("could not read the result: {err}"))?.into_iter().collect();
    postprocess(values)
}

/// The preprocessing U²-Net was trained with: resize, scale by the picture's
/// brightest value, ImageNet mean/std, planar RGB.
fn preprocess(picture: &RgbImage) -> Vec<f32> {
    let small = image::imageops::resize(picture, INPUT_SIZE, INPUT_SIZE, FilterType::Lanczos3);
    let max = small.pixels().flat_map(|p| p.0).max().unwrap_or(1).max(1) as f32;
    let (mean, std) = ([0.485f32, 0.456, 0.406], [0.229f32, 0.224, 0.225]);
    let plane = (INPUT_SIZE * INPUT_SIZE) as usize;
    let mut chw = vec![0f32; 3 * plane];
    for (i, pixel) in small.pixels().enumerate() {
        for c in 0..3 {
            chw[c * plane + i] = (pixel.0[c] as f32 / max - mean[c]) / std[c];
        }
    }
    chw
}

/// Stretches the map to 0..1 by its own minimum and maximum (the network's
/// confidence varies a lot between pictures). A map with no spread at all
/// has found nothing.
fn postprocess(values: Vec<f32>) -> Result<Vec<f32>, String> {
    let expected = (INPUT_SIZE * INPUT_SIZE) as usize;
    if values.len() != expected {
        return Err(format!("the model returned {} values, expected {expected}", values.len()));
    }
    let (lo, hi) = values.iter().fold((f32::MAX, f32::MIN), |(l, h), &v| (l.min(v), h.max(v)));
    if hi - lo <= 1e-3 {
        return Ok(vec![0.0; expected]);
    }
    Ok(values.iter().map(|v| (v - lo) / (hi - lo)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_accepts_only_the_exact_pinned_file() {
        assert!(verify(&vec![0u8; MODEL_BYTES]).unwrap_err().contains("checksum"));
        assert!(verify(&[1, 2, 3]).unwrap_err().contains("expected"));
        assert!(verify(&vec![0u8; MODEL_BYTES + 1]).unwrap_err().contains("expected"));
    }

    #[test]
    fn preprocessing_produces_normalised_planar_input() {
        let grey = RgbImage::from_pixel(50, 40, image::Rgb([128, 128, 128]));
        let chw = preprocess(&grey);
        let plane = (INPUT_SIZE * INPUT_SIZE) as usize;
        assert_eq!(chw.len(), 3 * plane);
        // A flat picture scales to 1.0 against its own maximum, so every
        // pixel is exactly (1 - mean) / std for its channel.
        let expected = [(1.0 - 0.485) / 0.229, (1.0 - 0.456) / 0.224, (1.0 - 0.406) / 0.225];
        for c in 0..3 {
            assert!((chw[c * plane] - expected[c]).abs() < 1e-3, "channel {c}: {}", chw[c * plane]);
            assert!((chw[c * plane + plane - 1] - expected[c]).abs() < 1e-3);
        }
    }

    #[test]
    fn postprocessing_stretches_to_unit_range_and_rejects_the_wrong_size() {
        let expected = (INPUT_SIZE * INPUT_SIZE) as usize;
        let mut raw = vec![0.2f32; expected];
        raw[0] = 0.1;
        raw[1] = 0.6;
        let map = postprocess(raw).unwrap();
        assert_eq!((map[0], map[1]), (0.0, 1.0));
        assert!((map[2] - 0.2).abs() < 1e-6);

        assert!(postprocess(vec![0.5; 10]).is_err());
        assert!(postprocess(vec![0.5; expected]).unwrap().iter().all(|&v| v == 0.0), "no spread means nothing was found");
    }

    /// End to end against the real model and Windows ML. Skipped unless
    /// `U2NETP_PATH` points at a downloaded copy, so the normal run stays
    /// offline: `U2NETP_PATH=...\u2netp.onnx cargo test --lib -- --ignored`.
    #[test]
    #[ignore = "needs the model file and Windows ML"]
    fn the_real_model_finds_a_bright_disc_on_a_dark_ground() {
        let Ok(path) = std::env::var("U2NETP_PATH") else { return };
        let picture = RgbImage::from_fn(200, 200, |x, y| {
            let (dx, dy) = (x as f32 - 100.0, y as f32 - 100.0);
            if dx * dx + dy * dy < 55.0 * 55.0 { image::Rgb([230, 180, 40]) } else { image::Rgb([25, 30, 60]) }
        });
        let map = predict(Path::new(&path), &picture).expect("inference should run");
        let at = |x: u32, y: u32| map[(y * INPUT_SIZE + x) as usize];
        assert!(at(160, 160) > 0.7, "centre should be object, got {}", at(160, 160));
        assert!(at(8, 8) < 0.3, "corner should be background, got {}", at(8, 8));
    }
}
