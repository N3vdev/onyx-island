//! Words -> lines -> blocks. Merge adjacent lines into blocks using
//! vertical gap, horizontal overlap, and left-edge delta thresholds.
//!
//! Lines are sorted top-to-bottom before merging. `OcrLine::Text` order from
//! Windows OCR is not reliably sequential — a long line that visually wraps
//! onto a second screen row is returned as a separate line object, and it
//! is not always positioned correctly relative to the rest in OCR's own
//! return order, which displaced wrapped continuations to the wrong spot in
//! copied text. Sorting by Y here is safe for multi-column layouts too: two
//! lines only ever merge into the same block when they already overlap
//! horizontally (see `should_merge`), so a block's lines are by
//! construction a single column — genuinely separate columns never merge
//! regardless of processing order.

use crate::ocr::OcrLine;

const MAX_GAP_MULTIPLIER: f64 = 1.6;
const MIN_HORIZONTAL_OVERLAP: f64 = 0.4;
const MAX_LEFT_EDGE_DELTA_PX: f64 = 15.0;

#[derive(Debug, Clone)]
pub struct Block {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub lines: Vec<OcrLine>,
}

impl Block {
    fn from_line(line: OcrLine) -> Self {
        Self { x: line.x, y: line.y, width: line.width, height: line.height, lines: vec![line] }
    }

    fn extend(&mut self, line: OcrLine) {
        let min_x = self.x.min(line.x);
        let min_y = self.y.min(line.y);
        let max_x = (self.x + self.width).max(line.x + line.width);
        let max_y = (self.y + self.height).max(line.y + line.height);
        self.x = min_x;
        self.y = min_y;
        self.width = max_x - min_x;
        self.height = max_y - min_y;
        self.lines.push(line);
    }

    /// Median line height within this block only — computed per-region, not
    /// globally, so a headline's larger scale doesn't warp the threshold
    /// used to grow an unrelated body-text block.
    fn median_line_height(&self) -> f64 {
        median(self.lines.iter().map(|l| l.height))
    }
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = values.collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = v.len() / 2;
    if v.len().is_multiple_of(2) { (v[mid - 1] + v[mid]) / 2.0 } else { v[mid] }
}

fn horizontal_overlap_fraction(a: &OcrLine, b: &OcrLine) -> f64 {
    let left = a.x.max(b.x);
    let right = (a.x + a.width).min(b.x + b.width);
    let overlap = (right - left).max(0.0);
    let narrower = a.width.min(b.width);
    if narrower <= 0.0 { 0.0 } else { overlap / narrower }
}

fn should_merge(block: &Block, line: &OcrLine, dpi_scale: f64) -> bool {
    let Some(last_line) = block.lines.last() else { return false };
    let median_height = block.median_line_height();
    if median_height <= 0.0 {
        return false;
    }

    let vertical_gap = line.y - (last_line.y + last_line.height);
    if vertical_gap >= median_height * MAX_GAP_MULTIPLIER {
        return false;
    }

    if horizontal_overlap_fraction(last_line, line) <= MIN_HORIZONTAL_OVERLAP {
        return false;
    }

    let left_edge_delta = (line.x - last_line.x).abs();
    if left_edge_delta >= MAX_LEFT_EDGE_DELTA_PX * dpi_scale {
        return false;
    }

    true
}

/// Merges OCR lines into blocks. `dpi_scale` is the monitor's scale factor
/// (1.0 at 100%, 1.5 at 150%) so the left-edge threshold scales with DPI.
pub fn build_blocks(mut lines: Vec<OcrLine>, dpi_scale: f64) -> Vec<Block> {
    lines.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap().then_with(|| a.x.partial_cmp(&b.x).unwrap()));

    let mut blocks: Vec<Block> = Vec::new();
    for line in lines {
        // Search back through every open block rather than only the most
        // recent one. Sorted by y, a two-column layout interleaves as
        // A1,B1,A2,B2..., so each line's immediate predecessor belongs to
        // the *other* column and can never merge — checking only the last
        // block would strand every column line in a block of its own.
        // Searching from the end keeps the nearest candidate preferred, and
        // `should_merge`'s vertical-gap rule bounds how far back a match can
        // plausibly reach, so this stays local rather than pairing lines
        // across unrelated regions of the screen.
        match blocks.iter().rposition(|block| should_merge(block, &line, dpi_scale)) {
            Some(index) => blocks[index].extend(line),
            None => blocks.push(Block::from_line(line)),
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(x: f64, y: f64, width: f64, height: f64, text: &str) -> OcrLine {
        OcrLine { text: text.into(), x, y, width, height, words: Vec::new() }
    }

    #[test]
    fn sorts_lines_by_position_before_merging_even_if_ocr_returns_them_out_of_order() {
        // A long line wraps onto a second screen row; OCR returns that
        // wrapped continuation before the paragraph's actual first line —
        // observed in practice, not hypothetical. Without sorting first,
        // the continuation's text lands before "line one" in the merged
        // block instead of after it.
        let lines = vec![
            line(100.0, 150.0, 200.0, 20.0, "not later."), // wrapped continuation, fed out of order
            line(100.0, 100.0, 400.0, 20.0, "line one"),
            line(100.0, 125.0, 400.0, 20.0, "continues onto"),
            line(100.0, 300.0, 400.0, 20.0, "unrelated paragraph"),
        ];

        let blocks = build_blocks(lines, 1.0);

        assert_eq!(blocks.len(), 2);
        let texts: Vec<&str> = blocks[0].lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["line one", "continues onto", "not later."]);
    }

    #[test]
    fn merges_each_column_of_a_two_column_layout_into_its_own_block() {
        // Sorted by y these interleave (A1, B1, A2, B2), so each line's
        // immediate predecessor is the other column's. A merge rule that
        // only considers the most recent block leaves all four stranded as
        // single-line blocks, and circling a paragraph here would select
        // one line instead of the paragraph.
        let lines = vec![
            line(100.0, 100.0, 200.0, 20.0, "A1"),
            line(400.0, 110.0, 200.0, 20.0, "B1"),
            line(100.0, 125.0, 200.0, 20.0, "A2"),
            line(400.0, 135.0, 200.0, 20.0, "B2"),
        ];

        let blocks = build_blocks(lines, 1.0);

        let texts: Vec<Vec<&str>> =
            blocks.iter().map(|b| b.lines.iter().map(|l| l.text.as_str()).collect()).collect();
        assert_eq!(blocks.len(), 2, "each column should merge into one block, got {texts:?}");
        assert!(texts.contains(&vec!["A1", "A2"]), "left column should merge, got {texts:?}");
        assert!(texts.contains(&vec!["B1", "B2"]), "right column should merge, got {texts:?}");
    }

    #[test]
    fn keeps_columns_apart_when_they_are_horizontally_disjoint() {
        // The backwards search must not reach across a gutter: these two
        // columns share a y band but no horizontal overlap.
        let lines = vec![
            line(100.0, 100.0, 200.0, 20.0, "left"),
            line(400.0, 100.0, 200.0, 20.0, "right"),
        ];

        let blocks = build_blocks(lines, 1.0);

        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn merges_tightly_stacked_paragraph_lines() {
        let lines = vec![
            line(100.0, 100.0, 400.0, 20.0, "line one"),
            line(100.0, 128.0, 380.0, 20.0, "line two"),
            line(100.0, 156.0, 410.0, 20.0, "line three"),
        ];

        let blocks = build_blocks(lines, 1.0);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].lines.len(), 3);
    }

    #[test]
    fn splits_on_large_vertical_gap() {
        let lines = vec![
            line(100.0, 100.0, 400.0, 20.0, "paragraph one"),
            line(100.0, 128.0, 400.0, 20.0, "paragraph one cont"),
            // gap far exceeds 1.6x the ~20px median line height above
            line(100.0, 400.0, 400.0, 20.0, "paragraph two"),
        ];

        let blocks = build_blocks(lines, 1.0);

        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].lines.len(), 2);
        assert_eq!(blocks[1].lines.len(), 1);
    }

    #[test]
    fn splits_on_left_edge_delta_even_when_gap_is_tight() {
        let lines = vec![
            line(100.0, 100.0, 400.0, 20.0, "left column"),
            line(100.0, 128.0, 400.0, 20.0, "left column cont"),
            // same y-band as an indented/bulleted item, but starts far right
            line(160.0, 128.0, 300.0, 20.0, "indented item"),
        ];

        let blocks = build_blocks(lines, 1.0);

        // the third line's left edge is 60px off the running block's edge,
        // well past the 15px (scaled) threshold
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn splits_on_low_horizontal_overlap() {
        let lines = vec![
            line(100.0, 100.0, 100.0, 20.0, "left column"),
            // barely overlaps horizontally with the line above
            line(500.0, 128.0, 100.0, 20.0, "right column"),
        ];

        let blocks = build_blocks(lines, 1.0);

        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn dpi_scale_widens_left_edge_tolerance() {
        let lines = vec![
            line(100.0, 100.0, 400.0, 20.0, "line one"),
            // 20px delta: fails at 1.0x (15px threshold) but should pass at 1.5x (22.5px)
            line(120.0, 128.0, 400.0, 20.0, "line two"),
        ];

        assert_eq!(build_blocks(lines.clone(), 1.0).len(), 2);
        assert_eq!(build_blocks(lines, 1.5).len(), 1);
    }

    #[test]
    fn body_block_growth_uses_its_own_median_not_an_earlier_headlines() {
        // Median height is computed per-block (`Block::median_line_height`
        // only ever sees its own lines), so a much larger headline earlier
        // on the page can't loosen the gap tolerance used while growing an
        // unrelated, smaller-font body block later.
        let lines = vec![
            line(100.0, 100.0, 400.0, 60.0, "HEADLINE"),           // bottom 160
            line(100.0, 300.0, 400.0, 20.0, "body line one"),      // gap 140: new block regardless
            line(100.0, 325.0, 400.0, 20.0, "body line two"),      // gap 5: merges, bottom 345
            // gap of 35px: exceeds the body block's own 1.6x20=32px
            // threshold, but is nowhere near the headline's 1.6x60=96px —
            // using the headline's median here would wrongly merge this in
            line(100.0, 380.0, 400.0, 20.0, "body line three, far enough to split"),
        ];

        let blocks = build_blocks(lines, 1.0);

        assert_eq!(blocks.len(), 3, "headline, body para one, and the split-off line stay separate");
        assert_eq!(blocks[1].lines.len(), 2, "the first two body lines still merge with each other");
    }
}
