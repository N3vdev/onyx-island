//! Windows.Media.Ocr binding. An overview pass starts at ARMED; a sharper
//! pass over the drawn region recovers fine print at SNAPPING.

use windows::Foundation::IAsyncOperation;
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform,
    ColorManagementMode, ExifOrientationMode,
};
use windows::Media::Ocr::{OcrEngine, OcrResult};
use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

// The overview stays fast enough to run while the overlay opens. The detail
// pass uses the original pixels (or upscales a small crop) after release.
const TARGET_LONG_EDGE: f64 = 1920.0;
const DETAIL_UPSCALE: f64 = 2.5;

#[derive(Clone, Copy)]
struct OcrMapping {
    x_scale: f64,
    y_scale: f64,
    x_offset: f64,
    y_offset: f64,
}

#[derive(Debug, Clone)]
pub struct OcrWord {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
pub struct OcrLine {
    pub text: String,
    /// Union of this line's word boxes, in the original (un-downscaled) frame's coordinate space.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub words: Vec<OcrWord>,
}

pub fn log_available_languages() {
    match OcrEngine::AvailableRecognizerLanguages() {
        Ok(langs) => {
            let names: Vec<String> =
                langs.into_iter().filter_map(|lang| lang.DisplayName().ok().map(|s| s.to_string())).collect();
            tracing::info!(languages = ?names, "OCR recognizer languages available");
        }
        Err(err) => tracing::error!(%err, "failed to query OCR recognizer languages"),
    }
}

/// A cheap, `Send + Sync` handle that can cancel an in-flight OCR pass from
/// any thread — cloning it just bumps a COM refcount, no OCR state itself.
/// Kept separate from `PendingOcr` so a canceller (e.g. dismiss, on the
/// Tauri command thread) doesn't need to touch the pass itself.
#[derive(Clone)]
pub struct OcrCancelHandle(IAsyncOperation<OcrResult>);

impl OcrCancelHandle {
    /// Requests cancellation. Safe to call from any thread, and safe to call
    /// on a pass that has already finished (a no-op error, ignored) — there
    /// is no separate "is it still running" check to get wrong.
    pub fn cancel(&self) {
        let _ = self.0.Cancel();
    }
}

/// An OCR pass that has been dispatched but not yet waited on.
pub struct PendingOcr {
    operation: IAsyncOperation<OcrResult>,
    mapping: OcrMapping,
}

impl PendingOcr {
    pub fn cancel_handle(&self) -> OcrCancelHandle {
        OcrCancelHandle(self.operation.clone())
    }

    /// Blocks until the pass completes, is cancelled, or fails. A caller
    /// that cancelled via a handle obtained from this same pass sees this
    /// return `Err` rather than hang or return a partial result.
    pub fn wait(self) -> anyhow::Result<Vec<OcrLine>> {
        let result = self.operation.get()?;
        lines_from_result(&result, self.mapping)
    }
}

/// Decodes, downscales, and dispatches OCR on a PNG-encoded frame, returning
/// immediately with a handle to the in-flight pass rather than blocking for
/// it to finish — the caller decides whether to wait for it or cancel it.
/// See `recognize` for the simple blocking version (used by tests and the
/// `expectations.rs` harness, where there's nothing to cancel).
pub fn start_recognize(png_bytes: &[u8]) -> anyhow::Result<PendingOcr> {
    start_recognize_scaled(png_bytes, TARGET_LONG_EDGE, 1.0, 0.0, 0.0)
}

/// Re-runs OCR on a crop from the original screenshot. The crop's origin is
/// restored to every returned word box so the overlay can use it unchanged.
pub fn recognize_detail(png_bytes: &[u8], origin_x: u32, origin_y: u32) -> anyhow::Result<Vec<OcrLine>> {
    start_recognize_scaled(
        png_bytes,
        f64::INFINITY,
        DETAIL_UPSCALE,
        origin_x as f64,
        origin_y as f64,
    )?
    .wait()
}

fn detail_crop(
    img: &image::DynamicImage,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Option<(image::DynamicImage, u32, u32)> {
    // Context prevents characters at the stroke edge from being cut off.
    // Return the crop origin so detected boxes map to screenshot pixels.
    const PADDING: f64 = 32.0;
    let x0 = (x - PADDING).floor().clamp(0.0, img.width() as f64) as u32;
    let y0 = (y - PADDING).floor().clamp(0.0, img.height() as f64) as u32;
    let x1 = (x + width + PADDING).ceil().clamp(0.0, img.width() as f64) as u32;
    let y1 = (y + height + PADDING).ceil().clamp(0.0, img.height() as f64) as u32;
    (x1 > x0 && y1 > y0).then(|| (img.crop_imm(x0, y0, x1 - x0, y1 - y0), x0, y0))
}

/// OCR the selected area from a decoded screenshot. Shared by the live
/// command and recorded-image expectations, so tests exercise this path.
pub fn recognize_detail_from_image(
    img: &image::DynamicImage,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> anyhow::Result<Vec<OcrLine>> {
    let (crop, origin_x, origin_y) = detail_crop(img, x, y, width, height)
        .ok_or_else(|| anyhow::anyhow!("detail OCR crop is empty"))?;
    let mut png = Vec::new();
    crop.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
    let mut lines = recognize_detail(&png, origin_x, origin_y)?;
    if lines.is_empty() {
        // Only pay for a second pass when normal OCR found nothing. A little
        // contrast and dark-screen inversion often recover fine UI text;
        // keep this on the selected crop and on the caller's worker thread.
        let mut enhanced = crop.grayscale().adjust_contrast(36.0);
        if average_luminance(&crop) < 115.0 { enhanced.invert(); }
        let mut retry_png = Vec::new();
        enhanced.write_to(&mut std::io::Cursor::new(&mut retry_png), image::ImageFormat::Png)?;
        if let Ok(recovered) = recognize_detail(&retry_png, origin_x, origin_y) {
            lines = recovered;
        }
    }
    Ok(lines)
}

fn average_luminance(img: &image::DynamicImage) -> f64 {
    use image::GenericImageView;
    let x_step = (img.width() / 32).max(1);
    let y_step = (img.height() / 32).max(1);
    let mut sum = 0.0;
    let mut count = 0_u32;
    for y in (0..img.height()).step_by(y_step as usize) {
        for x in (0..img.width()).step_by(x_step as usize) {
            let pixel = img.get_pixel(x, y);
            sum += 0.2126 * f64::from(pixel[0]) + 0.7152 * f64::from(pixel[1]) + 0.0722 * f64::from(pixel[2]);
            count += 1;
        }
    }
    if count == 0 { 0.0 } else { sum / f64::from(count) }
}

fn start_recognize_scaled(
    png_bytes: &[u8],
    target_long_edge: f64,
    max_upscale: f64,
    x_offset: f64,
    y_offset: f64,
) -> anyhow::Result<PendingOcr> {
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(png_bytes)?;
    writer.StoreAsync()?.get()?;
    stream.Seek(0)?;

    let decoder = BitmapDecoder::CreateAsync(&stream)?.get()?;

    let width = decoder.PixelWidth()?;
    let height = decoder.PixelHeight()?;
    if width == 0 || height == 0 {
        anyhow::bail!("OCR image is empty");
    }
    let long_edge = width.max(height) as f64;
    // Windows OCR has a device-dependent maximum image dimension. A crop
    // can be enlarged for tiny glyphs, but never beyond that engine limit.
    let engine_limit = OcrEngine::MaxImageDimension()? as f64;
    if engine_limit < 1.0 {
        anyhow::bail!("Windows OCR reported no supported image dimension");
    }
    let scale = (target_long_edge.min(engine_limit) / long_edge).min(max_upscale);
    let scaled_width = ((width as f64) * scale).round().max(1.0) as u32;
    let scaled_height = ((height as f64) * scale).round().max(1.0) as u32;
    let mapping = OcrMapping {
        x_scale: width as f64 / scaled_width as f64,
        y_scale: height as f64 / scaled_height as f64,
        x_offset,
        y_offset,
    };

    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(scaled_width)?;
    transform.SetScaledHeight(scaled_height)?;
    if scale > 1.0 {
        transform.SetInterpolationMode(BitmapInterpolationMode::Cubic)?;
    }

    let bitmap = decoder
        .GetSoftwareBitmapTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .get()?;

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    let operation = engine.RecognizeAsync(&bitmap)?;

    Ok(PendingOcr { operation, mapping })
}

/// Blocking overview OCR for tests and fixtures. Word boxes are mapped back
/// to the PNG's pixel dimensions, matching the overlay canvas coordinates.
pub fn recognize(png_bytes: &[u8]) -> anyhow::Result<Vec<OcrLine>> {
    start_recognize(png_bytes)?.wait()
}

fn overlap_of_smaller(
    a: (f64, f64, f64, f64),
    b: (f64, f64, f64, f64),
) -> f64 {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    let overlap_width = (ax + aw).min(bx + bw) - ax.max(bx);
    let overlap_height = (ay + ah).min(by + bh) - ay.max(by);
    let smaller_area = (aw * ah).min(bw * bh);
    if overlap_width <= 0.0 || overlap_height <= 0.0 || smaller_area <= 0.0 {
        0.0
    } else {
        overlap_width * overlap_height / smaller_area
    }
}

fn line_box(line: &OcrLine) -> (f64, f64, f64, f64) {
    (line.x, line.y, line.width, line.height)
}

fn word_box(word: &OcrWord) -> (f64, f64, f64, f64) {
    (word.x, word.y, word.width, word.height)
}

fn refresh_line(line: &mut OcrLine) {
    line.words.sort_by(|a, b| a.x.total_cmp(&b.x));
    line.text = line.words.iter().map(|word| word.text.as_str()).collect::<Vec<_>>().join(" ");
    if let Some(first) = line.words.first() {
        let mut left = first.x;
        let mut top = first.y;
        let mut right = first.x + first.width;
        let mut bottom = first.y + first.height;
        for word in &line.words[1..] {
            left = left.min(word.x);
            top = top.min(word.y);
            right = right.max(word.x + word.width);
            bottom = bottom.max(word.y + word.height);
        }
        line.x = left;
        line.y = top;
        line.width = right - left;
        line.height = bottom - top;
    }
}

/// Keep the overview's full lines (the detail crop may cut a line at its
/// edge), while adding words and lines that only the sharper pass found.
pub fn merge_detail_lines(mut overview: Vec<OcrLine>, detail: Vec<OcrLine>) -> Vec<OcrLine> {
    for detail_line in detail {
        let matching_index = overview.iter().enumerate()
            .map(|(index, line)| (index, overlap_of_smaller(line_box(line), line_box(&detail_line))))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, overlap)| *overlap >= 0.45)
            .map(|(index, _)| index);
        if let Some(index) = matching_index {
            let line = &mut overview[index];
            let mut added = false;
            for word in detail_line.words {
                if line.words.iter().all(|existing| overlap_of_smaller(word_box(existing), word_box(&word)) < 0.4) {
                    line.words.push(word);
                    added = true;
                }
            }
            if added {
                refresh_line(line);
            }
        } else {
            overview.push(detail_line);
        }
    }
    overview
}

fn lines_from_result(result: &OcrResult, mapping: OcrMapping) -> anyhow::Result<Vec<OcrLine>> {
    let mut lines = Vec::new();
    for line in result.Lines()? {
        let mut words = Vec::new();
        for word in line.Words()? {
            let rect = word.BoundingRect()?;
            words.push(OcrWord {
                text: word.Text()?.to_string(),
                x: rect.X as f64 * mapping.x_scale + mapping.x_offset,
                y: rect.Y as f64 * mapping.y_scale + mapping.y_offset,
                width: rect.Width as f64 * mapping.x_scale,
                height: rect.Height as f64 * mapping.y_scale,
            });
        }
        // `OcrLine::Words()` is not guaranteed left-to-right — mixed fonts
        // within a line (e.g. prose next to a monospace code span) can come
        // back out of visual order, which scrambles copied text since the
        // text layer's DOM order (and so native selection) follows this
        // order directly. Sort explicitly rather than trust it.
        words.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());

        // Rebuild the line's text from the sorted words rather than taking
        // `OcrLine::Text()`: that returns words in the engine's original
        // order, so whenever the sort above actually changes something the
        // two disagree. Everything downstream (the overlay's word spans,
        // and so anything copied out of them) is built from `words`, so
        // deriving here keeps one source of truth. Assumes a
        // space-separated script, which holds for the Latin-script
        // recognizers this targets.
        let text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");

        let Some((min_x, min_y, max_x, max_y)) = words.iter().fold(None::<(f64, f64, f64, f64)>, |acc, w| {
            let (x0, y0, x1, y1) = (w.x, w.y, w.x + w.width, w.y + w.height);
            Some(match acc {
                None => (x0, y0, x1, y1),
                Some((ax0, ay0, ax1, ay1)) => (ax0.min(x0), ay0.min(y0), ax1.max(x1), ay1.max(y1)),
            })
        }) else {
            continue;
        };

        lines.push(OcrLine {
            text,
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
            words,
        });
    }

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(words: &[(&str, f64, f64, f64, f64)]) -> OcrLine {
        let words = words.iter().map(|&(text, x, y, width, height)| OcrWord {
            text: text.into(), x, y, width, height,
        }).collect();
        let mut line = OcrLine { text: String::new(), x: 0.0, y: 0.0, width: 0.0, height: 0.0, words };
        refresh_line(&mut line);
        line
    }

    #[test]
    fn detail_recovers_a_small_line_missing_from_overview() {
        let overview = vec![line(&[("Heading", 10.0, 10.0, 90.0, 22.0)])];
        let fine_print = line(&[("fine", 10.0, 48.0, 24.0, 7.0), ("print", 38.0, 48.0, 28.0, 7.0)]);
        let merged = merge_detail_lines(overview, vec![fine_print]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].text, "fine print");
    }

    #[test]
    fn detail_adds_a_missing_word_without_repeating_existing_text() {
        let overview = vec![line(&[("small", 10.0, 10.0, 30.0, 9.0), ("text", 82.0, 10.0, 24.0, 9.0)])];
        let detail = vec![line(&[("small", 10.0, 10.0, 30.0, 9.0), ("extra", 46.0, 10.0, 30.0, 9.0), ("text", 82.0, 10.0, 24.0, 9.0)])];
        let merged = merge_detail_lines(overview, detail);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "small extra text");
        assert_eq!(merged[0].words.len(), 3);
    }

    #[test]
    fn detail_does_not_replace_a_full_line_with_a_clipped_crop() {
        let overview = vec![line(&[("full", 10.0, 10.0, 30.0, 12.0), ("sentence", 46.0, 10.0, 58.0, 12.0)])];
        let detail = vec![line(&[("sentence", 46.0, 10.0, 58.0, 12.0)])];
        let merged = merge_detail_lines(overview, detail);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].text, "full sentence");
    }

    #[test]
    fn detail_crop_keeps_context_and_original_pixel_origin() {
        let img = image::DynamicImage::new_rgba8(200, 120);
        let (crop, origin_x, origin_y) = detail_crop(&img, 50.4, 40.2, 30.1, 12.1).unwrap();
        assert_eq!((origin_x, origin_y), (18, 8));
        assert_eq!((crop.width(), crop.height()), (95, 77));
    }

    #[test]
    fn detail_crop_clamps_at_screen_edges() {
        let img = image::DynamicImage::new_rgba8(100, 80);
        let (crop, origin_x, origin_y) = detail_crop(&img, 0.0, 65.0, 20.0, 15.0).unwrap();
        assert_eq!((origin_x, origin_y), (0, 33));
        assert_eq!((crop.width(), crop.height()), (52, 47));
    }

    #[test]
    fn contrast_retry_distinguishes_dark_and_light_crops() {
        let dark = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            40, 20, image::Rgba([20, 20, 20, 255]),
        ));
        let light = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            40, 20, image::Rgba([230, 230, 230, 255]),
        ));
        assert!(average_luminance(&dark) < 115.0);
        assert!(average_luminance(&light) > 115.0);
    }
}
