//! "Redact" (Pro): detects emails, phone numbers, credit card numbers
//! (Luhn-checked), common API key formats, and IPv4 addresses in an
//! already-OCR'd selection, and reports the screen-space box each match
//! occupies so the WebView can paint an opaque cover over it.
//!
//! Preview-first by design: this command only detects and reports boxes —
//! nothing is altered here. Copy and Save each separately honor whatever
//! the WebView currently has redacted once the user acts on the preview.
//!
//! Detection runs per OCR line, over the words re-joined with a single
//! space each — the exact same join `ocr.rs` uses to build copyable text
//! (see `ocr.rs`'s `lines_from_result`) — so a byte offset into that string
//! always lands on the same word boundary that produced it.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::entitlements::Plan;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactWord {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactLine {
    pub words: Vec<RedactWord>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RedactedBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub kind: &'static str,
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}").unwrap());
static PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\+?\d{1,3}[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b").unwrap());
static CARD_CANDIDATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:\d[ -]?){13,19}\b").unwrap());
static IPV4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b").unwrap());
// Prefix-specific key formats only — deliberately no generic "long
// hex/base64 run" fallback, which would false-positive on ordinary OCR'd
// hashes/ids often enough to make the preview untrustworthy.
static STRIPE_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bsk_[a-zA-Z0-9_]{20,}\b").unwrap());
static GITHUB_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bgh[pousr]_[A-Za-z0-9]{36,}\b").unwrap());
static AWS_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap());
static GOOGLE_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bAIza[0-9A-Za-z_-]{35}\b").unwrap());

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RedactResult {
    pub boxes: Vec<RedactedBox>,
    /// The whole selection's text, one line per OCR line, with every
    /// detected span replaced by `[redacted]`. This is what Copy uses
    /// instead of the native selection once Redact has run for the current
    /// selection (see `main.ts`) — the boxes alone only cover the image.
    pub text: String,
}

/// Pro-only, enforced here and not just by the overlay hiding the button —
/// the plan is derived from a verified token on the Rust side, which the
/// WebView can't influence.
#[tauri::command]
pub fn detect_sensitive(state: tauri::State<AppState>, lines: Vec<RedactLine>) -> Result<RedactResult, String> {
    if state.entitlements().plan == Plan::Free {
        return Err("Redact is a Pro feature".to_string());
    }
    Ok(find_sensitive(lines))
}

pub fn find_sensitive(lines: Vec<RedactLine>) -> RedactResult {
    let mut boxes = Vec::new();
    let mut redacted_lines = Vec::new();
    for line in &lines {
        let (line_boxes, line_text) = detect_in_line(line);
        boxes.extend(line_boxes);
        redacted_lines.push(line_text);
    }
    RedactResult { boxes, text: redacted_lines.join("\n") }
}

/// One line's words joined with single spaces, plus each word's `[start,
/// end)` byte span in that joined string.
fn line_text_with_word_spans(line: &RedactLine) -> (String, Vec<(usize, usize)>) {
    let mut text = String::new();
    let mut spans = Vec::with_capacity(line.words.len());
    for word in &line.words {
        if !text.is_empty() {
            text.push(' ');
        }
        let start = text.len();
        text.push_str(&word.text);
        spans.push((start, text.len()));
    }
    (text, spans)
}

fn bounding_box_of(words: &[RedactWord], indices: &[usize]) -> Option<(f64, f64, f64, f64)> {
    indices.iter().fold(None, |acc, &i| {
        let w = &words[i];
        Some(match acc {
            None => (w.x, w.y, w.x + w.width, w.y + w.height),
            Some((x0, y0, x1, y1)) => (x0.min(w.x), y0.min(w.y), x1.max(w.x + w.width), y1.max(w.y + w.height)),
        })
    })
}

/// Returns this line's redaction boxes and its text with every kept match
/// replaced by `[redacted]`. Line spacing between OCR'd paragraphs (the
/// blank-line gaps `codeformat.rs` reconstructs for code) isn't
/// reconstructed here — a cosmetic simplification, not a correctness gap,
/// since nothing about *which* spans get redacted depends on it.
fn detect_in_line(line: &RedactLine) -> (Vec<RedactedBox>, String) {
    if line.words.is_empty() {
        return (Vec::new(), String::new());
    }
    let (text, spans) = line_text_with_word_spans(line);

    // (word indices touched, char range, kind) per match, before turning
    // into boxes/text — collected first so overlapping matches from
    // different patterns (e.g. a phone-shaped run of digits inside a
    // longer card number) can be deduplicated by index set rather than
    // stacking a redundant nested box, or double-redacting the same text.
    let mut matches: Vec<(Vec<usize>, (usize, usize), &'static str)> = Vec::new();
    let mut collect = |regex: &Regex, kind: &'static str, validate: &dyn Fn(&str) -> bool| {
        for m in regex.find_iter(&text) {
            if !validate(m.as_str()) {
                continue;
            }
            let touched: Vec<usize> =
                spans.iter().enumerate().filter(|(_, (s, e))| *s < m.end() && *e > m.start()).map(|(i, _)| i).collect();
            if !touched.is_empty() {
                matches.push((touched, (m.start(), m.end()), kind));
            }
        }
    };

    collect(&EMAIL, "email", &|_| true);
    collect(&STRIPE_KEY, "api-key", &|_| true);
    collect(&GITHUB_TOKEN, "api-key", &|_| true);
    collect(&AWS_KEY, "api-key", &|_| true);
    collect(&GOOGLE_KEY, "api-key", &|_| true);
    collect(&CARD_CANDIDATE, "card", &|candidate| passes_luhn(candidate));
    collect(&IPV4, "ip", &|candidate| is_plausible_ipv4(candidate));
    collect(&PHONE, "phone", &|_| true);

    // Drop any match whose word set is a subset of (or identical to)
    // another match's — keeps one box per underlying span instead of
    // stacking a phone-shaped box inside a card box that already covers it.
    matches.sort_by_key(|(indices, ..)| std::cmp::Reverse(indices.len()));
    let mut kept: Vec<(Vec<usize>, (usize, usize), &'static str)> = Vec::new();
    'matches: for (indices, range, kind) in matches {
        for (kept_indices, ..) in &kept {
            if indices.iter().all(|i| kept_indices.contains(i)) {
                continue 'matches;
            }
        }
        kept.push((indices, range, kind));
    }

    let boxes = kept
        .iter()
        .filter_map(|(indices, _, kind)| {
            bounding_box_of(&line.words, indices)
                .map(|(x0, y0, x1, y1)| RedactedBox { x: x0, y: y0, width: x1 - x0, height: y1 - y0, kind })
        })
        .collect();

    let mut ranges: Vec<(usize, usize)> = kept.iter().map(|(_, range, _)| *range).collect();
    ranges.sort_unstable();
    let mut redacted_text = String::new();
    let mut cursor = 0;
    for (start, end) in ranges {
        redacted_text.push_str(&text[cursor..start]);
        redacted_text.push_str("[redacted]");
        cursor = end;
    }
    redacted_text.push_str(&text[cursor..]);

    (boxes, redacted_text)
}

fn passes_luhn(candidate: &str) -> bool {
    let digits: Vec<u32> = candidate.chars().filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| if i % 2 == 1 { if d * 2 > 9 { d * 2 - 9 } else { d * 2 } } else { d })
        .sum();
    sum.is_multiple_of(10)
}

fn is_plausible_ipv4(candidate: &str) -> bool {
    let octets: Vec<&str> = candidate.split('.').collect();
    octets.len() == 4 && octets.iter().all(|octet| octet.len() <= 3 && octet.parse::<u16>().is_ok_and(|n| n <= 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, x: f64) -> RedactWord {
        RedactWord { text: text.to_string(), x, y: 0.0, width: (text.len() as f64) * 8.0, height: 16.0 }
    }

    #[test]
    fn detects_an_email_within_a_single_word() {
        let lines = vec![RedactLine { words: vec![word("Contact:", 0.0), word("nevin@example.com", 70.0)] }];
        let result = find_sensitive(lines);
        assert_eq!(result.boxes.len(), 1);
        assert_eq!(result.boxes[0].kind, "email");
        // Only the email word's box, not "Contact:"'s.
        assert_eq!(result.boxes[0].x, 70.0);
        assert_eq!(result.text, "Contact: [redacted]");
    }

    #[test]
    fn detects_a_credit_card_spanning_four_words_and_unions_their_boxes() {
        let lines = vec![RedactLine {
            words: vec![word("4111", 0.0), word("1111", 40.0), word("1111", 80.0), word("1111", 120.0)],
        }];
        let result = find_sensitive(lines);
        assert_eq!(result.boxes.len(), 1);
        assert_eq!(result.boxes[0].kind, "card");
        assert_eq!(result.boxes[0].x, 0.0);
        assert_eq!(result.boxes[0].width, 120.0 + 4.0 * 8.0); // last word's x + its width
        assert_eq!(result.text, "[redacted]");
    }

    #[test]
    fn a_digit_run_that_fails_luhn_is_not_flagged_as_a_card() {
        let lines = vec![RedactLine {
            words: vec![word("4111", 0.0), word("1111", 40.0), word("1111", 80.0), word("1112", 120.0)],
        }];
        let result = find_sensitive(lines);
        assert!(result.boxes.iter().all(|b| b.kind != "card"));
    }

    #[test]
    fn detects_common_api_key_prefixes() {
        let lines = vec![RedactLine { words: vec![word("sk_test_abcdefghijklmnopqrstuvwxyz", 0.0)] }];
        let result = find_sensitive(lines);
        assert_eq!(result.boxes.len(), 1);
        assert_eq!(result.boxes[0].kind, "api-key");
    }

    #[test]
    fn detects_a_plausible_ipv4_address() {
        let lines = vec![RedactLine { words: vec![word("192.168.1.100", 0.0)] }];
        let result = find_sensitive(lines);
        assert_eq!(result.boxes.len(), 1);
        assert_eq!(result.boxes[0].kind, "ip");
    }

    #[test]
    fn an_out_of_range_octet_is_not_flagged_as_an_ip() {
        let lines = vec![RedactLine { words: vec![word("999.999.999.999", 0.0)] }];
        let result = find_sensitive(lines);
        assert!(result.boxes.iter().all(|b| b.kind != "ip"));
    }

    #[test]
    fn does_not_double_flag_a_phone_shaped_run_already_covered_by_a_card_match() {
        // A card number whose trailing 10 digits also happen to look like a
        // phone number should produce exactly one box (the card), not two
        // overlapping ones, and exactly one "[redacted]" in the text.
        let lines = vec![RedactLine {
            words: vec![word("4111", 0.0), word("1111", 40.0), word("1111", 80.0), word("1111", 120.0)],
        }];
        let result = find_sensitive(lines);
        assert_eq!(result.boxes.len(), 1);
        assert_eq!(result.text, "[redacted]");
    }

    #[test]
    fn ordinary_text_produces_no_boxes_and_is_returned_unchanged() {
        let lines = vec![RedactLine { words: vec![word("The", 0.0), word("quick", 30.0), word("fox", 70.0)] }];
        let result = find_sensitive(lines);
        assert!(result.boxes.is_empty());
        assert_eq!(result.text, "The quick fox");
    }

    #[test]
    fn multiple_matches_on_one_line_are_each_redacted_in_order() {
        let lines = vec![RedactLine {
            words: vec![word("a@b.com", 0.0), word("and", 60.0), word("c@d.com", 90.0)],
        }];
        let result = find_sensitive(lines);
        assert_eq!(result.boxes.len(), 2);
        assert_eq!(result.text, "[redacted] and [redacted]");
    }

    #[test]
    fn empty_lines_are_ignored() {
        let result = find_sensitive(vec![RedactLine { words: vec![] }]);
        assert!(result.boxes.is_empty());
        assert_eq!(result.text, "");
    }
}
