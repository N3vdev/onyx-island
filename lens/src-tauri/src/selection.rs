//! Lasso hit-test against layout blocks, expand to whole block/line, sort
//! into reading order.

use geo::{Area, BooleanOps, Contains, Coord, Intersects, LineString, Point, Polygon, Rect};

use crate::layout::Block;
use crate::ocr::{OcrLine, OcrWord};

const WHOLE_BLOCK_AREA_FRACTION: f64 = 0.25;
const WHOLE_BLOCK_MIN_LINES_TOUCHED: usize = 2;
const MIN_WORD_OVERLAP: f64 = 0.45;

#[derive(Debug, Clone)]
pub enum Selection {
    Block(Block),
    Line(OcrLine),
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect<f64> {
    Rect::new(Coord { x, y }, Coord { x: x + width, y: y + height })
}

fn overlap_fraction(lasso: &Polygon<f64>, area_rect: Rect<f64>) -> f64 {
    let rect_area = area_rect.width() * area_rect.height();
    if rect_area <= 0.0 {
        return 0.0;
    }
    lasso.intersection(&area_rect.to_polygon()).unsigned_area() / rect_area
}

/// Hit-tests a closed lasso path against layout blocks using the fixed
/// thresholds below — no scoring engine. Do not add weighted candidate
/// ranking here until the test harness shows real failures against real
/// thresholds; see plan.md Phase 3.
///
/// | overlap with a block           | result              |
/// |---------------------------------|---------------------|
/// | >= 25% of block area, or >= 2 lines touched | whole block |
/// | touches exactly 1 line, < 25% area          | that line   |
/// | otherwise                                   | ignored     |
pub fn hit_test(lasso_points: &[(f64, f64)], blocks: &[Block]) -> Vec<Selection> {
    if lasso_points.len() < 3 {
        return Vec::new();
    }

    let lasso = Polygon::new(LineString::from(lasso_points.to_vec()), vec![]);

    let mut selections = Vec::new();
    for block in blocks {
        let block_rect = rect(block.x, block.y, block.width, block.height);
        let block_overlap = overlap_fraction(&lasso, block_rect);

        let touched_lines: Vec<&OcrLine> = block
            .lines
            .iter()
            .filter(|line| lasso.intersects(&rect(line.x, line.y, line.width, line.height)))
            .collect();

        if block_overlap >= WHOLE_BLOCK_AREA_FRACTION || touched_lines.len() >= WHOLE_BLOCK_MIN_LINES_TOUCHED {
            selections.push(Selection::Block(block.clone()));
        } else if touched_lines.len() == 1 {
            selections.push(Selection::Line(touched_lines[0].clone()));
        }
        // else: ignored (covers <10% and the unspecified 10-25%-with-zero-
        // lines-touched band — safer to under-select than guess).
    }

    sort_reading_order(selections)
}

fn line_from_words(words: Vec<OcrWord>) -> OcrLine {
    let x = words.iter().map(|word| word.x).fold(f64::INFINITY, f64::min);
    let y = words.iter().map(|word| word.y).fold(f64::INFINITY, f64::min);
    let right = words.iter().map(|word| word.x + word.width).fold(f64::NEG_INFINITY, f64::max);
    let bottom = words.iter().map(|word| word.y + word.height).fold(f64::NEG_INFINITY, f64::max);
    let text = words.iter().map(|word| word.text.as_str()).collect::<Vec<_>>().join(" ");
    OcrLine { text, x, y, width: right - x, height: bottom - y, words }
}

/// Text mode is deliberately stricter than the old paragraph-snapping hit
/// test: include an OCR word whose center is inside or whose box substantially
/// overlaps the drawn region. OCR boxes often graze a tight lasso even when
/// the photographed glyphs are visible inside it. The web layer clips the
/// resulting highlight to the exact drawn boundary. Split disjoint runs so
/// a line's hit box never bridges unrelated words outside a concave lasso.
pub fn hit_test_contained_words(points: &[(f64, f64)], blocks: &[Block]) -> Vec<Selection> {
    if points.len() < 3 { return Vec::new(); }
    let boundary = Polygon::new(LineString::from(points.to_vec()), vec![]);
    let mut selections = Vec::new();
    for line in blocks.iter().flat_map(|block| &block.lines) {
        if line.words.is_empty() {
            let center = Point::new(line.x + line.width / 2.0, line.y + line.height / 2.0);
            if boundary.contains(&center) || overlap_fraction(&boundary, rect(line.x, line.y, line.width, line.height)) >= MIN_WORD_OVERLAP {
                selections.push(Selection::Line(line.clone()));
            }
            continue;
        }
        let mut run = Vec::new();
        for word in &line.words {
            let center = Point::new(word.x + word.width / 2.0, word.y + word.height / 2.0);
            let inside = boundary.contains(&center)
                || overlap_fraction(&boundary, rect(word.x, word.y, word.width, word.height)) >= MIN_WORD_OVERLAP;
            if inside {
                run.push(word.clone());
            } else if !run.is_empty() {
                selections.push(Selection::Line(line_from_words(std::mem::take(&mut run))));
            }
        }
        if !run.is_empty() {
            selections.push(Selection::Line(line_from_words(run)));
        }
    }
    sort_reading_order(selections)
}

fn bounds(selection: &Selection) -> (f64, f64, f64, f64) {
    match selection {
        Selection::Block(b) => (b.x, b.y, b.width, b.height),
        Selection::Line(l) => (l.x, l.y, l.width, l.height),
    }
}

/// Sorts top-to-bottom, then left-to-right within a horizontal band, so a
/// lasso spanning multiple columns reads naturally instead of scrambling.
fn sort_reading_order(mut selections: Vec<Selection>) -> Vec<Selection> {
    selections.sort_by(|a, b| bounds(a).1.partial_cmp(&bounds(b).1).unwrap());

    let mut bands: Vec<Vec<Selection>> = Vec::new();
    for selection in selections {
        let (_, y, _, height) = bounds(&selection);
        let band = bands.iter_mut().find(|band| {
            let (_, band_y, _, band_height) = bounds(&band[0]);
            y < band_y + band_height && band_y < y + height
        });
        match band {
            Some(band) => band.push(selection),
            None => bands.push(vec![selection]),
        }
    }

    for band in &mut bands {
        band.sort_by(|a, b| bounds(a).0.partial_cmp(&bounds(b).0).unwrap());
    }

    bands.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::OcrWord;

    fn line(x: f64, y: f64, width: f64, height: f64, text: &str) -> OcrLine {
        OcrLine { text: text.into(), x, y, width, height, words: vec![OcrWord { text: text.into(), x, y, width, height }] }
    }

    fn block(x: f64, y: f64, width: f64, height: f64, lines: Vec<OcrLine>) -> Block {
        Block { x, y, width, height, lines }
    }

    fn lasso_rect(x: f64, y: f64, width: f64, height: f64) -> Vec<(f64, f64)> {
        vec![(x, y), (x + width, y), (x + width, y + height), (x, y + height)]
    }

    #[test]
    fn selecting_most_of_a_block_selects_the_whole_block() {
        let l1 = line(100.0, 100.0, 400.0, 20.0, "line one");
        let l2 = line(100.0, 128.0, 400.0, 20.0, "line two");
        let l3 = line(100.0, 156.0, 400.0, 20.0, "line three");
        let b = block(100.0, 100.0, 400.0, 76.0, vec![l1, l2, l3]);

        // lasso covers about 40% of the block's area (top two lines' worth)
        let lasso = lasso_rect(100.0, 100.0, 400.0, 48.0);
        let selections = hit_test(&lasso, &[b]);

        assert_eq!(selections.len(), 1);
        assert!(matches!(selections[0], Selection::Block(_)));
    }

    #[test]
    fn touching_two_lines_selects_the_whole_block_even_under_25_percent_area() {
        let l1 = line(100.0, 100.0, 400.0, 20.0, "line one");
        let l2 = line(100.0, 128.0, 400.0, 20.0, "line two");
        let l3 = line(100.0, 156.0, 400.0, 20.0, "line three");
        let b = block(100.0, 100.0, 400.0, 76.0, vec![l1, l2, l3]);

        // a thin sliver clipping the bottom of line one and top of line two:
        // small area, but it touches 2 lines.
        let lasso = lasso_rect(100.0, 115.0, 400.0, 18.0);
        let selections = hit_test(&lasso, &[b]);

        assert_eq!(selections.len(), 1);
        assert!(matches!(selections[0], Selection::Block(_)));
    }

    #[test]
    fn touching_one_line_with_small_area_selects_just_that_line() {
        let l1 = line(100.0, 100.0, 400.0, 20.0, "line one");
        let l2 = line(100.0, 128.0, 400.0, 20.0, "line two");
        let b = block(100.0, 100.0, 400.0, 48.0, vec![l1.clone(), l2]);

        // small lasso fully inside line one only
        let lasso = lasso_rect(150.0, 102.0, 100.0, 10.0);
        let selections = hit_test(&lasso, &[b]);

        assert_eq!(selections.len(), 1);
        match &selections[0] {
            Selection::Line(l) => assert_eq!(l.text, l1.text),
            other => panic!("expected a Line selection, got {other:?}"),
        }
    }

    #[test]
    fn barely_grazing_a_block_is_ignored() {
        let l1 = line(100.0, 100.0, 400.0, 20.0, "line one");
        let b = block(100.0, 100.0, 400.0, 20.0, vec![l1]);

        // lasso far off to the side, not touching the block at all
        let lasso = lasso_rect(1000.0, 1000.0, 50.0, 50.0);
        let selections = hit_test(&lasso, &[b]);

        assert!(selections.is_empty());
    }

    #[test]
    fn reading_order_sorts_top_to_bottom_then_left_to_right() {
        let top_right = line(500.0, 100.0, 200.0, 20.0, "top right");
        let top_left = line(100.0, 100.0, 200.0, 20.0, "top left");
        let bottom = line(100.0, 400.0, 200.0, 20.0, "bottom");

        let selections = vec![
            Selection::Line(bottom.clone()),
            Selection::Line(top_right.clone()),
            Selection::Line(top_left.clone()),
        ];

        let sorted = sort_reading_order(selections);
        let texts: Vec<&str> = sorted
            .iter()
            .map(|s| match s {
                Selection::Line(l) => l.text.as_str(),
                Selection::Block(_) => unreachable!(),
            })
            .collect();

        assert_eq!(texts, vec!["top left", "top right", "bottom"]);
    }

    #[test]
    fn reading_order_does_not_scramble_two_columns() {
        // interleaved vertically: column A's second line sits at roughly
        // the same height as column B's first line.
        let a1 = line(100.0, 100.0, 200.0, 20.0, "A1");
        let b1 = line(400.0, 110.0, 200.0, 20.0, "B1");
        let a2 = line(100.0, 200.0, 200.0, 20.0, "A2");
        let b2 = line(400.0, 210.0, 200.0, 20.0, "B2");

        let selections = vec![
            Selection::Line(b2.clone()),
            Selection::Line(a1.clone()),
            Selection::Line(b1.clone()),
            Selection::Line(a2.clone()),
        ];

        let sorted = sort_reading_order(selections);
        let texts: Vec<&str> = sorted
            .iter()
            .map(|s| match s {
                Selection::Line(l) => l.text.as_str(),
                Selection::Block(_) => unreachable!(),
            })
            .collect();

        // band 1 (~y100-130): A1 then B1; band 2 (~y200-230): A2 then B2
        assert_eq!(texts, vec!["A1", "B1", "A2", "B2"]);
    }

    #[test]
    fn text_mode_keeps_only_words_inside_the_drawn_rectangle() {
        let words = vec![
            OcrWord { text: "inside".into(), x: 110.0, y: 110.0, width: 50.0, height: 18.0 },
            OcrWord { text: "edge".into(), x: 175.0, y: 110.0, width: 40.0, height: 18.0 },
            OcrWord { text: "outside".into(), x: 230.0, y: 110.0, width: 65.0, height: 18.0 },
        ];
        let line = OcrLine { text: "inside edge outside".into(), x: 110.0, y: 110.0, width: 185.0, height: 18.0, words };
        let found = hit_test_contained_words(&lasso_rect(100.0, 100.0, 100.0, 40.0), &[block(110.0, 110.0, 185.0, 18.0, vec![line])]);
        assert_eq!(found.len(), 1);
        match &found[0] {
            Selection::Line(line) => assert_eq!(line.text, "inside edge"),
            _ => panic!("text mode should return a contained line"),
        }
    }

    #[test]
    fn text_mode_does_not_expand_to_adjacent_paragraph_lines() {
        let first = line(110.0, 110.0, 90.0, 18.0, "inside");
        let second = line(110.0, 140.0, 90.0, 18.0, "outside");
        let found = hit_test_contained_words(&lasso_rect(100.0, 100.0, 110.0, 35.0), &[block(110.0, 110.0, 90.0, 48.0, vec![first, second])]);
        assert_eq!(found.len(), 1);
        assert!(matches!(&found[0], Selection::Line(line) if line.text == "inside"));
    }

    #[test]
    fn text_mode_respects_a_concave_lasso_not_just_its_bounding_box() {
        let path = vec![
            (100.0, 100.0), (300.0, 100.0), (300.0, 130.0), (130.0, 130.0),
            (130.0, 270.0), (300.0, 270.0), (300.0, 300.0), (100.0, 300.0),
        ];
        let words = vec![
            OcrWord { text: "inside".into(), x: 105.0, y: 170.0, width: 20.0, height: 15.0 },
            OcrWord { text: "outside".into(), x: 170.0, y: 170.0, width: 50.0, height: 15.0 },
        ];
        let line = OcrLine { text: "inside outside".into(), x: 105.0, y: 170.0, width: 115.0, height: 15.0, words };
        let found = hit_test_contained_words(&path, &[block(105.0, 170.0, 115.0, 15.0, vec![line])]);
        assert_eq!(found.len(), 1);
        assert!(matches!(&found[0], Selection::Line(line) if line.text == "inside"));
    }

    #[test]
    fn text_mode_ignores_a_word_that_only_grazes_the_border() {
        let word = OcrWord { text: "outside".into(), x: 190.0, y: 110.0, width: 50.0, height: 18.0 };
        let line = OcrLine { text: word.text.clone(), x: word.x, y: word.y, width: word.width, height: word.height, words: vec![word] };
        let found = hit_test_contained_words(&lasso_rect(100.0, 100.0, 100.0, 40.0), &[block(190.0, 110.0, 50.0, 18.0, vec![line])]);
        assert!(found.is_empty());
    }
}
