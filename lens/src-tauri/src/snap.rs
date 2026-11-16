//! Edit mode (Pro): "snap to edges". Turns a loose loop drawn around
//! something into that thing's actual outline, like a one-click background
//! remover, so the crop that follows is a cut-out instead of a box.
//!
//! The loop the user draws is treated as an *upper bound*: everything
//! outside it is definitely background, everything inside is unknown. From
//! there it is GrabCut-shaped, but built to stay fast and dependency-free:
//!
//! Optionally first, when the small object network is available (see
//! `objectnet.rs`): it marks which pixels are the main object. That is only a
//! soft hint, but it settles *what* was circled, which colour alone can't
//! always do. It is a prior for the steps below, never the final edge.
//!
//! 0. People circle *around* things, so the strip just inside the loop's edge
//!    is almost always background. Flood from it through flat, uniformly
//!    coloured pixels, stopping at any real edge: whatever that reaches is
//!    certain background. This is what separates a white sneaker from its
//!    white product-photo backdrop — nothing outside the loop looks like that
//!    backdrop, so colour models alone call it "object".
//! 1. Fit a colour model (a small mixture of Gaussians) to "object" and to
//!    "background" pixels. The first pass guesses object = everything inside
//!    the loop that step 0 didn't claim; the background model comes from the
//!    pixels outside it and from step 0.
//! 2. Score every pixel inside the loop as object-or-background.
//! 3. Run that score through a colour-guided filter (He et al.). This is
//!    the step that makes the boundary *snap*: the filter smooths the score
//!    inside regions but refuses to smooth across real colour edges in the
//!    screenshot, so a soft, blobby score map comes out with a crisp edge
//!    exactly where the image has one. A real graph cut would be slightly
//!    more principled, but needs a max-flow solver that is slow in debug
//!    builds; this runs in tens of milliseconds.
//! 4. Re-fit the models from the new split and repeat a few times.
//!
//! Steps 1-4 run on a downscaled copy of the region around the loop, so the
//! cost is bounded however large the selection is. That alone is only
//! accurate to a few pixels on a big selection, so a second pass makes it
//! exact: every pixel within a few pixels of the coarse edge is re-decided at
//! full resolution, straight from its own colour, using the models the
//! first pass learned. Pixels well inside or outside are not re-examined,
//! they can't change the outline. The outline is then traced along pixel
//! edges, so it lands on the real boundary to within a pixel.
//!
//! Holes are deliberately filled: the app's selection pipeline (crop mask,
//! spotlight, stroke) is built around one closed outline, so the doughnut
//! hole of an "O" keeps its background. Several separate objects collapse to
//! the largest one.

use image::{RgbImage, RgbaImage};
use tauri::Emitter;

use crate::entitlements::Plan;
use crate::state::AppState;

/// Longest side of the working copy. Bounds the cost of every pass below;
/// the outline loses at most `region / MAX_WORK_DIM` pixels of precision on
/// very large selections.
const MAX_WORK_DIM: f64 = 512.0;
const ITERATIONS: usize = 4;
const GMM_COMPONENTS: usize = 5;
const MAX_SAMPLES: usize = 4000;
/// A smaller loop than this has nothing to snap.
const MIN_LOOP_SIZE: f64 = 8.0;
/// Variance floor per colour channel (0..1 scale): stops a perfectly flat
/// region collapsing into a model that rejects its own anti-aliased edge.
const VARIANCE_FLOOR: f32 = 0.0002;
const GUIDED_EPS: f32 = 0.004;
/// A result smaller than this share of the loop is treated as "found
/// nothing" rather than returned as a speck.
const MIN_AREA_FRACTION: f64 = 0.02;
/// Width of the strip inside the loop's edge that seeds the background
/// flood, as a share of the loop's shorter side.
const EDGE_STRIP_FRACTION: f64 = 0.05;
/// A strip pixel only seeds the flood if the colour barely changes around
/// it (0..1 RGB distance to its neighbour) — object edges and texture
/// don't, flat backdrops do.
const FLAT_GRADIENT: f32 = 0.02;
/// The flood may step to a neighbour whose colour differs from the pixel it
/// came from by at most this (or three times the backdrop's own noise).
const FLOOD_STEP: f32 = 0.04;
/// ...and must stay within this many standard deviations of the backdrop's
/// colour model, so a slow gradient (a soft shadow) can't carry it into the
/// object one small step at a time.
const FLOOD_PALETTE_SQ: f32 = 18.0;
/// A colour must make up at least this share of the edge strip to be
/// treated as backdrop.
const DOMINANT_SHARE: f32 = 0.2;
/// If the flood claims this much of the loop it has swallowed the object
/// (the loop was drawn tight, or the object is flat-coloured); the prior is
/// dropped and the plain colour-model result used instead.
const MAX_BACKDROP_FRACTION: f64 = 0.97;
/// A prior-guided result below this share of the loop looks like the flood
/// ate most of the object; try again without the prior.
const MIN_PRIOR_RESULT_FRACTION: f64 = 0.04;
/// Spacing of the returned outline, in frame pixels. Deliberately dense, not
/// simplified: the frontend draws and masks with curves through the
/// *midpoints* between points, so a handful of long edges would be cut
/// across at every corner (a card came out as a faceted blob). At this
/// spacing that smoothing can only move the outline by a fraction of a pixel.
const OUTLINE_SPACING: f64 = 2.0;
const MAX_OUTLINE_POINTS: usize = 2500;
/// Outline smoothing: passes of a (2*half+1)-point moving average along the
/// pixel-edge chain (one point per pixel of edge).
const SMOOTH_PASSES: usize = 3;
const SMOOTH_HALF_WINDOW: usize = 3;
/// A direction change this sharp, over a few pixels of edge, is a corner.
const CORNER_DEGREES: f64 = 50.0;
/// How far (in working pixels) the coarse edge may be wrong. The full
/// resolution pass re-decides everything inside this band around it.
const BAND_WORK_RADIUS: usize = 2;
/// Above this many pixels in the region the exact pass is skipped and the
/// coarse outline is used instead (a near-full-screen 8K selection would
/// otherwise spend its time allocating masks).
const MAX_FULL_RES_PIXELS: usize = 12_000_000;

/// The network's map is trusted this far: below the first it is confident a
/// pixel is background, above the second that it is the object. Between the
/// two (the soft edge, and anything it was unsure of) the colour models and
/// the edge snapping decide.
const NET_BACKGROUND_BELOW: f32 = 0.10;
const NET_OBJECT_ABOVE: f32 = 0.90;
/// If the network thinks less than this, or more than the upper share, of the
/// loop is object it hasn't understood the picture (nothing found, or
/// everything) and is ignored.
const NET_MIN_OBJECT_SHARE: f64 = 0.03;
const NET_MAX_OBJECT_SHARE: f64 = 0.97;

type Rgb = [f32; 3];

/// Pro-only, enforced here and not just by the toolbar hiding the button —
/// same rule as Redact.
#[tauri::command]
pub async fn snap_selection(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    points: Vec<(f64, f64)>,
) -> Result<Option<Vec<(f64, f64)>>, String> {
    if state.entitlements().plan == Plan::Free {
        return Err("Snap is a Pro feature".to_string());
    }
    let frame = state.frame_cache().ok_or_else(|| "No active capture to snap against".to_string())?;
    tokio::task::spawn_blocking(move || -> Result<Option<Vec<(f64, f64)>>, String> {
        let image = image::load_from_memory(&frame.png_bytes)
            .map_err(|err| format!("Could not read the capture: {err}"))?
            .to_rgba8();
        // The first snap on a machine downloads the 4.6 MB model; the overlay
        // is told so it can say why that one takes a couple of seconds.
        let model = crate::objectnet::ensure_model(&app, || {
            let _ = app.emit("snap-status", "downloading");
        });
        let network = |picture: &RgbImage| -> Option<Vec<f32>> {
            let path = model.as_ref()?;
            match crate::objectnet::predict(path, picture) {
                Ok(map) => Some(map),
                Err(err) => {
                    tracing::warn!(%err, "object network failed; snapping on colour alone");
                    None
                }
            }
        };
        Ok(snap_outline(&image, &points, &network))
    })
    .await
    .map_err(|err| format!("snap worker failed: {err}"))?
}

/// `None` when there is no clear object inside the loop to snap to (flat
/// area, loop too small, or the result would be a speck) — callers keep the
/// user's own outline in that case.
#[cfg(test)]
pub fn snap_polygon(image: &RgbaImage, polygon: &[(f64, f64)]) -> Option<Vec<(f64, f64)>> {
    snap_polygon_with(image, polygon, &|_| None)
}

/// `network` is handed the region around the loop and returns a square map,
/// 0..1, of how much each pixel is the main object — or `None` to carry on
/// without it. It is only ever asked once, and only after the cheap checks
/// have passed.
pub fn snap_polygon_with(
    image: &RgbaImage,
    polygon: &[(f64, f64)],
    network: &dyn Fn(&RgbImage) -> Option<Vec<f32>>,
) -> Option<Vec<(f64, f64)>> {
    let snapped = snap_mask(image, polygon, network, None)?;
    outline_from_mask(&snapped.mask, snapped.w, snapped.h, (snapped.x0 as f64, snapped.y0 as f64), (snapped.sx, snapped.sy))
}

/// Edit's snap: the object inside the loop, or, when the loop is round a
/// window, a card or a panel, that frame.
///
/// The colour cut-out is built for photos: an object against a backdrop.
/// On a dark window over a dark desktop it has little to go on and keeps a
/// bright strip (a title bar) and a blob or two, cutting into the window.
/// So when the loop holds a frame (straight sides, see
/// [`crate::rectsnap::frame_in_loop`]) the frame is taken, unless the
/// cut-out found an object sitting free inside it (a product in a photo),
/// which is what a cut-out is for.
pub fn snap_outline(
    image: &RgbaImage,
    polygon: &[(f64, f64)],
    network: &dyn Fn(&RgbImage) -> Option<Vec<f32>>,
) -> Option<Vec<(f64, f64)>> {
    let object = snap_polygon_with(image, polygon, network);
    let Some(frame) = crate::rectsnap::frame_in_loop(image, polygon) else {
        return object;
    };
    let frame_outline = crate::rectsnap::frame_outline(frame);
    let Some(object) = object else {
        return Some(frame_outline);
    };
    if object_is_free(&object, frame.region) {
        Some(object)
    } else {
        Some(frame_outline)
    }
}

/// The cut-out is a thing of its own inside the frame: clearly smaller
/// than it and not clinging to its sides. One that fills most of the frame
/// is the frame (drawn less cleanly); one that runs along its border is a
/// piece of it, like the title bar of a window.
fn object_is_free(object: &[(f64, f64)], frame: crate::rectsnap::Region) -> bool {
    let area = shoelace(object);
    let frame_area = frame.width * frame.height;
    if frame_area <= 0.0 {
        return true;
    }
    let ratio = area / frame_area;
    if ratio > 1.15 {
        // Bigger than the frame: the frame is a detail of the object.
        return true;
    }
    if ratio >= 0.6 {
        return false;
    }
    let (x0, y0, x1, y1) = (frame.x, frame.y, frame.x + frame.width, frame.y + frame.height);
    let near = (frame.width.min(frame.height) * 0.03).clamp(3.0, 10.0);
    let clinging = object
        .iter()
        .filter(|&&(x, y)| {
            let inside = x >= x0 - near && x <= x1 + near && y >= y0 - near && y <= y1 + near;
            let to_side = (x - x0).abs().min((x1 - x).abs()).min((y - y0).abs()).min((y1 - y).abs());
            inside && to_side <= near
        })
        .count() as f64
        / object.len().max(1) as f64;
    clinging < 0.2
}

fn shoelace(points: &[(f64, f64)]) -> f64 {
    let mut sum = 0.0;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        sum += a.0 * b.1 - b.0 * a.1;
    }
    sum.abs() / 2.0
}

/// Pixels the caller already knows, in frame coordinates. Used by the Edit
/// brush, whose loop hugs the existing selection: what was already selected
/// stays object, and what it is erasing is background, and the usual
/// pipeline decides the rest.
pub struct Hints<'a> {
    pub object: &'a dyn Fn(f64, f64) -> bool,
    pub background: &'a dyn Fn(f64, f64) -> bool,
}

/// The snapped object as a mask over a region of the frame.
pub struct SnapMask {
    mask: Vec<bool>,
    w: usize,
    h: usize,
    x0: usize,
    y0: usize,
    /// Mask pixels per frame pixel (1.0 unless only the coarse pass ran).
    sx: f64,
    sy: f64,
}

impl SnapMask {
    /// Whether the frame point (x, y) is part of the snapped object.
    fn at(&self, x: f64, y: f64) -> bool {
        let (mx, my) = ((x - self.x0 as f64) * self.sx, (y - self.y0 as f64) * self.sy);
        if mx < 0.0 || my < 0.0 {
            return false;
        }
        let (ix, iy) = (mx as usize, my as usize);
        ix < self.w && iy < self.h && self.mask[iy * self.w + ix]
    }
}

/// Edit mode's whole pipeline, up to the pixel mask (before it is turned
/// into an outline).
pub fn snap_mask(
    image: &RgbaImage,
    polygon: &[(f64, f64)],
    network: &dyn Fn(&RgbImage) -> Option<Vec<f32>>,
    hints: Option<&Hints>,
) -> Option<SnapMask> {
    if polygon.len() < 3 {
        return None;
    }
    let work = Work::build(image, polygon)?;
    let (w, h) = (work.w, work.h);

    let polygon_work: Vec<(f64, f64)> =
        polygon.iter().map(|&(x, y)| ((x - work.x0 as f64) * work.sx, (y - work.y0 as f64) * work.sy)).collect();
    let inside = rasterize(&polygon_work, w, h);
    let inside_count = inside.iter().filter(|&&b| b).count();
    if inside_count < 64 {
        return None;
    }

    // Filter radius scales with the working size so the same fraction of the
    // object is smoothed whether the selection was tiny or huge.
    let radius = (w.max(h) / 55).max(2);

    // What the caller already knows, at working resolution.
    let (hint_object, hint_background) = match hints {
        Some(hints) => {
            let mut object = vec![false; w * h];
            let mut background = vec![false; w * h];
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    if !inside[i] {
                        continue;
                    }
                    let fx = work.x0 as f64 + (x as f64 + 0.5) / work.sx;
                    let fy = work.y0 as f64 + (y as f64 + 0.5) / work.sy;
                    object[i] = (hints.object)(fx, fy);
                    background[i] = !object[i] && (hints.background)(fx, fy);
                }
            }
            (Some(object), Some(background))
        }
        None => (None, None),
    };

    // Best first, each with the next as its safety net: the object network's
    // opinion; else the backdrop flooded in from the loop's edge; else the
    // colour models alone. A prior that leaves almost nothing was wrong.
    // (The flood assumes the strip just inside the loop is backdrop, which
    // is exactly what a brush loop hugging the object is not — skipped then.)
    let min_prior_area = inside_count as f64 * MIN_PRIOR_RESULT_FRACTION;
    let has_area = |mask: &Vec<bool>| mask.iter().filter(|&&m| m).count() as f64 >= min_prior_area;

    let from_network = network_prior(&work, &inside, inside_count, network)
        .map(|mut prior| {
            if let (Some(object), Some(background)) = (&hint_object, &hint_background) {
                for i in 0..w * h {
                    prior.object[i] |= object[i];
                    prior.background[i] = (prior.background[i] || background[i]) && !object[i];
                }
            }
            prior
        })
        .and_then(|prior| segment(&work, &inside, radius, Some(&prior.background), Some(&prior.object)))
        .filter(|mask| mask.iter().filter(|&&m| m).count() as f64 >= inside_count as f64 * MIN_AREA_FRACTION);
    let filled = match from_network {
        Some(mask) => mask,
        None if hints.is_some() => segment(&work, &inside, radius, hint_background.as_deref(), hint_object.as_deref())?,
        None => {
            let backdrop = boundary_backdrop(&work, &inside, inside_count, &polygon_work);
            let guided = backdrop.as_deref().and_then(|b| segment(&work, &inside, radius, Some(b), None)).filter(has_area);
            match guided {
                Some(mask) => mask,
                None => segment(&work, &inside, radius, None, None)?,
            }
        }
    };

    let area = filled.iter().filter(|&&b| b).count();
    if (area as f64) < inside_count as f64 * MIN_AREA_FRACTION {
        return None;
    }

    // Pass two: exact edge at full resolution. Falls back to the coarse mask
    // (still correct, just a few pixels looser) if it can't run.
    let exact = refine_full_resolution(image, &work, polygon, &filled).and_then(|mask| {
        let main = largest_component(&mask, work.region_w, work.region_h)?;
        Some(fill_holes(&main, work.region_w, work.region_h))
    });
    let (mask, mask_w, mask_h, mask_sx, mask_sy) = match exact {
        Some(mask) => (mask, work.region_w, work.region_h, 1.0, 1.0),
        None => (filled, w, h, work.sx, work.sy),
    };

    Some(SnapMask { mask, w: mask_w, h: mask_h, x0: work.x0, y0: work.y0, sx: mask_sx, sy: mask_sy })
}

/// A filled mask → the final smooth outline in frame pixels. `origin` is the
/// mask's top-left in the frame, `scale` its pixels per frame pixel.
fn outline_from_mask(mask: &[bool], w: usize, h: usize, origin: (f64, f64), scale: (f64, f64)) -> Option<Vec<(f64, f64)>> {
    // Per-pixel classification leaves single-pixel nicks and specks along the
    // edge; an open/close takes those off before tracing.
    let mask = morph(&morph(mask, w, h, 1, true), w, h, 1, false);
    let mask = morph(&morph(&mask, w, h, 1, false), w, h, 1, true);
    let outline = trace_outline(&mask, w, h)?;
    let frame_points: Vec<(f64, f64)> = outline
        .iter()
        .map(|&(x, y)| (origin.0 + x as f64 / scale.0, origin.1 + y as f64 / scale.1))
        .collect();
    // Heavy enough that the result reads as a curve, like a background
    // remover's cut-out, not a pixel staircase: three wide passes act as a
    // Gaussian of a few pixels. On a curve of any real size that moves the
    // edge by a fraction of a pixel; only true corners round off slightly.
    let smoothed = smooth_keeping_corners(&frame_points, SMOOTH_PASSES, SMOOTH_HALF_WINDOW);
    let outline = resample_closed(&smoothed, OUTLINE_SPACING);
    (outline.len() >= 3).then_some(outline)
}

/// One brush stroke from the Edit-mode brush: a path of centres and a radius,
/// in frame pixels.
#[derive(serde::Deserialize)]
pub struct BrushStroke {
    pub add: bool,
    pub radius: f64,
    pub points: Vec<(f64, f64)>,
}

const MIN_BRUSH_RADIUS: f64 = 3.0;
const MAX_BRUSH_RADIUS: f64 = 300.0;

/// Pro-only. Applies brush strokes to a selection outline — painting adds to
/// the selection, erasing takes away — and returns the new smooth outline.
/// Pure geometry: no capture is needed.
#[tauri::command]
pub async fn brush_selection(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    points: Vec<(f64, f64)>,
    strokes: Vec<BrushStroke>,
    frame_size: (f64, f64),
) -> Result<Option<Vec<(f64, f64)>>, String> {
    if state.entitlements().plan == Plan::Free {
        return Err("Snap is a Pro feature".to_string());
    }
    let frame = state.frame_cache();
    tokio::task::spawn_blocking(move || {
        // With the capture to hand a stroke goes through Edit mode's own
        // snapping, network included; without it, it falls back to the
        // brush's plain shape.
        let image = frame.and_then(|f| image::load_from_memory(&f.png_bytes).ok()).map(|i| i.to_rgba8());
        let model = crate::objectnet::ensure_model(&app, || {
            let _ = app.emit("snap-status", "downloading");
        });
        let network = |picture: &RgbImage| -> Option<Vec<f32>> {
            let path = model.as_ref()?;
            match crate::objectnet::predict(path, picture) {
                Ok(map) => Some(map),
                Err(err) => {
                    tracing::warn!(%err, "object network failed; brushing on colour alone");
                    None
                }
            }
        };
        brush_polygon(image.as_ref(), &points, &strokes, frame_size, &network)
    })
    .await
    .map_err(|err| format!("brush worker failed: {err}"))
}

pub fn brush_polygon(
    image: Option<&RgbaImage>,
    polygon: &[(f64, f64)],
    strokes: &[BrushStroke],
    frame_size: (f64, f64),
    network: &dyn Fn(&RgbImage) -> Option<Vec<f32>>,
) -> Option<Vec<(f64, f64)>> {
    let frame_size = image.map_or(frame_size, |i| (i.width() as f64, i.height() as f64));
    if polygon.len() < 3 || !(frame_size.0 >= 1.0 && frame_size.1 >= 1.0) {
        return None;
    }
    let finite = |p: &(f64, f64)| p.0.is_finite() && p.1.is_finite();
    if !polygon.iter().all(finite) || strokes.iter().any(|s| !s.points.iter().all(finite) || !s.radius.is_finite()) {
        return None;
    }
    // The region: the outline and every stroke (with its radius), kept to the
    // frame and to a size we are willing to rasterize.
    let mut min = (f64::MAX, f64::MAX);
    let mut max = (f64::MIN, f64::MIN);
    let mut grow = |p: (f64, f64), r: f64| {
        min = (min.0.min(p.0 - r), min.1.min(p.1 - r));
        max = (max.0.max(p.0 + r), max.1.max(p.1 + r));
    };
    polygon.iter().for_each(|&p| grow(p, 0.0));
    for stroke in strokes {
        let r = stroke.radius.clamp(MIN_BRUSH_RADIUS, MAX_BRUSH_RADIUS);
        stroke.points.iter().for_each(|&p| grow(p, r));
    }
    let pad = 2.0 + strokes.iter().map(|s| s.radius.clamp(MIN_BRUSH_RADIUS, MAX_BRUSH_RADIUS)).fold(0.0, f64::max) * BRUSH_RING;
    let x0 = (min.0.floor() - pad).clamp(0.0, frame_size.0 - 1.0) as usize;
    let y0 = (min.1.floor() - pad).clamp(0.0, frame_size.1 - 1.0) as usize;
    let x1 = (max.0.ceil() + pad).clamp(x0 as f64 + 1.0, frame_size.0) as usize;
    let y1 = (max.1.ceil() + pad).clamp(y0 as f64 + 1.0, frame_size.1) as usize;
    let (w, h) = (x1 - x0, y1 - y0);
    if w * h > MAX_FULL_RES_PIXELS {
        return None;
    }

    let local: Vec<(f64, f64)> = polygon.iter().map(|&(x, y)| (x - x0 as f64, y - y0 as f64)).collect();
    let mut mask = rasterize(&local, w, h);
    for stroke in strokes {
        let r = stroke.radius.clamp(MIN_BRUSH_RADIUS, MAX_BRUSH_RADIUS);
        let centres: Vec<(f64, f64)> = stroke.points.iter().map(|&(x, y)| (x - x0 as f64, y - y0 as f64)).collect();
        let snapped = image.and_then(|image| snap_stroke(image, network, (x0, y0, w, h), &mask, &centres, r, stroke.add));
        match snapped {
            Some(next) => mask = next,
            None => paint_capsules(&mut mask, w, h, &centres, r, stroke.add),
        }
    }
    let main = largest_component(&mask, w, h)?;
    let filled = fill_holes(&main, w, h);
    outline_from_mask(&filled, w, h, (x0 as f64, y0 as f64), (1.0, 1.0))
}

/// How far past the brush the region is padded, as a fraction of the radius,
/// so the snapping has background to learn from beside the stroke.
const BRUSH_RING: f64 = 0.7;
/// The very middle of the brush (this fraction of its radius) is always taken
/// as meant; only the rest is left to the picture to decide.
const BRUSH_CORE: f64 = 0.2;
/// Edit mode's snapped selection is eroded by this much before being used as
/// "already certain", so the old boundary can still be re-snapped.
const BRUSH_KEEP_MARGIN: usize = 2;

/// One brush stroke, snapped with Edit mode's own pipeline.
///
/// - **Adding:** the loop handed to the pipeline is the current selection
///   plus the brush, and the selection (and the middle of the brush) is given
///   as known object. The usual machinery — the object network, the colour
///   models, the edge-snapping filter and the full-resolution pass — then
///   decides which of the newly brushed pixels belong to the object, and
///   where its real edge is.
/// - **Erasing:** the loop is the current selection; the selection away from
///   the brush is known object and the middle of the brush is known
///   background, and the pipeline decides the rest of what was brushed.
///
/// Only pixels under the brush can change. `None` (the pipeline found no
/// object) means the caller applies the plain round brush.
fn snap_stroke(
    image: &RgbaImage,
    network: &dyn Fn(&RgbImage) -> Option<Vec<f32>>,
    (x0, y0, w, h): (usize, usize, usize, usize),
    selection: &[bool],
    centres: &[(f64, f64)],
    radius: f64,
    add: bool,
) -> Option<Vec<bool>> {
    let n = w * h;
    let mut brush = vec![false; n];
    paint_capsules(&mut brush, w, h, centres, radius, true);
    let mut core = vec![false; n];
    paint_capsules(&mut core, w, h, centres, radius * BRUSH_CORE, true);

    let known_selection = morph(selection, w, h, BRUSH_KEEP_MARGIN, true);
    let (known_object, known_background): (Vec<bool>, Vec<bool>) = if add {
        ((0..n).map(|i| known_selection[i] || core[i]).collect(), vec![false; n])
    } else {
        let near_brush = morph(&brush, w, h, BRUSH_KEEP_MARGIN, false);
        ((0..n).map(|i| known_selection[i] && !near_brush[i]).collect(), (0..n).map(|i| core[i] && selection[i]).collect())
    };

    let loop_mask: Vec<bool> = if add { (0..n).map(|i| selection[i] || brush[i]).collect() } else { selection.to_vec() };
    let main = largest_component(&loop_mask, w, h)?;
    let polygon = outline_from_mask(&main, w, h, (x0 as f64, y0 as f64), (1.0, 1.0))?;

    // Frame point → this region's pixel.
    let lookup = |mask: &[bool], x: f64, y: f64| -> bool {
        let (lx, ly) = (x - x0 as f64, y - y0 as f64);
        if lx < 0.0 || ly < 0.0 {
            return false;
        }
        let (ix, iy) = (lx as usize, ly as usize);
        ix < w && iy < h && mask[iy * w + ix]
    };
    let object = |x: f64, y: f64| lookup(&known_object, x, y);
    let background = |x: f64, y: f64| lookup(&known_background, x, y);
    let snapped = snap_mask(image, &polygon, network, Some(&Hints { object: &object, background: &background }))?;

    Some(
        (0..n)
            .map(|i| {
                if !brush[i] {
                    return selection[i];
                }
                let (fx, fy) = ((x0 + i % w) as f64 + 0.5, (y0 + i / w) as f64 + 0.5);
                let inside_snapped = snapped.at(fx, fy);
                if add { selection[i] || inside_snapped } else { selection[i] && inside_snapped }
            })
            .collect(),
    )
}

/// Sets (or clears) every pixel whose centre is within `radius` of the path.
fn paint_capsules(mask: &mut [bool], w: usize, h: usize, path: &[(f64, f64)], radius: f64, value: bool) {
    let segments: Vec<((f64, f64), (f64, f64))> = match path.len() {
        0 => return,
        1 => vec![(path[0], path[0])],
        _ => path.windows(2).map(|pair| (pair[0], pair[1])).collect(),
    };
    for (a, b) in segments {
        let xs = (a.0.min(b.0) - radius).floor().max(0.0) as usize;
        let ys = (a.1.min(b.1) - radius).floor().max(0.0) as usize;
        let xe = ((a.0.max(b.0) + radius).ceil().max(0.0) as usize).min(w.saturating_sub(1));
        let ye = ((a.1.max(b.1) + radius).ceil().max(0.0) as usize).min(h.saturating_sub(1));
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let length_sq = dx * dx + dy * dy;
        for y in ys..=ye {
            for x in xs..=xe {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let t = if length_sq > 0.0 { (((px - a.0) * dx + (py - a.1) * dy) / length_sq).clamp(0.0, 1.0) } else { 0.0 };
                let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
                if (px - cx).powi(2) + (py - cy).powi(2) <= radius * radius {
                    mask[y * w + x] = value;
                }
            }
        }
    }
}

/// Re-decides the pixels near the coarse edge at the screenshot's real
/// resolution. Everything clearly inside the coarse shape stays object,
/// everything clearly outside stays background, and each pixel in the band
/// between is classified from its *own* colour against the object and
/// background models fitted to the final coarse split. Returns a mask over
/// the whole region (not the downscaled copy).
fn refine_full_resolution(image: &RgbaImage, work: &Work, polygon: &[(f64, f64)], coarse: &[bool]) -> Option<Vec<bool>> {
    let (rw, rh) = (work.region_w, work.region_h);
    if rw * rh > MAX_FULL_RES_PIXELS {
        return None;
    }
    let (w, h) = (work.w, work.h);

    let object_model = Gmm::fit(&sample(&work.pixels, coarse, true))?;
    let background_model = Gmm::fit(&sample(&work.pixels, coarse, false))?;

    let certain_object = morph(coarse, w, h, BAND_WORK_RADIUS, true);
    let possible_object = morph(coarse, w, h, BAND_WORK_RADIUS, false);

    // The user's loop is still an upper bound, now measured in real pixels.
    let shifted: Vec<(f64, f64)> = polygon.iter().map(|&(x, y)| (x - work.x0 as f64, y - work.y0 as f64)).collect();
    let in_loop = rasterize(&shifted, rw, rh);

    let raw = image.as_raw();
    let image_w = image.width() as usize;
    let mut labels = vec![false; rw * rh];
    let mut in_band = vec![false; rw * rh];
    for y in 0..rh {
        let work_row = ((y as f64 * work.sy) as usize).min(h - 1) * w;
        for x in 0..rw {
            let i = y * rw + x;
            if !in_loop[i] {
                continue;
            }
            let wi = work_row + ((x as f64 * work.sx) as usize).min(w - 1);
            if certain_object[wi] {
                labels[i] = true;
            } else if possible_object[wi] {
                in_band[i] = true;
                let p = ((work.y0 + y) * image_w + work.x0 + x) * 4;
                let colour = [raw[p] as f32 / 255.0, raw[p + 1] as f32 / 255.0, raw[p + 2] as f32 / 255.0];
                labels[i] = object_model.log_likelihood(colour) > background_model.log_likelihood(colour);
            }
        }
    }

    // Despeckle only: a band pixel flips if it is all alone against its
    // eight neighbours. A plain majority vote would also shave every sharp
    // corner by a pixel (a corner pixel has just four of nine neighbours on
    // its side), which is exactly the precision this pass exists for.
    let mut out = labels.clone();
    for y in 0..rh {
        for x in 0..rw {
            let i = y * rw + x;
            if !in_band[i] {
                continue;
            }
            let mut object_neighbours = 0;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < rw && (ny as usize) < rh && labels[ny as usize * rw + nx as usize] {
                        object_neighbours += 1;
                    }
                }
            }
            if labels[i] && object_neighbours <= 1 {
                out[i] = false;
            } else if !labels[i] && object_neighbours >= 7 {
                out[i] = true;
            }
        }
    }
    Some(out)
}

/// The colour-model + guided-filter iteration (steps 1-4). `backdrop` is the
/// certain background from the earlier steps, if any: it is never allowed to
/// become object, and it joins the background model's samples. `sure_object`
/// is the network's confident object pixels, held as object throughout —
/// that is what keeps a white shoe's white parts from being argued into the
/// backdrop by colour alone.
fn segment(work: &Work, inside: &[bool], radius: usize, backdrop: Option<&[bool]>, sure_object: Option<&[bool]>) -> Option<Vec<bool>> {
    let (w, h) = (work.w, work.h);
    let n = w * h;
    let inside_count = inside.iter().filter(|&&b| b).count();
    let is_backdrop = |i: usize| backdrop.is_some_and(|b| b[i]);
    let is_sure = |i: usize| sure_object.is_some_and(|o| o[i]);

    let mut object: Vec<bool> = (0..n).map(|i| inside[i] && !is_backdrop(i)).collect();
    let mut probability = vec![0f32; n];

    for iteration in 0..ITERATIONS {
        let object_samples = sample(&work.pixels, &object, true);
        let background_samples = sample(&work.pixels, &object, false);
        let object_model = Gmm::fit(&object_samples)?;
        let background_model = Gmm::fit(&background_samples)?;

        for i in 0..n {
            probability[i] = if is_sure(i) {
                1.0
            } else if inside[i] && !is_backdrop(i) {
                let evidence = background_model.log_likelihood(work.pixels[i]) - object_model.log_likelihood(work.pixels[i]);
                1.0 / (1.0 + evidence.clamp(-30.0, 30.0).exp())
            } else {
                0.0
            };
        }

        let refined = guided_filter(&work.pixels, &probability, w, h, radius, GUIDED_EPS);
        let next: Vec<bool> = (0..n).map(|i| inside[i] && !is_backdrop(i) && (is_sure(i) || refined[i] > 0.5)).collect();
        let changed = next.iter().zip(&object).filter(|(a, b)| a != b).count();
        object = next;
        // Settled: fewer than 0.2% of the loop's pixels moved. The first
        // pass is always followed by one more — it fits the object model to
        // the whole loop, background included, so it can't be trusted yet.
        if iteration > 0 && changed * 500 < inside_count {
            break;
        }
    }

    let closed = morph(&morph(&object, w, h, (radius / 2).max(1), false), w, h, (radius / 2).max(1), true);
    let closed: Vec<bool> = closed.iter().zip(inside).map(|(&c, &i)| c && i).collect();

    let main = largest_component(&closed, w, h)?;
    Some(fill_holes(&main, w, h))
}

/// The object network's opinion, at working resolution.
struct NetPrior {
    background: Vec<bool>,
    object: Vec<bool>,
}

/// Runs the network on the region and turns its soft map into two *confident*
/// masks, background and object, leaving everything it was unsure of to the
/// colour models. `None` if there is no network, it failed, or what it said
/// doesn't hold together (a map of the wrong shape, or "nothing"/"everything"
/// inside the loop).
fn network_prior(
    work: &Work,
    inside: &[bool],
    inside_count: usize,
    network: &dyn Fn(&RgbImage) -> Option<Vec<f32>>,
) -> Option<NetPrior> {
    let (w, h) = (work.w, work.h);
    let picture = RgbImage::from_fn(w as u32, h as u32, |x, y| {
        let p = work.pixels[y as usize * w + x as usize];
        image::Rgb([0, 1, 2].map(|c| (p[c] * 255.0).round().clamp(0.0, 255.0) as u8))
    });
    let map = network(&picture)?;
    let side = (map.len() as f64).sqrt() as usize;
    if side < 2 || side * side != map.len() {
        return None;
    }

    // Bilinear lookup of the square map at each working pixel.
    let at = |x: usize, y: usize| -> f32 {
        let fx = ((x as f32 + 0.5) / w as f32 * side as f32 - 0.5).clamp(0.0, side as f32 - 1.0);
        let fy = ((y as f32 + 0.5) / h as f32 * side as f32 - 0.5).clamp(0.0, side as f32 - 1.0);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(side - 1), (y0 + 1).min(side - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let top = map[y0 * side + x0] * (1.0 - tx) + map[y0 * side + x1] * tx;
        let bottom = map[y1 * side + x0] * (1.0 - tx) + map[y1 * side + x1] * tx;
        top * (1.0 - ty) + bottom * ty
    };

    let mut background = vec![false; w * h];
    let mut object = vec![false; w * h];
    let mut object_pixels = 0usize;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !inside[i] {
                continue;
            }
            let value = at(x, y);
            background[i] = value < NET_BACKGROUND_BELOW;
            object[i] = value > NET_OBJECT_ABOVE;
            if value > 0.5 {
                object_pixels += 1;
            }
        }
    }
    let share = object_pixels as f64 / inside_count as f64;
    ((NET_MIN_OBJECT_SHARE..=NET_MAX_OBJECT_SHARE).contains(&share)).then_some(NetPrior { background, object })
}

/// Step 0: the pixels inside the loop that are connected to its edge through
/// flat, uniformly coloured ground, without crossing a real edge. `None` when
/// the edge strip isn't flat enough to say anything, or when the flood would
/// swallow nearly everything (a tight loop, or a flat-coloured object).
fn boundary_backdrop(work: &Work, inside: &[bool], inside_count: usize, polygon_work: &[(f64, f64)]) -> Option<Vec<bool>> {
    let (w, h) = (work.w, work.h);
    let n = w * h;
    let colour = |i: usize| work.pixels[i];

    // How much each pixel's colour differs from its right/down neighbour.
    let mut gradient = vec![0f32; n];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut g = 0f32;
            if x + 1 < w {
                g = g.max(distance_sq(colour(i), colour(i + 1)).sqrt());
            }
            if y + 1 < h {
                g = g.max(distance_sq(colour(i), colour(i + w)).sqrt());
            }
            gradient[i] = g;
        }
    }

    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in polygon_work {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let strip = (((max_x - min_x).min(max_y - min_y) * EDGE_STRIP_FRACTION).round() as usize).max(2);
    let core = morph(inside, w, h, strip, true);

    let seeds: Vec<usize> = (0..n).filter(|&i| inside[i] && !core[i] && gradient[i] < FLAT_GRADIENT).collect();
    if seeds.len() < 40 {
        return None;
    }
    let seed_colours: Vec<Rgb> = seeds.iter().step_by((seeds.len() / MAX_SAMPLES).max(1)).map(|&i| colour(i)).collect();
    // Only the strip's *dominant* colours count as backdrop. Where the loop
    // crosses the object, a few of its flat pixels land in the strip too; as a
    // minority they must not be allowed to start a flood through the object.
    let palette = Gmm::fit(&seed_colours)?.dominant(DOMINANT_SHARE);
    let noise = seeds.iter().map(|&i| gradient[i]).sum::<f32>() / seeds.len() as f32;
    let step_limit = FLOOD_STEP.max(3.0 * noise);

    let mut backdrop = vec![false; n];
    let mut queue = std::collections::VecDeque::new();
    for &i in &seeds {
        if palette.mahalanobis_sq(colour(i)) <= FLOOD_PALETTE_SQ {
            backdrop[i] = true;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % w, i / w);
        let mut neighbours = [usize::MAX; 4];
        if x > 0 {
            neighbours[0] = i - 1;
        }
        if x + 1 < w {
            neighbours[1] = i + 1;
        }
        if y > 0 {
            neighbours[2] = i - w;
        }
        if y + 1 < h {
            neighbours[3] = i + w;
        }
        for j in neighbours {
            if j == usize::MAX || backdrop[j] || !inside[j] {
                continue;
            }
            if distance_sq(colour(i), colour(j)).sqrt() <= step_limit && palette.mahalanobis_sq(colour(j)) <= FLOOD_PALETTE_SQ {
                backdrop[j] = true;
                queue.push_back(j);
            }
        }
    }

    let claimed = backdrop.iter().filter(|&&b| b).count();
    (claimed > 0 && (claimed as f64) < inside_count as f64 * MAX_BACKDROP_FRACTION).then_some(backdrop)
}

/// The region around the loop, downscaled, as 0..1 RGB.
struct Work {
    w: usize,
    h: usize,
    /// Frame-pixel origin and size of the region, and work-pixels per
    /// frame-pixel.
    x0: usize,
    y0: usize,
    region_w: usize,
    region_h: usize,
    sx: f64,
    sy: f64,
    pixels: Vec<Rgb>,
}

impl Work {
    fn build(image: &RgbaImage, polygon: &[(f64, f64)]) -> Option<Work> {
        let (image_w, image_h) = (image.width() as usize, image.height() as usize);
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in polygon {
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
        let (box_w, box_h) = (max_x - min_x, max_y - min_y);
        if box_w < MIN_LOOP_SIZE || box_h < MIN_LOOP_SIZE {
            return None;
        }

        // Background has to be *seen* to be modelled: the margin is where the
        // background model's samples come from.
        let margin = (0.15 * box_w.max(box_h)).max(12.0);
        let x0 = (min_x - margin).floor().max(0.0) as usize;
        let y0 = (min_y - margin).floor().max(0.0) as usize;
        let x1 = ((max_x + margin).ceil().max(0.0) as usize).min(image_w);
        let y1 = ((max_y + margin).ceil().max(0.0) as usize).min(image_h);
        if x1 < x0 + 8 || y1 < y0 + 8 {
            return None;
        }
        let (region_w, region_h) = (x1 - x0, y1 - y0);

        let scale = (MAX_WORK_DIM / region_w.max(region_h) as f64).min(1.0);
        let w = ((region_w as f64 * scale).round() as usize).max(1);
        let h = ((region_h as f64 * scale).round() as usize).max(1);
        let (sx, sy) = (w as f64 / region_w as f64, h as f64 / region_h as f64);

        let raw = image.as_raw();
        let mut pixels = Vec::with_capacity(w * h);
        for j in 0..h {
            let ys = (j as f64 / sy) as usize;
            let ye = (((j + 1) as f64 / sy).ceil() as usize).clamp(ys + 1, region_h);
            for i in 0..w {
                let xs = (i as f64 / sx) as usize;
                let xe = (((i + 1) as f64 / sx).ceil() as usize).clamp(xs + 1, region_w);
                let mut sum = [0f32; 3];
                for y in ys..ye {
                    let row = ((y0 + y) * image_w + x0) * 4;
                    for x in xs..xe {
                        let p = row + x * 4;
                        sum[0] += raw[p] as f32;
                        sum[1] += raw[p + 1] as f32;
                        sum[2] += raw[p + 2] as f32;
                    }
                }
                let count = ((ye - ys) * (xe - xs)) as f32 * 255.0;
                pixels.push([sum[0] / count, sum[1] / count, sum[2] / count]);
            }
        }
        Some(Work { w, h, x0, y0, region_w, region_h, sx, sy, pixels })
    }
}

/// Even-odd scanline fill, evaluated at pixel centres.
fn rasterize(polygon: &[(f64, f64)], w: usize, h: usize) -> Vec<bool> {
    let mut mask = vec![false; w * h];
    let mut crossings: Vec<f64> = Vec::new();
    for y in 0..h {
        let yc = y as f64 + 0.5;
        crossings.clear();
        for i in 0..polygon.len() {
            let (x1, y1) = polygon[i];
            let (x2, y2) = polygon[(i + 1) % polygon.len()];
            if (y1 <= yc && y2 > yc) || (y2 <= yc && y1 > yc) {
                crossings.push(x1 + (yc - y1) / (y2 - y1) * (x2 - x1));
            }
        }
        crossings.sort_by(|a, b| a.total_cmp(b));
        for pair in crossings.chunks_exact(2) {
            let from = (pair[0] - 0.5).ceil().max(0.0) as usize;
            let to = ((pair[1] - 0.5).ceil().max(0.0) as usize).min(w);
            for x in from..to {
                mask[y * w + x] = true;
            }
        }
    }
    mask
}

/// Evenly spaced picks of the pixels whose mask value equals `wanted`.
fn sample(pixels: &[Rgb], mask: &[bool], wanted: bool) -> Vec<Rgb> {
    let total = mask.iter().filter(|&&m| m == wanted).count();
    let step = (total / MAX_SAMPLES).max(1);
    let mut seen = 0usize;
    let mut out = Vec::with_capacity(total.min(MAX_SAMPLES) + 1);
    for (pixel, &m) in pixels.iter().zip(mask) {
        if m == wanted {
            if seen.is_multiple_of(step) {
                out.push(*pixel);
            }
            seen += 1;
        }
    }
    out
}

struct Component {
    log_weight: f32,
    mean: Rgb,
    inverse_variance: Rgb,
    log_norm: f32,
}

/// A few diagonal Gaussians fitted by k-means — enough to describe "the
/// colours this thing is made of" without a full EM loop.
struct Gmm {
    components: Vec<Component>,
}

impl Gmm {
    fn fit(samples: &[Rgb]) -> Option<Gmm> {
        let n = samples.len();
        if n < 8 {
            return None;
        }
        let k = GMM_COMPONENTS.min(n / 40).max(1);

        // Deterministic init: centres at brightness quantiles.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            let (pa, pb) = (samples[a], samples[b]);
            (pa[0] + pa[1] + pa[2]).total_cmp(&(pb[0] + pb[1] + pb[2]))
        });
        let mut centers: Vec<Rgb> = (0..k).map(|i| samples[order[(2 * i + 1) * n / (2 * k)]]).collect();
        let mut assignment = vec![0usize; n];

        for _ in 0..8 {
            for (i, s) in samples.iter().enumerate() {
                assignment[i] = (0..k)
                    .min_by(|&a, &b| distance_sq(*s, centers[a]).total_cmp(&distance_sq(*s, centers[b])))
                    .unwrap_or(0);
            }
            let mut sums = vec![[0f32; 3]; k];
            let mut counts = vec![0usize; k];
            for (i, s) in samples.iter().enumerate() {
                let c = assignment[i];
                counts[c] += 1;
                for ch in 0..3 {
                    sums[c][ch] += s[ch];
                }
            }
            for c in 0..k {
                if counts[c] > 0 {
                    for ch in 0..3 {
                        centers[c][ch] = sums[c][ch] / counts[c] as f32;
                    }
                }
            }
        }

        let mut components = Vec::new();
        for c in 0..k {
            let members: Vec<&Rgb> = samples.iter().zip(&assignment).filter(|(_, &a)| a == c).map(|(s, _)| s).collect();
            if members.len() < 3 {
                continue;
            }
            let count = members.len() as f32;
            let mut mean = [0f32; 3];
            for m in &members {
                for ch in 0..3 {
                    mean[ch] += m[ch];
                }
            }
            for ch in mean.iter_mut() {
                *ch /= count;
            }
            let mut variance = [0f32; 3];
            for m in &members {
                for ch in 0..3 {
                    variance[ch] += (m[ch] - mean[ch]).powi(2);
                }
            }
            for ch in variance.iter_mut() {
                *ch = (*ch / count).max(VARIANCE_FLOOR);
            }
            let log_norm = -0.5 * variance.iter().map(|v| v.ln()).sum::<f32>() - 1.5 * (2.0 * std::f32::consts::PI).ln();
            components.push(Component {
                log_weight: (count / n as f32).ln(),
                mean,
                inverse_variance: [1.0 / variance[0], 1.0 / variance[1], 1.0 / variance[2]],
                log_norm,
            });
        }
        (!components.is_empty()).then_some(Gmm { components })
    }

    /// Keeps only the components holding at least `min_share` of the total
    /// weight (always at least the heaviest one).
    fn dominant(mut self, min_share: f32) -> Gmm {
        let total: f32 = self.components.iter().map(|c| c.log_weight.exp()).sum();
        let heaviest = self.components.iter().map(|c| c.log_weight).fold(f32::MIN, f32::max);
        self.components.retain(|c| c.log_weight.exp() >= min_share * total || c.log_weight >= heaviest);
        self
    }

    /// Squared distance to the nearest component, in standard deviations
    /// summed over the three channels.
    fn mahalanobis_sq(&self, x: Rgb) -> f32 {
        self.components
            .iter()
            .map(|c| (0..3).map(|ch| (x[ch] - c.mean[ch]).powi(2) * c.inverse_variance[ch]).sum::<f32>())
            .fold(f32::MAX, f32::min)
    }

    fn log_likelihood(&self, x: Rgb) -> f32 {
        let mut terms = [f32::NEG_INFINITY; GMM_COMPONENTS];
        let mut best = f32::NEG_INFINITY;
        for (slot, c) in terms.iter_mut().zip(&self.components) {
            let mut exponent = 0.0;
            for (ch, value) in x.iter().enumerate() {
                exponent += (value - c.mean[ch]).powi(2) * c.inverse_variance[ch];
            }
            *slot = c.log_weight + c.log_norm - 0.5 * exponent;
            best = best.max(*slot);
        }
        let sum: f32 = terms.iter().filter(|t| t.is_finite()).map(|t| (t - best).exp()).sum();
        best + sum.ln()
    }
}

fn distance_sq(a: Rgb, b: Rgb) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Mean over a square window (clipped at the borders), as two 1-D passes.
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut prefix = vec![0f64; w.max(h) + 1];
    let mut horizontal = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            prefix[x + 1] = prefix[x] + src[y * w + x] as f64;
        }
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(r), (x + r + 1).min(w));
            horizontal[y * w + x] = ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32;
        }
    }
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        for y in 0..h {
            prefix[y + 1] = prefix[y] + horizontal[y * w + x] as f64;
        }
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(r), (y + r + 1).min(h));
            out[y * w + x] = ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32;
        }
    }
    out
}

/// Colour-guided filter (He, Sun & Tang): smooths `p` while following the
/// edges of `guide`.
fn guided_filter(guide: &[Rgb], p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let n = w * h;
    let channel = |c: usize| -> Vec<f32> { guide.iter().map(|g| g[c]).collect() };
    let (ir, ig, ib) = (channel(0), channel(1), channel(2));
    let product = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| x * y).collect() };

    let mean_i = [box_mean(&ir, w, h, r), box_mean(&ig, w, h, r), box_mean(&ib, w, h, r)];
    let mean_p = box_mean(p, w, h, r);
    let corr_ip = [
        box_mean(&product(&ir, p), w, h, r),
        box_mean(&product(&ig, p), w, h, r),
        box_mean(&product(&ib, p), w, h, r),
    ];
    let corr_ii = [
        box_mean(&product(&ir, &ir), w, h, r),
        box_mean(&product(&ir, &ig), w, h, r),
        box_mean(&product(&ir, &ib), w, h, r),
        box_mean(&product(&ig, &ig), w, h, r),
        box_mean(&product(&ig, &ib), w, h, r),
        box_mean(&product(&ib, &ib), w, h, r),
    ];

    let mut a = [vec![0f32; n], vec![0f32; n], vec![0f32; n]];
    let mut b = vec![0f32; n];
    for i in 0..n {
        let m = [mean_i[0][i], mean_i[1][i], mean_i[2][i]];
        let s00 = corr_ii[0][i] - m[0] * m[0] + eps;
        let s01 = corr_ii[1][i] - m[0] * m[1];
        let s02 = corr_ii[2][i] - m[0] * m[2];
        let s11 = corr_ii[3][i] - m[1] * m[1] + eps;
        let s12 = corr_ii[4][i] - m[1] * m[2];
        let s22 = corr_ii[5][i] - m[2] * m[2] + eps;
        let cov = [
            corr_ip[0][i] - m[0] * mean_p[i],
            corr_ip[1][i] - m[1] * mean_p[i],
            corr_ip[2][i] - m[2] * mean_p[i],
        ];

        let det = s00 * (s11 * s22 - s12 * s12) - s01 * (s01 * s22 - s12 * s02) + s02 * (s01 * s12 - s11 * s02);
        let inv_det = 1.0 / det;
        let i00 = (s11 * s22 - s12 * s12) * inv_det;
        let i01 = (s02 * s12 - s01 * s22) * inv_det;
        let i02 = (s01 * s12 - s02 * s11) * inv_det;
        let i11 = (s00 * s22 - s02 * s02) * inv_det;
        let i12 = (s01 * s02 - s00 * s12) * inv_det;
        let i22 = (s00 * s11 - s01 * s01) * inv_det;

        let coeff = [
            i00 * cov[0] + i01 * cov[1] + i02 * cov[2],
            i01 * cov[0] + i11 * cov[1] + i12 * cov[2],
            i02 * cov[0] + i12 * cov[1] + i22 * cov[2],
        ];
        for c in 0..3 {
            a[c][i] = coeff[c];
        }
        b[i] = mean_p[i] - coeff[0] * m[0] - coeff[1] * m[1] - coeff[2] * m[2];
    }

    let mean_a = [box_mean(&a[0], w, h, r), box_mean(&a[1], w, h, r), box_mean(&a[2], w, h, r)];
    let mean_b = box_mean(&b, w, h, r);
    (0..n)
        .map(|i| mean_a[0][i] * guide[i][0] + mean_a[1][i] * guide[i][1] + mean_a[2][i] * guide[i][2] + mean_b[i])
        .collect()
}

/// Dilate (`erode = false`) or erode a square window of radius `r`. Erosion
/// treats the area outside the image as set, so a shape touching the border
/// isn't eaten by it.
fn morph(mask: &[bool], w: usize, h: usize, r: usize, erode: bool) -> Vec<bool> {
    let stride = w + 1;
    let mut sat = vec![0u32; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += mask[y * w + x] as u32;
            sat[(y + 1) * stride + x + 1] = sat[y * stride + x + 1] + row;
        }
    }
    let mut out = vec![false; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let sum = (sat[y1 * stride + x1] + sat[y0 * stride + x0]) - (sat[y0 * stride + x1] + sat[y1 * stride + x0]);
            out[y * w + x] = if erode { sum == ((x1 - x0) * (y1 - y0)) as u32 } else { sum > 0 };
        }
    }
    out
}

/// Flood-fills (4-connected) from `start`, appending visited indices to `out`.
fn flood(mask: &[bool], w: usize, h: usize, start: usize, visited: &mut [bool], out: &mut Vec<usize>) {
    let mut stack = vec![start];
    visited[start] = true;
    while let Some(i) = stack.pop() {
        out.push(i);
        let (x, y) = (i % w, i / w);
        let mut push = |j: usize| {
            if mask[j] && !visited[j] {
                visited[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if y > 0 {
            push(i - w);
        }
        if y + 1 < h {
            push(i + w);
        }
    }
}

fn largest_component(mask: &[bool], w: usize, h: usize) -> Option<Vec<bool>> {
    let mut visited = vec![false; w * h];
    let mut best: Vec<usize> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if mask[start] && !visited[start] {
            current.clear();
            flood(mask, w, h, start, &mut visited, &mut current);
            if current.len() > best.len() {
                std::mem::swap(&mut best, &mut current);
            }
        }
    }
    if best.is_empty() {
        return None;
    }
    let mut out = vec![false; w * h];
    for i in best {
        out[i] = true;
    }
    Some(out)
}

/// Marks every pixel that is not reachable from the border without crossing
/// the shape, then inverts: what's left is the shape with its holes closed.
fn fill_holes(mask: &[bool], w: usize, h: usize) -> Vec<bool> {
    let open: Vec<bool> = mask.iter().map(|&m| !m).collect();
    let mut reached = vec![false; w * h];
    let mut scratch = Vec::new();
    for x in 0..w {
        for y in [0, h - 1] {
            let i = y * w + x;
            if open[i] && !reached[i] {
                flood(&open, w, h, i, &mut reached, &mut scratch);
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            let i = y * w + x;
            if open[i] && !reached[i] {
                flood(&open, w, h, i, &mut reached, &mut scratch);
            }
        }
    }
    reached.iter().map(|&r| !r).collect()
}

/// Walks the outer boundary of a single filled shape along pixel *edges*
/// (lattice corners), clockwise, keeping the shape on the right-hand side.
/// Returns every lattice point on the way, so staircase corners stay exact
/// for the smoothing that follows.
fn trace_outline(mask: &[bool], w: usize, h: usize) -> Option<Vec<(i32, i32)>> {
    let pixel = |x: i32, y: i32| -> bool { x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize] };
    let first = mask.iter().position(|&m| m)?;
    let (start_x, start_y) = ((first % w) as i32, (first / w) as i32);

    // Right, Down, Left, Up; +1 turns right, +3 turns left.
    const DIRS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let (mut vx, mut vy, mut d) = (start_x, start_y, 0usize);
    let mut out = Vec::new();
    let limit = 4 * (w + 1) * (h + 1);
    for _ in 0..limit {
        out.push((vx, vy));
        // The two pixels just beyond this corner, ahead-left and ahead-right
        // of the direction we arrived in.
        let (ahead_left, ahead_right) = match d {
            0 => ((vx, vy - 1), (vx, vy)),
            1 => ((vx, vy), (vx - 1, vy)),
            2 => ((vx - 1, vy), (vx - 1, vy - 1)),
            _ => ((vx - 1, vy - 1), (vx, vy - 1)),
        };
        if pixel(ahead_left.0, ahead_left.1) {
            d = (d + 3) % 4;
        } else if !pixel(ahead_right.0, ahead_right.1) {
            d = (d + 1) % 4;
        }
        vx += DIRS[d].0;
        vy += DIRS[d].1;
        if (vx, vy) == (start_x, start_y) {
            return Some(out);
        }
    }
    None
}

/// Circular moving average — rounds the one-pixel staircase a raster edge
/// leaves without shifting the outline anywhere that matters.
fn smooth_closed(points: &[(f64, f64)], half_window: usize) -> Vec<(f64, f64)> {
    let n = points.len();
    if n < 2 * half_window + 1 {
        return points.to_vec();
    }
    let window = (2 * half_window + 1) as f64;
    (0..n)
        .map(|i| {
            let (mut sx, mut sy) = (0.0, 0.0);
            for k in 0..=2 * half_window {
                let (x, y) = points[(i + n + k - half_window) % n];
                sx += x;
                sy += y;
            }
            (sx / window, sy / window)
        })
        .collect()
}

/// Real corners (a screen, a card, a box) have to stay sharp while curves get
/// smoothed. Corners are found on the raw pixel-edge chain as points where the
/// direction turns sharply over a few pixels; each stretch between two corners
/// is then smoothed as an open line whose ends are pinned, so the corner
/// itself never moves. With no corners it is plain circular smoothing.
fn smooth_keeping_corners(points: &[(f64, f64)], passes: usize, half_window: usize) -> Vec<(f64, f64)> {
    let n = points.len();
    let reach = 6usize;
    if n < 4 * reach {
        let mut out = points.to_vec();
        for _ in 0..passes {
            out = smooth_closed(&out, half_window);
        }
        return out;
    }
    let turn = |i: usize| -> f64 {
        let (a, b, c) = (points[(i + n - reach) % n], points[i], points[(i + reach) % n]);
        let (u, v) = ((b.0 - a.0, b.1 - a.1), (c.0 - b.0, c.1 - b.1));
        (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1).abs().to_degrees()
    };
    let angles: Vec<f64> = (0..n).map(turn).collect();
    let mut corners: Vec<usize> = (0..n)
        .filter(|&i| {
            angles[i] >= CORNER_DEGREES
                && (1..=reach).all(|k| angles[i] > angles[(i + k) % n] || (angles[i] == angles[(i + k) % n] && k > 0 && (i + k) % n > i))
                && (1..=reach).all(|k| angles[i] >= angles[(i + n - k) % n])
        })
        .collect();
    corners.dedup();

    if corners.len() < 2 {
        let mut out = points.to_vec();
        for _ in 0..passes {
            out = smooth_closed(&out, half_window);
        }
        return out;
    }

    let mut out = points.to_vec();
    for w in 0..corners.len() {
        let (start, end) = (corners[w], corners[(w + 1) % corners.len()]);
        let len = (end + n - start) % n;
        if len < 2 {
            continue;
        }
        let mut line: Vec<(f64, f64)> = (0..=len).map(|k| points[(start + k) % n]).collect();
        for _ in 0..passes {
            let source = line.clone();
            for (i, slot) in line.iter_mut().enumerate().take(len).skip(1) {
                // Window shrinks near the pinned ends so they never move.
                let h = half_window.min(i).min(len - i);
                let window = &source[i - h..=i + h];
                let count = window.len() as f64;
                *slot = (window.iter().map(|p| p.0).sum::<f64>() / count, window.iter().map(|p| p.1).sum::<f64>() / count);
            }
        }
        for (k, &pt) in line.iter().enumerate().take(len) {
            out[(start + k) % n] = pt;
        }
    }
    out
}

/// Re-spaces a closed outline evenly by arc length. Corners stay put (they
/// are points on the path like any other); only the *density* changes.
fn resample_closed(points: &[(f64, f64)], spacing: f64) -> Vec<(f64, f64)> {
    let n = points.len();
    if n < 3 {
        return points.to_vec();
    }
    let lengths: Vec<f64> = (0..n).map(|i| dist_sq(points[i], points[(i + 1) % n]).sqrt()).collect();
    let perimeter: f64 = lengths.iter().sum();
    if perimeter <= 0.0 {
        return points.to_vec();
    }
    let count = ((perimeter / spacing).round() as usize).clamp(12, MAX_OUTLINE_POINTS);
    let step = perimeter / count as f64;

    let mut out = Vec::with_capacity(count);
    let (mut segment, mut walked) = (0usize, 0.0f64);
    for k in 0..count {
        let target = k as f64 * step;
        while segment + 1 < n && walked + lengths[segment] < target {
            walked += lengths[segment];
            segment += 1;
        }
        let t = if lengths[segment] > 0.0 { ((target - walked) / lengths[segment]).clamp(0.0, 1.0) } else { 0.0 };
        let (a, b) = (points[segment], points[(segment + 1) % n]);
        out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
    }
    out
}

fn dist_sq(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap deterministic noise so test backgrounds aren't perfectly flat
    /// (a flat colour is a degenerate case for a colour model).
    fn noise(x: u32, y: u32) -> i32 {
        let v = x.wrapping_mul(374_761_393).wrapping_add(y.wrapping_mul(668_265_263)) ^ 0x5bd1_e995;
        ((v ^ (v >> 13)).wrapping_mul(1_274_126_177) >> 28) as i32 - 8
    }

    fn make_image(w: u32, h: u32, color: impl Fn(u32, u32) -> [u8; 3]) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let c = color(x, y);
            let n = noise(x, y) / 2;
            image::Rgba([
                (c[0] as i32 + n).clamp(0, 255) as u8,
                (c[1] as i32 + n).clamp(0, 255) as u8,
                (c[2] as i32 + n).clamp(0, 255) as u8,
                255,
            ])
        })
    }

    fn regular_polygon(cx: f64, cy: f64, radius: f64, sides: usize) -> Vec<(f64, f64)> {
        (0..sides)
            .map(|i| {
                let a = i as f64 / sides as f64 * std::f64::consts::TAU;
                (cx + radius * a.cos(), cy + radius * a.sin())
            })
            .collect()
    }

    fn polygon_area(points: &[(f64, f64)]) -> f64 {
        let n = points.len();
        (0..n)
            .map(|i| {
                let (x1, y1) = points[i];
                let (x2, y2) = points[(i + 1) % n];
                x1 * y2 - x2 * y1
            })
            .sum::<f64>()
            .abs()
            / 2.0
    }

    #[test]
    fn snaps_a_loose_loop_onto_a_disc_on_a_gradient_background() {
        let image = make_image(300, 300, |x, y| {
            let dx = x as f64 - 150.0;
            let dy = y as f64 - 150.0;
            if dx * dx + dy * dy <= 50.0 * 50.0 {
                [230, 60, 50]
            } else {
                [40, 70, (150 + y / 8) as u8]
            }
        });
        // A sloppy loop well outside the disc (radius 85 vs 50).
        let outline = snap_polygon(&image, &regular_polygon(150.0, 150.0, 85.0, 14)).expect("a disc is a clear object");

        for &(x, y) in &outline {
            let d = ((x - 150.0).powi(2) + (y - 150.0).powi(2)).sqrt();
            assert!((48.4..=51.6).contains(&d), "outline point ({x:.1},{y:.1}) is {d:.1}px from the centre, expected 50");
        }
        let expected = std::f64::consts::PI * 50.0 * 50.0;
        let area = polygon_area(&outline);
        assert!((area - expected).abs() / expected < 0.03, "area {area:.0} should be within 3% of {expected:.0}");
    }

    #[test]
    fn snaps_to_a_rectangle_over_a_two_tone_background() {
        let image = make_image(360, 240, |x, y| {
            if (140..220).contains(&x) && (80..160).contains(&y) {
                [40, 170, 70]
            } else if x < 180 {
                [205, 205, 205]
            } else {
                [60, 60, 70]
            }
        });
        let loose = vec![(105.0, 45.0), (255.0, 45.0), (255.0, 195.0), (105.0, 195.0)];
        let outline = snap_polygon(&image, &loose).expect("the green block is a clear object");

        let min_x = outline.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        let max_x = outline.iter().map(|p| p.0).fold(f64::MIN, f64::max);
        let min_y = outline.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        let max_y = outline.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        for (got, want, name) in [(min_x, 140.0, "left"), (max_x, 220.0, "right"), (min_y, 80.0, "top"), (max_y, 160.0, "bottom")] {
            assert!((got - want).abs() <= 1.0, "{name} edge {got:.2} should be exactly {want}");
        }
    }

    #[test]
    fn a_loop_around_nothing_does_not_invent_an_object() {
        let image = make_image(200, 200, |_, _| [120, 130, 140]);
        let result = snap_polygon(&image, &regular_polygon(100.0, 100.0, 60.0, 12));
        // Flat colour has no object. Either answer is acceptable as long as
        // it never claims something outside the loop.
        if let Some(outline) = result {
            for &(x, y) in &outline {
                let d = ((x - 100.0).powi(2) + (y - 100.0).powi(2)).sqrt();
                assert!(d <= 64.0, "outline escaped the loop: {d:.1}");
            }
        }
    }

    #[test]
    fn degenerate_loops_are_rejected() {
        let image = make_image(100, 100, |_, _| [10, 20, 30]);
        assert!(snap_polygon(&image, &[(10.0, 10.0), (20.0, 20.0)]).is_none());
        assert!(snap_polygon(&image, &[(10.0, 10.0), (12.0, 10.0), (11.0, 12.0)]).is_none());
        assert!(snap_polygon(&image, &[(f64::NAN, 1.0), (5.0, 50.0), (60.0, 5.0)]).is_none());
    }

    #[test]
    fn trace_outline_walks_the_edges_of_a_block() {
        let (w, h) = (10, 10);
        let mut mask = vec![false; w * h];
        for y in 3..7 {
            for x in 3..7 {
                mask[y * w + x] = true;
            }
        }
        let outline = trace_outline(&mask, w, h).unwrap();
        assert_eq!(outline.len(), 16, "a 4x4 block has a 16-edge perimeter");
        assert_eq!(outline.iter().map(|p| p.0).min(), Some(3));
        assert_eq!(outline.iter().map(|p| p.0).max(), Some(7));
        assert_eq!(outline.iter().map(|p| p.1).min(), Some(3));
        assert_eq!(outline.iter().map(|p| p.1).max(), Some(7));
    }

    #[test]
    fn fill_holes_closes_interior_gaps_only() {
        let (w, h) = (9, 9);
        let mut mask = vec![false; w * h];
        for y in 2..7 {
            for x in 2..7 {
                mask[y * w + x] = true;
            }
        }
        mask[4 * w + 4] = false; // a hole
        let filled = fill_holes(&mask, w, h);
        assert!(filled[4 * w + 4], "interior hole is filled");
        assert!(!filled[0], "outside stays outside");
    }

    #[test]
    fn guided_filter_snaps_a_soft_ramp_onto_the_guide_edge() {
        let (w, h) = (40, 20);
        let guide: Vec<Rgb> = (0..w * h).map(|i| if i % w < 20 { [0.1, 0.1, 0.1] } else { [0.9, 0.9, 0.9] }).collect();
        // A deliberately soft input: a gentle ramp that changes by only
        // 0.06 per pixel, straddling the guide's hard edge at x = 20.
        let p: Vec<f32> = (0..w * h).map(|i| (((i % w) as f32 - 12.0) / 16.0).clamp(0.0, 1.0)).collect();
        let q = guided_filter(&guide, &p, w, h, 4, 0.004);
        let row = 10 * w;
        let input_jump = p[row + 20] - p[row + 19];
        let output_jump = q[row + 20] - q[row + 19];
        assert!(output_jump > 0.25 && output_jump > input_jump * 4.0, "expected a crisp step at the guide edge, got {output_jump} (input {input_jump})");
        // ...and it stays smooth away from the edge rather than turning blocky.
        assert!((q[row + 16] - q[row + 15]).abs() < 0.1);
    }

    /// Big enough that the coarse pass runs well below full resolution
    /// (region ~1100px against a 512px working copy), so any precision here
    /// is down to the full-resolution pass.
    #[test]
    fn stays_pixel_exact_on_a_selection_far_larger_than_the_working_copy() {
        let image = make_image(1600, 1200, |x, y| {
            let dx = x as f64 + 0.5 - 800.0;
            let dy = y as f64 + 0.5 - 600.0;
            if dx * dx + dy * dy <= 300.0 * 300.0 {
                [230, 60, 50]
            } else {
                [40, 70, (150 + y / 16) as u8]
            }
        });
        let outline = snap_polygon(&image, &regular_polygon(800.0, 600.0, 430.0, 16)).expect("a big disc is a clear object");
        let worst = outline
            .iter()
            .map(|&(x, y)| (((x - 800.0).powi(2) + (y - 600.0).powi(2)).sqrt() - 300.0).abs())
            .fold(0.0, f64::max);
        assert!(worst <= 1.5, "worst deviation from the true edge was {worst:.2}px; a downscaled-only result would be several px off");
    }

    #[test]
    fn sharp_corners_stay_sharp_at_full_resolution() {
        // Hard-edged block; its real corners are at (400,300) and (1000,700).
        let image = make_image(1400, 1000, |x, y| if (400..1000).contains(&x) && (300..700).contains(&y) { [30, 150, 90] } else { [210, 205, 200] });
        let loose = vec![(250.0, 150.0), (1150.0, 150.0), (1150.0, 850.0), (250.0, 850.0)];
        let outline = snap_polygon(&image, &loose).expect("block");
        for corner in [(400.0, 300.0), (1000.0, 300.0), (1000.0, 700.0), (400.0, 700.0)] {
            let nearest = outline.iter().map(|&p| dist_sq(p, corner).sqrt()).fold(f64::MAX, f64::min);
            assert!(nearest <= 1.5, "corner {corner:?} landed {nearest:.2}px away");
        }
        let longest = (0..outline.len()).map(|i| dist_sq(outline[i], outline[(i + 1) % outline.len()]).sqrt()).fold(0.0, f64::max);
        assert!(longest <= 2.6, "outline too sparse: {longest:.2}px gap");
    }

    /// The sneaker case, synthetically: a pale object on a near-white backdrop,
    /// the whole thing sitting on a dark surround, with a loop that spills
    /// over onto the dark. Nothing outside the loop looks like the backdrop,
    /// so colour models alone call the pale backdrop "object".
    fn pale_on_pale_scene(object_half_width: f64) -> RgbaImage {
        make_image(400, 300, move |x, y| {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            let in_card = (60..340).contains(&x);
            let in_object = ((fx - 200.0) / object_half_width).powi(2) + ((fy - 150.0) / 50.0).powi(2) <= 1.0;
            // A real card edge is anti-aliased: a pixel or two of in-between
            // colour that is neither backdrop nor object.
            let on_edge_line = (58..60).contains(&x) || (340..342).contains(&x);
            if on_edge_line {
                [130, 130, 134]
            } else if !in_card {
                [30, 30, 34]
            } else if in_object {
                [222, 222, 226]
            } else {
                [245, 245, 245]
            }
        })
    }

    #[test]
    fn separates_a_pale_object_from_its_pale_backdrop() {
        let image = pale_on_pale_scene(90.0);
        let outline = snap_polygon(&image, &regular_polygon(200.0, 150.0, 150.0, 20)).expect("the object is distinct from its backdrop");
        let min_x = outline.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        let max_x = outline.iter().map(|p| p.0).fold(f64::MIN, f64::max);
        let min_y = outline.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        let max_y = outline.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        for (got, want, name) in [(min_x, 110.0, "left"), (max_x, 290.0, "right"), (min_y, 100.0, "top"), (max_y, 200.0, "bottom")] {
            assert!((got - want).abs() <= 3.0, "{name} edge {got:.1} should be near {want}: it snapped to the card, not the object");
        }
    }

    #[test]
    fn does_not_run_off_along_a_scene_edge_the_object_touches() {
        // Object spans the full card width, so it touches the card's dark
        // border on both sides; that border must not be dragged in.
        let image = pale_on_pale_scene(140.0);
        let outline = snap_polygon(&image, &regular_polygon(200.0, 150.0, 190.0, 24)).expect("object");
        let min_y = outline.iter().map(|p| p.1).fold(f64::MAX, f64::min);
        let max_y = outline.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        assert!(min_y >= 96.0 && max_y <= 204.0, "outline spans y {min_y:.0}..{max_y:.0}; the object only spans 100..200 (a spur along the card edge)");
    }

    /// Stands in for the real network: scores a pixel 1 when `is_object` says
    /// so and 0 otherwise, on a 320x320 grid like the real thing.
    fn fake_network(is_object: impl Fn(u8, u8, u8) -> bool) -> impl Fn(&RgbImage) -> Option<Vec<f32>> {
        move |picture: &RgbImage| {
            let small = image::imageops::resize(picture, 320, 320, image::imageops::FilterType::Triangle);
            Some(small.pixels().map(|p| if is_object(p.0[0], p.0[1], p.0[2]) { 1.0 } else { 0.0 }).collect())
        }
    }

    fn two_discs() -> RgbaImage {
        // A big red disc and a small blue one on grey. Colour alone keeps the
        // bigger of the two.
        make_image(400, 300, |x, y| {
            let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
            if (fx - 130.0).powi(2) + (fy - 150.0).powi(2) <= 45.0 * 45.0 {
                [210, 50, 50]
            } else if (fx - 270.0).powi(2) + (fy - 150.0).powi(2) <= 30.0 * 30.0 {
                [40, 70, 220]
            } else {
                [200, 200, 200]
            }
        })
    }

    #[test]
    fn the_network_decides_which_object_was_meant() {
        let image = two_discs();
        let loop_around_both = vec![(20.0, 40.0), (380.0, 40.0), (380.0, 260.0), (20.0, 260.0)];

        // Without it: the bigger red disc.
        let plain = snap_polygon(&image, &loop_around_both).expect("two clear objects");
        let plain_x = plain.iter().map(|p| p.0).sum::<f64>() / plain.len() as f64;
        assert!(plain_x < 200.0, "colour alone should pick the larger, left disc (centre x {plain_x:.0})");

        // With a network that says the blue one is the object.
        let blue_is_the_object = fake_network(|r, _, b| b > r.saturating_add(100));
        let steered = snap_polygon_with(&image, &loop_around_both, &blue_is_the_object).expect("blue disc");
        for &(x, y) in &steered {
            let d = ((x - 270.0).powi(2) + (y - 150.0).powi(2)).sqrt();
            assert!((27.5..=32.5).contains(&d), "point ({x:.1},{y:.1}) is {d:.1}px from the blue disc's centre, expected 30");
        }
    }

    #[test]
    fn an_implausible_network_is_ignored() {
        let image = make_image(300, 300, |x, y| {
            let (dx, dy) = (x as f64 + 0.5 - 150.0, y as f64 + 0.5 - 150.0);
            if dx * dx + dy * dy <= 50.0 * 50.0 { [230, 60, 50] } else { [40, 70, 190] }
        });
        let loose = regular_polygon(150.0, 150.0, 85.0, 14);
        let want_a_disc = |outline: Option<Vec<(f64, f64)>>, what: &str| {
            let outline = outline.unwrap_or_else(|| panic!("{what}: no outline"));
            for &(x, y) in &outline {
                let d = ((x - 150.0).powi(2) + (y - 150.0).powi(2)).sqrt();
                assert!((48.4..=51.6).contains(&d), "{what}: ({x:.1},{y:.1}) is {d:.1}px out, expected 50");
            }
        };
        // "Nothing found", "everything is object", the wrong shape, and a
        // network that simply isn't there must all fall back to colour.
        want_a_disc(snap_polygon_with(&image, &loose, &|_| Some(vec![0.0; 320 * 320])), "nothing found");
        want_a_disc(snap_polygon_with(&image, &loose, &|_| Some(vec![1.0; 320 * 320])), "everything");
        want_a_disc(snap_polygon_with(&image, &loose, &|_| Some(vec![0.5; 7])), "wrong shape");
        want_a_disc(snap_polygon_with(&image, &loose, &|_| None), "no network");
    }

    #[test]
    fn the_outline_is_smooth_even_when_the_pixels_are_noisy() {
        // A textured disc: the per-pixel decision wobbles along the edge, but
        // the outline must still turn gently, never zig-zag.
        let image = make_image(300, 300, |x, y| {
            let (dx, dy) = (x as f64 + 0.5 - 150.0, y as f64 + 0.5 - 150.0);
            let n = noise(x, y) % 40;
            let base = if dx * dx + dy * dy <= 60.0 * 60.0 { [200, 70, 60] } else { [60, 90, 190] };
            base.map(|c| (c + n - 20).clamp(0, 255) as u8)
        });
        let outline = snap_polygon(&image, &regular_polygon(150.0, 150.0, 95.0, 14)).expect("a disc");
        let n = outline.len();
        let mut worst: f64 = 0.0;
        for i in 0..n {
            let (a, b, c) = (outline[i], outline[(i + 1) % n], outline[(i + 2) % n]);
            let (u, v) = ((b.0 - a.0, b.1 - a.1), (c.0 - b.0, c.1 - b.1));
            let turn = (u.0 * v.1 - u.1 * v.0).atan2(u.0 * v.0 + u.1 * v.1).abs().to_degrees();
            worst = worst.max(turn);
        }
        assert!(worst < 12.0, "sharpest turn along a disc's edge was {worst:.1} degrees");
        for &(x, y) in &outline {
            let d = ((x - 150.0).powi(2) + (y - 150.0).powi(2)).sqrt();
            assert!((58.0..=62.0).contains(&d), "({x:.1},{y:.1}) is {d:.1}px out, expected 60");
        }
    }

    #[test]
    fn brush_adds_and_removes_area_and_stays_smooth() {
        let square = vec![(100.0, 100.0), (200.0, 100.0), (200.0, 200.0), (100.0, 200.0)];
        let area = |poly: &[(f64, f64)]| {
            let n = poly.len();
            (0..n).map(|i| poly[i].0 * poly[(i + 1) % n].1 - poly[(i + 1) % n].0 * poly[i].1).sum::<f64>().abs() / 2.0
        };
        let base = brush_polygon(None, &square, &[], (400.0, 400.0), &|_| None).expect("outline");
        assert!((area(&base) - 10_000.0).abs() < 300.0, "untouched area {}", area(&base));

        // Paint a 20px-wide band out of the right edge.
        let add = BrushStroke { add: true, radius: 10.0, points: vec![(200.0, 130.0), (250.0, 130.0)] };
        let grown = brush_polygon(None, &square, &[add], (400.0, 400.0), &|_| None).expect("grown");
        assert!(area(&grown) > area(&base) + 800.0, "adding should grow it: {} vs {}", area(&grown), area(&base));

        // Erase a strip off the left side.
        let erase = BrushStroke { add: false, radius: 10.0, points: vec![(105.0, 100.0), (105.0, 200.0)] };
        let shrunk = brush_polygon(None, &square, &[erase], (400.0, 400.0), &|_| None).expect("shrunk");
        assert!(area(&shrunk) < area(&base) - 500.0, "erasing should shrink it: {} vs {}", area(&shrunk), area(&base));
        let min_x = shrunk.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        assert!(min_x > 112.0, "left edge should move in, got {min_x}");

        assert!(brush_polygon(None, &square, &[], (0.0, 0.0), &|_| None).is_none());
        let bad = BrushStroke { add: true, radius: f64::NAN, points: vec![(1.0, 1.0)] };
        assert!(brush_polygon(None, &square, &[bad], (400.0, 400.0), &|_| None).is_none());
    }

    #[test]
    fn a_brush_stroke_snaps_to_the_edge_it_was_painted_towards() {
        // A dark square on a pale ground with a bright bump sticking out of
        // its right side. The selection is the square alone; the user paints
        // over the bump with a brush much wider than the bump.
        let image = make_image(400, 300, |x, y| {
            let in_square = (100..200).contains(&x) && (100..200).contains(&y);
            let in_bump = (200..240).contains(&x) && (140..170).contains(&y);
            if in_square || in_bump { [30, 90, 200] } else { [225, 220, 210] }
        });
        let square = vec![(100.0, 100.0), (200.0, 100.0), (200.0, 200.0), (100.0, 200.0)];
        let stroke = BrushStroke { add: true, radius: 40.0, points: vec![(215.0, 150.0), (225.0, 155.0)] };
        let outline = brush_polygon(Some(&image), &square, &[stroke], (400.0, 300.0), &|_| None).expect("outline");
        let max_x = outline.iter().map(|p| p.0).fold(f64::MIN, f64::max);
        let min_y_at_bump = outline.iter().filter(|p| p.0 > 205.0).map(|p| p.1).fold(f64::MAX, f64::min);
        let max_y_at_bump = outline.iter().filter(|p| p.0 > 205.0).map(|p| p.1).fold(f64::MIN, f64::max);
        // The bump is picked up, to its real edge, and the brush's round
        // outline (which reaches x=265 and y=110..200) is not what's kept.
        assert!((238.0..=242.0).contains(&max_x), "bump's far edge should be ~240, got {max_x:.1}");
        assert!((138.0..=142.0).contains(&min_y_at_bump), "bump top ~140, got {min_y_at_bump:.1}");
        assert!((168.0..=172.0).contains(&max_y_at_bump), "bump bottom ~170, got {max_y_at_bump:.1}");

        // The same stroke without the picture is just a disc.
        let plain = BrushStroke { add: true, radius: 40.0, points: vec![(215.0, 150.0), (225.0, 155.0)] };
        let blob = brush_polygon(None, &square, &[plain], (400.0, 300.0), &|_| None).expect("outline");
        let blob_max_x = blob.iter().map(|p| p.0).fold(f64::MIN, f64::max);
        assert!(blob_max_x > 255.0, "without the picture the brush shape wins, got {blob_max_x:.1}");
    }

    #[test]
    fn an_erase_stroke_snaps_to_the_edge_too() {
        // The selection wrongly includes a pale strip on the left of a dark
        // block; the eraser is dragged over it, wider than the strip.
        let image = make_image(400, 300, |x, y| {
            if (150..250).contains(&x) && (100..200).contains(&y) { [30, 90, 200] } else { [225, 220, 210] }
        });
        let loose = vec![(120.0, 100.0), (250.0, 100.0), (250.0, 200.0), (120.0, 200.0)];
        let stroke = BrushStroke { add: false, radius: 30.0, points: vec![(135.0, 120.0), (135.0, 180.0)] };
        let outline = brush_polygon(Some(&image), &loose, &[stroke], (400.0, 300.0), &|_| None).expect("outline");
        let min_x = outline.iter().map(|p| p.0).fold(f64::MAX, f64::min);
        assert!((148.0..=152.0).contains(&min_x), "left edge should land on the block at ~150, got {min_x:.1}");
    }

    #[test]
    fn resampling_spaces_points_evenly_and_keeps_the_shape() {
        let circle = regular_polygon(0.0, 0.0, 50.0, 400);
        let even = resample_closed(&circle, 3.0);
        assert!((100..=110).contains(&even.len()), "kept {} points", even.len());
        let gaps: Vec<f64> = (0..even.len()).map(|i| dist_sq(even[i], even[(i + 1) % even.len()]).sqrt()).collect();
        let (lo, hi) = (gaps.iter().cloned().fold(f64::MAX, f64::min), gaps.iter().cloned().fold(f64::MIN, f64::max));
        assert!(hi - lo < 0.2, "uneven spacing: {lo:.2}..{hi:.2}");
        assert!((polygon_area(&even) - polygon_area(&circle)).abs() / polygon_area(&circle) < 0.01);
    }

    #[test]
    fn the_outline_is_dense_enough_that_corners_are_not_cut() {
        let image = make_image(360, 240, |x, y| if (140..220).contains(&x) && (80..160).contains(&y) { [40, 170, 70] } else { [205, 205, 205] });
        let loose = vec![(105.0, 45.0), (255.0, 45.0), (255.0, 195.0), (105.0, 195.0)];
        let outline = snap_polygon(&image, &loose).unwrap();

        let longest = (0..outline.len()).map(|i| dist_sq(outline[i], outline[(i + 1) % outline.len()]).sqrt()).fold(0.0, f64::max);
        assert!(longest <= 2.6, "a {longest:.1}px gap would let the midpoint curves cut a corner");
        // Every real corner has an outline point right next to it.
        for corner in [(140.0, 80.0), (220.0, 80.0), (220.0, 160.0), (140.0, 160.0)] {
            let nearest = outline.iter().map(|&p| dist_sq(p, corner).sqrt()).fold(f64::MAX, f64::min);
            assert!(nearest <= 4.0, "corner {corner:?} is {nearest:.1}px from the nearest outline point");
        }
    }

    /// Runs Edit's snap on a real picture and draws what it did:
    /// `SNAP_IMAGE=shot.png SNAP_LOOP=x,y,w,h [SNAP_MODEL=u2netp.onnx]
    /// SNAP_OUT=out.png cargo test --release --lib -- --ignored real_picture --nocapture`.
    /// The loop is an ellipse in that box (as a hand-drawn circle is), or
    /// `SNAP_POINTS=x,y;x,y;...`. Out: the picture dimmed outside the
    /// result, the drawn loop in blue and the snapped outline in red.
    #[test]
    #[ignore = "needs a picture"]
    fn real_picture() {
        let Ok(path) = std::env::var("SNAP_IMAGE") else { return };
        let image = image::open(&path).unwrap().to_rgba8();
        let points: Vec<(f64, f64)> = if let Ok(points) = std::env::var("SNAP_POINTS") {
            points
                .split(';')
                .filter(|p| !p.trim().is_empty())
                .map(|p| {
                    let (x, y) = p.split_once(',').unwrap();
                    (x.trim().parse().unwrap(), y.trim().parse().unwrap())
                })
                .collect()
        } else {
            let v: Vec<f64> = std::env::var("SNAP_LOOP").unwrap().split(',').map(|p| p.trim().parse().unwrap()).collect();
            let (cx, cy, rx, ry) = (v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, v[2] / 2.0, v[3] / 2.0);
            (0..96).map(|i| {
                let a = i as f64 / 96.0 * std::f64::consts::TAU;
                (cx + rx * a.cos(), cy + ry * a.sin())
            }).collect()
        };
        let model = std::env::var("SNAP_MODEL").ok().map(std::path::PathBuf::from);
        let network = |picture: &RgbImage| -> Option<Vec<f32>> {
            let path = model.as_ref()?;
            match crate::objectnet::predict(path, picture) {
                Ok(map) => Some(map),
                Err(err) => {
                    println!("network failed: {err}");
                    None
                }
            }
        };
        let started = std::time::Instant::now();
        println!("frame in loop: {:?}", crate::rectsnap::frame_in_loop(&image, &points));
        let outline = snap_outline(&image, &points, &network);
        println!("snap took {:?}; {} outline points", started.elapsed(), outline.as_ref().map_or(0, Vec::len));
        let Some(outline) = outline else {
            println!("no outline (kept the drawn loop)");
            return;
        };
        let (w, h) = image.dimensions();
        let inside = rasterize(&outline, w as usize, h as usize);
        let mut out = image::RgbImage::new(w, h);
        for (x, y, pixel) in out.enumerate_pixels_mut() {
            let source = image.get_pixel(x, y);
            let keep = inside[y as usize * w as usize + x as usize];
            let f = if keep { 1.0 } else { 0.28 };
            *pixel = image::Rgb([(source[0] as f32 * f) as u8, (source[1] as f32 * f) as u8, (source[2] as f32 * f) as u8]);
        }
        let mut dot = |x: f64, y: f64, c: [u8; 3]| {
            let (x, y) = (x.round() as i64, y.round() as i64);
            if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                out.put_pixel(x as u32, y as u32, image::Rgb(c));
            }
        };
        for line in [(&points, [60, 140, 255]), (&outline, [255, 40, 40])] {
            let path = line.0;
            for i in 0..path.len() {
                let (a, b) = (path[i], path[(i + 1) % path.len()]);
                let steps = (dist_sq(a, b).sqrt().ceil() as usize).max(1);
                for s in 0..=steps {
                    let t = s as f64 / steps as f64;
                    dot(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, line.1);
                }
            }
        }
        out.save(std::env::var("SNAP_OUT").unwrap_or_else(|_| "snap-out.png".into())).unwrap();
    }

    #[test]
    fn a_window_is_taken_whole_and_a_free_object_is_cut_out() {
        // A clinging strip along the top of a frame is a piece of it.
        let frame = crate::rectsnap::Region { x: 100.0, y: 100.0, width: 400.0, height: 300.0 };
        let strip = vec![(100.0, 100.0), (500.0, 100.0), (500.0, 130.0), (100.0, 130.0)];
        assert!(!object_is_free(&strip, frame));
        // Nearly the whole frame: the frame, drawn less cleanly.
        let most = vec![(104.0, 103.0), (497.0, 101.0), (498.0, 396.0), (102.0, 398.0)];
        assert!(!object_is_free(&most, frame));
        // A disc in the middle of a photo is the point of a cut-out.
        let disc = regular_polygon(300.0, 250.0, 80.0, 120);
        assert!(object_is_free(&disc, frame));
    }
}
