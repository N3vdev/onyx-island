//! One-time codes from email ("Your verification code is 482913"): finding
//! the code in a message, and the island's card that shows it.
//!
//! Only messages that say they carry a code are read for one, and only the
//! number that the message presents as the code is taken: a nearby word such
//! as "code", "OTP" or "verification" is required, so an order number or a
//! price in an ordinary email is never shown.

use crate::geometry::Rect;

/// The card: the island grows to this to show a code.
pub const WIDTH: f32 = 300.0;
pub const HEIGHT: f32 = 58.0;
/// How long a code stays on the island, and after it is copied.
pub const VISIBLE_MS: u32 = 60_000;
pub const COPIED_MS: u32 = 1_400;

/// A code on the island.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeCard {
    pub code: String,
    /// Who sent it ("Google", "GitHub"): the sender's name, or its domain.
    pub from: String,
    /// Copied to the clipboard: the card says so, then goes.
    pub copied: bool,
}

/// What is under the pointer on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeItem {
    /// The code or its copy button: both copy.
    Copy,
    /// The close mark.
    Close,
}

pub mod layout {
    use super::{HEIGHT, WIDTH};
    use crate::geometry::Rect;

    pub const INSET: f32 = 16.0;
    pub const ICON: f32 = 26.0;
    pub const BUTTON: f32 = 30.0;
    pub const CLOSE: f32 = 20.0;
    /// The mail icon, left.
    pub const MAIL: Rect = Rect::new(INSET, (HEIGHT - ICON) * 0.5, ICON, ICON);
    /// The sender over the code.
    pub const TEXT_X: f32 = INSET + ICON + 12.0;
    /// The copy button, right, with the close mark beyond it.
    pub const CLOSE_MARK: Rect = Rect::new(WIDTH - INSET - CLOSE, (HEIGHT - CLOSE) * 0.5, CLOSE, CLOSE);
    pub const COPY: Rect = Rect::new(WIDTH - INSET - CLOSE - 8.0 - BUTTON, (HEIGHT - BUTTON) * 0.5, BUTTON, BUTTON);
}

impl CodeCard {
    /// What a click at (`x`, `y`) on the card does.
    #[must_use]
    pub fn item_at(&self, x: f32, y: f32) -> Option<CodeItem> {
        let slop = 4.0;
        let hit = |rect: Rect| {
            x >= rect.x - slop && x <= rect.right() + slop && y >= rect.y - slop && y <= rect.bottom() + slop
        };
        if hit(layout::CLOSE_MARK) {
            return Some(CodeItem::Close);
        }
        // Anywhere else on the card copies: the code is the point of it.
        ((0.0..=WIDTH).contains(&x) && (0.0..=HEIGHT).contains(&y)).then_some(CodeItem::Copy)
    }

    /// The code as shown: long numbers split in two ("482 913").
    #[must_use]
    pub fn shown(&self) -> String {
        let chars: Vec<char> = self.code.chars().collect();
        if chars.len() >= 6 && chars.iter().all(char::is_ascii_digit) {
            let half = chars.len() / 2;
            format!("{} {}", chars[..half].iter().collect::<String>(), chars[half..].iter().collect::<String>())
        } else {
            self.code.clone()
        }
    }
}

/// Words that say a message carries a code.
const CUES: &[&str] = &[
    "code",
    "otp",
    "one-time",
    "one time",
    "verification",
    "verify",
    "passcode",
    "security code",
    "login",
    "log in",
    "sign in",
    "sign-in",
    "2fa",
    "two-factor",
    "authentication",
    "confirm",
    "pin",
];

/// A word just before a number that makes it not a code.
const NOT_CODE_BEFORE: &[&str] = &[
    "$", "₹", "€", "£", "rs", "rs.", "inr", "usd", "order", "#", "invoice", "account", "a/c", "ref", "no.", "phone", "call",
];

/// The code a message gives, from its subject and the start of its text.
#[must_use]
pub fn find_code(subject: &str, text: &str) -> Option<String> {
    let subject_cued = has_cue(subject);
    if !subject_cued && !has_cue(text) {
        return None;
    }
    // The subject first ("482913 is your code"), then the text.
    // Text under a subject that says "code" may not say it again: then the
    // code is near its start.
    code_in(subject, false).or_else(|| code_in(text, subject_cued))
}

fn has_cue(text: &str) -> bool {
    let lower = text.to_lowercase();
    CUES.iter().any(|cue| contains_word(&lower, cue))
}

/// `cue` in `text` as a word ("pin" not in "shopping").
fn contains_word(text: &str, cue: &str) -> bool {
    text.match_indices(cue).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + cue.len()..].chars().next();
        !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
    })
}

/// A candidate: where it starts in the words, and the code.
struct Candidate {
    word: usize,
    code: String,
}

/// The candidate nearest a cue word (or, `cued_above`, the start).
fn code_in(text: &str, cued_above: bool) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|word| word.to_lowercase()).collect();
    let mut cues: Vec<usize> = (0..words.len())
        .filter(|&index| CUES.iter().any(|cue| cue_at(&lower, index, cue)))
        .collect();
    if cues.is_empty() && cued_above {
        cues.push(0);
    }
    if cues.is_empty() {
        return None;
    }
    let mut candidates = Vec::new();
    let mut index = 0;
    while index < words.len() {
        // "482 913" or "482-913" is one code over two words.
        let joined = words
            .get(index + 1)
            .map(|next| (clean(words[index]), clean(next)))
            .filter(|(a, b)| a.len() == 3 && b.len() == 3 && all_digits(a) && all_digits(b))
            .map(|(a, b)| format!("{a}{b}"));
        if let Some(code) = joined {
            candidates.push(Candidate { word: index, code });
            index += 2;
            continue;
        }
        if let Some(code) = as_code(words[index]) {
            let before = index.checked_sub(1).map(|at| lower[at].as_str()).unwrap_or("");
            let money = NOT_CODE_BEFORE.iter().any(|word| before == *word || before.ends_with(word))
                || words[index].starts_with(['$', '₹', '€', '£', '#']);
            if !money && !looks_like_a_date_or_time(words[index]) {
                candidates.push(Candidate { word: index, code });
            }
        }
        index += 1;
    }
    candidates
        .into_iter()
        .min_by_key(|candidate| cues.iter().map(|&cue| cue.abs_diff(candidate.word)).min().unwrap_or(usize::MAX))
        .filter(|candidate| cues.iter().any(|&cue| cue.abs_diff(candidate.word) <= 12))
        .map(|candidate| candidate.code)
}

/// The cue's words start at `index` ("one time" spans two).
fn cue_at(lower: &[String], index: usize, cue: &str) -> bool {
    let parts: Vec<&str> = cue.split(' ').collect();
    parts.iter().enumerate().all(|(offset, part)| {
        lower.get(index + offset).is_some_and(|word| {
            let word = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
            word == *part || (parts.len() == 1 && word.strip_suffix('s') == Some(part))
        })
    })
}

/// The word with surrounding punctuation taken off ("482913." or "(482913)").
fn clean(word: &str) -> &str {
    word.trim_matches(|c: char| !c.is_alphanumeric())
}

fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// A word that is a code: 4 to 8 digits, Google's "G-482913", or 6 to 8
/// capitals and digits with at least two digits ("K7Q2ZP").
fn as_code(word: &str) -> Option<String> {
    let word = clean(word);
    let word = word.strip_prefix("G-").unwrap_or(word);
    let digits = word.bytes().filter(u8::is_ascii_digit).count();
    if all_digits(word) && (4..=8).contains(&word.len()) {
        return Some(word.to_owned());
    }
    let mixed = (6..=8).contains(&word.len())
        && digits >= 2
        && word.bytes().all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase());
    mixed.then(|| word.to_owned())
}

/// "2026", "12:30", "16/11" are not codes.
fn looks_like_a_date_or_time(word: &str) -> bool {
    let word = clean(word);
    if word.contains([':', '/']) {
        return true;
    }
    word.len() == 4 && all_digits(word) && (word.starts_with("19") || word.starts_with("20"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_found_where_the_message_says_there_is_one() {
        assert_eq!(find_code("Your verification code", "Your code is 482913. It expires in 10 minutes."), Some("482913".into()));
        assert_eq!(find_code("G-482913 is your Google verification code", ""), Some("482913".into()));
        assert_eq!(find_code("Sign in to GitHub", "Use 482 913 to sign in."), Some("482913".into()));
        assert_eq!(find_code("Your OTP", "OTP: 4829. Do not share it."), Some("4829".into()));
        assert_eq!(find_code("Login code", "Enter K7Q2ZP to finish logging in"), Some("K7Q2ZP".into()));
        assert_eq!(find_code("", "Your one time password is 90817263"), Some("90817263".into()));
    }

    #[test]
    fn ordinary_mail_shows_no_code() {
        assert_eq!(find_code("Your order has shipped", "Order 48291345 is on its way. Total $1299."), None);
        assert_eq!(find_code("Meeting at 12:30", "See you in room 4021"), None);
        assert_eq!(find_code("Shopping list", "Spinach, 2 pins, 1500 grams"), None);
    }

    #[test]
    fn money_dates_and_order_numbers_near_a_cue_are_skipped() {
        assert_eq!(
            find_code("Confirm your payment", "Pay ₹4999 by 2026. Your code is 731904."),
            Some("731904".into())
        );
        assert_eq!(find_code("Verify", "Order #483920 needs your code 5521"), Some("5521".into()));
    }

    #[test]
    fn long_codes_show_in_two_halves_and_clicks_copy_or_close() {
        let card = CodeCard { code: "482913".into(), from: "Google".into(), copied: false };
        assert_eq!(card.shown(), "482 913");
        assert_eq!(CodeCard { code: "4829".into(), ..card.clone() }.shown(), "4829");
        let close = layout::CLOSE_MARK;
        assert_eq!(card.item_at(close.x + 4.0, close.y + 4.0), Some(CodeItem::Close));
        assert_eq!(card.item_at(layout::TEXT_X + 10.0, HEIGHT * 0.5), Some(CodeItem::Copy));
        assert_eq!(card.item_at(WIDTH + 20.0, 10.0), None);
    }
}
