//! Loads test-images/expectations.json (if present) and asserts each
//! recorded lasso selects the expected text end-to-end: overview + detail OCR -> block
//! merging -> hit-test. A missing expectations file is not a failure — the
//! user supplies the test images; see plan.md Phase 3 Step 4.

use std::path::PathBuf;

use circle_search_lib::{layout, ocr, selection};

#[derive(serde::Deserialize)]
struct Expectation {
    image: String,
    lasso: Vec<(f64, f64)>,
    #[serde(default)]
    expect_contains: Option<String>,
    #[serde(default)]
    expect_not_contains: Option<String>,
}

fn test_images_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("test-images")
}

fn selected_text(selections: &[selection::Selection]) -> String {
    selections
        .iter()
        .map(|sel| match sel {
            selection::Selection::Block(b) => {
                b.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
            }
            selection::Selection::Line(l) => l.text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn selection_matches_recorded_expectations() {
    let dir = test_images_dir();
    let expectations_path = dir.join("expectations.json");

    let Ok(raw) = std::fs::read_to_string(&expectations_path) else {
        println!(
            "no {} yet, skipping (see plan.md Phase 3 Step 4 — test images are supplied separately)",
            expectations_path.display()
        );
        return;
    };

    let expectations: Vec<Expectation> =
        serde_json::from_str(&raw).expect("expectations.json must be valid JSON");

    assert!(!expectations.is_empty(), "expectations.json exists but has no cases");

    let mut failures = Vec::new();

    for expectation in &expectations {
        let image_path = dir.join(&expectation.image);
        let png_bytes = match std::fs::read(&image_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                failures.push(format!("{}: failed to read image ({err})", expectation.image));
                continue;
            }
        };

        let overview_lines = match ocr::recognize(&png_bytes) {
            Ok(lines) => lines,
            Err(err) => {
                failures.push(format!("{}: overview OCR failed ({err})", expectation.image));
                continue;
            }
        };

        let Some(&(first_x, first_y)) = expectation.lasso.first() else {
            failures.push(format!("{}: lasso is empty", expectation.image));
            continue;
        };
        let (mut left, mut top, mut right, mut bottom) = (first_x, first_y, first_x, first_y);
        for &(x, y) in &expectation.lasso[1..] {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x);
            bottom = bottom.max(y);
        }
        let detail_lines = match image::load_from_memory(&png_bytes)
            .map_err(anyhow::Error::from)
            .and_then(|img| ocr::recognize_detail_from_image(&img, left, top, right - left, bottom - top))
        {
            Ok(lines) => lines,
            Err(err) => {
                failures.push(format!("{}: detail OCR failed ({err})", expectation.image));
                continue;
            }
        };
        let lines = ocr::merge_detail_lines(overview_lines, detail_lines);

        // Test images are static files, not live per-monitor captures, so
        // there is no real DPI scale to recover; assume 100%.
        let blocks = layout::build_blocks(lines, 1.0);
        let selections = selection::hit_test(&expectation.lasso, &blocks);
        let text = selected_text(&selections);

        if let Some(expected) = &expectation.expect_contains {
            if !text.contains(expected.as_str()) {
                failures.push(format!(
                    "{}: expected selection to contain {expected:?}, got {text:?}",
                    expectation.image
                ));
            }
        }

        if let Some(unexpected) = &expectation.expect_not_contains {
            if text.contains(unexpected.as_str()) {
                failures.push(format!(
                    "{}: expected selection to NOT contain {unexpected:?}, got {text:?}",
                    expectation.image
                ));
            }
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
