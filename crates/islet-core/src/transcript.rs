//! Cleaning up recognised speech before it is typed.
//!
//! Parakeet already returns punctuated, capitalised text, so this stage only
//! does what the model cannot: drop filler words and stutters, turn spoken commands into
//! punctuation, and apply the user's own spellings and snippets. It works on
//! words with their attached punctuation, so "Um, so" and "claude code." are
//! handled without regular expressions.

use crate::config::{DictationConfig, Replacement};

/// What to clean, taken from the user's settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextOptions {
    pub filler_removal: bool,
    pub spoken_punctuation: bool,
    pub dictionary: Vec<Replacement>,
    pub snippets: Vec<Replacement>,
}

impl TextOptions {
    #[must_use]
    pub fn from_config(config: &DictationConfig) -> Self {
        Self {
            filler_removal: config.filler_removal,
            spoken_punctuation: config.spoken_punctuation,
            dictionary: config.dictionary.clone(),
            snippets: config.snippets.clone(),
        }
    }
}

/// A word with the punctuation stuck to its ends: `("(", "hello", ",")`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Token {
    lead: String,
    word: String,
    trail: String,
    /// Replacement text that must be emitted verbatim (not re-capitalised).
    verbatim: bool,
}

impl Token {
    fn parse(raw: &str) -> Self {
        if !raw.chars().any(char::is_alphanumeric) {
            // A dash or ellipsis standing alone is a word of its own, spaced
            // like any other (bare marks are only ever made by the rules).
            return Self {
                lead: String::new(),
                word: raw.to_owned(),
                trail: String::new(),
                verbatim: true,
            };
        }
        let start = raw.find(|c: char| c.is_alphanumeric()).unwrap_or(raw.len());
        let end = raw
            .rfind(|c: char| c.is_alphanumeric())
            .map_or(start, |index| {
                index + raw[index..].chars().next().map_or(1, char::len_utf8)
            });
        // Keep inner apostrophes and hyphens ("don't", "well-known") in the word.
        Self {
            lead: raw[..start].to_owned(),
            word: raw[start..end].to_owned(),
            trail: raw[end..].to_owned(),
            verbatim: false,
        }
    }

    fn key(&self) -> String {
        self.word.to_lowercase()
    }

    fn ends_sentence(&self) -> bool {
        self.trail.contains(['.', '?', '!'])
    }

    fn render(&self) -> String {
        format!("{}{}{}", self.lead, self.word, self.trail)
    }
}

#[derive(Clone, Debug)]
enum Rule {
    /// Replace the words, keeping surrounding punctuation.
    Text(String),
    /// Replace the words and swallow trailing sentence punctuation.
    Snippet(String),
    Punct(&'static str),
    /// A line break; the next word starts a sentence.
    Break(&'static str),
}

struct Phrase {
    words: Vec<String>,
    rule: Rule,
}

fn phrase_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|part| Token::parse(part).key())
        .filter(|word| !word.is_empty())
        .collect()
}

fn build_phrases(options: &TextOptions) -> Vec<Phrase> {
    let mut phrases = Vec::new();
    for entry in &options.snippets {
        let words = phrase_words(&entry.from);
        if !words.is_empty() {
            phrases.push(Phrase {
                words,
                rule: Rule::Snippet(entry.to.clone()),
            });
        }
    }
    if options.spoken_punctuation {
        for (spoken, rule) in [
            ("new paragraph", Rule::Break("\n\n")),
            ("new line", Rule::Break("\n")),
            ("bullet point", Rule::Break("\n- ")),
            ("new bullet", Rule::Break("\n- ")),
            ("next bullet", Rule::Break("\n- ")),
            ("full stop", Rule::Punct(".")),
            ("question mark", Rule::Punct("?")),
            ("exclamation mark", Rule::Punct("!")),
            ("exclamation point", Rule::Punct("!")),
            ("comma", Rule::Punct(",")),
        ] {
            phrases.push(Phrase {
                words: phrase_words(spoken),
                rule,
            });
        }
    }
    for entry in &options.dictionary {
        let words = phrase_words(&entry.from);
        if !words.is_empty() {
            phrases.push(Phrase {
                words,
                rule: Rule::Text(entry.to.clone()),
            });
        }
    }
    // Longest phrase first, so "new paragraph" beats a dictionary "new".
    phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.words.len()));
    phrases
}

/// Applies the phrase rules left to right.
fn apply_phrases(tokens: &[Token], phrases: &[Phrase]) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    'next: while index < tokens.len() {
        for phrase in phrases {
            let end = index + phrase.words.len();
            if end > tokens.len() {
                continue;
            }
            let span = &tokens[index..end];
            if span
                .iter()
                .zip(&phrase.words)
                .all(|(token, word)| &token.key() == word)
            {
                let first = &span[0];
                let last = &span[span.len() - 1];
                match &phrase.rule {
                    Rule::Text(text) => out.push(Token {
                        lead: first.lead.clone(),
                        word: text.clone(),
                        trail: last.trail.clone(),
                        verbatim: true,
                    }),
                    Rule::Snippet(text) => out.push(Token {
                        lead: first.lead.clone(),
                        word: text.clone(),
                        trail: last
                            .trail
                            .trim_start_matches(['.', ',', '?', '!'])
                            .to_owned(),
                        verbatim: true,
                    }),
                    Rule::Punct(mark) => {
                        // Attach to the previous word, replacing its own
                        // trailing mark ("Hello, comma" -> "Hello,").
                        if let Some(previous) = out.last_mut() {
                            previous.trail = (*mark).to_owned();
                        } else {
                            out.push(Token {
                                lead: String::new(),
                                word: String::new(),
                                trail: (*mark).to_owned(),
                                verbatim: true,
                            });
                        }
                    }
                    Rule::Break(text) => out.push(Token {
                        lead: String::new(),
                        word: String::new(),
                        trail: (*text).to_owned(),
                        verbatim: true,
                    }),
                }
                index = end;
                continue 'next;
            }
        }
        out.push(tokens[index].clone());
        index += 1;
    }
    out
}

const FILLERS: &[&str] = &[
    "um", "umm", "uh", "uhh", "uhhh", "uhm", "er", "erm", "ehm", "hmm", "mhm", "mmm",
];

/// Padding that adds nothing when it follows a linking word with no
/// punctuation: "what I want is basically let's say a car".
const HEDGES: &[&[&str]] = &[&["basically"], &["let's", "say"], &["kind", "of", "like"]];

/// Words a hedge can follow and be dropped.
const HEDGE_LEAD_INS: &[&str] = &[
    "is", "are", "was", "were", "so", "and", "that", "but", "to", "be", "because", "it's",
];

/// Lowercased word with typographic apostrophes straightened.
fn norm(token: &Token) -> String {
    token.key().replace('\u{2019}', "'")
}

fn hedge_at(tokens: &[Token], index: usize) -> usize {
    HEDGES
        .iter()
        .find(|words| {
            words.iter().enumerate().all(|(offset, word)| {
                tokens.get(index + offset).is_some_and(|token| {
                    !token.verbatim && norm(token) == *word && token.lead.is_empty()
                        && (offset + 1 == words.len() || token.trail.is_empty())
                })
            })
        })
        .map_or(0, |words| words.len())
}

fn is_break(token: &Token) -> bool {
    token.word.is_empty() && token.trail.starts_with('\n')
}

/// Drops filler words and a clause-initial "you know".
fn remove_fillers(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let at_clause_start = out
            .last()
            .is_none_or(|previous| previous.ends_sentence() || is_break(previous));

        // "oh" is a word too ("Oh, nice."), so it only counts as a filler when
        // it is dropped into the middle of a clause with no punctuation.
        let stray_oh = !at_clause_start
            && !token.verbatim
            && token.key() == "oh"
            && token.trail.is_empty()
            && tokens.get(index + 1).is_some_and(|next| !next.word.is_empty());
        let hedge = hedge_at(&tokens, index);
        if hedge > 0
            && out.last().is_some_and(|previous| {
                previous.trail.is_empty() && HEDGE_LEAD_INS.contains(&norm(previous).as_str())
            })
            && tokens[index + hedge - 1].trail.is_empty()
            && tokens.get(index + hedge).is_some_and(|next| !next.word.is_empty())
        {
            index += hedge;
            continue;
        }
        let is_filler =
            !token.verbatim && (FILLERS.contains(&token.key().as_str()) || stray_oh);
        let is_you_know = at_clause_start
            && !token.verbatim
            && token.key() == "you"
            && tokens
                .get(index + 1)
                .is_some_and(|next| next.key() == "know");

        if is_filler || is_you_know {
            let removed = if is_you_know {
                &tokens[index + 1]
            } else {
                token
            };
            // Keep what the filler carried: a sentence end moves to the
            // previous word ("... that, um." -> "... that.").
            if removed.ends_sentence() {
                if let Some(previous) = out.last_mut() {
                    if !previous.ends_sentence() {
                        previous.trail = removed.trail.clone();
                    }
                }
            }
            index += if is_you_know { 2 } else { 1 };
            // The word after a dropped sentence-initial filler starts the
            // sentence, so it gets its capital back.
            if at_clause_start {
                if let Some(next) = tokens.get(index) {
                    let mut next = next.clone();
                    capitalize(&mut next);
                    out.push(next);
                    index += 1;
                }
            }
            continue;
        }
        out.push(token.clone());
        index += 1;
    }
    out
}

/// Doubled words that people say on purpose ("very very good", "no no",
/// "what it is is"), so a repeat of them is not treated as a stutter.
const INTENDED_REPEATS: &[&str] = &[
    "very", "really", "no", "yes", "yeah", "bye", "ha", "haha", "had", "that", "is", "go", "knock",
    "tick", "blah", "far", "more", "many", "much", "again", "over",
];

/// Short real words that may start a longer word without being a stutter
/// ("a apple" is a mistake, but "in inside", "to today" are sentences).
const SHORT_WORDS: &[&str] = &[
    "a", "an", "i", "in", "on", "to", "at", "as", "is", "it", "of", "or", "by", "my", "me", "us",
    "up", "do", "go", "so", "no", "he", "we", "be", "am", "if", "oh", "ok", "and", "the", "for",
    "but", "not", "you", "all", "can", "her", "his", "our", "out", "one", "are", "was", "has",
    "had", "how", "who", "why", "may", "see", "any", "too", "let", "get", "got", "new", "now",
    "old", "own", "way", "day", "two", "its", "off", "say", "she", "use", "man", "far", "few",
    "yes", "yet", "car", "her", "him", "end", "run", "set", "put", "per", "pro", "pre", "re", "un",
    "ex", "co", "de", "non", "sub", "bi", "mid", "con", "dis", "mis", "out", "over",
];

/// Prefixes that make real hyphenated words ("re-read", "co-op", "pre-pay").
const HYPHEN_PREFIXES: &[&str] = &[
    "re", "co", "de", "un", "in", "ex", "bi", "pre", "pro", "non", "sub", "mid", "tri", "anti",
    "self", "semi", "post", "over", "out", "mis", "dis",
];

fn plain_word(token: &Token) -> bool {
    !token.verbatim && !token.word.is_empty() && token.word.chars().all(char::is_alphabetic)
}

/// "b-b-but" -> "but", "th-the" -> "the"; "re-read", "well-known" stay.
fn undo_hyphen_stutter(token: &mut Token) {
    if token.verbatim || !token.word.contains('-') {
        return;
    }
    let parts: Vec<&str> = token.word.split('-').collect();
    let Some((last, fragments)) = parts.split_last() else {
        return;
    };
    let last_key = last.to_lowercase();
    let stutter = !fragments.is_empty()
        && fragments.iter().all(|fragment| {
            let key = fragment.to_lowercase();
            !key.is_empty()
                && key.len() <= 3
                && key.len() < last_key.len()
                && last_key.starts_with(&key)
        })
        && (fragments.len() >= 2
            || fragments[0].chars().count() == 1
            || !HYPHEN_PREFIXES.contains(&fragments[0].to_lowercase().as_str()));
    if stutter {
        let capital = token.word.chars().next().is_some_and(char::is_uppercase);
        token.word = (*last).to_owned();
        if capital {
            capitalize(token);
        }
    }
}

/// True when `token` is a broken-off start of `next` ("th the", "wh- what").
fn is_false_start(token: &Token, next: &Token) -> bool {
    if !plain_word(token) || !plain_word(next) {
        return false;
    }
    let (key, next_key) = (token.key(), next.key());
    let cut_off = token.trail.starts_with('-');
    if !(token.trail.is_empty() || cut_off || token.trail == ",") {
        return false;
    }
    next_key.len() > key.len()
        && next_key.starts_with(&key)
        && (cut_off || (key.len() <= 3 && !SHORT_WORDS.contains(&key.as_str())))
}

/// How many words starting at `index` are said again straight after: a
/// repeated word or a repeated phrase of up to four words. 0 when none.
fn repeated_run(tokens: &[Token], index: usize) -> usize {
    for length in (1..=4).rev() {
        let Some(second) = tokens.get(index + length..index + 2 * length) else {
            continue;
        };
        let first = &tokens[index..index + length];
        let same = first
            .iter()
            .zip(second)
            .all(|(a, b)| plain_word(a) && plain_word(b) && a.key() == b.key());
        // A sentence end inside the first copy means it was said twice on
        // purpose ("Go. Go."), not a restart.
        let restarted = first.iter().all(|token| !token.ends_sentence());
        if same && restarted {
            if length == 1 && INTENDED_REPEATS.contains(&first[0].key().as_str()) {
                return 0;
            }
            return length;
        }
    }
    0
}

/// Removes stutters: "I I I want" -> "I want", "I want to I want to go" ->
/// "I want to go", "th- the" -> "the", "b-b-but" -> "but". The last copy is
/// kept, and a sentence start keeps its capital.
fn remove_stutters(tokens: Vec<Token>) -> Vec<Token> {
    let mut tokens = tokens;
    for token in &mut tokens {
        undo_hyphen_stutter(token);
    }
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    let mut capital_pending = false;
    while index < tokens.len() {
        let skip = match tokens.get(index + 1) {
            Some(next) if is_false_start(&tokens[index], next) => 1,
            _ => repeated_run(&tokens, index),
        };
        if skip > 0 {
            let dropped = &tokens[index];
            let at_clause_start = out
                .last()
                .is_none_or(|previous| previous.ends_sentence() || is_break(previous));
            if at_clause_start && dropped.word.chars().next().is_some_and(char::is_uppercase) {
                capital_pending = true;
            }
            // Keep an opening bracket or quote that was on the dropped word.
            if !dropped.lead.is_empty() && tokens[index + skip].lead.is_empty() {
                tokens[index + skip].lead = dropped.lead.clone();
            }
            index += skip;
            continue;
        }
        let mut token = tokens[index].clone();
        if capital_pending {
            capitalize(&mut token);
            capital_pending = false;
        }
        out.push(token);
        index += 1;
    }
    out
}

#[derive(Clone, Copy)]
enum Cue {
    /// "five cars, I mean six cars": swap the words just before for the ones
    /// after. `strong` cues are unambiguous; the others are also ordinary
    /// speech ("I mean, it works"), so they only count in the middle of a clause.
    Replace { strong: bool },
    /// "scratch that": drop the clause just spoken.
    Scratch,
    /// "sorry my bad. Instead of X, ...": the sentence after is a redo of the
    /// words before, so it takes their place.
    Restate,
}

const CUES: &[(&[&str], Cue)] = &[
    (&["sorry", "i", "mean"], Cue::Replace { strong: true }),
    (&["no", "wait"], Cue::Replace { strong: true }),
    (&["wait", "no"], Cue::Replace { strong: true }),
    (&["actually", "no"], Cue::Replace { strong: true }),
    (&["or", "rather"], Cue::Replace { strong: true }),
    (&["i", "mean"], Cue::Replace { strong: false }),
    (&["no", "i", "meant"], Cue::Replace { strong: true }),
    (&["sorry", "i", "meant"], Cue::Replace { strong: true }),
    (&["wait", "i", "meant"], Cue::Replace { strong: true }),
    (&["actually", "i", "meant"], Cue::Replace { strong: true }),
    (&["or", "actually"], Cue::Replace { strong: true }),
    (&["correction"], Cue::Replace { strong: true }),
    (&["i", "meant"], Cue::Replace { strong: false }),
    (&["scratch", "that"], Cue::Scratch),
    (&["strike", "that"], Cue::Scratch),
    (&["forget", "that"], Cue::Scratch),
    (&["ignore", "that"], Cue::Scratch),
    (&["let", "me", "start", "over"], Cue::Restate),
    (&["let", "me", "say", "that", "again"], Cue::Restate),
    (&["sorry", "let", "me", "rephrase"], Cue::Restate),
    (&["oops"], Cue::Restate),
    (&["sorry", "my", "bad"], Cue::Restate),
    (&["my", "bad"], Cue::Restate),
    (&["my", "mistake"], Cue::Restate),
    (&["let", "me", "rephrase"], Cue::Restate),
];

/// Most words a spoken correction replaces; people fix a word or a short
/// phrase, and a longer guess would eat text that was meant to stay.
const CORRECTION_WORDS: usize = 2;

fn cue_at(tokens: &[Token], index: usize) -> Option<(usize, Cue)> {
    CUES.iter().find_map(|(words, cue)| {
        let matches = words.iter().enumerate().all(|(offset, word)| {
            tokens
                .get(index + offset)
                .is_some_and(|token| !token.verbatim && token.key() == *word)
        });
        matches.then_some((words.len(), *cue))
    })
}

fn clause_boundary(token: &Token) -> bool {
    token.ends_sentence() || is_break(token)
}

fn closes_phrase(token: &Token) -> bool {
    token.trail.contains(['.', '?', '!', ',', ';', ':'])
}

/// Words that open a question or a clause, so two of them back to back can be
/// a restart: "can I just can't we just use".
const RESTART_OPENERS: &[&str] = &[
    "can", "can't", "could", "couldn't", "should", "shouldn't", "would", "wouldn't", "do",
    "don't", "does", "doesn't", "is", "isn't", "are", "aren't", "will", "won't", "what",
    "why", "how", "i", "we", "let's", "it", "they",
];

/// "can I just can't we just use" -> "can't we just use": two phrases of the
/// same length with different openers that end on the same word, with no
/// punctuation between them, are one thought started twice.
fn drop_restarts(tokens: Vec<Token>) -> Vec<Token> {
    let plain = |token: &Token| !token.verbatim && !token.word.is_empty();
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    'outer: while index < tokens.len() {
        for length in 2..=4usize {
            let Some(first) = tokens.get(index..index + length) else {
                break;
            };
            let Some(second) = tokens.get(index + length..index + 2 * length) else {
                break;
            };
            let clean_first = first
                .iter()
                .all(|token| plain(token) && token.trail.is_empty() && token.lead.is_empty());
            if !clean_first || !second.iter().all(plain) {
                continue;
            }
            let (a, b) = (norm(&first[0]), norm(&second[0]));
            if a != b
                && RESTART_OPENERS.contains(&a.as_str())
                && RESTART_OPENERS.contains(&b.as_str())
                && norm(&first[length - 1]) == norm(&second[length - 1])
                && !RESTART_OPENERS.contains(&norm(&first[length - 1]).as_str())
            {
                let starts_clause = out.last().is_none_or(clause_boundary);
                let mut kept = second[0].clone();
                if starts_clause {
                    capitalize(&mut kept);
                }
                out.push(kept);
                out.extend(second[1..].iter().cloned());
                index += 2 * length;
                continue 'outer;
            }
        }
        out.push(tokens[index].clone());
        index += 1;
    }
    out
}

/// Applies spoken self-corrections: "I mean", "no wait", "or rather" swap the
/// words just said for the ones that follow, and "scratch that" drops the
/// clause just spoken.
fn resolve_corrections(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        if let Some((len, cue)) = cue_at(&tokens, index) {
            if let Some(next) = apply_cue(&tokens, &mut out, index, len, cue) {
                index = next;
                continue;
            }
        }
        out.push(tokens[index].clone());
        index += 1;
    }
    out
}

/// Tries one cue; returns where to carry on, or `None` if it is ordinary speech.
fn apply_cue(
    tokens: &[Token],
    out: &mut Vec<Token>,
    index: usize,
    len: usize,
    cue: Cue,
) -> Option<usize> {
    let cue_end = index + len;
    let last_cue = &tokens[cue_end - 1];
    match cue {
        Cue::Scratch => {
            // Only when it ends its clause ("scratch that." / "scratch that, ...").
            if !(closes_phrase(last_cue) || cue_end == tokens.len()) || out.is_empty() {
                return None;
            }
            // Drop the clause being spoken, or the sentence just finished.
            let mut keep = out.len();
            let mut first = true;
            while keep > 0 && (first || !clause_boundary(&out[keep - 1])) {
                keep -= 1;
                first = false;
            }
            out.truncate(keep);
            let starts_clause = out.last().is_none_or(clause_boundary);
            if starts_clause {
                if let Some(next) = tokens.get(cue_end) {
                    let mut next = next.clone();
                    capitalize(&mut next);
                    out.push(next);
                    return Some(cue_end + 1);
                }
            }
            Some(cue_end)
        }
        Cue::Restate => {
            let mut end = cue_end;
            while end < tokens.len() {
                end += 1;
                if tokens[end - 1].ends_sentence() {
                    break;
                }
            }
            let mut redo: Vec<Token> = tokens[cue_end..end].to_vec();
            // "let me rephrase that: ..." carries a "that" before the redo.
            if redo.first().is_some_and(|t| t.key() == "that") && redo.len() > 2 {
                redo.remove(0);
            }
            if redo.len() < 2 || redo.iter().any(|t| t.verbatim) {
                return None;
            }
            // The redo starts with the same words the first try did.
            let start = (0..out.len().saturating_sub(1)).rev().find(|&at| {
                out[at].key() == redo[0].key() && out[at + 1].key() == redo[1].key()
            })?;
            out.truncate(start);
            let starts_clause = out.last().is_none_or(clause_boundary);
            if !starts_clause && redo[0].word != "I" {
                redo[0].word = redo[0].word.to_lowercase();
            }
            if starts_clause {
                capitalize(&mut redo[0]);
            }
            out.extend(redo);
            Some(end)
        }
        Cue::Replace { strong } => {
            if !strong && closes_phrase(last_cue) {
                return None; // "I mean, ..." is just talk.
            }
            let mut replacement: Vec<Token> = Vec::new();
            let mut next = cue_end;
            while replacement.len() < CORRECTION_WORDS {
                let Some(token) = tokens.get(next) else { break };
                if token.verbatim || token.word.is_empty() {
                    break;
                }
                replacement.push(token.clone());
                next += 1;
                if closes_phrase(token) {
                    break;
                }
            }
            if replacement.is_empty() {
                return None;
            }
            let mut removable = 0;
            for position in (0..out.len()).rev() {
                let last = position + 1 == out.len();
                if removable == replacement.len() || is_break(&out[position]) {
                    break;
                }
                if !last && out[position].ends_sentence() {
                    break;
                }
                removable += 1;
            }
            let crosses_sentence = out.last().is_some_and(Token::ends_sentence);
            if removable == 0 || (!strong && crosses_sentence && replacement.len() != 1) {
                return None;
            }
            let carried_end = out
                .last()
                .filter(|last| last.ends_sentence())
                .map(|last| last.trail.clone());
            out.truncate(out.len() - removable);
            let starts_clause = out.last().is_none_or(clause_boundary);
            if let Some(first) = replacement.first_mut() {
                if starts_clause {
                    capitalize(first);
                } else if first.word != "I" && first.word.chars().skip(1).all(|c| !c.is_uppercase())
                {
                    first.word = first.word.to_lowercase();
                }
            }
            if let (Some(trail), Some(last)) = (carried_end, replacement.last_mut()) {
                if last.trail.is_empty() {
                    last.trail = trail;
                }
            }
            out.extend(replacement);
            Some(next)
        }
    }
}

/// Words that name what a list holds: "a few things", "three steps".
const LIST_NOUNS: &[&str] = &[
    "points", "items", "things", "features", "steps", "ideas", "options", "tasks", "reasons",
    "requirements", "notes", "changes", "problems", "issues", "questions", "goals", "suggestions",
    "tips", "ways", "examples", "parts", "bugs", "fixes", "apps", "screens", "pages", "rules",
    "priorities", "todos", "topics", "benefits", "pros", "cons", "concerns", "updates", "areas",
    "list", "agenda", "stuff",
];

/// Words that announce a number of something: "here are", "a few", "three".
const LIST_ANNOUNCERS: &[&str] = &[
    "here are", "here's", "these are", "the following", "list of", "a few", "a couple", "some",
    "several", "multiple", "few", "two", "three", "four", "five", "six", "2", "3", "4", "5",
    "my", "our", "main", "key",
];

fn introduces_list(sentence: &str) -> bool {
    let lower = sentence.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| !word.is_empty())
        .collect();
    let joined = format!(" {} ", words.join(" "));
    let announces = LIST_ANNOUNCERS
        .iter()
        .any(|phrase| joined.contains(&format!(" {phrase} ")));
    announces && words.iter().any(|word| LIST_NOUNS.contains(word))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    /// "first", "second", "number three".
    Rank(u8),
    /// "next", "then", "finally": a new item, once a list has started.
    Continue,
}

/// Spoken item markers, longest first so "first of all" beats "first".
const MARKERS: &[(&str, Marker)] = &[
    ("first of all", Marker::Rank(1)),
    ("number one", Marker::Rank(1)),
    ("point one", Marker::Rank(1)),
    ("step one", Marker::Rank(1)),
    ("firstly", Marker::Rank(1)),
    ("first", Marker::Rank(1)),
    ("number two", Marker::Rank(2)),
    ("point two", Marker::Rank(2)),
    ("step two", Marker::Rank(2)),
    ("secondly", Marker::Rank(2)),
    ("second", Marker::Rank(2)),
    ("number three", Marker::Rank(3)),
    ("point three", Marker::Rank(3)),
    ("step three", Marker::Rank(3)),
    ("thirdly", Marker::Rank(3)),
    ("third", Marker::Rank(3)),
    ("number four", Marker::Rank(4)),
    ("point four", Marker::Rank(4)),
    ("step four", Marker::Rank(4)),
    ("fourth", Marker::Rank(4)),
    ("number five", Marker::Rank(5)),
    ("point five", Marker::Rank(5)),
    ("step five", Marker::Rank(5)),
    ("fifth", Marker::Rank(5)),
    ("after that", Marker::Continue),
    ("and finally", Marker::Continue),
    ("finally", Marker::Continue),
    ("lastly", Marker::Continue),
    ("next", Marker::Continue),
    ("then", Marker::Continue),
];

/// "Second, we race." -> (Rank(2), "we race.").
fn split_marker(sentence: &str) -> Option<(Marker, &str)> {
    MARKERS.iter().find_map(|(phrase, marker)| {
        let head = sentence.get(..phrase.len())?;
        let rest = &sentence[phrase.len()..];
        let whole_word = rest.chars().next().is_none_or(|c| !c.is_alphanumeric());
        (head.eq_ignore_ascii_case(phrase) && whole_word)
            .then(|| (*marker, rest.trim_start_matches([',', ':', ' ', '-'])))
    })
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Splits after . ? ! (and a colon) that are followed by a space.
fn sentences(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if matches!(c, '.' | '?' | '!' | ':')
            && chars.peek().is_none_or(|(_, next)| next.is_whitespace())
        {
            parts.push(text[start..=at].trim());
            start = at + 1;
        }
    }
    if !text[start..].trim().is_empty() {
        parts.push(text[start..].trim());
    }
    parts
}

/// Joins the lead-in sentences and turns the last one into a heading.
fn heading(lead: &[&str]) -> String {
    let mut out = lead.join(" ");
    if !out.is_empty() {
        let trimmed = out.trim_end_matches(['.', ':']).len();
        out.truncate(trimmed);
        out.push(':');
    }
    out
}

/// "First, X. Second, Y. Finally, Z." -> one bullet per spoken marker; the
/// sentences between markers stay with their item. Needs a "first" and a
/// "second" so a lone "First, ..." is never a list.
fn spoken_list(parts: &[&str]) -> Option<String> {
    let first = parts
        .iter()
        .position(|part| split_marker(part).is_some_and(|(m, _)| m == Marker::Rank(1)))?;
    parts[first + 1..]
        .iter()
        .any(|part| split_marker(part).is_some_and(|(m, _)| m == Marker::Rank(2)))
        .then_some(())?;
    let mut items: Vec<String> = Vec::new();
    for part in &parts[first..] {
        match split_marker(part) {
            Some((_, rest)) if !rest.is_empty() => items.push(capitalized(rest)),
            Some(_) => items.push(String::new()),
            None => match items.last_mut() {
                Some(item) if !item.is_empty() => {
                    item.push(' ');
                    item.push_str(part);
                }
                Some(item) => item.push_str(part),
                None => items.push((*part).to_owned()),
            },
        }
    }
    let mut out = heading(&parts[..first]);
    for item in items.iter().filter(|item| !item.is_empty()) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("- ");
        out.push_str(item);
    }
    Some(out)
}

/// Spots a list in what was said and lays it out as bullets:
/// "Here are a few points. A. B." or "I want three things: A. B." puts the
/// sentences after the announcement on bullet lines, and "First, A. Second,
/// B." puts each marked item on its own line. An announcement needs two or
/// more sentences after it, so one stray sentence is never a list.
fn bulletize(text: String) -> String {
    if text.contains('\n') {
        return text;
    }
    let parts = sentences(&text);
    if let Some(list) = spoken_list(&parts) {
        return list;
    }
    let Some(intro) = parts.iter().position(|part| introduces_list(part)) else {
        return text;
    };
    if parts.len() < intro + 3 {
        return text;
    }
    let mut out = heading(&parts[..=intro]);
    for part in &parts[intro + 1..] {
        out.push_str("\n- ");
        out.push_str(&capitalized(part));
    }
    out
}

fn capitalize(token: &mut Token) {
    if token.verbatim {
        return;
    }
    let mut chars = token.word.chars();
    if let Some(first) = chars.next() {
        token.word = first.to_uppercase().chain(chars).collect();
    }
}

/// Spacing rules: words separated by one space, marks attached to the
/// previous word, a capital after a line break.
fn render(tokens: &[Token]) -> String {
    let mut out = String::new();
    let mut capitalize_next = false;
    for token in tokens {
        let mut token = token.clone();
        if capitalize_next && !token.word.is_empty() {
            capitalize(&mut token);
            capitalize_next = false;
        }
        if token.word.is_empty() {
            // A bare mark or line break.
            let trail = token.trail.as_str();
            if trail.starts_with('\n') {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push_str(trail);
                capitalize_next = true;
            } else {
                while out.ends_with(' ') {
                    out.pop();
                }
                out.push_str(trail);
            }
            continue;
        }
        if !out.is_empty() && !out.ends_with(['\n', ' ']) {
            out.push(' ');
        }
        out.push_str(&token.render());
    }
    out.trim().to_owned()
}

/// Words of a text, lowercased, letters and digits only.
fn content_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|word| word.trim_matches('\'').to_lowercase())
        .filter(|word| !word.is_empty())
        .collect()
}

/// Edit distance, capped: only "is it within `limit`" matters.
fn within_edits(a: &str, b: &str, limit: usize) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > limit {
        return false;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(previous + usize::from(ca != cb));
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()] <= limit
}

/// Openers a chat model uses when it answers instead of rewriting.
const ANSWER_OPENERS: &[&str] = &[
    "sure", "certainly", "of course", "here is", "here's the", "the cleaned", "cleaned text",
    "i'm sorry", "as an ai",
];

/// Strips what a model sometimes wraps around its answer.
#[must_use]
pub fn unwrap_rewrite(text: &str) -> String {
    let mut text = text.trim();
    for tag in ["<dictation>", "</dictation>"] {
        text = text.trim_start_matches(tag).trim_end_matches(tag).trim();
    }
    let quoted = text.len() > 1 && text.starts_with('"') && text.ends_with('"');
    if quoted {
        text = &text[1..text.len() - 1];
    }
    text.trim().to_owned()
}

/// Whether a language model's rewrite of `original` can be typed instead of
/// it. The model may drop words, fix grammar, and lay out a list, but it must
/// not answer, add content, or wander off: anything suspicious keeps the
/// rule-cleaned text, which is never worse than today.
#[must_use]
pub fn accept_rewrite(original: &str, rewrite: &str) -> bool {
    let before = content_words(original);
    let after = content_words(rewrite);
    if after.is_empty() || before.is_empty() {
        return false;
    }
    // Shorter is expected (fillers, restarts); much longer means new content.
    if after.len() > before.len() + before.len() / 5 + 3 || after.len() * 4 < before.len() {
        return false;
    }
    let lower = rewrite.trim_start().to_lowercase();
    let opener = |text: &str| ANSWER_OPENERS.iter().any(|opener| text.starts_with(opener));
    if opener(&lower) && !opener(&original.trim_start().to_lowercase()) {
        return false;
    }
    // A question must stay a question, or the model answered it.
    let asked = original.contains('?');
    if asked && !rewrite.contains('?') {
        return false;
    }
    // Every longer word must come from what was said (or be a spelling fix
    // of it), apart from the odd synonym ("instead" for "alternative").
    let novel = after
        .iter()
        .filter(|word| word.chars().count() >= 4)
        .filter(|word| {
            !before.iter().any(|said| {
                said == *word
                    || within_edits(said, word, 2)
                    || (said.len() >= 4 && word.starts_with(said.as_str()))
                    || (word.len() >= 4 && said.starts_with(word.as_str()))
            })
        })
        .count();
    novel <= 1 + after.len() / 10
}

/// Cleans one recognised utterance.
#[must_use]
pub fn clean(text: &str, options: &TextOptions) -> String {
    let tokens: Vec<Token> = text.split_whitespace().map(Token::parse).collect();
    if tokens.is_empty() {
        return String::new();
    }
    let phrases = build_phrases(options);
    let mut tokens = if phrases.is_empty() {
        tokens
    } else {
        apply_phrases(&tokens, &phrases)
    };
    if options.filler_removal {
        tokens = remove_stutters(resolve_corrections(drop_restarts(remove_fillers(tokens))));
    }
    let text = render(&tokens);
    if options.filler_removal {
        bulletize(text)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> TextOptions {
        TextOptions {
            filler_removal: true,
            spoken_punctuation: true,
            dictionary: Vec::new(),
            snippets: Vec::new(),
        }
    }

    fn entry(from: &str, to: &str) -> Replacement {
        Replacement {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(
            clean("Hello, world. How are you?", &options()),
            "Hello, world. How are you?"
        );
        assert_eq!(clean("   ", &options()), "");
        assert_eq!(clean("", &options()), "");
    }

    #[test]
    fn removes_filler_words_and_keeps_the_rest() {
        let o = options();
        assert_eq!(
            clean("I was, um, thinking about it.", &o),
            "I was, thinking about it."
        );
        assert_eq!(clean("So uh we should go.", &o), "So we should go.");
        assert_eq!(clean("Um, so we should go.", &o), "So we should go.");
        assert_eq!(clean("We should go, um.", &o), "We should go.");
        assert_eq!(
            clean("It was good. Uh, really good.", &o),
            "It was good. Really good."
        );
        // Words that merely contain a filler are untouched.
        assert_eq!(
            clean("The umbrella and the hummus.", &o),
            "The umbrella and the hummus."
        );
    }

    #[test]
    fn removes_you_know_only_at_clause_starts() {
        let o = options();
        assert_eq!(clean("You know, I think so.", &o), "I think so.");
        assert_eq!(
            clean("It works. You know, it really does.", &o),
            "It works. It really does."
        );
        assert_eq!(
            clean("Do you know the answer?", &o),
            "Do you know the answer?"
        );
    }

    #[test]
    fn stuttered_words_and_restarts_are_cleaned() {
        let o = options();
        assert_eq!(clean("I I I want to go home.", &o), "I want to go home.");
        assert_eq!(clean("I, I, I think so.", &o), "I think so.");
        assert_eq!(clean("Can can you send it?", &o), "Can you send it?");
        assert_eq!(
            clean("We need to to finish it.", &o),
            "We need to finish it."
        );
        // A restarted phrase keeps only the last attempt.
        assert_eq!(
            clean("I want to I want to go to the store.", &o),
            "I want to go to the store."
        );
        assert_eq!(
            clean("Can you can you check this?", &o),
            "Can you check this?"
        );
        // Broken-off starts of a word.
        assert_eq!(
            clean("Th the meeting is at five.", &o),
            "The meeting is at five."
        );
        assert_eq!(
            clean("Let's meet on wed- Wednesday.", &o),
            "Let's meet on Wednesday."
        );
        assert_eq!(clean("B-b-but it works.", &o), "But it works.");
        assert_eq!(clean("It's th-th-the best.", &o), "It's the best.");
        // Fillers and stutters together.
        assert_eq!(clean("I, um, I want, uh, want that.", &o), "I want that.");
        assert_eq!(clean("So um so we go.", &o), "So we go.");
        // What Parakeet actually returned for stuttered test speech.
        assert_eq!(
            clean("The meeting is at five, so please, please be on time.", &o),
            "The meeting is at five, so please be on time."
        );
        assert_eq!(
            clean("Can can you send me the the report by Friday?", &o),
            "Can you send me the report by Friday?"
        );
    }

    #[test]
    fn deliberate_repeats_and_real_words_are_kept() {
        let o = options();
        assert_eq!(
            clean("It was very very good.", &o),
            "It was very very good."
        );
        assert_eq!(clean("No no, that's fine.", &o), "No no, that's fine.");
        assert_eq!(clean("He had had enough.", &o), "He had had enough.");
        assert_eq!(clean("Go. Go now.", &o), "Go. Go now.");
        assert_eq!(
            clean("Please re-read the well-known co-op rules.", &o),
            "Please re-read the well-known co-op rules."
        );
        assert_eq!(
            clean("Put it in inside the box.", &o),
            "Put it in inside the box."
        );
        assert_eq!(clean("I want to today.", &o), "I want to today.");
        assert_eq!(
            clean("Bring a apple and an answer.", &o),
            "Bring a apple and an answer."
        );
        assert_eq!(clean("Room 1 1 is free.", &o), "Room 1 1 is free.");
        let mut off = options();
        off.filler_removal = false;
        assert_eq!(clean("I I want.", &off), "I I want.");
    }

    #[test]
    fn filler_removal_can_be_turned_off() {
        let mut o = options();
        o.filler_removal = false;
        assert_eq!(clean("Um, so we go.", &o), "Um, so we go.");
    }

    #[test]
    fn spoken_punctuation_becomes_marks_and_breaks() {
        let o = options();
        assert_eq!(
            clean("Dear team new paragraph Thanks for the update", &o),
            "Dear team\n\nThanks for the update"
        );
        assert_eq!(
            clean("Hello. New line. How are you?", &o),
            "Hello.\nHow are you?"
        );
        assert_eq!(clean("first new line second", &o), "first\nSecond");
        assert_eq!(clean("Wait comma what question mark", &o), "Wait, what?");
        assert_eq!(clean("Great exclamation mark", &o), "Great!");
        assert_eq!(clean("That is all full stop", &o), "That is all.");
    }

    #[test]
    fn spoken_punctuation_can_be_turned_off() {
        let mut o = options();
        o.spoken_punctuation = false;
        assert_eq!(clean("a new line b", &o), "a new line b");
    }

    #[test]
    fn dictionary_fixes_spellings_and_keeps_punctuation() {
        let mut o = options();
        o.dictionary = vec![
            entry("claude code", "Claude Code"),
            entry("kubernetes", "Kubernetes"),
        ];
        assert_eq!(
            clean("I use claude code every day.", &o),
            "I use Claude Code every day."
        );
        assert_eq!(
            clean("Run kubernetes, then claude code.", &o),
            "Run Kubernetes, then Claude Code."
        );
        assert_eq!(clean("Claude Code, please", &o), "Claude Code, please");
    }

    #[test]
    fn snippets_expand_and_drop_sentence_punctuation() {
        let mut o = options();
        o.snippets = vec![entry("insert my email", "me@example.com")];
        assert_eq!(
            clean("Send it to insert my email.", &o),
            "Send it to me@example.com"
        );
        assert_eq!(
            clean("Insert my email, thanks", &o),
            "me@example.com thanks"
        );
    }

    #[test]
    fn longer_phrases_win() {
        let mut o = options();
        o.dictionary = vec![entry("new", "NEW")];
        assert_eq!(clean("a new paragraph b", &o), "a\n\nB");
    }

    #[test]
    fn non_english_text_is_left_alone() {
        let o = options();
        assert_eq!(
            clean("ÐŸÑ€Ð¸Ð²ÐµÑ‚, ÐºÐ°Ðº Ð´ÐµÐ»Ð°?", &o),
            "ÐŸÑ€Ð¸Ð²ÐµÑ‚, ÐºÐ°Ðº Ð´ÐµÐ»Ð°?"
        );
        assert_eq!(
            clean("Das ist schÃ¶n â€” nicht wahr?", &o),
            "Das ist schÃ¶n â€” nicht wahr?"
        );
    }

    #[test]
    fn tokens_keep_their_punctuation() {
        let token = Token::parse("(Hello,");
        assert_eq!(
            (
                token.lead.as_str(),
                token.word.as_str(),
                token.trail.as_str()
            ),
            ("(", "Hello", ",")
        );
        let token = Token::parse("don't.");
        assert_eq!((token.word.as_str(), token.trail.as_str()), ("don't", "."));
        let token = Token::parse("...");
        assert_eq!((token.word.as_str(), token.lead.as_str()), ("...", ""));
    }

    #[test]
    fn spoken_corrections_replace_what_was_just_said() {
        let o = options();
        assert_eq!(
            clean("We have five cars, I mean six cars on the track.", &o),
            "We have six cars on the track."
        );
        assert_eq!(
            clean("All the outcomes will be different. No wait, randomized.", &o),
            "All the outcomes will be randomized."
        );
        assert_eq!(clean("It is blue or rather green.", &o), "It is green.");
        assert_eq!(clean("Send it. Scratch that. Hold on.", &o), "Hold on.");
    }

    #[test]
    fn ordinary_speech_with_cue_words_is_kept() {
        let o = options();
        assert_eq!(clean("I mean, it works.", &o), "I mean, it works.");
        assert_eq!(
            clean("It is fine. I mean it works.", &o),
            "It is fine. I mean it works."
        );
        assert_eq!(clean("Oh, nice work.", &o), "Oh, nice work.");
    }

    #[test]
    fn stray_oh_is_a_filler_and_points_become_bullets() {
        let o = options();
        assert_eq!(
            clean("Point at a Oh empty space.", &o),
            "Point at a empty space."
        );
        assert_eq!(
            clean(
                "Here are a few points for the app. It needs AR. It needs races. It needs bets.",
                &o
            ),
            "Here are a few points for the app:\n- It needs AR.\n- It needs races.\n- It needs bets."
        );
        assert_eq!(
            clean("Here are the points. Just one thing.", &o),
            "Here are the points. Just one thing."
        );
    }

    #[test]
    fn spoken_bullet_points_start_list_lines() {
        let o = options();
        assert_eq!(
            clean("Goals bullet point fast bullet point simple", &o),
            "Goals\n- Fast\n- Simple"
        );
    }

    #[test]
    fn a_redone_sentence_replaces_the_first_try() {
        let o = options();
        assert_eq!(
            clean(
                "Okay, so what I want now is instead of VS Code, can't I just use X code. Sorry my bad. Instead of X code can I just use VS Code?",
                &o
            ),
            "Okay, so what I want now is instead of X code can I just use VS Code?"
        );
        assert_eq!(clean("My bad, it was late.", &o), "My bad, it was late.");
    }

    #[test]
    fn padding_and_restarts_are_dropped() {
        let o = options();
        assert_eq!(
            clean(
                "Okay, so what I want now is basically let's say can I just can't we just use VS Code?",
                &o
            ),
            "Okay, so what I want now is can't we just use VS Code?"
        );
        assert_eq!(clean("It basically works.", &o), "It basically works.");
        assert_eq!(clean("Let's say hello to them.", &o), "Let's say hello to them.");
        assert_eq!(
            clean("Can I just can't we just go now?", &o),
            "Can't we just go now?"
        );
        assert_eq!(clean("Is it good is it bad?", &o), "Is it good is it bad?");
    }

    #[test]
    fn more_correction_cues_work() {
        let o = options();
        assert_eq!(clean("Meet at noon, no I meant one.", &o), "Meet at one.");
        assert_eq!(clean("Buy milk. Strike that. Buy eggs.", &o), "Buy eggs.");
        assert_eq!(clean("I meant to call you.", &o), "I meant to call you.");
        assert_eq!(clean("Er, hello there.", &o), "Hello there.");
    }

    #[test]
    fn lists_are_spotted_without_a_command() {
        let o = options();
        assert_eq!(
            clean("I want three things in the app. It needs AR. It needs races. It needs bets.", &o),
            "I want three things in the app:\n- It needs AR.\n- It needs races.\n- It needs bets."
        );
        assert_eq!(
            clean("Some ideas for the game: cars race on the desk. Friends can bet.", &o),
            "Some ideas for the game:\n- Cars race on the desk.\n- Friends can bet."
        );
        assert_eq!(
            clean(
                "Okay so the plan. First, we build the track. It should be fast. Second, we add cars. Finally, we add betting.",
                &o
            ),
            "Okay so the plan:\n- We build the track. It should be fast.\n- We add cars.\n- We add betting."
        );
        assert_eq!(
            clean("Number one, speed. Number two, price.", &o),
            "- Speed.\n- Price."
        );
    }

    #[test]
    fn ordinary_sentences_are_not_made_into_lists() {
        let o = options();
        assert_eq!(
            clean("First, let me say thanks. Then we can talk.", &o),
            "First, let me say thanks. Then we can talk."
        );
        assert_eq!(
            clean("I have some ideas. Let me think.", &o),
            "I have some ideas. Let me think."
        );
        assert_eq!(
            clean("The first car won. The second car lost. It was close.", &o),
            "The first car won. The second car lost. It was close."
        );
    }

    #[test]
    fn rewrites_that_answer_or_add_content_are_refused() {
        let asked = "What is the capital of France?";
        assert!(!accept_rewrite(asked, "The capital of France is Paris."));
        assert!(accept_rewrite(asked, "What is the capital of France?"));
        assert!(!accept_rewrite(
            "Can I just use VS Code?",
            "Sure, instead of X code, can I just use VS Code?"
        ));
        assert!(accept_rewrite(
            "Okay, so what I want now is basically let's say can I just can't we just use VS Code? as an alternative to X code.",
            "Can we just use VS Code instead of X code?"
        ));
        assert!(accept_rewrite(
            "Here are few points that I want inside my app. It should have dark mode. It should sync with iCloud.",
            "Here are a few points I want in my app:\n- Dark mode\n- Sync with iCloud"
        ));
        assert!(accept_rewrite(
            "Point my phone onto a Oh empty space. Hotfield tracks.",
            "Point my phone at an empty space. Hot Wheels tracks."
        ));
        assert!(!accept_rewrite(
            "Write a poem.",
            "Roses are red, violets are blue, sugar is sweet and so are you."
        ));
        assert!(!accept_rewrite("Hello there.", ""));
        assert_eq!(unwrap_rewrite("<dictation>Hi.</dictation>"), "Hi.");
        assert_eq!(unwrap_rewrite("\"Hi.\""), "Hi.");
    }
}
