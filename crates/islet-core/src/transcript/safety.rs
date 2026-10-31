//! Conservative checks on observable facts. This is not semantic verification.
use super::{facts, norm, RewriteRejection, Token};
use std::collections::BTreeSet;

fn tokens(text: &str) -> Vec<Token> {
    text.split_whitespace().map(Token::parse).collect()
}

fn list_items(text: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(body) = ["- ", "* ", "\u{2022} "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
        {
            items.push(body.to_owned());
        } else if let Some(item) = items.last_mut().filter(|_| !line.is_empty()) {
            item.push(' ');
            item.push_str(line);
        }
    }
    items
}

fn loses_list_content(before: &str, after: &str) -> bool {
    let words = |text: &str| {
        super::content_words(text)
            .into_iter()
            .filter(|word| word.len() >= 4)
            .collect::<BTreeSet<_>>()
    };
    let old = words(before);
    let new = words(after);
    let missing = old
        .iter()
        .filter(|word| {
            !new.iter()
                .any(|other| word == &other || super::within_edits(word, other, 2))
        })
        .count();
    let scope = |text: &str| {
        super::content_words(text)
            .into_iter()
            .filter(|word| {
                matches!(
                    word.as_str(),
                    "or" | "only" | "unless" | "except" | "until" | "without"
                )
            })
            .collect::<Vec<_>>()
    };
    missing > (old.len() / 6).max(1) || scope(before) != scope(after)
}

fn question_at(words: &[Token], at: usize) -> bool {
    let key = |i| words.get(i).map(norm).unwrap_or_default();
    if at > 0 && matches!(key(at - 1).as_str(), "if" | "whether") {
        return false;
    }
    if words[at].lead.contains(['"', '\u{201c}']) {
        return false;
    }
    let auxiliary = |w: &str| {
        matches!(
            w,
            "can"
                | "can't"
                | "could"
                | "couldn't"
                | "would"
                | "wouldn't"
                | "should"
                | "shouldn't"
                | "will"
                | "won't"
                | "do"
                | "does"
                | "did"
                | "is"
                | "are"
                | "was"
                | "were"
                | "has"
                | "have"
                | "may"
                | "might"
                | "must"
        )
    };
    let pronoun = |w: &str| {
        matches!(
            w,
            "you" | "we" | "i" | "he" | "she" | "they" | "it" | "this" | "that"
        )
    };
    let clause_start = at == 0
        || words[at - 1].ends_sentence()
        || words[at - 1].word.is_empty()
        || matches!(key(at - 1).as_str(), "also" | "and" | "then" | "so");
    let direct_request =
        matches!(key(at).as_str(), "can" | "could" | "would" | "will") && key(at + 1) == "you";
    if !clause_start && !direct_request {
        return false;
    }
    (auxiliary(&key(at)) && pronoun(&key(at + 1)))
        || (matches!(
            key(at).as_str(),
            "what" | "why" | "how" | "when" | "where" | "who" | "which"
        ) && auxiliary(&key(at + 1)))
}

fn command_boundary(words: &[Token], at: usize) -> bool {
    let key = |i| words.get(i).map(norm).unwrap_or_default();
    (key(at) == "and" && key(at + 1) == "one" && key(at + 2) == "more" && key(at + 3) == "thing")
        || (matches!(key(at).as_str(), "and" | "also" | "but")
            && matches!(key(at + 1).as_str(), "please" | "don't" | "dont"))
        || (key(at) == "then"
            && matches!(
                key(at + 1).as_str(),
                "send" | "save" | "keep" | "publish" | "delete" | "open" | "close"
            ))
}

/// Only direct English question patterns; indirect clauses are left alone.
pub(super) fn punctuate_questions(words: &mut [Token]) {
    let starts: Vec<_> = (0..words.len())
        .filter(|&at| question_at(words, at))
        .collect();
    for &start in &starts {
        let mut end = words.len();
        for at in start + 2..words.len() {
            if at > start + 3 && command_boundary(words, at) {
                end = at;
                break;
            }
            if words[at - 1].ends_sentence() {
                end = at;
                break;
            }
        }
        if end <= start + 1
            || words[start..end]
                .iter()
                .any(|t| t.verbatim && t.word.contains('\n'))
        {
            continue;
        }
        let direct_you_request = (start == 0 || words[start - 1].ends_sentence())
            && matches!(
                norm(&words[start]).as_str(),
                "can" | "could" | "would" | "will"
            )
            && words.get(start + 1).is_some_and(|t| norm(t) == "you");
        let last = &mut words[end - 1];
        if last.verbatim || (last.ends_sentence() && !(last.trail == "." && direct_you_request)) {
            continue;
        }
        last.trail = format!(
            "?{}",
            last.trail.trim_start_matches(['.', '!', ',', ';', ':'])
        );
        if let Some(next) = words.get_mut(end) {
            super::capitalize(next);
        }
    }
}

fn stem(word: &str) -> String {
    let word = word
        .strip_suffix("ing")
        .or_else(|| word.strip_suffix("ed"))
        .unwrap_or(word);
    let word = word.strip_suffix('s').unwrap_or(word);
    word.trim_end_matches('e').to_owned()
}

fn negative(word: &str) -> bool {
    word.ends_with("n't")
        || matches!(
            word,
            "not"
                | "no"
                | "never"
                | "without"
                | "cannot"
                | "dont"
                | "doesnt"
                | "didnt"
                | "isnt"
                | "arent"
                | "wasnt"
                | "werent"
                | "hasnt"
                | "havent"
                | "hadnt"
                | "cant"
                | "couldnt"
                | "shouldnt"
                | "wouldnt"
                | "wont"
                | "mustnt"
        )
}

fn helper(word: &str) -> bool {
    matches!(
        word,
        "a" | "an"
            | "the"
            | "to"
            | "be"
            | "been"
            | "being"
            | "have"
            | "has"
            | "had"
            | "do"
            | "does"
            | "did"
            | "is"
            | "are"
            | "was"
            | "were"
            | "ever"
            | "yet"
            | "really"
            | "actually"
            | "just"
            | "please"
            | "i"
            | "you"
            | "we"
            | "they"
            | "he"
            | "she"
            | "it"
    )
}

fn negations(words: &[Token]) -> Vec<String> {
    let mut result = Vec::new();
    for (at, word) in words.iter().enumerate() {
        if !negative(&norm(word)) {
            continue;
        }
        let key = norm(word);
        if key == "no"
            && at > 0
            && word.trail.is_empty()
            && words.get(at + 1).is_some_and(|t| {
                matches!(
                    norm(t).as_str(),
                    "what" | "which" | "who" | "where" | "when" | "why" | "how" | "tell"
                )
            })
        {
            continue; // an explicit question restart, not "no files/errors"
        }
        let previous = at
            .checked_sub(1)
            .and_then(|i| words.get(i))
            .map(norm)
            .unwrap_or_default();
        let mode = match key.as_str() {
            "can't" | "cant" | "cannot" => "can",
            "couldn't" | "couldnt" => "could",
            "won't" | "wont" => "will",
            "wouldn't" | "wouldnt" => "would",
            "shouldn't" | "shouldnt" => "should",
            "mustn't" | "mustnt" => "must",
            "never" => "never",
            "without" => "without",
            "not"
                if matches!(
                    previous.as_str(),
                    "can" | "could" | "will" | "would" | "should" | "must" | "may" | "might"
                ) =>
            {
                previous.as_str()
            }
            _ => "not",
        };
        let mut anchor = None;
        let mut end = at + 1;
        while end < words.len() && end <= at + 6 {
            let next = norm(&words[end]);
            if !helper(&next) && !next.is_empty() {
                anchor = Some(stem(&next));
                break;
            }
            if words[end].ends_sentence() {
                break;
            }
            end += 1;
        }
        result.push(format!(
            "{mode}:{}",
            anchor.unwrap_or_else(|| "<negation>".into())
        ));
        // "Do not change the amount or delete the backup": both predicates
        // belong to the negative instruction. A new sentence ends that scope.
        for next in end + 1..words.len().min(at + 16) {
            if words[next - 1].ends_sentence() || norm(&words[next]) == "but" {
                break;
            }
            let key = norm(&words[next]);
            if negative(&key)
                || matches!(key.as_str(), "please" | "also")
                || (key == "do" && words.get(next + 1).is_some_and(|t| norm(t) == "not"))
            {
                break;
            }
            if norm(&words[next]) == "or" {
                if let Some(verb) = words.get(next + 1) {
                    let key = norm(verb);
                    if !helper(&key) && !negative(&key) {
                        result.push(format!("{mode}:{}", stem(&key)));
                    }
                }
            }
        }
    }
    result.sort();
    result
}

fn bare_not(words: &[Token]) -> usize {
    words
        .iter()
        .enumerate()
        .filter(|(at, word)| {
            if norm(word) != "not" {
                return false;
            }
            if words
                .get(at + 1)
                .is_some_and(|t| matches!(norm(t).as_str(), "to" | "only" | "just"))
            {
                return false;
            }
            let mut previous = *at;
            while previous > 0 && at - previous < 4 {
                previous -= 1;
                let key = norm(&words[previous]);
                if matches!(
                    key.as_str(),
                    "do" | "does"
                        | "did"
                        | "am"
                        | "is"
                        | "are"
                        | "was"
                        | "were"
                        | "has"
                        | "have"
                        | "had"
                        | "can"
                        | "could"
                        | "will"
                        | "would"
                        | "should"
                        | "must"
                        | "may"
                        | "might"
                ) {
                    return false;
                }
                if !matches!(
                    key.as_str(),
                    "really"
                        | "usually"
                        | "still"
                        | "yet"
                        | "actually"
                        | "currently"
                        | "definitely"
                        | "certainly"
                        | "ever"
                ) {
                    break;
                }
            }
            true
        })
        .count()
}

fn direction(words: &[Token]) -> Vec<&'static str> {
    let mut result = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let key = norm(&words[at]);
        let next = words.get(at + 1).map(norm).unwrap_or_default();
        let value = match key.as_str() {
            "login" | "signin" => Some("sign-in"),
            "logout" | "signout" => Some("sign-out"),
            "log" | "logs" | "logging" | "logged" | "sign" | "signs" | "signing" | "signed"
                if next == "in" =>
            {
                at += 1;
                Some("sign-in")
            }
            "log" | "logs" | "logging" | "logged" | "sign" | "signs" | "signing" | "signed"
                if next == "out" =>
            {
                at += 1;
                Some("sign-out")
            }
            "enable" | "enabled" | "enabling" | "enables" => Some("enable"),
            "disable" | "disabled" | "disabling" | "disables" => Some("disable"),
            "increase" | "increased" | "increasing" | "increases" => Some("increase"),
            "decrease" | "decreased" | "decreasing" | "decreases" => Some("decrease"),
            "upload" | "uploaded" | "uploading" | "uploads" => Some("upload"),
            "download" | "downloaded" | "downloading" | "downloads" => Some("download"),
            _ => None,
        };
        if let Some(value) = value {
            result.push(value);
        }
        at += 1;
    }
    // Mention count may change when a repeated phrase is removed, but neither
    // an opposite action nor a new direction may appear.
    result.sort();
    result.dedup();
    result
}

fn calendar_value(words: &[Token], at: usize) -> Option<&'static str> {
    let token = &words[at];
    let key = norm(token);
    let value = facts::calendar(&key)?;
    let full_day = facts::weekday(&key).is_some_and(|day| day == key);
    let date_context = words
        .get(at + 1)
        .is_some_and(|t| t.word.chars().any(|c| c.is_ascii_digit()));
    let proper = token.word.chars().next().is_some_and(char::is_uppercase);
    // "may restart" and "sat by the sun" are not dates.
    if key == "may" && !date_context {
        let previous = at
            .checked_sub(1)
            .and_then(|i| words.get(i))
            .map(norm)
            .unwrap_or_default();
        if !matches!(
            previous.as_str(),
            "in" | "by" | "before" | "after" | "until" | "during" | "of" | "next" | "this"
        ) {
            return None;
        }
    }
    let full_month = value == key;
    if full_day || full_month || proper || date_context {
        Some(value)
    } else {
        None
    }
}

fn calendar(words: &[Token]) -> Vec<&'static str> {
    (0..words.len())
        .filter_map(|at| calendar_value(words, at))
        .collect()
}

fn entities(words: &[Token]) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for (at, token) in words.iter().enumerate() {
        let key = norm(token);
        let named_modal = matches!(token.word.as_str(), "May" | "Will")
            && words
                .get(at + 1)
                .is_some_and(|t| matches!(norm(t).as_str(), "has" | "had" | "is" | "was" | "does"));
        if key.is_empty()
            || helper(&key)
            || negative(&key)
            || (matches!(
                key.as_str(),
                "and"
                    | "but"
                    | "so"
                    | "okay"
                    | "here"
                    | "there"
                    | "what"
                    | "why"
                    | "how"
                    | "who"
                    | "where"
                    | "when"
                    | "which"
                    | "whose"
                    | "this"
                    | "that"
                    | "these"
                    | "those"
                    | "my"
                    | "your"
                    | "our"
                    | "their"
                    | "his"
                    | "its"
                    | "may"
                    | "might"
                    | "must"
                    | "can"
                    | "could"
                    | "will"
                    | "should"
                    | "would"
            ) && !named_modal)
            || calendar_value(words, at).is_some()
        {
            continue;
        }
        if (at.saturating_sub(2)..=at)
            .any(|i| facts::quantity(words, i).is_some_and(|q| q.end > at))
        {
            continue;
        }
        let letters: Vec<_> = token.word.chars().filter(|c| c.is_alphabetic()).collect();
        let acronym = letters.len() >= 2 && letters.iter().all(|c| c.is_uppercase());
        let mixed_case = token.word.chars().skip(1).any(char::is_uppercase);
        let identifier = token.word.contains(['@', '/', '\\', '_'])
            || (token.word.chars().any(|c| c.is_ascii_digit()) && !letters.is_empty());
        let subject = token.word.chars().next().is_some_and(char::is_uppercase)
            && words.get(at + 1).is_some_and(|t| {
                negative(&norm(t))
                    || matches!(
                        norm(t).as_str(),
                        "is" | "are"
                            | "was"
                            | "were"
                            | "has"
                            | "have"
                            | "had"
                            | "does"
                            | "do"
                            | "did"
                            | "will"
                            | "would"
                            | "can"
                            | "could"
                            | "should"
                    )
            });
        let previous = at
            .checked_sub(1)
            .and_then(|i| words.get(i))
            .map(norm)
            .unwrap_or_default();
        let addressed = matches!(
            previous.as_str(),
            "email" | "contact" | "notify" | "named" | "met" | "call" | "ask" | "give"
        ) || (matches!(previous.as_str(), "to" | "from")
            && words[at.saturating_sub(7)..at].iter().any(|t| {
                matches!(
                    norm(t).as_str(),
                    "send"
                        | "sent"
                        | "email"
                        | "forward"
                        | "give"
                        | "share"
                        | "assign"
                        | "message"
                        | "reply"
                )
            }));
        if acronym || mixed_case || identifier || subject || addressed {
            result.insert(key);
            if addressed && !token.ends_sentence() {
                for next in words.iter().skip(at + 1).take(2) {
                    let next_key = norm(next);
                    if next
                        .word
                        .chars()
                        .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
                        && !helper(&next_key)
                        && !negative(&next_key)
                        && !matches!(
                            next_key.as_str(),
                            "and"
                                | "or"
                                | "but"
                                | "for"
                                | "from"
                                | "with"
                                | "about"
                                | "before"
                                | "after"
                                | "at"
                                | "on"
                                | "in"
                                | "by"
                                | "when"
                                | "who"
                                | "which"
                                | "that"
                                | "this"
                        )
                        && facts::calendar(&next_key).is_none()
                    {
                        result.insert(next_key);
                    } else {
                        break;
                    }
                    if next.ends_sentence() {
                        break;
                    }
                }
            }
        }
    }
    result
}

pub(super) fn rejection(original: &str, rewrite: &str) -> Option<RewriteRejection> {
    let quoted = |text: &str| {
        let mut spans = Vec::new();
        let mut opened = None;
        for (at, ch) in text.char_indices() {
            if let Some((start, close)) = opened {
                if ch == close {
                    spans.push(
                        text[start..at]
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                    opened = None;
                }
            } else if ch == '"' || ch == '\u{201c}' {
                opened = Some((at + ch.len_utf8(), if ch == '"' { '"' } else { '\u{201d}' }));
            }
        }
        spans
    };
    if quoted(original) != quoted(rewrite) {
        return Some(RewriteRejection::QuotedText);
    }
    let before = tokens(original);
    let after = tokens(rewrite);
    // Names may change capitalization, but paths, addresses and filenames
    // must retain their literal spelling, including case.
    let identifiers = |words: &[Token]| {
        words
            .iter()
            .enumerate()
            .filter_map(|(at, token)| {
                if (at.saturating_sub(2)..=at).any(|i| {
                    facts::quantity(words, i)
                        .is_some_and(|q| q.end > at && q.unit.as_deref() != Some("identifier"))
                }) {
                    return None;
                }
                let dotted_file = token
                    .word
                    .rsplit_once('.')
                    .is_some_and(|(name, extension)| {
                        name.chars().any(char::is_alphabetic)
                            && !extension.is_empty()
                            && extension.len() <= 8
                            && extension.chars().all(char::is_alphabetic)
                    });
                (token.word.contains(['@', '/', '\\', '_']) || dotted_file)
                    .then(|| token.word.clone())
            })
            .collect::<Vec<_>>()
    };
    if identifiers(&before) != identifiers(&after) {
        return Some(RewriteRejection::Entity);
    }
    if facts::quantities(&before) != facts::quantities(&after) {
        return Some(RewriteRejection::Quantity);
    }
    let bullets = |text: &str| {
        text.lines()
            .filter(|line| {
                let line = line.trim_start();
                ["- ", "* ", "\u{2022} "]
                    .iter()
                    .any(|prefix| line.starts_with(prefix))
            })
            .count()
    };
    let items = bullets(original);
    if items > 0 && bullets(rewrite) != items {
        return Some(RewriteRejection::ListStructure);
    }
    if items > 0 {
        // Equal item counts do not prove equal content. Keep facts, scope and
        // the substance of every item associated with that same item.
        for (old, new) in list_items(original).iter().zip(list_items(rewrite)) {
            if rejection(old, &new).is_some() || loses_list_content(old, &new) {
                return Some(RewriteRejection::ListStructure);
            }
        }
    }
    if calendar(&before) != calendar(&after) {
        return Some(RewriteRejection::Calendar);
    }
    if negations(&before) != negations(&after) {
        return Some(RewriteRejection::Negation);
    }
    if bare_not(&after) > bare_not(&before) {
        return Some(RewriteRejection::MissingAuxiliary);
    }
    if direction(&before) != direction(&after) {
        return Some(RewriteRejection::Action);
    }
    let before_words: BTreeSet<_> = before.iter().map(norm).collect();
    let after_words: BTreeSet<_> = after.iter().map(norm).collect();
    // Capitalization can change during cleanup without introducing or losing
    // a name. Compare protected names against all words on the opposite side.
    if !entities(&before).is_subset(&after_words) || !entities(&after).is_subset(&before_words) {
        return Some(RewriteRejection::Entity);
    }
    let missing_terminal_punctuation = before.last().is_some_and(|t| !t.ends_sentence());
    let asked = missing_terminal_punctuation
        && before
            .iter()
            .enumerate()
            .any(|(at, _)| question_at(&before, at));
    if (asked || original.contains('?')) && !rewrite.contains('?') {
        return Some(RewriteRejection::Question);
    }
    if original.matches('?').count() > rewrite.matches('?').count() {
        return Some(RewriteRejection::Question);
    }
    // A rewrite cannot append an answer to a question that ended the input.
    if let Some((_, tail)) = original.rsplit_once('?') {
        if !tail.chars().any(char::is_alphanumeric) {
            let Some((_, tail)) = rewrite.rsplit_once('?') else {
                return Some(RewriteRejection::Question);
            };
            if tail.chars().any(char::is_alphanumeric) {
                return Some(RewriteRejection::AppendedAnswer);
            }
        }
    }
    None
}
