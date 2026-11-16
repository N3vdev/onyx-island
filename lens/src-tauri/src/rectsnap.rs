//! "Slash to select": a quick straight swipe across a picture (or a card, a
//! panel, a video) selects the whole thing. The swipe's bounding box is only
//! a rough guess - people start and stop a little inside or outside the real
//! corners - so each side is moved to the nearest strong straight edge in the
//! screenshot, if there is one. A side with no clear edge nearby stays where
//! the swipe put it.

use image::RgbaImage;

use crate::state::AppState;

/// Two neighbouring pixels this different (sum of |dR|+|dG|+|dB|) count as an
/// edge between them.
const EDGE_STEP: i32 = 36;
/// A side snaps only to a line that is an edge along at least this share of
/// the side's length: the border of a picture runs the whole way, detail
/// inside it rarely does.
const MIN_EDGE_SHARE: f64 = 0.55;
/// How far (as a share of the box's larger side) a side may move.
const SEARCH_SHARE: f64 = 0.2;
const MIN_SEARCH_PX: f64 = 12.0;
const MAX_SEARCH_PX: f64 = 220.0;
/// The ends of each side are skipped when scoring: near the corners the
/// other two sides' edges would otherwise count.
const CORNER_SKIP: f64 = 0.1;
const MIN_SIDE_PX: i64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Region {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Free: works on the frame the overlay is already showing.
#[tauri::command]
pub async fn detect_region(state: tauri::State<'_, AppState>, region: Region) -> Result<Option<Region>, String> {
    let frame = state.frame_cache().ok_or_else(|| "No active capture".to_string())?;
    tokio::task::spawn_blocking(move || {
        let image = image::load_from_memory(&frame.png_bytes)
            .map_err(|err| format!("Could not read the capture: {err}"))?
            .to_rgba8();
        Ok(snap_region(&image, region))
    })
    .await
    .map_err(|err| format!("region worker failed: {err}"))?
}

/// `None` when no side found an edge to snap to (the caller keeps the box).
pub fn snap_region(image: &RgbaImage, region: Region) -> Option<Region> {
    let (w, h) = (image.width() as i64, image.height() as i64);
    if w < 4 || h < 4 || ![region.x, region.y, region.width, region.height].iter().all(|v| v.is_finite()) {
        return None;
    }
    // Sides are pixel *boundaries*: left/top are the first pixel inside,
    // right/bottom the first pixel past the box.
    let mut left = (region.x.round() as i64).clamp(0, w - 1);
    let mut top = (region.y.round() as i64).clamp(0, h - 1);
    let mut right = ((region.x + region.width).round() as i64).clamp(left + 1, w);
    let mut bottom = ((region.y + region.height).round() as i64).clamp(top + 1, h);
    let reach = (SEARCH_SHARE * (right - left).max(bottom - top) as f64).clamp(MIN_SEARCH_PX, MAX_SEARCH_PX) as i64;

    let raw = image.as_raw();
    let at = |x: i64, y: i64| -> [i32; 3] {
        let p = ((y * w + x) * 4) as usize;
        [raw[p] as i32, raw[p + 1] as i32, raw[p + 2] as i32]
    };
    let differs = |a: [i32; 3], b: [i32; 3]| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs() >= EDGE_STEP;

    // The best boundary near `around`. A vertical boundary at x lies between
    // columns x-1 and x and is scored over the rows in `span`; horizontal
    // boundaries likewise.
    let best = |vertical: bool, around: i64, span: (i64, i64)| -> Option<i64> {
        let limit = if vertical { w } else { h };
        let (from, to) = ((around - reach).max(1), (around + reach).min(limit - 1));
        let (s0, s1) = span;
        let length = (s1 - s0).max(1) as f64;
        let mut choice: Option<(f64, i64)> = None;
        for line in from..=to {
            let hits = (s0..s1)
                .filter(|&t| if vertical { differs(at(line - 1, t), at(line, t)) } else { differs(at(t, line - 1), at(t, line)) })
                .count() as f64
                / length;
            if hits < MIN_EDGE_SHARE {
                continue;
            }
            // Stronger wins; among equals, the one nearer the swipe.
            let better = match choice {
                None => true,
                Some((score, chosen)) => {
                    hits > score + 1e-9 || ((hits - score).abs() <= 1e-9 && (line - around).abs() < (chosen - around).abs())
                }
            };
            if better {
                choice = Some((hits, line));
            }
        }
        choice.map(|(_, line)| line)
    };
    let inner = |a: i64, b: i64| {
        let skip = ((b - a) as f64 * CORNER_SKIP) as i64;
        (a + skip, (b - skip).max(a + skip + 1))
    };

    let mut snapped_any = false;
    // Twice: each side's span depends on where the other two ended up.
    for _ in 0..2 {
        let rows = inner(top, bottom);
        let columns = inner(left, right);
        for (side, vertical, span) in [(&mut left, true, rows), (&mut right, true, rows), (&mut top, false, columns), (&mut bottom, false, columns)] {
            if let Some(line) = best(vertical, *side, span) {
                *side = line;
                snapped_any = true;
            }
        }
        if right - left < MIN_SIDE_PX || bottom - top < MIN_SIDE_PX {
            return None;
        }
    }
    snapped_any.then_some(Region { x: left as f64, y: top as f64, width: (right - left) as f64, height: (bottom - top) as f64 })
}

/// A frame found inside a hand-drawn loop: a window, card, panel or
/// picture, with the radius of its rounded corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub region: Region,
    pub radius: f64,
}

/// A side of a frame in a loop must be an edge along this share of its
/// length: stricter than a swipe, since it must beat the lines inside.
const LOOP_EDGE_SHARE: f64 = 0.7;
/// The frame must fill at least this share of the loop's bounding box.
const LOOP_MIN_FILL: f64 = 0.3;

/// Edit mode on a window or a card: the loop is drawn *around* it, so each
/// side is the first strong straight edge met moving inward from the loop's
/// bounding box (the thing's border), not the strongest one (which can be a
/// line inside it, a tab bar or a divider). `None` unless at least three
/// sides find an edge and the frame fills a fair share of the loop: then
/// the loop holds no rectangle, and the colour cut-out decides alone.
pub fn frame_in_loop(image: &RgbaImage, polygon: &[(f64, f64)]) -> Option<Frame> {
    let (w, h) = (image.width() as i64, image.height() as i64);
    if polygon.len() < 3 || w < 8 || h < 8 {
        return None;
    }
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in polygon {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let outer = (
        (min_x.floor() as i64).clamp(0, w - 2),
        (min_y.floor() as i64).clamp(0, h - 2),
        (max_x.ceil() as i64).clamp(2, w),
        (max_y.ceil() as i64).clamp(2, h),
    );
    if outer.2 - outer.0 < 3 * MIN_SIDE_PX || outer.3 - outer.1 < 3 * MIN_SIDE_PX {
        return None;
    }
    let raw = image.as_raw();
    let at = |x: i64, y: i64| -> [i32; 3] {
        let p = ((y * w + x) * 4) as usize;
        [raw[p] as i32, raw[p + 1] as i32, raw[p + 2] as i32]
    };
    let differs = |a: [i32; 3], b: [i32; 3]| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs() >= EDGE_STEP;
    let share = |vertical: bool, line: i64, span: (i64, i64)| -> f64 {
        let (s0, s1) = span;
        let hits = (s0..s1)
            .filter(|&t| if vertical { differs(at(line - 1, t), at(line, t)) } else { differs(at(t, line - 1), at(t, line)) })
            .count();
        hits as f64 / (s1 - s0).max(1) as f64
    };
    let inside = |x: f64, y: f64| point_in_polygon(polygon, x, y);

    let limit_of = |vertical: bool| if vertical { w } else { h };
    let edge_at = |vertical: bool, line: i64, t: i64| {
        (1..limit_of(vertical)).contains(&line)
            && (0..limit_of(!vertical)).contains(&t)
            && if vertical { differs(at(line - 1, t), at(line, t)) } else { differs(at(t, line - 1), at(t, line)) }
    };
    // A side of the circled thing ends where the thing ends, inside the
    // loop. A line running on unbroken out past the loop is something
    // longer: a neighbouring window's side, a divider in the app behind.
    // (Panes tiled in a row line up, but a gap parts their edges.)
    let runs_on = |vertical: bool, line: i64, span: (i64, i64)| -> bool {
        let (lo, hi) = if vertical { (outer.1, outer.3) } else { (outer.0, outer.2) };
        let limit = limit_of(!vertical);
        let walk = |from: i64, step: i64| -> i64 {
            let (mut t, mut gap, mut last) = (from, 0, from);
            while (0..limit).contains(&t) && gap <= 3 {
                if edge_at(vertical, line, t) {
                    last = t;
                    gap = 0;
                } else {
                    gap += 1;
                }
                t += step;
            }
            last
        };
        // From the middle of the side outward: the gap at its corners stops
        // a real side, however far the span being scored reaches.
        let middle = (span.0 + span.1) / 2;
        walk(middle, 1) >= (hi + 3).min(limit - 1) || walk(middle, -1) <= (lo - 3).max(0)
    };
    let x_mid = (outer.0 + outer.2) / 2;
    let y_mid = (outer.1 + outer.3) / 2;
    // The first few straight edges met moving inward from the loop's box.
    let candidates = |frame: (i64, i64, i64, i64)| -> [Vec<i64>; 4] {
        let (left, top, right, bottom) = frame;
        let skip = |a: i64, b: i64| {
            let skip = ((b - a) as f64 * CORNER_SKIP) as i64;
            (a + skip, (b - skip).max(a + skip + 1))
        };
        let (rows, columns) = (skip(top, bottom), skip(left, right));
        let (mid_x, mid_y) = ((left + right) as f64 / 2.0, (top + bottom) as f64 / 2.0);
        let scan = |vertical: bool, from: i64, to: i64, span: (i64, i64)| -> Vec<i64> {
            let step = if to >= from { 1 } else { -1 };
            let mut found: Vec<i64> = Vec::new();
            let mut line = from;
            while line != to && found.len() < 4 {
                let on_loop = if vertical { inside(line as f64, mid_y) } else { inside(mid_x, line as f64) };
                if on_loop
                    && found.last().is_none_or(|&last| (line - last).abs() > 2)
                    && share(vertical, line, span) >= LOOP_EDGE_SHARE
                    && !runs_on(vertical, line, span)
                {
                    found.push(line);
                }
                line += step;
            }
            found
        };
        [
            scan(true, outer.0 + 1, x_mid, rows),
            scan(true, outer.2 - 1, x_mid, rows),
            scan(false, outer.1 + 1, y_mid, columns),
            scan(false, outer.3 - 1, y_mid, columns),
        ]
    };
    // A real frame's sides meet at its corners, rounded or square: an arc
    // of edge, the top side carrying on from it one way and the left side
    // the other. A neighbour's edge paired with the frame's top meets
    // nothing there (the gap between them). The best radius, or `None`.
    let corner = |px: i64, py: i64, dx: i64, dy: i64| -> Option<i64> {
        let sample = |x: f64, y: f64, ox: f64, oy: f64| -> bool {
            let (x0, y0) = (x.round() as i64, y.round() as i64);
            let (x1, y1) = ((x + ox * 2.0).round() as i64, (y + oy * 2.0).round() as i64);
            [x0, x1].iter().all(|x| (0..w).contains(x)) && [y0, y1].iter().all(|y| (0..h).contains(y)) && differs(at(x0, y0), at(x1, y1))
        };
        let mut best: Option<(f64, i64)> = None;
        for r in 0..=16i64 {
            let (cx, cy) = ((px + dx * r) as f64, (py + dy * r) as f64);
            let arc = if r == 0 {
                f64::from(u8::from(sample(px as f64, py as f64, -dx as f64, -dy as f64)))
            } else {
                (0..8)
                    .filter(|k| {
                        let a = (*k as f64 + 0.5) / 8.0 * std::f64::consts::FRAC_PI_2;
                        let (ox, oy) = (-dx as f64 * a.cos(), -dy as f64 * a.sin());
                        sample(cx + ox * r as f64, cy + oy * r as f64, ox, oy)
                    })
                    .count() as f64
                    / 8.0
            };
            let along_top = (0..6).filter(|k| edge_at(false, if dy > 0 { py } else { py + 1 }, px + dx * (r + k))).count() as f64 / 6.0;
            let along_side = (0..6).filter(|k| edge_at(true, if dx > 0 { px } else { px + 1 }, py + dy * (r + k))).count() as f64 / 6.0;
            let score = arc.min(along_top).min(along_side);
            if best.is_none_or(|(b, _)| score > b + 1e-9) {
                best = Some((score, r));
            }
        }
        best.filter(|(score, _)| *score >= 0.6).map(|(_, r)| r)
    };
    // The largest rectangle of candidate sides whose corners are there (one
    // may be covered, by the pointer say), with its corners' radius.
    let choose = |sides: &[Vec<i64>; 4]| -> Option<((i64, i64, i64, i64), f64)> {
        let mut best: Option<((i64, i64, i64, i64), f64, i64)> = None;
        for &left in &sides[0] {
            for &right in &sides[1] {
                for &top in &sides[2] {
                    for &bottom in &sides[3] {
                        if right - left < MIN_SIDE_PX || bottom - top < MIN_SIDE_PX {
                            continue;
                        }
                        let area = (right - left) * (bottom - top);
                        if best.is_some_and(|(_, _, a)| a >= area) {
                            continue;
                        }
                        let radii: Vec<i64> = [
                            corner(left, top, 1, 1),
                            corner(right - 1, top, -1, 1),
                            corner(left, bottom - 1, 1, -1),
                            corner(right - 1, bottom - 1, -1, -1),
                        ]
                        .into_iter()
                        .flatten()
                        .collect();
                        if radii.len() >= 3 {
                            let mut radii = radii;
                            radii.sort_unstable();
                            best = Some(((left, top, right, bottom), radii[radii.len() / 2] as f64, area));
                        }
                    }
                }
            }
        }
        best.map(|(frame, radius, _)| (frame, radius))
    };
    // Twice: each side is scored along the span between the other two, so
    // a frame much smaller than the loop's box is found on the second pass.
    let first = choose(&candidates(outer));
    let second = first.and_then(|(frame, _)| choose(&candidates(frame)));
    let ((left, top, right, bottom), radius) = second.or(first)?;
    let fill = ((right - left) * (bottom - top)) as f64 / ((outer.2 - outer.0) * (outer.3 - outer.1)) as f64;
    if fill < LOOP_MIN_FILL {
        return None;
    }
    let region = Region { x: left as f64, y: top as f64, width: (right - left) as f64, height: (bottom - top) as f64 };
    Some(Frame { region, radius })
}

/// The frame as a closed outline, its corners rounded.
pub fn frame_outline(frame: Frame) -> Vec<(f64, f64)> {
    let Region { x, y, width, height } = frame.region;
    let r = frame.radius.clamp(0.0, width.min(height) / 2.0);
    if r < 1.0 {
        return vec![(x, y), (x + width, y), (x + width, y + height), (x, y + height)];
    }
    let steps = ((r * std::f64::consts::FRAC_PI_2 / 2.0).ceil() as usize).clamp(3, 24);
    let mut points = Vec::with_capacity(steps * 4 + 4);
    for (cx, cy, start) in [
        (x + width - r, y + r, -std::f64::consts::FRAC_PI_2),
        (x + width - r, y + height - r, 0.0),
        (x + r, y + height - r, std::f64::consts::FRAC_PI_2),
        (x + r, y + r, std::f64::consts::PI),
    ] {
        for i in 0..=steps {
            let a = start + std::f64::consts::FRAC_PI_2 * i as f64 / steps as f64;
            points.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    points
}

pub fn point_in_polygon(polygon: &[(f64, f64)], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let ((xi, yi), (xj, yj)) = (polygon[i], polygon[j]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white page with a "photo" (busy texture) at x 100..300, y 80..230.
    fn page_with_photo() -> RgbaImage {
        RgbaImage::from_fn(420, 320, |x, y| {
            if (100..300).contains(&x) && (80..230).contains(&y) {
                let v = ((x * 7 + y * 13) % 97) as u8;
                image::Rgba([40 + v, 90 + v / 2, 140 - v / 3, 255])
            } else {
                image::Rgba([248, 248, 246, 255])
            }
        })
    }

    const PHOTO: Region = Region { x: 100.0, y: 80.0, width: 200.0, height: 150.0 };

    #[test]
    fn a_swipe_that_stops_short_grows_to_the_picture() {
        let swipe = Region { x: 115.0, y: 96.0, width: 170.0, height: 118.0 };
        assert_eq!(snap_region(&page_with_photo(), swipe), Some(PHOTO));
    }

    #[test]
    fn a_swipe_that_overshoots_shrinks_to_the_picture() {
        let swipe = Region { x: 88.0, y: 66.0, width: 228.0, height: 178.0 };
        assert_eq!(snap_region(&page_with_photo(), swipe), Some(PHOTO));
    }

    #[test]
    fn a_flat_area_has_nothing_to_snap_to() {
        let flat = RgbaImage::from_pixel(300, 200, image::Rgba([250, 250, 250, 255]));
        assert_eq!(snap_region(&flat, Region { x: 50.0, y: 40.0, width: 150.0, height: 100.0 }), None);
    }

    #[test]
    fn a_picture_against_the_screen_edge_keeps_the_sides_it_can_find() {
        // The photo runs off the left of the screen: there is no left border
        // to find, so that side stays at the swipe; the other three snap.
        let image = RgbaImage::from_fn(320, 240, |x, y| {
            if x < 200 && (60..180).contains(&y) {
                let v = ((x * 5 + y * 11) % 89) as u8;
                image::Rgba([60 + v, 40 + v, 120, 255])
            } else {
                image::Rgba([245, 245, 245, 255])
            }
        });
        let swipe = Region { x: 6.0, y: 70.0, width: 180.0, height: 100.0 };
        assert_eq!(snap_region(&image, swipe), Some(Region { x: 6.0, y: 60.0, width: 194.0, height: 120.0 }));
    }

    /// Snaps a swipe on a real screenshot and prints the result:
    /// `RECT_IMAGE=shot.png RECT_REGION=x,y,w,h cargo test --lib -- --ignored real_screenshot --nocapture`
    #[test]
    #[ignore = "needs a screenshot"]
    fn real_screenshot() {
        let (Ok(path), Ok(region)) = (std::env::var("RECT_IMAGE"), std::env::var("RECT_REGION")) else { return };
        let v: Vec<f64> = region.split(',').map(|p| p.trim().parse().unwrap()).collect();
        let image = image::open(path).unwrap().to_rgba8();
        let swipe = Region { x: v[0], y: v[1], width: v[2], height: v[3] };
        println!("swipe {swipe:?} -> {:?}", snap_region(&image, swipe));
    }

    /// A dark app window on a dark desktop: a 1px lighter border with
    /// rounded corners, a title bar, a line under the tabs and text inside.
    fn dark_window() -> RgbaImage {
        let (x0, y0, x1, y1, r) = (60i64, 40i64, 380i64, 300i64, 10i64);
        RgbaImage::from_fn(440, 340, |x, y| {
            let (x, y) = (x as i64, y as i64);
            let corner = |cx: i64, cy: i64| (x - cx).pow(2) + (y - cy).pow(2);
            let outside_round = (x < x0 + r && y < y0 + r && corner(x0 + r, y0 + r) > r * r)
                || (x >= x1 - r && y < y0 + r && corner(x1 - r - 1, y0 + r) > r * r)
                || (x < x0 + r && y >= y1 - r && corner(x0 + r, y1 - r - 1) > r * r)
                || (x >= x1 - r && y >= y1 - r && corner(x1 - r - 1, y1 - r - 1) > r * r);
            let inside = (x0..x1).contains(&x) && (y0..y1).contains(&y) && !outside_round;
            // The 1 px border follows the rounded corners too, as Windows 11
            // draws it.
            let on_arc = |cx: i64, cy: i64| {
                let d = ((x - cx).pow(2) + (y - cy).pow(2)) as f64;
                d.sqrt() > (r - 1) as f64
            };
            let arc_border = inside
                && ((x < x0 + r && y < y0 + r && on_arc(x0 + r, y0 + r))
                    || (x >= x1 - r && y < y0 + r && on_arc(x1 - r - 1, y0 + r))
                    || (x < x0 + r && y >= y1 - r && on_arc(x0 + r, y1 - r - 1))
                    || (x >= x1 - r && y >= y1 - r && on_arc(x1 - r - 1, y1 - r - 1)));
            // Panes tiled either side, lined up with it, a 10 px gap apart,
            // and a divider in the app behind running across above it.
            let pane = (y0..y1).contains(&y) && (x < x0 - 10 || x >= x1 + 10);
            let pane_border = pane && (x == x0 - 11 || x == x1 + 10 || y == y0 || y == y1 - 1);
            let v: u8 = if !inside {
                if y == 20 {
                    90
                } else if pane_border {
                    70
                } else if pane {
                    26
                } else {
                    18
                }
            } else if x == x0 || x == x1 - 1 || y == y0 || y == y1 - 1 || arc_border {
                70
            } else if y < y0 + 30 {
                30
            } else if y == y0 + 60 {
                64
            } else if (y - y0) % 24 < 3 && x < x0 + 40 + (y % 7) * 30 {
                200
            } else {
                10
            };
            image::Rgba([v, v, v + 2, 255])
        })
    }

    fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Vec<(f64, f64)> {
        (0..64).map(|i| {
            let a = i as f64 / 64.0 * std::f64::consts::TAU;
            (cx + rx * a.cos(), cy + ry * a.sin())
        }).collect()
    }

    #[test]
    fn a_loop_round_a_window_finds_its_border_not_the_lines_inside() {
        let image = dark_window();
        // Drawn loosely round it, as a hand does.
        let loop_ = ellipse(220.0, 170.0, 215.0, 165.0);
        let frame = frame_in_loop(&image, &loop_).expect("a frame");
        assert_eq!(frame.region, Region { x: 60.0, y: 40.0, width: 320.0, height: 260.0 });
        assert!((6.0..=14.0).contains(&frame.radius), "corner radius {:.1}", frame.radius);
        assert!(frame_outline(frame).len() > 12);
    }

    #[test]
    fn a_loop_with_no_frame_in_it_finds_none() {
        // A round blob on a flat background has no straight sides.
        let image = RgbaImage::from_fn(300, 240, |x, y| {
            let d = (x as f64 - 150.0).powi(2) + (y as f64 - 120.0).powi(2);
            if d < 70.0 * 70.0 { image::Rgba([200, 60, 60, 255]) } else { image::Rgba([240, 240, 240, 255]) }
        });
        assert_eq!(frame_in_loop(&image, &ellipse(150.0, 120.0, 120.0, 100.0)), None);
    }
}
