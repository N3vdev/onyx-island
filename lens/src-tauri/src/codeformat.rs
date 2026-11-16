//! "Code from screen": reconstructs indentation and does light, contextual
//! OCR-error cleanup on text the app has already recognized, for the
//! "Copy as code" text action. Operates purely on OCR word/line geometry
//! already produced by `ocr.rs` — no model, no new dependency.
//!
//! Fixes are deliberately scoped to contexts where they're unambiguous
//! (e.g. l/1 confusion only inside an already-mostly-numeric token) —
//! never a blanket substitution, which would corrupt an identifier like
//! `for` or `Ok`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeWord {
    pub text: String,
    pub x: f64,
    pub width: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeLine {
    pub y: f64,
    pub height: f64,
    pub words: Vec<CodeWord>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodeResult {
    pub code: String,
    pub language: String,
}

/// How much taller than a normal line-to-line gap counts as "there was a
/// blank line here". Mirrors `layout.rs`'s own line-merge gap multiplier
/// (kept as a separate constant since the two modules have no reason to
/// share one — this just avoids inventing an unrelated threshold).
const BLANK_LINE_GAP_MULTIPLIER: f64 = 1.6;

#[tauri::command]
pub fn format_as_code(lines: Vec<CodeLine>) -> CodeResult {
    let mut lines = lines;
    lines.retain(|line| !line.words.is_empty());
    lines.sort_by(|a, b| a.y.total_cmp(&b.y));

    let code = reconstruct(&lines);
    let language = detect_language(&code);
    CodeResult { code, language }
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(values[values.len() / 2])
}

fn median_line_height(lines: &[CodeLine]) -> f64 {
    median(lines.iter().map(|l| l.height).collect()).unwrap_or(16.0)
}

/// Median px-per-character across every word, used to turn a word's `x`
/// offset into a column (space) count for indentation.
fn char_width(lines: &[CodeLine]) -> f64 {
    let widths = lines
        .iter()
        .flat_map(|l| &l.words)
        .filter(|w| !w.text.is_empty())
        .map(|w| w.width / w.text.chars().count() as f64)
        .filter(|w| w.is_finite() && *w > 0.0)
        .collect();
    median(widths).unwrap_or(8.0)
}

fn reconstruct(lines: &[CodeLine]) -> String {
    let Some(left_margin) =
        lines.iter().filter_map(|l| l.words.first()).map(|w| w.x).fold(None, |acc: Option<f64>, x| {
            Some(acc.map_or(x, |min| min.min(x)))
        })
    else {
        return String::new();
    };
    let char_w = char_width(lines).max(1.0);
    let line_h = median_line_height(lines).max(1.0);

    let mut out = String::new();
    let mut previous: Option<&CodeLine> = None;
    for line in lines {
        if let Some(prev) = previous {
            out.push('\n');
            let gap = line.y - (prev.y + prev.height);
            if gap > line_h * BLANK_LINE_GAP_MULTIPLIER {
                out.push('\n');
            }
        }
        let indent_cols = ((line.words[0].x - left_margin) / char_w).round().max(0.0) as usize;
        out.push_str(&" ".repeat(indent_cols));
        let joined = line.words.iter().map(|w| clean_token(&w.text)).collect::<Vec<_>>().join(" ");
        out.push_str(&joined);
        previous = Some(line);
    }
    out
}

/// Contextual OCR cleanup for a single token — never a blanket substitution.
fn clean_token(token: &str) -> String {
    let straightened: String = token
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            other => other,
        })
        .collect();

    let digit_like = straightened.chars().filter(char::is_ascii_digit).count();
    let letter_like = straightened.chars().filter(char::is_ascii_alphabetic).count();
    // Only touch a token where digits already dominate — e.g. "l23" or
    // "1O5" — never an identifier like "for" or "Ok" that merely contains
    // one of these letters.
    if digit_like > 0 && digit_like >= letter_like {
        straightened
            .chars()
            .map(|c| match c {
                'l' | 'I' | '|' => '1',
                'O' | 'o' => '0',
                other => other,
            })
            .collect()
    } else {
        straightened
    }
}

/// Keyword/symbol scoring across common languages — this only labels the
/// "Copied as X" flash, so it doesn't need to be more than a good guess.
fn detect_language(code: &str) -> String {
    const SCORED: &[(&str, &[&str])] = &[
        ("Python", &["def ", "elif ", "import ", "self.", "None", "print(", "except "]),
        ("Rust", &["fn ", "let mut ", "impl ", "pub fn", "->", "::", "#[", "match "]),
        ("TypeScript", &["interface ", ": string", ": number", "=>", "export ", "const "]),
        ("JavaScript", &["function ", "const ", "let ", "=>", "console.log", "require("]),
        ("Java", &["public class", "private ", "static void", "System.out"]),
        ("C#", &["using ", "namespace ", "public class", "Console."]),
        ("C++", &["#include", "std::", "int main("]),
        ("Go", &["func ", "package ", ":="]),
        ("SQL", &["SELECT ", "FROM ", "WHERE ", "INSERT INTO"]),
    ];
    SCORED
        .iter()
        .map(|(name, keywords)| (*name, keywords.iter().filter(|kw| code.contains(*kw)).count()))
        .filter(|(_, score)| *score > 0)
        .max_by_key(|(_, score)| *score)
        .map_or_else(|| "Text".to_string(), |(name, _)| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, x: f64, width: f64) -> CodeWord {
        CodeWord { text: text.to_string(), x, width }
    }

    #[test]
    fn reconstructs_indentation_from_word_x_offsets() {
        // "def f():" at column 0, "return 1" indented one level (4 chars
        // wide at an 8px char width = 32px).
        let lines = vec![
            CodeLine { y: 0.0, height: 16.0, words: vec![word("def", 0.0, 24.0), word("f():", 24.0, 32.0)] },
            CodeLine { y: 16.0, height: 16.0, words: vec![word("return", 32.0, 48.0), word("1", 80.0, 8.0)] },
        ];
        let result = format_as_code(lines);
        assert_eq!(result.code, "def f():\n    return 1");
    }

    #[test]
    fn inserts_a_blank_line_for_a_large_vertical_gap() {
        let lines = vec![
            CodeLine { y: 0.0, height: 16.0, words: vec![word("a", 0.0, 8.0)] },
            // Gap of 40px after a 16px line, on a 16px median height — well
            // past the 1.6x threshold, so this should read as one blank line.
            CodeLine { y: 56.0, height: 16.0, words: vec![word("b", 0.0, 8.0)] },
        ];
        let result = format_as_code(lines);
        assert_eq!(result.code, "a\n\nb");
    }

    #[test]
    fn does_not_insert_a_blank_line_for_normal_line_spacing() {
        let lines = vec![
            CodeLine { y: 0.0, height: 16.0, words: vec![word("a", 0.0, 8.0)] },
            CodeLine { y: 18.0, height: 16.0, words: vec![word("b", 0.0, 8.0)] },
        ];
        let result = format_as_code(lines);
        assert_eq!(result.code, "a\nb");
    }

    #[test]
    fn fixes_digit_letter_confusion_only_inside_numeric_tokens() {
        assert_eq!(clean_token("l23"), "123");
        assert_eq!(clean_token("1O5"), "105");
        // Never touched: these are letter-dominant, even though they
        // contain the same characters that get swapped above.
        assert_eq!(clean_token("for"), "for");
        assert_eq!(clean_token("Ok"), "Ok");
        assert_eq!(clean_token("I"), "I");
    }

    #[test]
    fn straightens_smart_quotes_unconditionally() {
        assert_eq!(clean_token("\u{2018}hi\u{2019}"), "'hi'");
        assert_eq!(clean_token("\u{201C}hi\u{201D}"), "\"hi\"");
    }

    #[test]
    fn detects_python_by_keyword_score() {
        let lines = vec![
            CodeLine { y: 0.0, height: 16.0, words: vec![word("def", 0.0, 24.0), word("f():", 24.0, 32.0)] },
            CodeLine {
                y: 16.0,
                height: 16.0,
                words: vec![word("self.x", 32.0, 48.0), word("=", 80.0, 8.0), word("None", 88.0, 32.0)],
            },
        ];
        let result = format_as_code(lines);
        assert_eq!(result.language, "Python");
    }

    #[test]
    fn falls_back_to_text_when_nothing_scores() {
        let lines = vec![CodeLine { y: 0.0, height: 16.0, words: vec![word("hello", 0.0, 40.0), word("world", 44.0, 40.0)] }];
        let result = format_as_code(lines);
        assert_eq!(result.language, "Text");
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let result = format_as_code(vec![]);
        assert_eq!(result.code, "");
        assert_eq!(result.language, "Text");
    }

    #[test]
    fn lines_with_no_words_are_ignored() {
        let lines = vec![
            CodeLine { y: 0.0, height: 16.0, words: vec![word("a", 0.0, 8.0)] },
            CodeLine { y: 16.0, height: 16.0, words: vec![] },
        ];
        let result = format_as_code(lines);
        assert_eq!(result.code, "a");
    }
}
