//! Bounded phrase replacement. Only consume an old span that ends at a cue;
//! unrelated clauses and the following instruction remain untouched.
use super::{closes_phrase, facts, norm, Token, DETERMINERS, FUNCTION_WORDS};

fn capitalized(token: &Token) -> bool {
    token.word.chars().next().is_some_and(char::is_uppercase)
}

fn day_span(tokens: &[Token], at: usize) -> Option<(usize, bool)> {
    let day = tokens.get(at)?;
    if day.verbatim {
        return None;
    }
    facts::weekday(&norm(day))?;
    if day.ends_sentence() {
        return Some((at + 1, false));
    }
    let with_at = tokens
        .get(at + 1)
        .is_some_and(|t| !t.verbatim && norm(t) == "at" && t.trail.is_empty());
    let time_at = if with_at { at + 2 } else { at + 1 };
    if let Some(time) = facts::clock(tokens, time_at) {
        Some((time.end, true))
    } else if with_at {
        // A weekday plus explicit "at" supplies time context even when
        // recognition omitted AM/PM. Do not invent a meridiem or minutes.
        let first = tokens.get(time_at)?;
        if first.verbatim {
            return None;
        }
        let hour = facts::spoken_number(tokens, time_at)
            .or_else(|| first.word.parse::<u64>().ok().map(|n| (n, time_at + 1)));
        match hour.filter(|(hour, _)| (1..=12).contains(hour)) {
            Some((_, end)) => Some((end, true)),
            None => Some((at + 1, false)),
        }
    } else {
        Some((at + 1, false))
    }
}

fn complete_scoped_span(tokens: &[Token], end: usize) -> bool {
    if tokens[end - 1].ends_sentence() || end == tokens.len() {
        return true;
    }
    // Recognition may omit the stop before an independent next instruction.
    // A predicate such as "is unavailable" belongs to this phrase instead.
    let next = norm(&tokens[end]);
    matches!(next.as_str(), "keep" | "save" | "please")
        || (matches!(next.as_str(), "we" | "i" | "they" | "you")
            && tokens.get(end + 1).is_some_and(|t| {
                matches!(
                    norm(t).as_str(),
                    "completed" | "recorded" | "counted" | "finished"
                )
            }))
}

fn scoped_resume(tokens: &[Token], out: &mut Vec<Token>, end: usize) -> usize {
    if !tokens[end - 1].ends_sentence() && out.last().is_some_and(Token::ends_sentence) {
        if let Some(next) = tokens.get(end) {
            let mut next = next.clone();
            super::capitalize(&mut next);
            out.push(next);
            return end + 1;
        }
    }
    end
}

/// Recognize a broken-off numeral only as a span to discard for an explicit
/// correction. Its value is never guessed. Require a scale, a known fragment,
/// and otherwise only cardinal vocabulary (no intervening ordinary words).
fn interrupted_cardinal(tokens: &[Token]) -> bool {
    if !(3..=12).contains(&tokens.len()) {
        return false;
    }
    let mut scale = false;
    let mut fragment = false;
    for (at, token) in tokens.iter().enumerate() {
        if token.verbatim || !token.lead.is_empty() || !token.trail.is_empty() {
            return false;
        }
        let word = norm(token);
        if matches!(word.as_str(), "hundred" | "thousand" | "million") {
            if at == 0 {
                return false;
            }
            scale = true;
        } else if word == "and" {
            if at == 0 || at + 1 == tokens.len() {
                return false;
            }
        } else if facts::spoken_number(std::slice::from_ref(token), 0).is_some() {
            // Complete number word.
        } else if at > 0
            && word.len() >= 3
            && [
                "three",
                "four",
                "five",
                "seven",
                "eight",
                "nine",
                "eleven",
                "twelve",
                "thirteen",
                "fourteen",
                "fifteen",
                "sixteen",
                "seventeen",
                "eighteen",
                "nineteen",
                "twenty",
                "thirty",
                "forty",
                "fifty",
                "sixty",
                "seventy",
                "eighty",
                "ninety",
            ]
            .iter()
            .any(|number| number.starts_with(&word) && number.len() > word.len())
        {
            fragment = true;
        } else {
            return false;
        }
    }
    scale && fragment
}

/// Short apologies/"actually" can correct the immediately preceding typed
/// value despite ASR sentence punctuation. Require complete matching types;
/// arbitrary names, incomplete numbers and ordinary discourse stay intact.
pub(super) fn scoped_after_sentence(
    tokens: &[Token],
    out: &mut Vec<Token>,
    start: usize,
) -> Option<usize> {
    let first = tokens.get(start)?;
    if first.verbatim || out.last().is_none_or(|t| t.trail != ".") {
        return None;
    }
    let prefix = norm(first);
    let day_at = start
        + usize::from(matches!(
            prefix.as_str(),
            "on" | "by" | "before" | "after" | "at"
        ));
    if let Some((end, true)) = day_span(tokens, day_at) {
        if complete_scoped_span(tokens, end)
            && (out.len().saturating_sub(9)..out.len())
                .any(|at| day_span(out, at).is_some_and(|(end, time)| time && end == out.len()))
        {
            let end = apply(tokens, out, start, false)?;
            return Some(scoped_resume(tokens, out, end));
        }
    }
    let new_at = start + usize::from(prefix == "at");
    let new = facts::quantity(tokens, new_at)?;
    let clause_start = out[..out.len() - 1]
        .iter()
        .rposition(Token::ends_sentence)
        .map_or(0, |at| at + 1);
    let (begin, old) = (clause_start..out.len())
        .find_map(|at| facts::quantity(out, at).map(|value| (at, value)))?;
    if old.end == out.len()
        && old.unit.is_some()
        && old.unit == new.unit
        && complete_scoped_span(tokens, new.end)
    {
        let end = apply(tokens, out, start, false)?;
        return Some(scoped_resume(tokens, out, end));
    }
    let old_noun_at = out.len() - 1;
    if old.unit.is_some()
        || new.unit.is_some()
        || (old.end != old_noun_at && !interrupted_cardinal(&out[begin..old_noun_at]))
    {
        return None;
    }
    let old_noun = &out[old_noun_at];
    let new_noun = tokens.get(new.end)?;
    if old_noun.verbatim
        || new_noun.verbatim
        || !old_noun.word.chars().all(char::is_alphabetic)
        || boundary(&norm(old_noun))
        || norm(old_noun) != norm(new_noun)
        || !complete_scoped_span(tokens, new.end + 1)
    {
        return None;
    }
    let mut replacement = tokens[start..=new.end].to_vec();
    carry_end(&out[begin..], &mut replacement);
    out.splice(begin.., replacement);
    Some(scoped_resume(tokens, out, new.end + 1))
}

/// "…is the login page. Sorry, the sign up page, not the dashboard." -> "…is
/// the sign up page, not the dashboard.": after a full stop, "sorry" and a
/// short noun phrase that ends on the same noun as the sentence before. The
/// new phrase replaces the old one at the end of that sentence.
///
/// Only a phrase of two to five plain words that opens with a determiner,
/// where the old phrase opens with the same determiner within the last six
/// words. "Sorry, the traffic was bad." ends on another word and stays.
pub(super) fn scoped_noun_phrase(tokens: &[Token], out: &mut Vec<Token>, start: usize) -> Option<usize> {
    let first = tokens.get(start)?;
    let opener = norm(first);
    if !DETERMINERS.contains(&opener.as_str()) || !out.last().is_some_and(|t| t.trail == "." || t.trail == "!") {
        return None;
    }
    let end = (start..tokens.len().min(start + 5)).find(|&at| closes_phrase(&tokens[at]))? + 1;
    let redo = &tokens[start..end];
    let plain = |t: &Token| !t.verbatim && t.lead.is_empty() && t.word.chars().all(char::is_alphabetic);
    if redo.len() < 2 || !redo.iter().all(plain) {
        return None;
    }
    let noun = norm(&redo[redo.len() - 1]);
    let old_end = out.len() - 1;
    if noun.len() < 3 || FUNCTION_WORDS.contains(&noun.as_str()) || norm(&out[old_end]) != noun {
        return None;
    }
    let at = (old_end.saturating_sub(5)..old_end).rev().find(|&at| {
        norm(&out[at]) == opener && out[at..].iter().all(plain) && !out[at..old_end].iter().any(closes_phrase)
    })?;
    let old: Vec<String> = out[at..].iter().map(norm).collect();
    let new: Vec<String> = redo.iter().map(norm).collect();
    if old == new {
        return None;
    }
    let mut replacement = redo.to_vec();
    if at == 0 || super::clause_boundary(&out[at - 1]) {
        super::capitalize(&mut replacement[0]);
    } else {
        replacement[0].word = out[at].word.clone();
    }
    out.splice(at.., replacement);
    Some(end)
}

fn boundary(word: &str) -> bool {
    matches!(
        word,
        "before"
            | "after"
            | "because"
            | "and"
            | "but"
            | "when"
            | "while"
            | "which"
            | "who"
            | "that"
            | "for"
            | "with"
            | "from"
            | "at"
            | "on"
            | "to"
            | "please"
            | "also"
            | "then"
            | "until"
            | "if"
            | "unless"
            | "including"
    )
}

fn skip_modifiers(tokens: &[Token], mut at: usize) -> usize {
    for _ in 0..3 {
        if tokens.get(at).is_some_and(|t| {
            !t.verbatim
                && !t.ends_sentence()
                && t.lead.is_empty()
                && (t.trail.is_empty() || t.trail == ",")
                && matches!(norm(t).as_str(), "actually" | "really" | "exactly")
        }) {
            at += 1;
        } else {
            break;
        }
    }
    at
}

fn replacement_start(tokens: &[Token], at: usize) -> Option<usize> {
    let bridges: &[&[&str]] = &[
        &["but", "it", "is"],
        &["but", "it's"],
        &["but", "it", "was"],
        &["it", "is"],
        &["it's"],
        &["it", "was"],
        &["that's"],
        &["but"],
    ];
    let length = bridges
        .iter()
        .find(|words| {
            words.iter().enumerate().all(|(i, word)| {
                tokens
                    .get(at + i)
                    .is_some_and(|t| !t.verbatim && norm(t) == *word)
            })
        })?
        .len();
    Some(skip_modifiers(tokens, at + length))
}

/// A denial plus an explicit replacement of the same recently stated value.
/// Keep both facts if the denied value, unit, recipient or noun does not match.
pub(super) fn contrast(tokens: &[Token], out: &mut Vec<Token>, start: usize) -> Option<usize> {
    let denied = skip_modifiers(tokens, start);
    quantity_contrast(tokens, out, denied).or_else(|| recipient_contrast(tokens, out, denied))
}

fn quantity_contrast(tokens: &[Token], out: &mut Vec<Token>, start: usize) -> Option<usize> {
    let denied = facts::quantity(tokens, start)?;
    let (begin, old) = (out.len().saturating_sub(16)..out.len()).find_map(|at| {
        let q = facts::quantity(out, at)?;
        (q.end == out.len()
            || (q.end + 1 == out.len()
                && out.last().is_some_and(|t| {
                    t.word.chars().all(char::is_alphabetic) && !boundary(&norm(t))
                })))
        .then_some((at, q))
    })?;
    if old.value != denied.value || (denied.unit.is_some() && old.unit != denied.unit) {
        return None;
    }
    let old_noun = (old.end < out.len()).then(|| out[old.end].clone());
    let mut bridge = denied.end;
    // The denial may repeat the same noun: "not 214 checks, it's 210 checks".
    if let Some(noun) = &old_noun {
        if tokens.get(bridge).is_some_and(|t| norm(t) == norm(noun)) {
            bridge += 1;
        }
    }
    // "No, not 214 inspections, 210 inspections": the denied quantity
    // and both counted nouns must match. A bare next number is not enough.
    let direct = replacement_start(tokens, bridge).is_none();
    let new_start = if direct {
        let noun = old_noun.as_ref()?;
        if bridge != denied.end + 1 || norm(&tokens[denied.end]) != norm(noun) {
            return None;
        }
        skip_modifiers(tokens, bridge)
    } else {
        replacement_start(tokens, bridge)?
    };
    let new = facts::quantity(tokens, new_start)?;
    if direct
        && tokens
            .get(new.end)
            .zip(old_noun.as_ref())
            .is_none_or(|(a, b)| norm(a) != norm(b))
    {
        return None;
    }
    if old.unit.as_deref() == Some("clock") && new.unit.as_deref() != Some("clock") {
        return None;
    }
    if old_noun.is_some() && new.unit.is_some() {
        return None;
    }
    let mut end = new.end;
    let mut replacement = tokens[new_start..end].to_vec();
    if let Some(mut noun) = old_noun {
        if tokens.get(end).is_some_and(|t| norm(t) == norm(&noun)) {
            replacement.push(tokens[end].clone());
            end += 1;
        } else {
            // A different stated noun could be an unrelated fact, not a redo.
            if tokens.get(end).is_some_and(|t| !boundary(&norm(t)))
                && !tokens[end - 1].ends_sentence()
            {
                return None;
            }
            if let Some(last) = replacement.last_mut() {
                if last.ends_sentence() {
                    noun.trail = std::mem::take(&mut last.trail);
                }
            }
            replacement.push(noun);
        }
    } else if new.unit.is_none() && old.unit.as_deref().is_some_and(|u| u != "identifier") {
        // Inherit a stated suffix unit, not the last digit of a currency
        // prefix or an identifier. Ambiguous attached units stay unchanged.
        if facts::unit(&norm(&out[old.end - 1])) != old.unit.as_deref() {
            return None;
        }
        let mut unit = out[old.end - 1].clone();
        if let Some(last) = replacement.last_mut() {
            if last.ends_sentence() {
                unit.trail = std::mem::take(&mut last.trail);
            }
        }
        replacement.push(unit);
    }
    finish_contrast(tokens, out, start, end, begin, replacement)
}

fn recipient_contrast(tokens: &[Token], out: &mut Vec<Token>, start: usize) -> Option<usize> {
    let role = |word: &str| {
        matches!(
            word,
            "to" | "from" | "with" | "contact" | "email" | "notify" | "call"
        )
    };
    let name_word = |t: &Token| {
        !t.verbatim
            && t.word
                .chars()
                .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
            && !boundary(&norm(t))
            && !matches!(
                norm(t).as_str(),
                "i" | "we"
                    | "you"
                    | "he"
                    | "she"
                    | "they"
                    | "it"
                    | "the"
                    | "a"
                    | "an"
                    | "nobody"
                    | "somebody"
                    | "someone"
                    | "anyone"
                    | "everyone"
                    | "none"
                    | "don't"
                    | "dont"
                    | "not"
                    | "is"
                    | "are"
                    | "has"
                    | "have"
                    | "will"
            )
    };
    let begin = (1..out.len()).rev().find(|&at| {
        role(&norm(&out[at - 1]))
            && out.len() - at <= 4
            && out[at..].iter().all(name_word)
            && out[at..out.len() - 1].iter().all(|t| !closes_phrase(t))
    })?;
    let role_word = norm(&out[begin - 1]);
    let denied_start = start + usize::from(tokens.get(start).is_some_and(|t| norm(t) == role_word));
    let denied_end = denied_start + out.len() - begin;
    let denied = tokens.get(denied_start..denied_end)?;
    if !denied
        .iter()
        .zip(&out[begin..])
        .all(|(a, b)| name_word(a) && norm(a) == norm(b))
    {
        return None;
    }
    let mut new_start = replacement_start(tokens, denied_end)?;
    if tokens.get(new_start).is_some_and(|t| norm(t) == role_word) {
        new_start += 1;
    }
    if !tokens.get(new_start).is_some_and(name_word) {
        return None;
    }
    let mut end = new_start + 1;
    while end < tokens.len()
        && end - new_start < 4
        && !closes_phrase(&tokens[end - 1])
        && name_word(&tokens[end])
        && facts::calendar(&norm(&tokens[end])).is_none()
        && (capitalized(&tokens[end])
            || matches!(
                norm(&tokens[end]).as_str(),
                "de" | "van" | "von" | "da" | "di" | "del"
            ))
    {
        end += 1;
    }
    let mut replacement = tokens[new_start..end].to_vec();
    if replacement[0].word.chars().all(char::is_lowercase) {
        super::capitalize(&mut replacement[0]);
    }
    finish_contrast(tokens, out, start, end, begin, replacement)
}

fn finish_contrast(
    tokens: &[Token],
    out: &mut Vec<Token>,
    start: usize,
    end: usize,
    begin: usize,
    mut replacement: Vec<Token>,
) -> Option<usize> {
    let quoted = |t: &Token| {
        t.verbatim
            || t.lead.contains(['"', '\'', '\u{201c}', '\u{2018}'])
            || t.trail.contains(['"', '\'', '\u{201d}', '\u{2019}'])
    };
    if out[begin..].iter().any(quoted) || tokens[start..end].iter().any(quoted) {
        return None;
    }
    let continues =
        tokens.get(end).is_some_and(|t| !capitalized(t)) && !tokens[end - 1].ends_sentence();
    if continues {
        if let Some(last) = replacement.last_mut().filter(|t| t.ends_sentence()) {
            last.trail.clear();
        }
    } else {
        carry_end(&out[begin..], &mut replacement);
    }
    out.splice(begin.., replacement);
    Some(end)
}

pub(super) fn apply(
    tokens: &[Token],
    out: &mut Vec<Token>,
    start: usize,
    names: bool,
) -> Option<usize> {
    let first = tokens.get(start)?;
    if first.verbatim || out.is_empty() {
        return None;
    }
    let prefix = norm(first);
    let new_day = if matches!(prefix.as_str(), "on" | "by" | "before" | "after" | "at") {
        start + 1
    } else {
        start
    };
    if let Some((end, new_time)) = day_span(tokens, new_day) {
        let old = (out.len().saturating_sub(9)..out.len())
            .rev()
            .find_map(|at| {
                day_span(out, at)
                    .filter(|(end, _)| *end == out.len())
                    .map(|span| (at, span))
            });
        if let Some((old_day, (old_end, old_time))) = old {
            let mut begin = old_day;
            if new_day != start
                && prefix != "at"
                && begin > 0
                && matches!(
                    norm(&out[begin - 1]).as_str(),
                    "on" | "by" | "before" | "after"
                )
            {
                begin -= 1;
            }
            let remove_end = if new_time || !old_time {
                old_end
            } else {
                old_day + 1
            };
            // "At Friday at 10" is an ASR preposition error. Retain the old
            // deadline relation (e.g. before), and omit only that extra "at".
            let replacement_start = if prefix == "at" { new_day } else { start };
            let mut replacement = tokens[replacement_start..end].to_vec();
            // Calendar names are proper nouns even in unpunctuated ASR text.
            super::capitalize(&mut replacement[new_day - replacement_start]);
            if remove_end < old_end {
                if let Some(last) = replacement.last_mut() {
                    if last.ends_sentence() {
                        let trail = std::mem::take(&mut last.trail);
                        if out[old_end - 1].trail.is_empty() {
                            out[old_end - 1].trail = trail;
                        }
                    }
                }
            }
            carry_end(&out[begin..remove_end], &mut replacement);
            out.splice(begin..remove_end, replacement);
            return Some(end);
        }
    }

    let new_at = if prefix == "at" { start + 1 } else { start };
    if let Some(new) = facts::quantity(tokens, new_at) {
        // Prefer the complete spoken number to its trailing component:
        // "two hundred and fourteen" must not match only "fourteen".
        let old = (out.len().saturating_sub(16)..out.len()).find_map(|at| {
            facts::quantity(out, at)
                .filter(|q| q.end == out.len())
                .map(|q| (at, q))
        });
        if let Some((mut begin, old)) = old {
            // A bare replacement value may inherit a quantity's stated unit,
            // but cannot silently discard a clock's AM/PM or minutes.
            if old.unit.as_deref() == Some("clock") && new.unit.as_deref() != Some("clock") {
                return None;
            }
            let mut replacement = tokens[start..new.end].to_vec();
            if new_at != start && begin > 0 && norm(&out[begin - 1]) == "at" {
                begin -= 1;
            }
            if new.unit.is_none() && old.unit.is_some() && old.end > begin + 1 {
                replacement.push(out[old.end - 1].clone());
            }
            carry_end(&out[begin..], &mut replacement);
            out.splice(begin.., replacement);
            return Some(new.end);
        }
        // An explicit correction may repeat a counted noun not in the unit
        // vocabulary. Require the same noun (or a small spelling repair),
        // rather than swallowing the beginning of an unrelated clause.
        if names && new.unit.is_none() {
            if let (Some(old_noun), Some(new_noun)) = (out.last(), tokens.get(new.end)) {
                let old_key = norm(old_noun);
                let new_key = norm(new_noun);
                let noun = |t: &Token| {
                    !t.verbatim
                        && t.word.chars().all(char::is_alphabetic)
                        && t.word.len() >= 3
                        && !boundary(&norm(t))
                };
                if noun(old_noun)
                    && noun(new_noun)
                    && (old_key == new_key || super::within_edits(&old_key, &new_key, 1))
                {
                    let old_at = (out.len().saturating_sub(16)..out.len() - 1).find(|&at| {
                        facts::quantity(out, at)
                            .is_some_and(|q| q.end == out.len() - 1 && q.unit.is_none())
                    });
                    if let Some(begin) = old_at {
                        let mut replacement = tokens[start..=new.end].to_vec();
                        carry_end(&out[begin..], &mut replacement);
                        out.splice(begin.., replacement);
                        return Some(new.end + 1);
                    }
                }
            }
        }
    }

    if !names {
        return None;
    }
    // An explicitly addressed name: "to Leonie, no sorry, to Darius".
    // Do not infer names from arbitrary words or cross a sentence boundary.
    let mut old_start = out.len() - 1;
    while old_start > 0
        && out.len() - old_start < 4
        && capitalized(&out[old_start])
        && capitalized(&out[old_start - 1])
        && !closes_phrase(&out[old_start - 1])
    {
        old_start -= 1;
    }
    if old_start > 0
        && matches!(
            norm(&out[old_start - 1]).as_str(),
            "to" | "from" | "with" | "contact" | "email" | "notify" | "call"
        )
    {
        let repeat_prefix = prefix == norm(&out[old_start - 1]);
        let name_at = start + usize::from(repeat_prefix);
        if let Some(name) = tokens.get(name_at) {
            if !name.verbatim
                && name.word.chars().all(char::is_alphabetic)
                && !boundary(&norm(name))
                && !matches!(
                    norm(name).as_str(),
                    "i" | "we" | "you" | "it" | "the" | "a" | "an"
                )
            {
                let mut end = name_at + 1;
                while end < tokens.len()
                    && end - name_at < 3
                    && !closes_phrase(&tokens[end - 1])
                    && capitalized(&tokens[end])
                    && !boundary(&norm(&tokens[end]))
                {
                    end += 1;
                }
                if repeat_prefix {
                    old_start -= 1;
                }
                let mut replacement = tokens[start..end].to_vec();
                carry_end(&out[old_start..], &mut replacement);
                out.splice(old_start.., replacement);
                return Some(end);
            }
        }
    }
    None
}

fn carry_end(old: &[Token], replacement: &mut [Token]) {
    if let (Some(old), Some(new)) = (old.last(), replacement.last_mut()) {
        if old.ends_sentence() && new.trail.is_empty() {
            new.trail = old.trail.clone();
        }
    }
}

/// Resolve only a full repetition of the immediately preceding clause with
/// one typed value changed. Never authorize a model to drop arbitrary facts.
pub(super) fn verified_restate(
    tokens: &[Token],
    out: &mut Vec<Token>,
    start: usize,
) -> Option<usize> {
    let limit = tokens
        .get(start..)?
        .iter()
        .take(32)
        .position(Token::ends_sentence)
        .map_or_else(|| (tokens.len() - start).min(32), |at| at + 1);
    for length in 3..=limit {
        if let Some(end) = verified_restate_at(tokens, out, start, length) {
            return Some(end);
        }
    }
    None
}

fn verified_restate_at(
    tokens: &[Token],
    out: &mut Vec<Token>,
    start: usize,
    length: usize,
) -> Option<usize> {
    if out.is_empty() || start >= tokens.len() {
        return None;
    }
    let begin = out[..out.len() - 1]
        .iter()
        .rposition(Token::ends_sentence)
        .map_or(0, |i| i + 1);
    let old = &out[begin..];
    if !(3..=32).contains(&old.len()) || !(3..=32).contains(&length) {
        return None;
    }
    let redo = &tokens[start..start + length];
    let quoted = |t: &Token| {
        t.verbatim
            || t.lead.contains(['"', '\'', '\u{201c}', '\u{2018}'])
            || t.trail.contains(['"', '\'', '\u{201d}', '\u{2019}'])
    };
    if old.iter().chain(redo).any(quoted) {
        return None;
    }
    let prefix = old
        .iter()
        .zip(redo)
        .take_while(|(a, b)| norm(a) == norm(b))
        .count();
    if prefix == 0 || prefix == old.len() || prefix == redo.len() {
        return None;
    }
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(redo[prefix..].iter().rev())
        .take_while(|(a, b)| norm(a) == norm(b))
        .count();
    let old_end = old.len() - suffix;
    let new_end = redo.len() - suffix;
    let quantity = facts::quantity(old, prefix)
        .zip(facts::quantity(redo, prefix))
        .is_some_and(|(a, b)| {
            a.end >= old_end
                && b.end >= new_end
                && a.end - old_end == b.end - new_end
                && a.unit == b.unit
        });
    let role = |s: &str| {
        matches!(
            s,
            "to" | "from" | "with" | "give" | "ask" | "notify" | "call" | "email"
        )
    };
    let name_start = (prefix.saturating_sub(3)..prefix)
        .rev()
        .find(|&at| role(&norm(&old[at])));
    let recipient = name_start.is_some_and(|at| {
        let at = at + 1;
        let name_word = |t: &Token| {
            t.word
                .chars()
                .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
                && !boundary(&norm(t))
                && !matches!(
                    norm(t).as_str(),
                    "i" | "we"
                        | "you"
                        | "he"
                        | "she"
                        | "they"
                        | "it"
                        | "her"
                        | "him"
                        | "them"
                        | "us"
                        | "me"
                        | "the"
                        | "a"
                        | "an"
                        | "not"
                        | "no"
                        | "nobody"
                        | "somebody"
                        | "someone"
                        | "anyone"
                        | "everyone"
                        | "none"
                        | "is"
                        | "are"
                        | "has"
                        | "have"
                        | "will"
                        | "don't"
                        | "dont"
                        | "cannot"
                        | "never"
                )
        };
        old_end > at
            && new_end > at
            && old_end - at <= 4
            && new_end - at <= 4
            && old[at..old_end]
                .iter()
                .chain(&redo[at..new_end])
                .all(name_word)
    });
    if !quantity && !recipient {
        return None;
    }
    let mut replacement = redo.to_vec();
    if let Some(first) = replacement.first_mut() {
        super::capitalize(first);
    }
    out.truncate(begin);
    out.extend(replacement);
    Some(start + length)
}

/// "I meant B, not A" must deny the complete immediately addressed A.
/// Consume that denial together with the replacement; retain unrelated facts.
pub(super) fn reverse_recipient(
    tokens: &[Token],
    out: &mut Vec<Token>,
    start: usize,
) -> Option<usize> {
    let role = |word: &str| {
        matches!(
            word,
            "to" | "from" | "with" | "contact" | "email" | "notify" | "call" | "give" | "ask"
        )
    };
    let name = |t: &Token| {
        !t.verbatim
            && !t.word.is_empty()
            && t.word
                .chars()
                .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
            && !boundary(&norm(t))
            && !matches!(
                norm(t).as_str(),
                "i" | "we"
                    | "you"
                    | "he"
                    | "she"
                    | "they"
                    | "it"
                    | "the"
                    | "a"
                    | "an"
                    | "not"
                    | "no"
                    | "is"
                    | "are"
                    | "will"
                    | "cannot"
                    | "never"
            )
    };
    let begin = (1..out.len()).rev().find(|&at| {
        role(&norm(&out[at - 1]))
            && out.len() - at <= 4
            && out[at..].iter().all(name)
            && out[at..out.len() - 1].iter().all(|t| !closes_phrase(t))
    })?;
    let new_start = start
        + usize::from(
            tokens
                .get(start)
                .is_some_and(|t| norm(t) == norm(&out[begin - 1])),
        );
    let not_at =
        (new_start + 1..tokens.len().min(new_start + 5)).find(|&at| norm(&tokens[at]) == "not")?;
    if !tokens[new_start..not_at].iter().all(name)
        || tokens[new_start..not_at].iter().any(Token::ends_sentence)
    {
        return None;
    }
    let denied_start = not_at
        + 1
        + usize::from(
            tokens
                .get(not_at + 1)
                .is_some_and(|t| norm(t) == norm(&out[begin - 1])),
        );
    let end = denied_start + out.len() - begin;
    let denied = tokens.get(denied_start..end)?;
    if !denied
        .iter()
        .zip(&out[begin..])
        .all(|(a, b)| norm(a) == norm(b))
    {
        return None;
    }
    let quoted = |t: &Token| {
        t.verbatim
            || t.lead.contains(['"', '\'', '\u{201c}', '\u{2018}'])
            || t.trail.contains(['"', '\'', '\u{201d}', '\u{2019}'])
    };
    if out[begin..].iter().chain(&tokens[start..end]).any(quoted) {
        return None;
    }
    let followup = tokens
        .get(end)
        .is_some_and(|t| matches!(norm(t).as_str(), "keep" | "save"))
        && (facts::quantity(tokens, end + 1).is_some()
            || tokens.get(end + 1).is_some_and(|t| {
                matches!(norm(t).as_str(), "the" | "both" | "these" | "our" | "my")
            }));
    if tokens.get(end).is_some_and(|t| !boundary(&norm(t)))
        && !denied.last()?.ends_sentence()
        && !followup
    {
        return None;
    }
    let mut replacement = tokens[new_start..not_at].to_vec();
    replacement.last_mut()?.trail = denied.last()?.trail.clone();
    if replacement
        .first()
        .is_some_and(|t| t.word.chars().all(char::is_lowercase))
    {
        super::capitalize(&mut replacement[0]);
    }
    finish_contrast(tokens, out, start, end, begin, replacement)
}

pub(super) fn reverse_denial(tokens: &[Token], start: usize) -> bool {
    let Some(not_at) =
        (start + 1..tokens.len().min(start + 6)).find(|&at| norm(&tokens[at]) == "not")
    else {
        return false;
    };
    tokens[start..not_at].iter().all(|t| {
        !t.ends_sentence()
            && !t.verbatim
            && !matches!(
                norm(t).as_str(),
                "i" | "we"
                    | "you"
                    | "he"
                    | "she"
                    | "they"
                    | "it"
                    | "is"
                    | "are"
                    | "do"
                    | "does"
                    | "will"
                    | "can"
            )
    })
}

/// Match a repeated noun/preposition, otherwise replace one word. Never take
/// the first word of the following clause just to fill a fixed two-word span.
pub(super) fn replacement_words(tokens: &[Token], start: usize, out: &[Token]) -> usize {
    let Some(last) = out.last() else {
        return 1;
    };
    for count in 2..=4usize {
        let Some(span) = tokens.get(start..start + count) else {
            break;
        };
        if span[..count - 1]
            .iter()
            .any(|t| closes_phrase(t) || t.verbatim)
        {
            break;
        }
        if count > 2 && boundary(&norm(&span[count - 2])) {
            break;
        }
        if norm(&span[count - 1]) == norm(last) && out.len() >= count {
            return count;
        }
    }
    if let (Some(next), Some(previous)) = (
        tokens.get(start),
        out.len().checked_sub(2).and_then(|i| out.get(i)),
    ) {
        if matches!(
            norm(next).as_str(),
            "to" | "at" | "in" | "on" | "for" | "from" | "with"
        ) && norm(next) == norm(previous)
        {
            return 2;
        }
    }
    1
}
