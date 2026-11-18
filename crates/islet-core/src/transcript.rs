//! Cleaning up recognised speech before it is typed.
//!
//! Parakeet already returns punctuated, capitalised text, so this stage only
//! does what the model cannot: drop filler words and stutters, turn spoken commands into
//! punctuation, and apply the user's own spellings and snippets. It works on
//! words with their attached punctuation, so "Um, so" and "claude code." are
//! handled without regular expressions.

use crate::config::{DictationConfig, Replacement};

mod corrections;
mod facts;
mod safety;

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
    "um", "umm", "uh", "uhh", "uhhh", "uhm", "er", "erm", "ehm", "hmm", "mhm", "mmm", "mm", "hm",
];

/// A hesitation sound, however long it was drawn out: "um", "Ummm",
/// "uhhm", "hmmm", "mm-hmm", "errm".
fn is_filler_sound(word: &str) -> bool {
    if FILLERS.contains(&word) {
        return true;
    }
    let word = word.replace('-', "");
    // Runs of one letter each, in order: "u" then any of "h"/"m".
    let shaped = |first: &[char], rest: &[char]| {
        let mut chars = word.chars().peekable();
        let mut leading = 0;
        while chars.peek().is_some_and(|c| first.contains(c)) {
            chars.next();
            leading += 1;
        }
        let tail: Vec<char> = chars.collect();
        leading > 0 && !tail.is_empty() && tail.iter().all(|c| rest.contains(c))
    };
    word.len() <= 8
        && word != "em"
        && (shaped(&['u'], &['h', 'm'])
            || shaped(&['h'], &['m'])
            || shaped(&['m'], &['h', 'm'])
            || shaped(&['e'], &['r', 'h', 'm']) && word.ends_with(['r', 'm']))
}

/// Linking words that open a clause and can pile up when someone restarts:
/// "and also and for the Leo character…".
const LINKING_WORDS: &[&str] = &["and", "also", "so", "but", "then", "plus", "or"];

/// "And Also and for the Leo character" -> "And also for the Leo character":
/// in a run of linking words with nothing else between, a word said again
/// is a restart and goes, and a stray capital inside the run is lowered.
fn collapse_linking_runs(tokens: Vec<Token>) -> Vec<Token> {
    let linking = |token: &Token| {
        !token.verbatim && token.lead.is_empty() && LINKING_WORDS.contains(&norm(token).as_str())
    };
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut run: Vec<String> = Vec::new();
    for token in tokens {
        let continues = linking(&token)
            && !run.is_empty()
            && out.last().is_some_and(|previous| previous.trail.is_empty() || previous.trail == ",");
        if !continues {
            run.clear();
        }
        // A run only counts where a clause opens: "so and so" in the middle
        // of a sentence is a phrase.
        let opens = out.last().is_none_or(|previous| clause_boundary(previous) || previous.trail == ",");
        if linking(&token) && !continues && !opens {
            out.push(token);
            continue;
        }
        if linking(&token) {
            let word = norm(&token);
            if continues && run.contains(&word) {
                // Its punctuation stays with the run ("and also and, for").
                if let Some(previous) = out.last_mut() {
                    if previous.trail.is_empty() {
                        previous.trail = token.trail.clone();
                    }
                }
                continue;
            }
            let mut token = token;
            if continues && token.word.chars().skip(1).all(char::is_lowercase) {
                token.word = token.word.to_lowercase();
            }
            run.push(word);
            out.push(token);
            continue;
        }
        out.push(token);
    }
    out
}

/// File types that make "name dot ext" a file name.
const FILE_TYPES: &[&str] = &[
    "glb", "gltf", "fbx", "obj", "blend", "usd", "usdz", "png", "jpg", "jpeg", "gif", "svg",
    "webp", "ico", "mp3", "mp4", "wav", "mov", "pdf", "doc", "docx", "xlsx", "pptx", "txt", "md",
    "json", "toml", "yaml", "yml", "csv", "html", "css", "js", "ts", "tsx", "jsx", "py", "rs",
    "exe", "zip", "dll", "lua", "cs", "cpp", "sh", "bat", "ps1", "com", "org", "net", "io", "dev",
];

/// Spoken file names: "cyberpunk underscore dot glb" -> "cyberpunk_.glb",
/// "my underscore model" -> "my_model", "config dot toml" -> "config.toml".
/// After "underscore", speech recognition often hears "dot" as "or"
/// ("cyberpunk underscore or glb"), which is no English, so that is read as
/// "dot" too. A plain "or" stays: "PNG or JPG".
fn spoken_file_names(tokens: Vec<Token>) -> Vec<Token> {
    let plain = |token: &Token| !token.verbatim && token.lead.is_empty();
    let file_type = |token: &Token| plain(token) && FILE_TYPES.contains(&norm(token).as_str());
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let joinable = out.last().is_some_and(|previous| {
            !previous.word.is_empty() && previous.trail.is_empty() && !previous.word.ends_with('.')
        });
        let key = norm(token);
        let after_underscore = out.last().is_some_and(|previous| previous.word.ends_with('_'));
        let dot = plain(token)
            && token.trail.is_empty()
            && (key == "dot" || (key == "or" && after_underscore))
            && tokens.get(index + 1).is_some_and(file_type);
        if joinable && dot {
            let ext = &tokens[index + 1];
            let previous = out.last_mut().unwrap();
            previous.word = format!("{}.{}", previous.word, ext.word.to_lowercase());
            previous.trail = ext.trail.clone();
            previous.verbatim = true;
            index += 2;
            continue;
        }
        if joinable && plain(token) && key == "underscore" && token.trail.is_empty() {
            let previous = out.last_mut().unwrap();
            previous.word.push('_');
            previous.verbatim = true;
            // "my underscore model": the next word joins on, unless it is
            // the "dot" of a file type.
            if let Some(next) = tokens.get(index + 1).filter(|next| {
                plain(next) && next.word.chars().all(|c| c.is_alphanumeric()) && {
                    let word = norm(next);
                    !(matches!(word.as_str(), "dot" | "or") && tokens.get(index + 2).is_some_and(file_type))
                }
            }) {
                previous.word.push_str(&next.word);
                previous.trail = next.trail.clone();
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        out.push(token.clone());
        index += 1;
    }
    out
}

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
                    !token.verbatim
                        && norm(token) == *word
                        && token.lead.is_empty()
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
            && tokens
                .get(index + 1)
                .is_some_and(|next| !next.word.is_empty());
        let hedge = hedge_at(&tokens, index);
        if hedge > 0
            && out.last().is_some_and(|previous| {
                previous.trail.is_empty() && HEDGE_LEAD_INS.contains(&norm(previous).as_str())
            })
            && tokens[index + hedge - 1].trail.is_empty()
            && tokens
                .get(index + hedge)
                .is_some_and(|next| !next.word.is_empty())
        {
            index += hedge;
            continue;
        }
        // "We should, you know, send it" / "the third one is, like, to…":
        // set off by commas on both sides, they say nothing.
        let after_comma = out.last().is_some_and(|previous| previous.trail == ",");
        let aside = if !token.verbatim && after_comma && token.key() == "like" && token.trail == "," {
            1
        } else if !token.verbatim
            && after_comma
            && token.key() == "you"
            && token.trail.is_empty()
            && tokens.get(index + 1).is_some_and(|next| next.key() == "know" && next.trail == ",")
        {
            2
        } else {
            0
        };
        if aside > 0 {
            if let Some(previous) = out.last_mut() {
                previous.trail.clear();
            }
            index += aside;
            continue;
        }
        let is_filler = !token.verbatim && (is_filler_sound(&token.key()) || stray_oh);
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
    // "we we're", "I I'm", "it it's": the word, then its own contraction.
    if plain_word(token) && token.trail.is_empty() && !next.verbatim {
        let contraction = next.word.to_lowercase().replace('\u{2019}', "'");
        if contraction.starts_with(&format!("{}'", token.key())) {
            return true;
        }
    }
    if !plain_word(token) || !plain_word(next) {
        return false;
    }
    let (key, next_key) = (token.key(), next.key());
    let cut_off = token.trail.starts_with('-');
    if !(token.trail.is_empty() || cut_off || token.trail == ",") {
        return false;
    }
    // "D drive", "C code": a capital letter on its own names a letter; a
    // broken-off word would be lower case or cut with a hyphen.
    let mut letters = token.word.chars();
    let lone_capital = letters.next().is_some_and(|c| c.is_ascii_uppercase())
        && letters.next().is_none()
        && !matches!(token.word.as_str(), "I" | "A");
    if lone_capital && !cut_off {
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
            // "is is" is meant only as in "what it is is…"; elsewhere ("the
            // page is is messy") it is a stutter.
            let meant_is = first[0].key() != "is"
                || index.checked_sub(1).is_some_and(|before| matches!(tokens[before].key().as_str(), "it" | "what"));
            if length == 1 && INTENDED_REPEATS.contains(&first[0].key().as_str()) && meant_is {
                return 0;
            }
            return length;
        }
    }
    0
}

/// Question words; two in a row with nothing between them are a swap.
const QUESTION_WORDS: &[&str] = &["how", "what", "when", "where", "why", "who", "whom", "whose", "which"];

/// Two quick self-corrections that carry no cue phrase:
/// - "enter not enter but enable" -> "enable": a word, "not", the same word,
///   "but", then the right one. The repeat is required, so ordinary "not red
///   but blue" stays as it is.
/// - "know how when to do that" -> "know when to do that": a question word
///   replaced straight away by another. Any punctuation between them ("what,
///   when") means two separate words, which stay.
fn resolve_quick_swaps(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let rejected = tokens.get(index + 3).is_some_and(|but| {
            tokens.len() > index + 4
                && tokens[index + 1].key() == "not"
                && tokens[index + 2].key() == token.key()
                && but.key() == "but"
                && !token.ends_sentence()
        });
        let swapped_question = tokens.get(index + 1).is_some_and(|next| {
            QUESTION_WORDS.contains(&token.key().as_str())
                && QUESTION_WORDS.contains(&next.key().as_str())
                && token.key() != next.key()
                && token.trail.is_empty()
                && next.lead.is_empty()
        });
        let skip = if rejected {
            4
        } else if swapped_question {
            1
        } else {
            0
        };
        if skip == 0 {
            out.push(token.clone());
            index += 1;
            continue;
        }
        // The replacement takes the dropped word's place: its opening
        // bracket or quote, and its capital at the start of a sentence.
        let mut kept = tokens[index + skip].clone();
        if kept.lead.is_empty() {
            kept.lead = token.lead.clone();
        }
        if token.word.chars().next().is_some_and(char::is_uppercase) {
            capitalize(&mut kept);
        }
        out.push(kept);
        index += skip + 1;
    }
    out
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
    /// "No, it's not X, it's Y": verify the explicitly denied value before
    /// replacing the preceding quantity. Never treat a general negation as a redo.
    Contrast,
    /// A full repeated clause may change a verified recipient or quantity.
    /// Other differences, including negation and actions, are not inferred.
    VerifiedRestate,
    /// "five cars, I mean six cars": swap the words just before for the ones
    /// after. `strong` cues are unambiguous; the others are also ordinary
    /// speech ("I mean, it works"), so they only count in the middle of a clause.
    Replace { strong: bool },
    /// "scratch that": drop the clause just spoken.
    Scratch,
    /// "sorry my bad. Instead of X, ...": the sentence after is a redo of the
    /// words before, so it takes their place.
    Restate,
    /// Single ambiguous words only act when a typed phrase can be resolved.
    Scoped,
    /// "never mind": the sentence being spoken is given up (see `given_up`).
    GiveUp,
}

const CUES: &[(&[&str], Cue)] = &[
    (&["no", "that", "is", "wrong"], Cue::VerifiedRestate),
    (&["let", "me", "correct", "that"], Cue::VerifiedRestate),
    (&["no", "it's", "not"], Cue::Contrast),
    (&["no", "it", "is", "not"], Cue::Contrast),
    (&["no", "it", "isn't"], Cue::Contrast),
    (&["no", "it", "was", "not"], Cue::Contrast),
    (&["no", "it", "wasn't"], Cue::Contrast),
    (&["no", "that's", "not"], Cue::Contrast),
    (&["no", "that", "is", "not"], Cue::Contrast),
    (&["actually", "it's", "not"], Cue::Contrast),
    (&["actually", "it", "is", "not"], Cue::Contrast),
    (&["no", "not"], Cue::Contrast),
    (&["no", "i'm", "sorry"], Cue::Replace { strong: true }),
    (&["no", "i", "am", "sorry"], Cue::Replace { strong: true }),
    (&["actually", "make", "that"], Cue::Replace { strong: true }),
    (&["make", "that"], Cue::Replace { strong: true }),
    (&["no", "sorry"], Cue::Replace { strong: true }),
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
    (&["never", "mind"], Cue::GiveUp),
    (&["nevermind"], Cue::GiveUp),
    (&["sorry"], Cue::Scoped),
    (&["i'm", "sorry"], Cue::Scoped),
    (&["i", "am", "sorry"], Cue::Scoped),
    (&["actually"], Cue::Scoped),
    (&["no"], Cue::Scoped),
];

/// Most words a spoken correction replaces; people fix a word or a short
/// phrase, and a longer guess would eat text that was meant to stay.
const CORRECTION_WORDS: usize = 4;

fn cue_at(tokens: &[Token], index: usize) -> Option<(usize, Cue)> {
    CUES.iter().find_map(|(words, cue)| {
        let matches = words.iter().enumerate().all(|(offset, word)| {
            tokens.get(index + offset).is_some_and(|token| {
                !token.verbatim
                    && norm(token) == *word
                    && !token.lead.contains(['"', '\'', '\u{201c}', '\u{2018}'])
                    && !token.trail.contains(['"', '\'', '\u{201d}', '\u{2019}'])
            })
        });
        matches.then_some((words.len(), *cue))
    })
}

/// Count candidate multiword correction phrases for data-free diagnostics.
/// This reports presence, not whether an ambiguous phrase is a correction.
#[must_use]
pub fn correction_cue_count(text: &str) -> usize {
    let tokens: Vec<_> = text.split_whitespace().map(Token::parse).collect();
    let mut at = 0;
    let mut count = 0;
    while at < tokens.len() {
        if let Some((length, _)) = cue_at(&tokens, at).filter(|(length, _)| *length > 1) {
            count += 1;
            at += length;
        } else {
            at += 1;
        }
    }
    count
}

/// A word without its surrounding punctuation, lower case.
fn bare_word(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'').to_lowercase()
}

/// Never the noun of a restarted phrase ("the hotel near the near the").
const PREPOSITIONS: &[&str] = &[
    "near", "about", "above", "across", "after", "against", "along", "among", "around", "before", "behind",
    "below", "beneath", "beside", "between", "beyond", "during", "except", "inside", "into", "onto", "outside",
    "over", "past", "since", "through", "toward", "towards", "under", "until", "upon", "within", "without",
];

const UNIT_WORDS: &[&str] = &["dollars", "dollar", "rupees", "rupee", "euros", "euro", "pounds", "percent", "bucks"];
const DETERMINERS: &[&str] = &["the", "a", "an", "this", "that", "my", "our", "your", "their"];

/// "The new contract the contract should…" → "The contract should…": a
/// noun phrase started, then said again shorter, ending on the same noun.
fn drop_noun_restarts(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut index = 0;
    'outer: while index < words.len() {
        if DETERMINERS.contains(&bare_word(words[index]).as_str()) {
            for length in 2..=4 {
                let Some(phrase) = words.get(index..index + length) else { break };
                if phrase[..length - 1].iter().any(|w| w.ends_with(|c: char| !c.is_alphanumeric())) {
                    break;
                }
                let noun = bare_word(phrase[length - 1]);
                if noun.len() < 3
                    || FUNCTION_WORDS.contains(&noun.as_str())
                    || PREPOSITIONS.contains(&noun.as_str())
                    || phrase[length - 1].ends_with(|c: char| !c.is_alphanumeric())
                {
                    continue;
                }
                let Some(next) = words.get(index + length) else { break };
                if !DETERMINERS.contains(&bare_word(next).as_str()) {
                    continue;
                }
                for gap in 1..length {
                    if words.get(index + length + gap).is_some_and(|w| bare_word(w) == noun) {
                        // Keep the second try, with the first try's capital.
                        let mut kept = words[index + length].to_owned();
                        if words[index].starts_with(char::is_uppercase) {
                            kept = capitalized(&kept);
                        }
                        out.push(kept);
                        index += length + 1;
                        continue 'outer;
                    }
                }
            }
        }
        out.push(words[index].to_owned());
        index += 1;
    }
    out.join(" ")
}

/// A number starting at `at`: one written word ("$25,000", "5") or spoken
/// words ("twenty two thousand"). Gives its value and how many words.
fn number_span(words: &[&str], at: usize) -> Option<(u64, usize)> {
    let first = words.get(at)?;
    let digits: String = first.chars().filter(char::is_ascii_digit).collect();
    if !digits.is_empty() && first.trim_matches(|c: char| !c.is_alphanumeric()).chars().all(|c| c.is_ascii_digit() || c == ',') {
        return digits.parse().ok().map(|value| (value, 1));
    }
    let unit = |word: &str| -> Option<u64> {
        Some(match word {
            "zero" => 0, "one" => 1, "two" => 2, "three" => 3, "four" => 4, "five" => 5, "six" => 6, "seven" => 7,
            "eight" => 8, "nine" => 9, "ten" => 10, "eleven" => 11, "twelve" => 12, "thirteen" => 13,
            "fourteen" => 14, "fifteen" => 15, "sixteen" => 16, "seventeen" => 17, "eighteen" => 18,
            "nineteen" => 19, "twenty" => 20, "thirty" => 30, "forty" => 40, "fifty" => 50, "sixty" => 60,
            "seventy" => 70, "eighty" => 80, "ninety" => 90,
            _ => return None,
        })
    };
    let (mut total, mut current, mut used, mut numeric) = (0u64, 0u64, 0usize, false);
    for word in &words[at..] {
        let bare = bare_word(word);
        match bare.as_str() {
            "hundred" if numeric => current = current.max(1) * 100,
            "thousand" if numeric => {
                total += current.max(1) * 1_000;
                current = 0;
            }
            "million" if numeric => {
                total += current.max(1) * 1_000_000;
                current = 0;
            }
            "and" if numeric && used > 0 => {}
            other => match unit(other) {
                Some(value) => {
                    current += value;
                    numeric = true;
                }
                None => break,
            },
        }
        used += 1;
        if word.ends_with(|c: char| !c.is_alphanumeric()) {
            break;
        }
    }
    // A trailing "and" isn't part of the number.
    while used > 0 && bare_word(words[at + used - 1]) == "and" {
        used -= 1;
    }
    numeric.then_some((total + current, used))
}

/// "around 25,000 dollars. No, no, not $25,000. Sorry, $30,000." → "around
/// $30,000.": a value denied by name ("not X") and then given again takes
/// the place of the earlier mention of X. Values may be written or spoken
/// ("not twenty two thousand, sorry, twenty five thousand").
fn resolve_denied_value(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let bare: Vec<String> = words.iter().map(|w| bare_word(w)).collect();
    let mark = |word: &str| -> String {
        word.chars().rev().take_while(|c| !c.is_alphanumeric()).collect::<Vec<_>>().into_iter().rev().collect()
    };
    let plain = |word: &str| word.trim_end_matches(|c: char| !c.is_alphanumeric()).to_owned();
    for start in 0..words.len() {
        if bare[start] != "no" {
            continue;
        }
        let mut at = start + 1;
        if bare.get(at).is_some_and(|w| w == "no") {
            at += 1;
        }
        if bare.get(at).is_none_or(|w| w != "not") {
            continue;
        }
        let Some((denied, denied_len)) = number_span(&words, at + 1) else { continue };
        let mut cue = at + 1 + denied_len;
        if bare.get(cue).is_some_and(|w| UNIT_WORDS.contains(&w.as_str())) {
            cue += 1;
        }
        let new_at = match (bare.get(cue).map(String::as_str), bare.get(cue + 1).map(String::as_str)) {
            (Some("sorry" | "actually"), _) => cue + 1,
            (Some("i"), Some("mean" | "meant")) => cue + 2,
            (Some("it's"), _) => cue + 1,
            _ => continue,
        };
        let Some((_, new_len)) = number_span(&words, new_at) else { continue };
        let mut end = new_at + new_len;
        let new_unit = words.get(end).filter(|w| UNIT_WORDS.contains(&bare_word(w).as_str())).copied();
        if new_unit.is_some() {
            end += 1;
        }
        // The earlier mention, the latest one before the correction.
        let Some((old, old_len)) = (0..start).rev().find_map(|i| {
            number_span(&words, i).filter(|&(value, len)| value == denied && i + len <= start).map(|(_, len)| (i, len))
        }) else {
            continue;
        };
        let old_unit = words.get(old + old_len).filter(|w| UNIT_WORDS.contains(&bare_word(w).as_str())).copied();
        let old_end = old + old_len + usize::from(old_unit.is_some());
        let new_words: Vec<String> = words[new_at..new_at + new_len].iter().map(|w| plain(w)).collect();
        let symbol = new_words[0].starts_with(|c: char| !c.is_alphanumeric());
        let mut replacement = new_words.join(" ");
        if !symbol {
            if let Some(unit) = new_unit.or(old_unit) {
                replacement = format!("{replacement} {}", bare_word(unit));
            }
        }
        let closing = mark(words[old_end - 1]);
        let tail_mark = mark(words[end - 1]);
        let mut out: Vec<String> = words[..old].iter().map(|w| (*w).to_owned()).collect();
        let mut between: Vec<String> = words[old_end..start].iter().map(|w| (*w).to_owned()).collect();
        if between.is_empty() {
            // The correction's own sentence end wins ("…dollars, no, not X.
            // Sorry, Y." ends where the correction ended).
            let ends = tail_mark.contains(['.', '?', '!']) || closing.is_empty();
            out.push(format!("{replacement}{}", if ends { &tail_mark } else { &closing }));
        } else {
            out.push(format!("{replacement}{}", closing.trim_end_matches(['.', '!', '?'])));
            if let Some(last) = between.last_mut() {
                *last = format!("{}{}", plain(last), tail_mark);
            }
            out.extend(between);
        }
        out.extend(words[end..].iter().map(|w| (*w).to_owned()));
        return resolve_denied_value(&out.join(" "));
    }
    text.to_owned()
}

/// "this settings page should be the settings page should be cleaner" →
/// "the settings page should be cleaner": three to six words said again
/// straight after, maybe with a different "the/this/a" in front.
fn drop_repeated_runs(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let keys: Vec<String> = words.iter().map(|w| bare_word(w)).collect();
    let ends = |w: &str| w.ends_with(['.', '?', '!', ',', ';', ':']);
    for start in 0..words.len() {
        for length in (3..=6).rev() {
            if start + 2 * length > words.len() {
                continue;
            }
            if words[start..start + length].iter().any(|w| ends(w)) {
                continue;
            }
            let run = &keys[start..start + length];
            let mut again = start + length;
            let det_between = words.get(again).is_some_and(|w| DETERMINERS.contains(&bare_word(w).as_str()) && !ends(w));
            if det_between {
                again += 1;
            }
            if keys.get(again..again + length) != Some(run) {
                continue;
            }
            // Drop the first try and the determiner in front of it, keeping
            // the second try (and its own determiner).
            let mut from = start;
            if det_between && start > 0 && DETERMINERS.contains(&keys[start - 1].as_str()) && !ends(words[start - 1]) {
                from -= 1;
            }
            let keep_from = if det_between { start + length } else { again };
            let mut out: Vec<String> = words[..from].iter().map(|w| (*w).to_owned()).collect();
            let mut rest: Vec<String> = words[keep_from..].iter().map(|w| (*w).to_owned()).collect();
            if words[from].starts_with(char::is_uppercase) {
                if let Some(first) = rest.first_mut() {
                    *first = capitalized(first);
                }
            }
            out.append(&mut rest);
            return drop_repeated_runs(&out.join(" "));
        }
    }
    text.to_owned()
}

/// "Tanks." / "Tanks for the update" → "Thanks": the speech model drops the
/// "h" of a quick "thanks". Only where it opens a sentence and stands alone
/// or goes on as thanks does ("for", "a lot", "again", "so much", a name),
/// never "the tanks" or "fish tanks".
fn misheard_thanks(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    for (index, word) in words.iter().enumerate() {
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
        let opens = index == 0 || words[index - 1].ends_with(['.', '?', '!']);
        let next = words.get(index + 1).map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase());
        let alone = word.ends_with(['.', '!', ',']) || next.is_none();
        let thanking = matches!(next.as_deref(), Some("for" | "a" | "again" | "so" | "you" | "everyone" | "guys" | "all"));
        if bare == "Tanks" && opens && (alone || thanking) {
            out.push(word.replacen("Tanks", "Thanks", 1));
        } else {
            out.push((*word).to_owned());
        }
    }
    out.join(" ")
}

/// Nouns that follow a describing word, so "using black. Color if
/// possible." is one sentence cut by a pause.
const DESCRIBED_NOUNS: &[&str] = &[
    "color", "colour", "colors", "colours", "background", "border", "borders", "edge", "edges",
    "size", "format", "version", "mode", "theme", "style", "font", "text", "tone", "shade",
    "line", "lines", "fill", "bars", "padding", "margin", "margins", "outline",
];

/// Sentence fragments made by pauses, put back together:
///
/// - "Expand the image using black. Color if possible." -> "…using black
///   color if possible.": a lone describing noun and "if possible" (or "if
///   needed", "please") goes on the sentence before.
/// - "It should be YouTube resolutions. YouTube thumbnail resolution size."
///   -> "It should be YouTube thumbnail resolution size.": a short phrase
///   with no subject or verb that starts like the end of the sentence before
///   and says it more exactly (it adds words inside it) replaces that end.
fn rejoin_fragments(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut sentences: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for word in words {
        current.push(word.to_owned());
        if word.ends_with(['.', '?', '!']) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    let tail = current;
    let mut out: Vec<Vec<String>> = Vec::new();
    for sentence in sentences {
        if let Some(previous) = out.last_mut() {
            if let Some(joined) = fragment_onto(previous, &sentence) {
                *previous = joined;
                continue;
            }
        }
        out.push(sentence);
    }
    out.push(tail);
    out.into_iter().flatten().collect::<Vec<_>>().join(" ")
}

fn fragment_onto(previous: &[String], fragment: &[String]) -> Option<Vec<String>> {
    let last = previous.last()?;
    if !last.ends_with('.') || last.ends_with("..") || !fragment.last()?.ends_with('.') {
        return None;
    }
    let bare: Vec<String> = fragment.iter().map(|w| bare_word(w)).collect();
    let before: Vec<String> = previous.iter().map(|w| bare_word(w)).collect();
    let mut joined: Vec<String> = previous.to_vec();
    let strip = |word: &mut String| {
        word.pop();
    };
    // "Color if possible."
    let polite = matches!(
        bare[1..].iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
        ["if", "possible"] | ["if", "needed"] | ["if", "necessary"] | ["if", "you", "can"] | ["please"]
    );
    if fragment.len() >= 2 && polite && DESCRIBED_NOUNS.contains(&bare[0].as_str()) {
        strip(joined.last_mut()?);
        joined.push(fragment[0].to_lowercase());
        joined.extend(fragment[1..].iter().cloned());
        return Some(joined);
    }
    // "YouTube resolutions. YouTube thumbnail resolution size."
    if !(2..=6).contains(&fragment.len()) || previous.len() < 3 {
        return None;
    }
    let verbish = |w: &String| {
        RESTART_SUBJECTS.contains(&w.as_str())
            || RESTART_HELPERS.contains(&w.as_str())
            || matches!(w.as_str(), "is" | "are" | "be" | "was" | "were" | "has" | "have" | "had" | "i'm" | "it's")
    };
    if bare.iter().any(verbish) || fragment.iter().any(|w| w.contains(',')) {
        return None;
    }
    let end = before.last()?;
    let stem = end.strip_suffix('s').unwrap_or(end);
    for at in previous.len().saturating_sub(4)..previous.len() - 1 {
        if before[at] != bare[0] || previous[at..previous.len() - 1].iter().any(|w| w.contains(',')) {
            continue;
        }
        let tail_len = previous.len() - at;
        // It says the end again with more inside: the last word comes later.
        let again = bare.iter().position(|w| w.len() >= 3 && (w == end || w.starts_with(stem)));
        if again.is_some_and(|j| j >= tail_len) {
            joined.truncate(at);
            joined.extend(fragment.iter().cloned());
            return Some(joined);
        }
    }
    None
}

/// Words no sentence ends on.
const DANGLING_WORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "because", "become", "becomes", "became", "my", "your",
    "our", "their", "very", "than", "into",
];

/// Common words that can go on a sentence cut by a pause.
const CONTINUING_WORDS: &[&str] = &[
    "around", "about", "almost", "nearly", "roughly", "maybe", "more", "less", "much", "really",
    "too", "very", "only", "even", "over", "under", "of", "than", "like", "to",
];

/// "Also, I wrote this prompt using the voice dictation model. Also, I wrote
/// this prompt using the voice dictation.": the same sentence said again
/// straight away, a word or two longer or shorter. The second try is the one
/// meant. Only exact restatements count, so "Fix the login page. Fix the
/// signup page." keeps both.
fn drop_resaid_sentences(text: &str) -> String {
    let mut sentences: Vec<Vec<&str>> = vec![Vec::new()];
    for word in text.split_whitespace() {
        sentences.last_mut().unwrap().push(word);
        if word.ends_with(['.', '?', '!']) && !word.ends_with("..") {
            sentences.push(Vec::new());
        }
    }
    sentences.retain(|sentence| !sentence.is_empty());
    let bare = |sentence: &[&str]| -> Vec<String> {
        sentence.iter().map(|w| bare_word(w)).filter(|w| !w.is_empty()).collect()
    };
    let mut kept: Vec<&[&str]> = Vec::with_capacity(sentences.len());
    for (index, sentence) in sentences.iter().enumerate() {
        let resaid = sentences.get(index + 1).is_some_and(|next| {
            let (a, b) = (bare(sentence), bare(next));
            let shorter = a.len().min(b.len());
            shorter >= 5 && a.len().abs_diff(b.len()) <= 3 && a[..shorter] == b[..shorter]
        });
        if !resaid {
            kept.push(sentence);
        }
    }
    kept.iter().map(|sentence| sentence.join(" ")).collect::<Vec<_>>().join(" ")
}

/// Words the speech model mishears, fixed only where the context gives
/// them away: "Oh some kind of indication" after a sentence is "Or some kind
/// of…", and "on whoever of the island" is "on hover of the island".
fn misheard_words(text: &str) -> String {
    let mut words: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    let bare: Vec<String> = words.iter().map(|w| bare_word(w)).collect();
    let at = |i: usize| bare.get(i).map_or("", String::as_str);
    for index in 0..words.len() {
        let after_sentence = index > 0 && words[index - 1].ends_with(['.', '?', '!']);
        let alternative = matches!((at(index + 1), at(index + 2), at(index + 3)), ("some", "kind" | "sort", "of"));
        if after_sentence && words[index] == "Oh" && alternative {
            words[index] = "Or".to_owned();
        }
        if index > 0 && at(index - 1) == "on" && at(index + 1) == "of" && words[index].eq_ignore_ascii_case("whoever") {
            words[index] = "hover".to_owned();
        }
        // "I want it to chart after 8 PM": nothing charts after a time.
        let when_follows = matches!(at(index + 1), "after" | "before" | "at" | "from" | "around");
        if index > 0 && at(index - 1) == "to" && at(index) == "chart" && when_follows {
            let capital = words[index].starts_with('C');
            let rest = &words[index][5..];
            words[index] = format!("{}tart{rest}", if capital { "S" } else { "s" });
        }
    }
    words.join(" ")
}

/// Words that open a sentence going on from the last one ("Also, the island").
const OPENING_CONNECTIVES: &[&str] = &["also", "and", "so", "but", "plus", "then"];

/// "Also, the island. Eye animation should show": a pause cut a phrase in
/// two. A "sentence" that is only a connective and a two- or three-word
/// noun phrase says nothing yet, so the next words belong to it.
fn rejoin_opened_phrases(text: &str) -> String {
    let mut words: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    let mut start = 0;
    for index in 0..words.len() {
        if !words[index].ends_with('.') || words[index].ends_with("..") {
            continue;
        }
        let sentence: Vec<String> = words[start..=index].iter().map(|w| bare_word(w)).collect();
        let opened = (3..=4).contains(&sentence.len())
            && OPENING_CONNECTIVES.contains(&sentence[0].as_str())
            && DETERMINERS.contains(&sentence[1].as_str())
            && !words[start..index].iter().skip(1).any(|w| w.ends_with([',', ';', ':']));
        if opened && index + 1 < words.len() && plain_capitalised_noun(&words, index + 1) {
            words[index].pop();
            let next = &mut words[index + 1];
            *next = next.to_lowercase().chars().take(1).chain(next.chars().skip(1)).collect();
            continue;
        }
        start = index + 1;
    }
    words.join(" ")
}

/// Helper verbs that follow the subject of a sentence.
const HELPER_VERBS: &[&str] = &[
    "should", "will", "would", "can", "could", "must", "might", "may", "is", "are", "was", "were", "has",
    "have", "needs", "need", "shows", "show", "gets", "keeps", "starts",
];

/// "Eye animation should…": a capitalised everyday word, a lower-case word
/// that goes with it, then a helper verb. Not a name or greeting ("Sarah
/// will call", "Thanks for watching"), which have no second noun.
fn plain_capitalised_noun(words: &[String], index: usize) -> bool {
    let word = words[index].as_str();
    let mut letters = word.chars();
    let second = words.get(index + 1).map(String::as_str).unwrap_or("");
    let third = words.get(index + 2).map(|w| bare_word(w)).unwrap_or_default();
    word.chars().count() >= 2
        && letters.next().is_some_and(char::is_uppercase)
        && letters.all(|c| c.is_lowercase())
        && second.chars().all(|c| c.is_lowercase())
        && !second.is_empty()
        && !HELPER_VERBS.contains(&bare_word(second).as_str())
        && HELPER_VERBS.contains(&third.as_str())
}

/// "But before that you just check the per performance uh before that I
/// want you to check…": after an "uh", the speaker said the same opening
/// words again and went on differently. The first try, up to the "uh", is
/// dropped. Only within one sentence, a few words back, with no comma
/// between (a list or two clauses keep their words).
fn restart_after_filler(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let bare: Vec<String> = words.iter().map(|w| bare_word(w)).collect();
    let mut drop = vec![false; words.len()];
    for filler in 1..words.len().saturating_sub(2) {
        // "…to Anna and uh send the slides": a joining word goes on, it
        // does not start over.
        let joined = matches!(bare[filler - 1].as_str(), "and" | "or" | "then" | "but" | "so" | "also" | "plus");
        if !is_filler_sound(&bare[filler]) || joined || words[filler - 1].ends_with(['.', '?', '!', ',']) {
            continue;
        }
        let again = [&bare[filler + 1], &bare[filler + 2]];
        if again.iter().any(|w| w.is_empty()) {
            continue;
        }
        let earliest = filler.saturating_sub(14);
        let first = (earliest..filler.saturating_sub(2)).rev().find(|&start| {
            [&bare[start], &bare[start + 1]] == again
                && !words[start..filler].iter().any(|w| w.ends_with(['.', '?', '!', ',', ';', ':']))
        });
        if let Some(start) = first {
            drop[start..=filler].iter_mut().for_each(|d| *d = true);
        }
    }
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut capital = false;
    for (index, word) in words.iter().enumerate() {
        if drop[index] {
            // The first try opened the sentence: the second does now.
            capital |= words[index].chars().next().is_some_and(char::is_uppercase)
                && (index == 0 || words[index - 1].ends_with(['.', '?', '!']));
            continue;
        }
        let mut word = (*word).to_owned();
        if std::mem::take(&mut capital) {
            word = capitalized(&word);
        }
        out.push(word);
    }
    out.join(" ")
}

/// Words after which "to" starts a verb: "I want you to update…".
const WANTING_WORDS: &[&str] = &[
    "want", "wants", "wanted", "need", "needs", "needed", "like", "love", "ask", "asked", "tell", "told",
    "try", "trying", "able", "how", "allowed", "supposed", "meant", "wish", "hope", "plan", "decided",
];

/// Who a wish can be about: "want you to", "need them to".
const OBJECT_PRONOUNS: &[&str] = &["you", "me", "him", "her", "them", "us", "it", "everyone", "someone"];

/// Whether the "to" at `to` begins a verb ("want you to update"), not a
/// place or a name ("talk to Daniel", "go to Paris").
fn starts_verb<S: AsRef<str>>(words: &[S], to: usize) -> bool {
    let same_sentence = |i: usize| !words[i].as_ref().ends_with(['.', '?', '!', ',', ';', ':']);
    let word = |i: usize| bare_word(words[i].as_ref());
    if to == 0 || word(to) != "to" || !same_sentence(to - 1) {
        return false;
    }
    if WANTING_WORDS.contains(&word(to - 1).as_str()) {
        return true;
    }
    to >= 2
        && OBJECT_PRONOUNS.contains(&word(to - 1).as_str())
        && same_sentence(to - 2)
        && WANTING_WORDS.contains(&word(to - 2).as_str())
}

/// Words that open what a verb acts on: "update the game", "set it up".
const OBJECT_STARTS: &[&str] = &[
    "the", "a", "an", "this", "that", "these", "those", "it", "my", "your", "our", "their", "his", "her",
    "me", "him", "them", "us", "all", "some", "any", "every", "everything", "something", "more", "out",
    "up", "down", "off", "on", "after", "before", "at", "from", "until", "every", "once",
];

/// "Update" but not "I", "OK", "iPhone" or the start of a name ("Visual
/// Studio"): a word that is capitalised only because a sentence began there.
fn plain_capitalised<S: AsRef<str>>(words: &[S], index: usize) -> bool {
    let core = words[index].as_ref().trim_end_matches([',', ';', ':', '.', '!', '?']);
    let mut letters = core.chars();
    let name_follows = words
        .get(index + 1)
        .and_then(|next| next.as_ref().chars().next())
        .is_some_and(char::is_uppercase);
    // A verb is followed by what it acts on ("Update the game"); a name by
    // what it does ("Paris is lovely"), so a name is never lowered.
    let object_follows = words.get(index + 1).is_some_and(|next| {
        let next = next.as_ref();
        !core.ends_with(|c: char| !c.is_alphanumeric())
            && !words[index].as_ref().ends_with(['.', ',', ';', ':', '!', '?'])
            && OBJECT_STARTS.contains(&bare_word(next).as_str())
    });
    core.chars().count() >= 2
        && object_follows
        && !name_follows
        && letters.next().is_some_and(char::is_uppercase)
        && letters.all(|c| c.is_lowercase() || c == '\'' || c == '\u{2019}')
}

/// Describing words tried after "a" and then said again with another one.
const RESTARTED_ADJECTIVES: &[&str] = &[
    "proper", "real", "good", "nice", "great", "simple", "basic", "clean", "modern", "full", "complete",
    "decent", "solid", "quick", "small", "big", "little", "new", "realistic", "better", "short", "long",
    "fast", "smooth", "cool", "huge", "tiny", "fancy", "beautiful", "polished", "detailed", "professional",
    "high", "proper-looking", "nice-looking", "serious", "fully", "really", "very", "super",
];

/// "a proper. A realistic open world game" → "a proper, realistic open world
/// game": the speaker paused after a describing word and started the phrase
/// over with another, so both describe the same thing.
fn restarted_articles(text: &str) -> String {
    let mut words: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    let article = |w: &str| matches!(w.to_lowercase().as_str(), "a" | "an");
    let mut index = 0;
    while index + 3 < words.len() {
        let describing = words[index + 1].trim_end_matches(['.', ',']);
        let restarted = article(&words[index])
            && !words[index + 1].ends_with("..")
            && describing.chars().all(|c| c.is_alphabetic() || c == '-')
            && RESTARTED_ADJECTIVES.contains(&describing.to_lowercase().as_str())
            && article(&words[index + 2])
            && words[index + 3].chars().next().is_some_and(char::is_alphabetic);
        if restarted {
            words[index + 1] = format!("{describing},");
            words.remove(index + 2);
        }
        index += 1;
    }
    words.join(" ")
}

/// "…to Daniel? before 5pm." → "…to Daniel before 5pm?", "pricing. and the
/// dates" → "pricing and the dates": the speech model put a stop where you
/// paused, but the next word, in lower case, shows the sentence went on.
fn continue_sentences(text: &str) -> String {
    const ABBREVIATIONS: &[&str] = &["mr.", "mrs.", "ms.", "dr.", "vs.", "etc.", "e.g.", "i.e.", "st.", "no."];
    let mut words: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    // Words that join "I …" into the sentence before it ("…that I want").
    const CLAUSE_LINKS: &[&str] = &[
        "and", "but", "so", "or", "because", "if", "when", "that", "what", "how", "why", "where", "which", "who",
        "as", "than", "since", "until", "while", "before", "after", "think", "know", "mean", "say", "said", "hope",
        "guess", "whether", "unless", "once",
    ];
    let mut question_pending = false;
    // Where the question's stop was taken off.
    let mut merged_at = 0;
    for index in 0..words.len() {
        // "…the AI model? like the Gemma model to fix the dictation I want to
        // explain it…": the question went on after the pause, but "I want"
        // with no stop before it starts the next sentence, so the question
        // ends before it, not at the next stop.
        if question_pending && index >= merged_at + 4 && index + 1 < words.len() && words[index] == "I" {
            let previous = &words[index - 1];
            let opens = matches!(
                bare_word(&words[index + 1]).as_str(),
                "want" | "wanted" | "need" | "needed" | "would" | "also" | "just" | "really"
            );
            let plain = previous.chars().last().is_some_and(char::is_alphanumeric);
            if opens && plain && !CLAUSE_LINKS.contains(&bare_word(previous).as_str()) {
                words[index - 1].push('?');
                question_pending = false;
            }
        }
        let command_follows = matches!(
            (words.get(index + 1).map(|w| bare_word(w)).as_deref(), words.get(index + 2).map(|w| bare_word(w)).as_deref()),
            (Some("new"), Some("paragraph" | "line" | "bullet")) | (Some("bullet"), Some("point")) | (Some("next"), Some("bullet"))
        );
        let next_lower = !command_follows
            && words
                .get(index + 1)
                .and_then(|w| w.chars().next())
                .is_some_and(|c| c.is_lowercase());
        let word = &words[index];
        let mark = word.chars().last();
        let stop = matches!(mark, Some('.' | '?')) && !word.ends_with("..");
        let abbreviation = ABBREVIATIONS.contains(&word.to_lowercase().as_str())
            || word.trim_end_matches('.').contains('.')
            || word.chars().rev().nth(1).is_some_and(|c| c.is_ascii_digit());
        if stop && next_lower && !abbreviation {
            if mark == Some('?') && !question_pending {
                question_pending = true;
                merged_at = index;
            }
            words[index].pop();
            continue;
        }
        // "So the 3 minutes long will become. Around 5 to 6 minutes": no
        // sentence ends on "become" or "the", so the pause was mid-sentence.
        // Only when the next word is a common one, safe to lower.
        if mark == Some('.') && !word.ends_with("..") && !command_follows {
            let dangling = DANGLING_WORDS.contains(&bare_word(word).as_str());
            let next = words.get(index + 1).map(|w| bare_word(w));
            // Not "the", "it" or "we": those open a new sentence as often.
            let common = next.as_deref().is_some_and(|w| CONTINUING_WORDS.contains(&w));
            // "I want you to. Update the game": "to" after a wish starts a
            // verb, and the verb is what follows the pause.
            let verb_follows = next.is_some()
                && starts_verb(&words, index)
                && plain_capitalised(&words, index + 1);
            if (dangling && common) || verb_follows {
                words[index].pop();
                let next = &mut words[index + 1];
                *next = next.to_lowercase().chars().take(1).chain(next.chars().skip(1)).collect();
            }
        }
        if question_pending && matches!(mark, Some('.' | '!')) {
            words[index].pop();
            words[index].push('?');
            question_pending = false;
        }
    }
    if question_pending {
        if let Some(last) = words.last_mut() {
            last.push('?');
        }
    }
    words.join(" ")
}

/// The last touches on what gets typed: a capital at the start of every
/// bullet, and currency signs spaced as written ("not $25,000", "$30,000").
#[must_use]
pub fn finish_layout(text: &str) -> String {
    let text = &even_list_stops(text);
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let line = match line.strip_prefix("- ") {
            Some(item) => format!("- {}", capitalized(&stray_capitals(item))),
            None => stray_capitals(line),
        };
        let chars: Vec<char> = line.chars().collect();
        let mut fixed = String::with_capacity(line.len() + 4);
        for (at, &c) in chars.iter().enumerate() {
            let currency = matches!(c, '$' | '\u{20b9}' | '\u{20ac}' | '\u{a3}');
            if currency && at > 0 && !chars[at - 1].is_whitespace() && chars[at - 1] != '(' && chars[at + 1..].first().is_some_and(char::is_ascii_digit) {
                fixed.push(' ');
            }
            if c == ' ' && at > 0 && matches!(chars[at - 1], '$' | '\u{20b9}' | '\u{20ac}' | '\u{a3}') && chars.get(at + 1).is_some_and(char::is_ascii_digit) {
                continue;
            }
            fixed.push(c);
        }
        out.push_str(&fixed);
    }
    out
}

/// Small words that are only capitalised at the start of a sentence.
const SENTENCE_START_ONLY: &[&str] = &[
    "about", "after", "also", "an", "and", "are", "as", "at", "because", "before", "but", "by", "can",
    "could", "do", "does", "for", "from", "he", "here", "how", "if", "in", "into", "is", "it", "it's",
    "just", "maybe", "my", "not", "of", "on", "onto", "or", "our", "please", "she", "should", "so",
    "than", "that", "the", "their", "then", "there", "these", "they", "this", "those", "to", "was",
    "we", "what", "when", "which", "who", "will", "with", "would", "you", "your",
];

/// "can you increase the hover On hover time": after a pause the speech
/// model starts a new sentence, and once the pieces are joined a small word
/// keeps its capital in mid-sentence. Lowercases those words when they
/// follow a lowercase word (not after a stop, and not inside a run of
/// capitalised words, which is likely a name or title).
fn stray_capitals(line: &str) -> String {
    let words: Vec<&str> = line.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    for (index, word) in words.iter().enumerate() {
        let core = word.trim_end_matches([',', ';', ':', '.', '!', '?']);
        let after_lowercase = index > 0 && {
            let previous = words[index - 1];
            !previous.ends_with(['.', '!', '?', ':', ';', '"', '\u{201d}'])
                && previous.chars().next().is_some_and(|c| c.is_lowercase() || c.is_ascii_digit())
        };
        // "play The Weeknd": the start of a capitalised run, a name. "I" is
        // always a capital, so "centric, And I want" is not one.
        let starts_name = words.get(index + 1).is_some_and(|next| {
            let bare = next.trim_end_matches([',', ';', ':', '.', '!', '?']).replace('\u{2019}', "'");
            next.chars().next().is_some_and(char::is_uppercase)
                && !matches!(bare.as_str(), "I" | "I'm" | "I'll" | "I've" | "I'd")
        });
        let mut letters = core.chars();
        let capitalised = !starts_name
            && letters.next().is_some_and(char::is_uppercase)
            && letters.all(|c| c.is_lowercase() || c == '\'' || c == '\u{2019}');
        let small_word = SENTENCE_START_ONLY.contains(&core.to_lowercase().replace('\u{2019}', "'").as_str());
        // "want you to Update": a verb after "to", not a place or name.
        let verb = index > 0 && starts_verb(&words, index - 1) && plain_capitalised(&words, index);
        if after_lowercase && capitalised && (small_word || verb) {
            out.push(word.to_lowercase());
        } else {
            out.push((*word).to_owned());
        }
    }
    out.join(" ")
}

/// In each run of bullets, items end the same way: with a full stop when
/// most do (or half), bare when most don't.
fn even_list_stops(text: &str) -> String {
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].starts_with("- ") {
            index += 1;
            continue;
        }
        let end = (index..lines.len()).find(|&i| !lines[i].starts_with("- ")).unwrap_or(lines.len());
        let stopped = lines[index..end].iter().filter(|l| l.ends_with(['.', '!', '?'])).count();
        let bare = lines[index..end].iter().filter(|l| l.ends_with(|c: char| c.is_alphanumeric())).count();
        for line in &mut lines[index..end] {
            if stopped * 2 >= stopped + bare && line.ends_with(|c: char| c.is_alphanumeric()) {
                line.push('.');
            } else if stopped * 2 < stopped + bare && line.ends_with('.') && !line.ends_with("..") {
                line.pop();
            }
        }
        index = end;
    }
    lines.join("\n")
}

/// Small words that say nothing about what a sentence is about.
const FUNCTION_WORDS: &[&str] = &[
    "the", "a", "an", "this", "that", "these", "those", "for", "to", "of", "and", "or", "but",
    "in", "on", "at", "by", "with", "from", "is", "are", "was", "were", "be", "it", "its", "i",
    "you", "we", "they", "he", "she", "me", "my", "your", "our", "if", "so", "can", "could",
    "would", "will", "do", "does", "did", "just", "like", "some", "any", "not", "have", "has",
    "should", "must", "may", "might", "let", "let's", "about", "also", "then", "there", "here",
];

/// "…search for a certain mus— I'm sorry. If I open Spotify and search for
/// a certain music…": a sentence started, given up with "sorry", and begun
/// again. The unfinished start and the apology go.
///
/// Only when the apology closes its phrase, the start is short, and the
/// next sentence picks up at least two of its words (a cut-off word counts
/// towards the whole one). When speech recognition already ended the start
/// with a full stop ("…should follow the system. I'm sorry. Can the dark
/// mode toggle follow the system theme…"), it looks like a finished
/// sentence, so the next one must repeat at least three of its words and
/// most of them. A real apology ("I'm late, I'm sorry. I missed the bus.")
/// shares little and stays.
fn abandoned_start(tokens: &[Token], out: &mut Vec<Token>, cue_end: usize) -> Option<usize> {
    let last_cue = &tokens[cue_end - 1];
    if norm(last_cue) != "sorry" || !(closes_phrase(last_cue) || cue_end == tokens.len()) {
        return None;
    }
    let finished = out.last().is_some_and(Token::ends_sentence);
    let before_end = out.len() - usize::from(finished);
    let start = out[..before_end].iter().rposition(Token::ends_sentence).map_or(0, |at| at + 1);
    let fragment = &out[start..];
    if fragment.is_empty() || fragment.len() > 12 || fragment.iter().any(|t| t.verbatim) {
        return None;
    }
    let mut end = cue_end;
    while end < tokens.len() {
        end += 1;
        if tokens[end - 1].ends_sentence() {
            break;
        }
    }
    let content = |token: &Token| {
        let word = norm(token);
        (word.len() >= 3 && !FUNCTION_WORDS.contains(&word.as_str())).then_some(word)
    };
    let before: Vec<String> = fragment.iter().filter_map(content).collect();
    let after: Vec<String> = tokens[cue_end..end].iter().filter_map(content).collect();
    let shared = before
        .iter()
        .filter(|a| after.iter().any(|b| a == &b || b.starts_with(a.as_str()) || a.starts_with(b.as_str())))
        .count();
    let enough = if finished {
        shared >= 3 && shared * 5 >= before.len() * 3
    } else {
        shared >= 2
    };
    if !enough {
        return None;
    }
    out.truncate(start);
    let Some(next) = tokens.get(cue_end) else {
        return Some(cue_end);
    };
    let mut next = next.clone();
    if out.last().is_none_or(clause_boundary) {
        capitalize(&mut next);
    }
    out.push(next);
    Some(cue_end + 1)
}

/// Words "never" belongs to when it is part of the sentence: "I never mind
/// the noise", "you'd never mind it", "try to never mind them".
const NEVER_OWNERS: &[&str] = &[
    "i", "you", "we", "they", "he", "she", "it", "who", "that", "to", "would", "will", "i'd",
    "you'd", "we'd", "they'd", "he'd", "she'd", "really", "just", "almost", "and", "or", "but",
];

/// Most words a "never mind" gives up: a short unfinished start, not a
/// paragraph of meant text.
const GIVE_UP_WORDS: usize = 16;

/// "I want you to create never mind. Sorry. First, I want you to…": a
/// sentence started and given up with "never mind" (and perhaps a "sorry").
/// The unfinished start, the "never mind" and the apology go.
///
/// Only when the start runs straight into "never mind" with no punctuation
/// between (a finished "Did you see it? Never mind." stays), "never mind"
/// (or its apology) closes the phrase, the start is short, and the word
/// before is not one "never" would belong to ("I never mind the noise.").
fn given_up(tokens: &[Token], out: &mut Vec<Token>, cue_end: usize) -> Option<usize> {
    let before = out.last()?;
    if clause_boundary(before) || closes_phrase(before) || NEVER_OWNERS.contains(&norm(before).as_str()) {
        return None;
    }
    let mut end = cue_end;
    for apology in [&["sorry"][..], &["i'm", "sorry"], &["i", "am", "sorry"]] {
        let said = apology.iter().enumerate().all(|(offset, word)| {
            tokens.get(end + offset).is_some_and(|token| !token.verbatim && norm(token) == *word)
        });
        let last = end + apology.len();
        if said && (last == tokens.len() || closes_phrase(&tokens[last - 1])) {
            end += apology.len();
            break;
        }
    }
    if !(closes_phrase(&tokens[end - 1]) || end == tokens.len()) {
        return None;
    }
    let start = out.iter().rposition(Token::ends_sentence).map_or(0, |at| at + 1);
    if out.len() - start > GIVE_UP_WORDS || out[start..].iter().any(|token| token.verbatim) {
        return None;
    }
    out.truncate(start);
    let Some(next) = tokens.get(end) else {
        return Some(end);
    };
    let mut next = next.clone();
    if out.last().is_none_or(clause_boundary) {
        capitalize(&mut next);
    }
    out.push(next);
    Some(end + 1)
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
    "can",
    "can't",
    "could",
    "couldn't",
    "should",
    "shouldn't",
    "would",
    "wouldn't",
    "do",
    "don't",
    "does",
    "doesn't",
    "is",
    "isn't",
    "are",
    "aren't",
    "will",
    "won't",
    "what",
    "why",
    "how",
    "i",
    "we",
    "let's",
    "it",
    "they",
];

/// Who a restarted clause is about.
const RESTART_SUBJECTS: &[&str] = &["it", "i", "we", "you", "they", "he", "she", "this", "that"];

/// The helper verb a restarted clause opens with after its subject.
const RESTART_HELPERS: &[&str] = &[
    "will", "won't", "would", "wouldn't", "does", "doesn't", "do", "don't", "did", "didn't", "is",
    "isn't", "was", "wasn't", "can", "can't", "could", "couldn't", "should", "shouldn't", "might",
];

/// Words that join two real clauses, so the same subject after one is meant.
const CLAUSE_JOINERS: &[&str] = &["and", "but", "or", "so", "because", "if", "when", "then", "while", "until"];

/// "it will just sound l does not it wouldn't it will not sound like a
/// loop" -> "it will not sound like a loop": the same subject and helper verb
/// begun again and again, with no punctuation or joining word between, are
/// tries at one clause. The last try stays.
///
/// Only when the last try picks up a word from the earlier ones ("sound"),
/// or an earlier one broke off mid-word ("l"); "it is what it is" stays.
fn drop_subject_restarts(tokens: Vec<Token>) -> Vec<Token> {
    let plain = |token: &Token| !token.verbatim && token.lead.is_empty() && token.trail.is_empty();
    let opens = |at: usize| {
        tokens.get(at).is_some_and(|t| plain(t) && RESTART_SUBJECTS.contains(&norm(t).as_str()))
            && tokens.get(at + 1).is_some_and(|t| plain(t) && RESTART_HELPERS.contains(&norm(t).as_str()))
    };
    let content = |token: &Token| {
        let word = norm(token);
        (word.len() >= 3 && !FUNCTION_WORDS.contains(&word.as_str()) && !RESTART_HELPERS.contains(&word.as_str()))
            .then_some(word)
    };
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        if opens(index) {
            let mut last_try = None;
            let mut j = index + 2;
            while j < tokens.len().min(index + 14) {
                let between = &tokens[index + 2..j];
                if !between.iter().all(plain) || between.iter().any(|t| CLAUSE_JOINERS.contains(&norm(t).as_str())) {
                    break;
                }
                if norm(&tokens[j]) == norm(&tokens[index]) && opens(j) {
                    let tried: Vec<String> = tokens[index + 2..j].iter().filter_map(content).collect();
                    let picked_up = tokens[j + 2..tokens.len().min(j + 7)]
                        .iter()
                        .filter_map(content)
                        .any(|word| tried.contains(&word));
                    let broke_off = between
                        .iter()
                        .any(|t| t.word.chars().count() == 1 && !matches!(norm(t).as_str(), "a" | "i"));
                    if picked_up || broke_off {
                        last_try = Some(j);
                    }
                }
                j += 1;
            }
            if let Some(j) = last_try {
                let mut kept = tokens[j].clone();
                if tokens[index].word.starts_with(char::is_uppercase) {
                    capitalize(&mut kept);
                }
                out.push(kept);
                index = j + 1;
                continue;
            }
        }
        out.push(tokens[index].clone());
        index += 1;
    }
    out
}

/// A bare number word or numeral, as a value.
fn small_number(token: &Token) -> Option<i64> {
    if token.verbatim && !token.word.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let word = norm(token);
    if let Ok(value) = word.parse() {
        return Some(value);
    }
    const WORDS: &[&str] = &[
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
        "nineteen", "twenty",
    ];
    WORDS.iter().position(|w| *w == word).map(|at| at as i64)
}

/// "around 6 5 to 6 minutes" -> "around 5 to 6 minutes": a number said and
/// at once replaced by a range it belongs to (its end, or next to its start).
fn drop_number_restarts(tokens: Vec<Token>) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::with_capacity(tokens.len());
    for (index, token) in tokens.iter().enumerate() {
        let range = (|| {
            let first = small_number(tokens.get(index + 1)?)?;
            (norm(tokens.get(index + 2)?) == "to" && tokens[index + 1].trail.is_empty())
                .then_some(())?;
            Some((first, small_number(tokens.get(index + 3)?)?))
        })();
        if let (Some(said), Some((from, to))) = (small_number(token).filter(|_| token.trail.is_empty() && token.lead.is_empty()), range) {
            if from != said && (said == to || (from < said && said < to) || (said - from).abs() == 1) {
                continue;
            }
        }
        out.push(token.clone());
    }
    out
}

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
            // "it" after a verb is its object ("I like it like that"), not
            // a second start.
            if a != b
                && b != "it"
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
            if matches!(cue, Cue::Replace { strong: true })
                && corrections::reverse_denial(&tokens, index + len)
            {
                if let Some(end) = corrections::reverse_recipient(&tokens, &mut out, index + len) {
                    index = end;
                } else {
                    out.extend_from_slice(&tokens[index..index + len]);
                    index += len;
                }
                continue;
            }
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
    if matches!(cue, Cue::Replace { .. })
        && tokens.get(cue_end).is_some_and(|t| {
            matches!(
                norm(t).as_str(),
                "i" | "we" | "you" | "he" | "she" | "they" | "it"
            )
        })
        && tokens.get(cue_end + 1).is_some_and(|t| {
            matches!(
                norm(t).as_str(),
                "am" | "is"
                    | "are"
                    | "was"
                    | "were"
                    | "have"
                    | "has"
                    | "do"
                    | "does"
                    | "can"
                    | "will"
                    | "should"
            )
        })
    {
        return corrections::verified_restate(tokens, out, cue_end);
    }
    if matches!(cue, Cue::Scoped) && out.last().is_some_and(Token::ends_sentence) {
        if norm(&tokens[index]) == "no" {
            return None;
        }
        return corrections::scoped_after_sentence(tokens, out, cue_end)
            .or_else(|| abandoned_start(tokens, out, cue_end));
    }
    if !matches!(
        cue,
        Cue::Contrast | Cue::VerifiedRestate | Cue::Scratch | Cue::Restate | Cue::GiveUp
    ) && (!matches!(cue, Cue::Replace { strong: false }) || !closes_phrase(last_cue))
    {
        if let Some(end) = corrections::apply(tokens, out, cue_end, !matches!(cue, Cue::Scoped)) {
            return Some(end);
        }
    }
    match cue {
        Cue::Contrast => corrections::contrast(tokens, out, cue_end),
        Cue::VerifiedRestate => corrections::verified_restate(tokens, out, cue_end),
        Cue::Scoped => abandoned_start(tokens, out, cue_end),
        Cue::GiveUp => given_up(tokens, out, cue_end),
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
            if redo.len() < 2 {
                return corrections::apply(tokens, out, cue_end, true);
            }
            if redo.iter().any(|t| t.verbatim) {
                return None;
            }
            // The redo starts with the same words the first try did.
            let start = (0..out.len().saturating_sub(1))
                .rev()
                .find(|&at| out[at].key() == redo[0].key() && out[at + 1].key() == redo[1].key());
            let Some(start) = start else {
                return corrections::apply(tokens, out, cue_end, true);
            };
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
            let wanted = corrections::replacement_words(tokens, cue_end, out).min(CORRECTION_WORDS);
            while replacement.len() < wanted {
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
                if removable == replacement.len()
                    || is_break(&out[position])
                    || (out[position].verbatim && out[position].word.starts_with(['"', '\u{201c}']))
                {
                    break;
                }
                if !last && out[position].ends_sentence() {
                    break;
                }
                removable += 1;
            }
            let crosses_sentence = out.last().is_some_and(Token::ends_sentence);
            if removable == 0 || (!strong && crosses_sentence) {
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
    "pointers",
    "points",
    "items",
    "things",
    "features",
    "steps",
    "ideas",
    "options",
    "tasks",
    "reasons",
    "requirements",
    "notes",
    "changes",
    "problems",
    "issues",
    "questions",
    "goals",
    "suggestions",
    "tips",
    "ways",
    "examples",
    "parts",
    "bugs",
    "fixes",
    "apps",
    "screens",
    "pages",
    "rules",
    "priorities",
    "todos",
    "topics",
    "benefits",
    "pros",
    "cons",
    "concerns",
    "updates",
    "areas",
    "list",
    "agenda",
    "stuff",
];

/// Words that announce a number of something: "here are", "a few", "three".
const LIST_ANNOUNCERS: &[&str] = &[
    "here are",
    "here's",
    "these are",
    "the following",
    "list of",
    "a few",
    "a couple",
    "some",
    "several",
    "multiple",
    "few",
    "two",
    "three",
    "four",
    "five",
    "six",
    "2",
    "3",
    "4",
    "5",
    "my",
    "our",
    "main",
    "key",
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
    ("start with", Marker::Rank(1)),
    ("first of all", Marker::Rank(1)),
    ("number one", Marker::Rank(1)),
    ("number 1", Marker::Rank(1)),
    ("point 1", Marker::Rank(1)),
    ("step 1", Marker::Rank(1)),
    ("point one", Marker::Rank(1)),
    ("step one", Marker::Rank(1)),
    ("firstly", Marker::Rank(1)),
    ("first", Marker::Rank(1)),
    ("number two", Marker::Rank(2)),
    ("number 2", Marker::Rank(2)),
    ("point 2", Marker::Rank(2)),
    ("step 2", Marker::Rank(2)),
    ("point two", Marker::Rank(2)),
    ("step two", Marker::Rank(2)),
    ("secondly", Marker::Rank(2)),
    ("second", Marker::Rank(2)),
    ("number three", Marker::Rank(3)),
    ("number 3", Marker::Rank(3)),
    ("point 3", Marker::Rank(3)),
    ("step 3", Marker::Rank(3)),
    ("point three", Marker::Rank(3)),
    ("step three", Marker::Rank(3)),
    ("thirdly", Marker::Rank(3)),
    ("third", Marker::Rank(3)),
    ("number four", Marker::Rank(4)),
    ("number 4", Marker::Rank(4)),
    ("point 4", Marker::Rank(4)),
    ("step 4", Marker::Rank(4)),
    ("point four", Marker::Rank(4)),
    ("step four", Marker::Rank(4)),
    ("fourth", Marker::Rank(4)),
    ("number five", Marker::Rank(5)),
    ("number 5", Marker::Rank(5)),
    ("point 5", Marker::Rank(5)),
    ("step 5", Marker::Rank(5)),
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

fn marker_prefix(sentence: &str) -> Option<(Marker, &str)> {
    MARKERS.iter().find_map(|(phrase, marker)| {
        let head = sentence.get(..phrase.len())?;
        let rest = &sentence[phrase.len()..];
        let whole_word = rest.chars().next().is_none_or(|c| !c.is_alphanumeric());
        (head.eq_ignore_ascii_case(phrase) && whole_word)
            .then(|| (*marker, rest.trim_start_matches([',', ':', ' ', '-', '.'])))
    })
}

fn strip_words<'a>(text: &'a str, words: &str) -> Option<&'a str> {
    let head = text.get(..words.len())?;
    let rest = &text[words.len()..];
    (head.eq_ignore_ascii_case(words) && rest.chars().next().is_none_or(|c| !c.is_alphanumeric()))
        .then(|| rest.trim_start_matches([',', ':', ' ', '-', '.']))
}

/// Recognize explicit item scaffolding without treating "the first car"
/// or other ordinary ordinal descriptions as list markers.
fn named_marker(sentence: &str) -> Option<(Marker, &str)> {
    let sentence = strip_words(sentence, "the").unwrap_or(sentence);
    let (marker, rest) = marker_prefix(sentence)?;
    if !matches!(marker, Marker::Rank(_)) {
        return None;
    }
    for link in ["will be to", "will be", "is to", "is"] {
        if let Some(body) = strip_words(rest, link) {
            return Some((marker, body));
        }
    }
    for noun in [
        "one",
        "feature",
        "point",
        "pointer",
        "item",
        "step",
        "thing",
        "task",
        "option",
        "requirement",
        "suggestion",
        "reason",
    ] {
        let Some(after) = strip_words(rest, noun) else {
            continue;
        };
        if after.is_empty() {
            return Some((marker, after));
        }
        for link in ["will be to", "will be", "is to", "is"] {
            if let Some(body) = strip_words(after, link) {
                return Some((marker, body));
            }
        }
    }
    None
}

/// "And the second feature will be to sync." -> (Rank(2), "sync.").
fn split_marker(sentence: &str) -> Option<(Marker, &str)> {
    let mut sentence = sentence;
    for _ in 0..3 {
        if let Some(rest) = strip_words(sentence, "and").or_else(|| strip_words(sentence, "so")) {
            sentence = rest;
        } else {
            break;
        }
    }
    named_marker(sentence).or_else(|| marker_prefix(sentence))
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
    let mut quoted = None;
    while let Some((at, c)) = chars.next() {
        if quoted == Some(c) {
            quoted = None;
        } else if quoted.is_none() && (c == '"' || c == '\u{201c}') {
            quoted = Some(if c == '"' { '"' } else { '\u{201d}' });
        }
        if quoted.is_none()
            && matches!(c, '.' | '?' | '!' | ':')
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
        let trimmed = out.trim_end_matches(['.', ':', ',', ';']).len();
        out.truncate(trimmed);
        out.push(':');
    }
    out
}

fn explicit_bullets(text: &str) -> bool {
    let text = text.to_lowercase();
    ["bullet points", "make a list", "as bullets"]
        .iter()
        .any(|phrase| text.contains(phrase))
}

/// "First, X. Second, Y. Finally, Z." -> one bullet per spoken marker; the
/// sentences between markers stay with their item. Needs a "first" and a
/// "second" so a lone "First, ..." is never a list.
fn spoken_list(parts: &[&str]) -> Option<String> {
    let first = parts
        .iter()
        .position(|part| split_marker(part).is_some_and(|(m, _)| m == Marker::Rank(1)))?;
    let ranked = parts[first + 1..]
        .iter()
        .any(|part| split_marker(part).is_some_and(|(m, _)| m == Marker::Rank(2)));
    let requested = explicit_bullets(&parts[..first].join(" "))
        && parts[first + 1..]
            .iter()
            .filter(|part| split_marker(part).is_some_and(|(m, _)| m == Marker::Continue))
            .count()
            >= 2;
    if !ranked && !requested {
        return None;
    }
    let mut items: Vec<String> = Vec::new();
    // A new list asked for after this one ("Also make a list of …") ends it.
    let mut next_block = None;
    for (offset, part) in parts[first..].iter().enumerate() {
        if !items.is_empty() && split_marker(part).is_none() && explicit_bullets(part) {
            next_block = Some(first + offset);
            break;
        }
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
    let mut out = even_stops(out);
    if let Some(at) = next_block {
        out.push('\n');
        out.push_str(&bulletize(parts[at..].join(" ")));
    }
    Some(out)
}

/// Splits "To set it up, step one, download the installer." into its
/// lead-in and the first item, when a second item follows later; otherwise
/// returns the sentences unchanged.
fn split_lead_in<'a>(parts: &[&'a str]) -> Vec<&'a str> {
    let rank = |part: &str, wanted: u8| split_marker(part).is_some_and(|(m, _)| m == Marker::Rank(wanted));
    for (index, part) in parts.iter().enumerate() {
        if split_marker(part).is_some() {
            continue;
        }
        let later_second = parts[index + 1..].iter().any(|later| rank(later, 2));
        if !later_second {
            continue;
        }
        let split = part
            .match_indices(", ")
            .map(|(at, _)| at)
            .find(|&at| rank(&part[at + 2..], 1));
        if let Some(at) = split {
            let mut out: Vec<&str> = parts[..index].to_vec();
            out.push(&part[..at]);
            out.push(&part[at + 2..]);
            out.extend_from_slice(&parts[index + 1..]);
            return out;
        }
    }
    parts.to_vec()
}

/// "Make a list of things to pack: passport, charger, headphones and a
/// jacket." → a bullet per item. Only when a list was asked for, the items
/// end the dictation, and there are three or more short ones. A counted
/// lead-in ("I need 3 things: milk, eggs and bread") counts as asking when
/// the count matches the items.
fn inline_list(parts: &[&str]) -> Option<String> {
    if parts.len() < 2 {
        return colon_less_list(parts);
    }
    let intro = parts.len() - 2;
    let asked = explicit_bullets(&parts[..=intro].join(" "));
    // "I need three things: ..." anywhere before the colon; after a full
    // stop only when the sentence ends on the count ("I need three things.").
    let counted = if parts[intro].ends_with(':') {
        announced_count(parts[intro])
    } else if parts[intro].ends_with('.') {
        counted_lead(parts[intro])
    } else {
        None
    };
    let introduced = parts[intro].ends_with(':')
        || (parts[intro].ends_with('.') && explicit_bullets(parts[intro]))
        || counted.is_some();
    if !introduced || (!asked && counted.is_none()) {
        return colon_less_list(parts);
    }
    let body = parts[intro + 1].trim_end_matches(['.', '!', ';']);
    let mut items: Vec<&str> = body.split(", ").collect();
    let last = items.pop()?;
    let last = last.strip_prefix("and ").or_else(|| last.strip_prefix("or ")).unwrap_or(last);
    let (before, after) = last
        .split_once(" and ")
        .or_else(|| last.split_once(" or "))
        .unwrap_or((last, ""));
    items.push(before);
    if !after.is_empty() {
        items.push(after);
    }
    let items: Vec<&str> = items.into_iter().map(str::trim).filter(|item| !item.is_empty()).collect();
    if items.iter().any(|item| item.split_whitespace().count() > 6) {
        return None;
    }
    match counted {
        // "Two things: X and Y" is a list too, but only if the count fits.
        Some(count) if !asked && (count != items.len() || count < 2) => return None,
        None if items.len() < 3 => return None,
        _ if items.len() < 2 => return None,
        _ => {}
    }
    let mut out = heading(&parts[..=intro]);
    for item in items {
        out.push_str("\n- ");
        out.push_str(&capitalized(item));
    }
    Some(out)
}

/// "Make a list of things to pack for a passport, charger, headphones and
/// a jacket": the colon was not heard, so the first item starts after the
/// last small linking word before the first comma.
fn colon_less_list(parts: &[&str]) -> Option<String> {
    const LINKS: &[&str] = &[
        "for", "to", "of", "with", "including", "include", "like", "are", "is", "a", "an", "the",
        "some", "these", "those", "my", "our",
    ];
    let last = *parts.last()?;
    if let Some(list) = counted_list(parts) {
        return Some(list);
    }
    if !explicit_bullets(last) || last.matches(", ").count() < 1 {
        return None;
    }
    let (head, tail) = last.split_once(", ")?;
    let words: Vec<&str> = head.split_whitespace().collect();
    let boundary = words.iter().rposition(|w| LINKS.contains(&w.to_lowercase().as_str()))?;
    if boundary + 1 >= words.len() {
        return None;
    }
    let mut lead_end = boundary + 1;
    while lead_end > 0 && LINKS.contains(&words[lead_end - 1].to_lowercase().as_str()) {
        lead_end -= 1;
    }
    let lead = words[..lead_end].join(" ");
    if !explicit_bullets(&lead) {
        return None;
    }
    let first = words[boundary + 1..].join(" ");
    let body = format!("{first}, {tail}");
    let mut lead_parts: Vec<&str> = parts[..parts.len() - 1].to_vec();
    let lead_line = format!("{lead}:");
    lead_parts.push(&lead_line);
    let list = format!("{}\n{}", lead_parts.join(" "), body);
    let lines: Vec<&str> = list.splitn(2, '\n').collect();
    inline_list(&[lines[0], lines[1]]).filter(|out| out.contains("\n- "))
}

/// An ASR result may omit every sentence stop. Require an explicit list
/// announcement followed by consecutive numbered markers; never split on
/// "or" or an ordinary continuation inside an item.
fn unpunctuated_list(text: &str) -> Option<String> {
    if text.contains(['"', '\u{201c}', '\u{201d}']) {
        return None;
    }
    let mut cursor = 0;
    let mut markers = Vec::new();
    for word in text.split_whitespace() {
        let at = cursor + text[cursor..].find(word)?;
        cursor = at + word.len();
        if let Some((marker, body)) = split_marker(&text[at..]) {
            let body_at = text.len() - body.len();
            if !markers
                .iter()
                .any(|&(_, _, previous_body)| previous_body == body_at)
            {
                markers.push((marker, at, body_at));
            }
        }
    }
    let first = markers
        .iter()
        .position(|&(marker, _, _)| marker == Marker::Rank(1))?;
    let (_, start, _) = markers[first];
    let lead = text[..start].trim();
    let last = lead
        .split_whitespace()
        .last()?
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    let requested = explicit_bullets(lead);
    if !(introduces_list(lead) || requested)
        || !(LIST_NOUNS.contains(&last.as_str())
            || matches!(last.as_str(), "want" | "include" | "need"))
    {
        return None;
    }
    let mut chosen = vec![markers[first]];
    let mut expected = 2;
    for &(marker, at, body) in &markers[first + 1..] {
        let tail = &text[at..];
        if marker == Marker::Rank(expected)
            || (requested
                && marker == Marker::Continue
                && ["next", "finally", "and finally"]
                    .iter()
                    .any(|marker| strip_words(tail, marker).is_some()))
        {
            chosen.push((marker, at, body));
            expected += 1;
        }
    }
    if chosen.len() < 2
        || (!chosen.iter().any(|&(m, _, _)| m == Marker::Rank(2)) && chosen.len() < 3)
    {
        return None;
    }
    let mut out = capitalized(&heading(&[lead]));
    for (index, &(_, _, body)) in chosen.iter().enumerate() {
        let stop = chosen.get(index + 1).map_or(text.len(), |&(_, at, _)| at);
        let item = text[body..stop].trim();
        if item.is_empty() {
            return None;
        }
        out.push_str("\n- ");
        out.push_str(&capitalized(item));
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
        // Each paragraph or line on its own; bullets already made stay.
        return text
            .split('\n')
            .map(|line| {
                if line.trim().is_empty() || line.starts_with("- ") {
                    line.to_owned()
                } else {
                    bulletize(line.to_owned())
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    let parts = sentences(&text);
    // "Firstly, X, secondly, Y, lastly, Z": markers after commas count too.
    if let Some(list) = spoken_list(&split_at_comma_markers(&parts)) {
        return list;
    }
    if let Some(list) = spoken_list(&parts) {
        return list;
    }
    // "To set it up, step one, download it. Step two, …": the first marker
    // follows a lead-in in the same sentence.
    if let Some(list) = spoken_list(&split_lead_in(&parts)) {
        return list;
    }
    if let Some(list) = inline_list(&parts) {
        return list;
    }
    if let Some(list) = unpunctuated_list(&text) {
        return even_stops(list);
    }
    let Some(intro) = parts.iter().position(|part| introduces_list(part)) else {
        return text;
    };
    if parts.len() < intro + 3 {
        return text;
    }
    let mut items: Vec<String> = Vec::new();
    for part in &parts[intro + 1..] {
        items.extend(parallel_clauses(part).into_iter().map(capitalized));
    }
    // "I want three things": three items; anything after is ordinary text.
    let rest = announced_count(parts[intro])
        .filter(|&count| count >= 2 && count < items.len())
        .map(|count| items.split_off(count));
    let mut out = heading(&parts[..=intro]);
    for item in &items {
        out.push_str("\n- ");
        out.push_str(item);
    }
    let mut out = even_stops(out);
    if let Some(rest) = rest {
        out.push('\n');
        out.push_str(&rest.join(" "));
    }
    out
}

/// The count in "I want three things" / "here are 4 points".
/// The count when `sentence` ends on it: "I need three things." -> 3.
fn counted_lead(sentence: &str) -> Option<usize> {
    let words: Vec<&str> = sentence.split_whitespace().collect();
    (words.len() >= 2)
        .then(|| announced_count(&words[words.len() - 2..].join(" ")))
        .flatten()
}

/// "I need three things, milk, eggs and bread", or with no stop heard at
/// all, "I need three things milk eggs and bread": the items start right
/// after the count. A list only when the items match the count.
fn counted_list(parts: &[&str]) -> Option<String> {
    let last = *parts.last()?;
    let words: Vec<&str> = last.split_whitespace().collect();
    let at = (0..words.len().saturating_sub(1))
        .rev()
        .find(|&i| announced_count(&format!("{} {}", words[i], words[i + 1])).is_some())?;
    let count = announced_count(&format!("{} {}", words[at], words[at + 1]))?;
    let lead = words[..=at + 1].join(" ");
    let rest = words[at + 2..].join(" ");
    let rest = rest.trim_start_matches([',', ':', ';', ' ']);
    if rest.is_empty() {
        return None;
    }
    let body = if rest.contains(", ") {
        rest.to_owned()
    } else {
        // No commas: one word per item, then "and" before the last one.
        let plain = rest.trim_end_matches(['.', '!', '?']);
        let (before, after) = plain.rsplit_once(" and ").or_else(|| plain.rsplit_once(" or "))?;
        let singles: Vec<&str> = before.split_whitespace().collect();
        if count < 3 || singles.len() + 1 != count || after.split_whitespace().count() > 3 {
            return None;
        }
        format!("{}, {after}.", singles.join(", "))
    };
    let mut lead_parts: Vec<&str> = parts[..parts.len() - 1].to_vec();
    let lead_line = format!("{}:", lead.trim_end_matches([',', ':', ';']));
    lead_parts.push(&lead_line);
    let lead = lead_parts.join(" ");
    inline_list(&[&lead, &body]).filter(|out| out.contains("\n- "))
}

fn announced_count(sentence: &str) -> Option<usize> {
    let words: Vec<String> = sentence.split_whitespace().map(bare_word).collect();
    words.iter().enumerate().find_map(|(index, word)| {
        let value = match word.as_str() {
            "two" => 2, "three" => 3, "four" => 4, "five" => 5, "six" => 6, "seven" => 7, "eight" => 8,
            other => other.parse::<usize>().ok().filter(|n| (2..=12).contains(n))?,
        };
        words.get(index + 1).is_some_and(|next| LIST_NOUNS.contains(&next.as_str()) || next == "things").then_some(value)
    })
}

/// When some bullets end with a full stop, the ones left bare (or ending in
/// a comma, from a split) get one too.
fn even_stops(list: String) -> String {
    let ends = |line: &str| line.ends_with(['.', '!', '?']);
    let bullets: Vec<&str> = list.lines().filter(|line| line.starts_with("- ")).collect();
    if !bullets.iter().any(|line| ends(line)) {
        return list;
    }
    list.lines()
        .map(|line| {
            if line.starts_with("- ") && !ends(line) {
                format!("{}.", line.trim_end_matches([',', ';', ' ']))
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// "We need a landing page, we need a demo video." is two items said in
/// one breath: clauses after a comma that open with the same two words.
fn parallel_clauses(sentence: &str) -> Vec<&str> {
    let opening: Vec<String> = sentence.split_whitespace().take(2).map(str::to_lowercase).collect();
    if opening.len() < 2 {
        return vec![sentence];
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (at, _) in sentence.match_indices(", ") {
        let next: Vec<String> = sentence[at + 2..].split_whitespace().take(2).map(str::to_lowercase).collect();
        if next == opening {
            out.push(sentence[start..at].trim());
            start = at + 2;
        }
    }
    out.push(sentence[start..].trim());
    out
}

/// Splits sentences where a list marker follows a comma ("…is broken,
/// lastly, the settings…"). Used only when the result reads as a list.
fn split_at_comma_markers<'a>(parts: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::with_capacity(parts.len() * 2);
    for part in parts {
        let mut start = 0;
        for (at, _) in part.match_indices(", ") {
            // Ordinals and closing words only: "cook rice, then add salt"
            // stays one step.
            let marker = split_marker(&part[at + 2..]).is_some_and(|(marker, _)| {
                matches!(marker, Marker::Rank(_))
                    || ["finally", "lastly", "and finally"]
                        .iter()
                        .any(|word| strip_words(&part[at + 2..], word).is_some())
            });
            if at > start && marker {
                out.push(&part[start..at]);
                start = at + 2;
            }
        }
        out.push(&part[start..]);
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
    "sure",
    "certainly",
    "of course",
    "here is",
    "here's the",
    "the cleaned",
    "cleaned text",
    "i'm sorry",
    "as an ai",
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

/// Data-free reasons for retaining the rule-cleaned fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewriteRejection {
    Empty,
    Quantity,
    Calendar,
    Negation,
    MissingAuxiliary,
    Action,
    Entity,
    Question,
    AppendedAnswer,
    Length,
    AssistantReply,
    NovelContent,
    ListStructure,
    QuotedText,
}

/// Whether a language model's rewrite of `original` can be typed instead of
/// it. The model may drop words, fix grammar, and lay out a list, but it must
/// not answer, add content, or wander off: anything suspicious keeps the
/// rule-cleaned fallback. This does not prove semantic equivalence.
#[must_use]
pub fn accept_rewrite(original: &str, rewrite: &str) -> bool {
    rewrite_rejection(original, rewrite).is_none()
}

/// Explains a rejection without returning or logging any dictated text.
#[must_use]
pub fn rewrite_rejection(original: &str, rewrite: &str) -> Option<RewriteRejection> {
    let before = content_words(original);
    let after = content_words(rewrite);
    if after.is_empty() || before.is_empty() {
        return Some(RewriteRejection::Empty);
    }
    if let Some(reason) = safety::rejection(original, rewrite) {
        return Some(reason);
    }
    // Line breaks were asked for ("new paragraph", "new line", bullets):
    // the rewrite keeps every one (it may add a list's, never remove one).
    if original.matches('\n').count() > rewrite.matches('\n').count() {
        return Some(RewriteRejection::Length);
    }
    // Shorter is expected (fillers, restarts); much longer means new content.
    if after.len() > before.len() + before.len() / 5 + 3 || after.len() * 4 < before.len() {
        return Some(RewriteRejection::Length);
    }
    let lower = rewrite.trim_start().to_lowercase();
    let opener = |text: &str| ANSWER_OPENERS.iter().any(|opener| text.starts_with(opener));
    if opener(&lower) && !opener(&original.trim_start().to_lowercase()) {
        return Some(RewriteRejection::AssistantReply);
    }
    // A question must stay a question, or the model answered it.
    let asked = original.contains('?');
    if asked && !rewrite.contains('?') {
        return Some(RewriteRejection::Question);
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
    (novel > 1 + after.len() / 10).then_some(RewriteRejection::NovelContent)
}

fn written_decimal_or_clock(tokens: &[Token], at: usize) -> Option<(String, usize)> {
    if tokens.get(at)?.verbatim {
        return None;
    }
    facts::spoken_decimal(tokens, at).or_else(|| {
        if tokens[at].word.chars().all(char::is_alphabetic) {
            facts::clock(tokens, at).and_then(|q| {
                let last = norm(&tokens[q.end - 1]).replace('.', "");
                if !matches!(last.as_str(), "am" | "pm") {
                    return None;
                }
                let (hour, minute) = q.value.split_once(':')?;
                let hour = hour.parse::<u32>().ok()?;
                Some((
                    format!(
                        "{}:{minute} {}",
                        if hour % 12 == 0 { 12 } else { hour % 12 },
                        last.to_uppercase()
                    ),
                    q.end,
                ))
            })
        } else {
            None
        }
    })
}

/// Normalize only explicit spoken decimal points and clock suffixes before
/// filler removal. In particular, "oh" inside "point oh five" is a digit.
fn written_numbers(tokens: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut at = 0;
    while at < tokens.len() {
        if let Some((value, end)) = written_decimal_or_clock(&tokens, at) {
            let mut replacement: Vec<_> = value.split_whitespace().map(Token::parse).collect();
            replacement.first_mut().unwrap().lead = tokens[at].lead.clone();
            let mut trail = tokens[end - 1].trail.clone();
            // "eight p.m. by default": the stop belonged to "p.m.", and
            // the sentence goes on, so "8:00 PM" takes no full stop.
            let goes_on = tokens
                .get(end)
                .and_then(|next| next.word.chars().next())
                .is_some_and(char::is_lowercase);
            if goes_on && trail == "." && tokens[end - 1].word.contains('.') {
                trail.clear();
            }
            replacement.last_mut().unwrap().trail = trail;
            out.extend(replacement);
            at = end;
        } else {
            out.push(tokens[at].clone());
            at += 1;
        }
    }
    out
}

/// Final presentation pass, after cleanup/fallback selection, so a language
/// model cannot turn numeric counts back into words. Keep whitespace, quoted
/// wording, likely proper names, and pronoun "one" intact.
#[must_use]
pub fn format_numbers(text: &str) -> String {
    let mut cursor = 0;
    let spans: Vec<_> = text
        .split_whitespace()
        .map(|word| {
            let start = cursor + text[cursor..].find(word).unwrap();
            cursor = start + word.len();
            (start, cursor)
        })
        .collect();
    let tokens: Vec<_> = text.split_whitespace().map(Token::parse).collect();
    let quote =
        |marks: &str| marks.contains(['"', '\'', '\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}']);
    let mut quoted = false;
    let protected: Vec<_> = tokens
        .iter()
        .map(|token| {
            if quote(&token.lead) {
                quoted = true;
            }
            let protected = quoted;
            if quote(&token.trail) {
                quoted = false;
            }
            protected
        })
        .collect();
    let capital = |token: &Token| token.word.chars().next().is_some_and(char::is_uppercase);
    let mut result = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < tokens.len() {
        if protected[at] {
            at += 1;
            continue;
        }
        let explicit = written_decimal_or_clock(&tokens, at);
        let cardinal = explicit.is_none();
        let value = explicit
            .or_else(|| facts::spoken_number(&tokens, at).map(|(n, end)| (n.to_string(), end)));
        let Some((value, end)) = value else {
            at += 1;
            continue;
        };
        let pronoun = cardinal
            && end == at + 1
            && norm(&tokens[at]) == "one"
            && facts::quantity(&tokens, at).is_none();
        let proper_name = cardinal
            && capital(&tokens[at])
            && ((at > 0 && capital(&tokens[at - 1]) && !tokens[at - 1].ends_sentence())
                || (!tokens[end - 1].ends_sentence() && tokens.get(end).is_some_and(capital)));
        if pronoun || proper_name || protected[at..end].iter().any(|v| *v) {
            at = end;
            continue;
        }
        let start = spans[at].0 + tokens[at].lead.len();
        let stop = spans[end - 1].1 - tokens[end - 1].trail.len();
        result.push_str(&text[copied..start]);
        result.push_str(&value);
        copied = stop;
        at = end;
    }
    result.push_str(&text[copied..]);
    let formatted: Vec<_> = result.split_whitespace().map(Token::parse).collect();
    if facts::quantities(&tokens) == facts::quantities(&formatted) {
        result
    } else {
        text.to_owned()
    }
}

/// Keep balanced quotations opaque to automatic word/phrase cleanup.
fn literal_tokens(text: &str) -> Vec<Token> {
    let mut result = Vec::new();
    let mut opened = None;
    let mut copied = 0;
    for (at, ch) in text.char_indices() {
        if let Some((start, close)) = opened {
            if ch == close {
                result.extend(text[copied..start].split_whitespace().map(Token::parse));
                let end = at + ch.len_utf8();
                let punctuation = text[end..]
                    .chars()
                    .take_while(|c| matches!(c, '.' | ',' | ';' | ':' | '?' | '!' | ')' | ']'))
                    .count();
                result.push(Token {
                    lead: String::new(),
                    word: text[start..end].to_owned(),
                    trail: text[end..end + punctuation].to_owned(),
                    verbatim: true,
                });
                copied = end + punctuation;
                opened = None;
            }
        } else if ch == '"' || ch == '\u{201c}' {
            opened = Some((at, if ch == '"' { '"' } else { '\u{201d}' }));
        }
    }
    result.extend(text[copied..].split_whitespace().map(Token::parse));
    result
}

/// Cleans one recognised utterance.
#[must_use]
pub fn clean(text: &str, options: &TextOptions) -> String {
    // "Mm." on its own: a hum, nothing to type.
    let hums = text.split_whitespace().map(bare_word).collect::<Vec<_>>();
    if options.filler_removal && !hums.is_empty() && hums.iter().all(|w| !w.is_empty() && is_filler_sound(w)) {
        return String::new();
    }
    // Existing paragraphs and list markers are structural boundaries, not
    // stutters or punctuation to flatten. Spoken breaks created below still
    // go through the usual whole-utterance path.
    if text.contains('\n') {
        return text
            .split('\n')
            .map(|line| {
                let line = line.trim_end_matches('\r');
                let body = line.trim_start();
                if let Some(marker) = ["- ", "* ", "\u{2022} "]
                    .iter()
                    .find(|marker| body.starts_with(**marker))
                {
                    let indent = &line[..line.len() - body.len()];
                    format!("{indent}{marker}{}", clean(&body[marker.len()..], options))
                } else {
                    clean(line, options)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    let joined;
    let text = if options.filler_removal {
        joined = misheard_thanks(&rejoin_fragments(&continue_sentences(&rejoin_opened_phrases(&restarted_articles(&resolve_denied_value(&drop_repeated_runs(&drop_noun_restarts(&drop_resaid_sentences(&misheard_words(&restart_after_filler(text)))))))))));
        joined.as_str()
    } else {
        text
    };
    let tokens = literal_tokens(text);
    if tokens.is_empty() {
        return String::new();
    }
    let phrases = build_phrases(options);
    let mut tokens = if phrases.is_empty() {
        tokens
    } else {
        apply_phrases(&tokens, &phrases)
    };
    if options.spoken_punctuation {
        tokens = spoken_file_names(written_numbers(tokens));
    }
    if options.filler_removal {
        tokens = remove_stutters(resolve_quick_swaps(resolve_corrections(drop_restarts(
            drop_number_restarts(drop_subject_restarts(collapse_linking_runs(remove_fillers(tokens)))),
        ))));
    }
    safety::punctuate_questions(&mut tokens);
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

    #[test]
    fn a_rejected_word_and_a_swapped_question_word_are_resolved() {
        // Dictated exactly like this; both corrections were missed before.
        assert_eq!(
            clean(
                "For the PDF support the users will have to enter not enter but enable allow access to file URLs inside the extension settings. How would the users know how when to do that?",
                &options()
            ),
            "For the PDF support the users will have to enable allow access to file URLs inside the extension settings. How would the users know when to do that?"
        );
        assert_eq!(clean("Click save, not save but submit.", &options()), "Click submit.");
        assert_eq!(clean("What when did it start?", &options()), "When did it start?");
    }

    #[test]
    fn a_restart_after_uh_and_a_dictation_of_only_hums_are_cleaned() {
        let o = options();
        // Dictated exactly like this (the speech model's own words).
        assert_eq!(
            clean("But before that you just check the per performance uh before that I want you to check the performance impact.", &o),
            "But before that I want you to check the performance impact."
        );
        assert_eq!(clean("Uh so the plan is um so the plan is to ship on Friday.", &o), "So the plan is to ship on Friday.");
        for hum in ["Mm.", "Hmm.", "Uh.", "Um, uh.", "Mm-hmm."] {
            assert_eq!(clean(hum, &o), "", "{hum}");
        }
        for kept in [
            "Before that, we eat. Um, before that we cook.",
            "Check the logs, uh, check the logs again tomorrow.",
            "Send the report to Anna and uh send the slides to Ben.",
        ] {
            assert_eq!(clean(kept, &o).contains("logs") || clean(kept, &o).contains("report") || clean(kept, &o).contains("eat"), true, "{kept}");
        }
        assert_eq!(
            clean("Send the report to Anna and uh send the slides to Ben.", &o),
            "Send the report to Anna and send the slides to Ben."
        );
    }

    #[test]
    fn a_drive_letter_a_time_mid_sentence_and_phrases_cut_by_pauses_are_kept_whole() {
        let o = options();
        // Dictated exactly like this (the speech model's own words).
        assert_eq!(
            clean("And I wanted to. Chart after eight p.m. by default and add a settings page as well.", &o),
            "And I wanted to start after 8:00 PM by default and add a settings page as well."
        );
        assert_eq!(
            clean("Also, the island. Eye animation should also show when the red light effect is gonna start.", &o),
            "Also, the island eye animation should also show when the red light effect is gonna start."
        );
        assert_eq!(clean("My audio is saved at D drive games folder.", &o), "My audio is saved at D drive games folder.");
        assert_eq!(clean("Meet at nine a.m. tomorrow.", &o), "Meet at 9:00 AM tomorrow.");
        for kept in [
            "Open the D drive.",
            "Write it in C code.",
            "I want to chart the sales data.",
            "Also, the end. Thanks for watching.",
            "So the plan. Sarah will call.",
            "And then. Next week we ship.",
        ] {
            assert_eq!(clean(kept, &o), kept);
        }
    }

    #[test]
    fn a_resaid_sentence_and_misheard_or_and_hover_are_fixed() {
        let o = options();
        // Dictated exactly like this.
        assert_eq!(
            clean("Also, I wrote this prompt using the voice dictation model. Also, I wrote this prompt using the voice dictation.", &o),
            "Also, I wrote this prompt using the voice dictation."
        );
        assert_eq!(
            clean("Warning, in the sense, it should show. Oh some kind of indication where it will give a hint.", &o),
            "Warning, in the sense, it should show. Or some kind of indication where it will give a hint."
        );
        assert_eq!(
            clean("And on whoever of the dynamic island, it will give the button to download.", &o),
            "And on hover of the dynamic island, it will give the button to download."
        );
        for kept in [
            "Fix the login page. Fix the signup page.",
            "I want you to fix the bug. I want you to fix the bug in the parser and add tests for it.",
            "Oh, some kind of error came up.",
            "Oh some kind of error came up.",
            "It depends on whoever wins.",
            "Thanks. Thanks.",
        ] {
            assert_eq!(clean(kept, &o), kept);
        }
    }

    #[test]
    fn a_verb_after_a_pause_and_a_restarted_describing_word_are_joined() {
        let o = options();
        // Dictated exactly like this; the speech model put stops at the pauses.
        assert_eq!(
            clean(
                "And I want you to. Update the game graphics design. I want it to look like a proper. A realistic open world game, browser game.",
                &o
            ),
            "And I want you to update the game graphics design. I want it to look like a proper, realistic open world game, browser game."
        );
        assert_eq!(clean("I asked them to. Set it up today.", &o), "I asked them to set it up today.");
        assert_eq!(clean("Build a simple a clean menu.", &o), "Build a simple, clean menu.");
        // After the AI rewrite drops the stop, the capital still goes.
        assert_eq!(
            finish_layout("And I want you to Update the game graphics design."),
            "And I want you to update the game graphics design."
        );
        for kept in [
            "Talk to Daniel. Tell him the plan.",
            "I want to. Paris is lovely.",
            "I need you to. Visual Studio is open.",
            "I have a dog. A big one.",
            "Please send it to Daniel.",
            "Go to Paris the city of light.",
        ] {
            assert_eq!(clean(kept, &o), kept);
            assert_eq!(finish_layout(kept), kept);
        }
    }

    #[test]
    fn fragments_cut_off_by_pauses_are_put_back() {
        let o = options();
        // Dictated exactly like this.
        assert_eq!(
            clean(
                "Expand the image using black. Color if possible. It should be YouTube resolutions. YouTube thumbnail resolution size.",
                &o
            ),
            "Expand the image using black color if possible. It should be YouTube thumbnail resolution size."
        );
        for kept in [
            "I love New York. New York pizza is great.",
            "I love New York. New York pizza.",
            "Use another app. Spotify if possible.",
            "Send the report. The report is late.",
            "Book the hotel. The hotel near the station.",
            "It needs a border. Thin black lines.",
        ] {
            assert_eq!(clean(kept, &o), kept);
        }
    }

    #[test]
    fn restarted_clauses_numbers_and_pauses_mid_sentence_are_cleaned() {
        let o = options();
        // Dictated exactly like this.
        assert_eq!(
            clean(
                "So the 3 minutes long will become. Around 6 5 to 6 minutes. Because if I just cut and put it at the end it will just sound l does not it wouldn't it will not sound like a loop.",
                &o
            ),
            "So the 3 minutes long will become around 5 to 6 minutes. Because if I just cut and put it at the end it will not sound like a loop."
        );
        assert_eq!(clean("It will rain and it will snow.", &o), "It will rain and it will snow.");
        for kept in [
            "It is what it is.",
            "I will send the report and I will send the slides.",
            "We need 5 to 6 people.",
            "Give me 2 4 to 6 inch boards.",
            "Pick a number. Around noon works.",
            "It became. The end.",
        ] {
            assert_eq!(clean(kept, &o), kept);
        }
    }

    #[test]
    fn a_spoken_file_name_a_drawn_out_um_and_a_repeated_and_are_cleaned() {
        let o = options();
        // Dictated exactly like this.
        assert_eq!(
            clean(
                "Okay, now replace one of the Roblox character with the cyberpunk underscore or glb. Ummm And Also and for the Leo character, use the Twin Cannon Baggy Pants.",
                &o
            ),
            "Okay, now replace one of the Roblox character with the cyberpunk_.glb. And also for the Leo character, use the Twin Cannon Baggy Pants."
        );
        assert_eq!(clean("Open config dot toml and my underscore model dot GLB.", &o), "Open config.toml and my_model.glb.");
        assert_eq!(clean("Go to example dot com.", &o), "Go to example.com.");
        for kept in [
            "Save it as PNG or JPG.",
            "Connect the dot to the line.",
            "Ask Mr so and so about it.",
            "Tell 'em it's done.",
            "And also the music.",
        ] {
            assert_eq!(clean(kept, &o), kept);
        }
        for sound in ["um", "Ummm", "uhhm", "hmmm", "mm-hmm", "errm", "Mhm"] {
            assert!(is_filler_sound(&sound.to_lowercase()), "{sound}");
        }
        for word in ["umbrella", "hum", "mum", "her", "em", "eh", "me", "uhuh"] {
            assert!(!is_filler_sound(word), "{word}");
        }
    }

    #[test]
    fn a_start_given_up_with_never_mind_is_dropped() {
        let o = options();
        assert_eq!(
            clean(
                "I want you to create Never mind. Sorry. First, I want you to connect to the MCP and then revamp the animations.",
                &o
            ),
            "First, I want you to connect to the MCP and then revamp the animations."
        );
        assert_eq!(
            clean("Send it. Can you open the never mind, can you close the window?", &o),
            "Send it. Can you close the window?"
        );
        assert_eq!(clean("Book a table for nevermind.", &o), "");
        for kept in [
            "Did you see the email? Never mind. I found it.",
            "I never mind the noise.",
            "Never mind the cost, ship it.",
            "You would never mind.",
            "Tell them to never mind, it's fine.",
        ] {
            assert_eq!(clean(kept, &o), kept);
        }
    }

    #[test]
    fn a_start_given_up_with_sorry_is_dropped() {
        // Dictated exactly like this.
        assert_eq!(
            clean(
                "For example, let's say if I ask the voice assistant to open Spotify and search for this type of music. This search  for a certain mus I'm sorry. If I open Spotify and ask it to search for a certain specific music, can it do it?",
                &options()
            ),
            "For example, let's say if I ask the voice assistant to open Spotify and search for this type of music. If I open Spotify and ask it to search for a certain specific music, can it do it?"
        );
        assert_eq!(
            clean("Can you send the quarterly rep, sorry, can you send the quarterly report today?", &options()),
            "Can you send the quarterly report today?"
        );
    }

    #[test]
    fn a_start_ended_with_a_full_stop_and_given_up_is_dropped() {
        // Dictated exactly like this.
        assert_eq!(
            clean(
                "Can we add a dark mode toggle to the settings page? This dark mode should follow the system. I'm sorry. Can the dark mode toggle follow the system theme automatically?",
                &options()
            ),
            "Can we add a dark mode toggle to the settings page? Can the dark mode toggle follow the system theme automatically?"
        );
    }

    #[test]
    fn real_apologies_stay() {
        for text in [
            "I'm late. I'm sorry. I missed the bus this morning.",
            "The quarterly report is late. I'm sorry. The quarterly report will be ready tomorrow.",
            "I broke the build. I'm sorry. I'll fix the build tonight.",
            "I'm late, I'm sorry. I missed the bus.",
            "I am so sorry. That was my fault.",
            "Sorry. Can we talk tomorrow?",
            "I'm sorry about the delay. The report is ready.",
        ] {
            assert_eq!(clean(text, &options()), text);
        }
    }

    #[test]
    fn ordinary_not_but_and_separate_question_words_stay() {
        for text in [
            "It is not red but blue.",
            "Use the light theme, not the dark one but the default.",
            "I know what, when and where.",
            "Tell me who, what and why.",
            "Not now but later.",
        ] {
            assert_eq!(clean(text, &options()), text);
        }
    }

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
            clean(
                "All the outcomes will be different. No wait, randomized.",
                &o
            ),
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
        assert_eq!(
            clean("Let's say hello to them.", &o),
            "Let's say hello to them."
        );
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
            clean(
                "I want three things in the app. It needs AR. It needs races. It needs bets.",
                &o
            ),
            "I want three things in the app:\n- It needs AR.\n- It needs races.\n- It needs bets."
        );
        assert_eq!(
            clean(
                "Some ideas for the game: cars race on the desk. Friends can bet.",
                &o
            ),
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
    fn pointers_with_named_ordinals_preserve_prelude_and_make_separate_items() {
        let source = "Send the report to Daniel that we have completed 222 checks. And I wanted to add a few pointers. The first one is add a new feature where the users can enter prompt and it will generate a data set. And the second feature will be to customize teams. And third feature will be to host the files on a cloud platform.";
        let expected = "Send the report to Daniel that we have completed 222 checks. And I wanted to add a few pointers:\n- Add a new feature where the users can enter prompt and it will generate a data set.\n- Customize teams.\n- Host the files on a cloud platform.";
        let result = clean(source, &options());
        assert_eq!(result, expected);
        assert_eq!(format_numbers(&result), expected);
        assert_eq!(
            clean("A few pointers. Keep the files. Save the logs.", &options()),
            "A few pointers:\n- Keep the files.\n- Save the logs."
        );
    }

    #[test]
    fn named_list_markers_keep_related_sentences_and_negative_instructions() {
        assert_eq!(clean("Here are some pointers. The first point is keep 222 files. Do not overwrite the backups. And the second item is to save the logs. And finally, notify Daniel.", &options()),
            "Here are some pointers:\n- Keep 222 files. Do not overwrite the backups.\n- Save the logs.\n- Notify Daniel.");
        for source in [
            "The first car won. The second car lost.",
            "The first one is finished. The meeting is tomorrow.",
            "I have a few pointers. Let me think.",
            "He said \"the first one is broken\". She said \"the second one is fine\".",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn unpunctuated_announced_lists_keep_alternatives_and_multi_sentence_items() {
        assert_eq!(clean("here are the tasks step one use custom lighting or use auto RGB step two ask Ingrid Cole to confirm step three keep 12 archived files", &options()),
            "Here are the tasks:\n- Use custom lighting or use auto RGB\n- Ask Ingrid Cole to confirm\n- Keep 12 archived files");
        assert_eq!(clean("here are the points that I want to include first add a prompt box the second one is customize teams and the third is host the files", &options()),
            "Here are the points that I want to include:\n- Add a prompt box\n- Customize teams\n- Host the files");
        for source in [
            "The first car won and the second car lost.",
            "Here are our notes about the first car and the second car.",
            "Here are the tasks review step one before discussing step two.",
            "The label says \"here are the tasks step one save step two close\".",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn cleaning_existing_lists_keeps_markers_paragraphs_and_item_boundaries() {
        for source in [
            "Tasks:\n- Keep 222 checks.\n- Customize teams.\n- Host the files.",
            "Keep archive_v4.csv.\n\n* Ask Ingrid Cole to check it.\n  * Do not upload the backup.\n",
            "- Save the logs.\n\u{2022} Keep 12 receipts.",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
        assert_eq!(
            clean(
                "Tasks:\n- Um, keep the the files.\n- Do not upload the backup.",
                &options()
            ),
            "Tasks:\n- Keep the files.\n- Do not upload the backup."
        );
        assert_eq!(
            clean(
                "Send it to Ingrid.\n\nLet me correct that: send it to Karim.",
                &options()
            ),
            "Send it to Ingrid.\n\nLet me correct that: send it to Karim."
        );
    }

    #[test]
    fn list_rewrites_cannot_drop_an_instruction_or_move_facts_between_items() {
        let source = "Tasks:\n- Check the sensors or ask the supervisor to review them. Keep the original calibration sheet with the instrument.\n- Send 12 receipts to Ingrid Cole.\n- Keep 8 archived files.";
        assert!(!accept_rewrite(source, "Tasks:\n- Check the sensors or ask the supervisor to review them.\n- Send 12 receipts to Ingrid Cole.\n- Keep 8 archived files."));
        assert!(!accept_rewrite(source, "Tasks:\n- Check the sensors. Keep the original calibration sheet with the instrument.\n- Send 12 receipts to Ingrid Cole.\n- Keep 8 archived files."));
        assert!(!accept_rewrite(
            "Tasks:\n- Send 12 receipts to Ingrid Cole.\n- Ask Karim Shah to review 8 files.",
            "Tasks:\n- Send 12 receipts to Karim Shah.\n- Ask Ingrid Cole to review 8 files."
        ));
        assert!(accept_rewrite(
            "Tasks:\n- She dont have the calibration sheets.\n- Keep 12 files.",
            "Tasks:\n- She does not have the calibration sheets.\n- Keep 12 files."
        ));
    }

    #[test]
    fn model_rewrites_must_preserve_existing_bullet_structure() {
        let source = "Pointers:\n- Keep 222 checks.\n- Customize teams.\n- Host the files.";
        assert_eq!(
            rewrite_rejection(
                source,
                "Pointers: Keep 222 checks. Customize teams. Host the files."
            ),
            Some(RewriteRejection::ListStructure)
        );
        assert_eq!(
            rewrite_rejection(
                source,
                "Pointers:\n- Keep 222 checks. Customize teams.\n- Host the files."
            ),
            Some(RewriteRejection::ListStructure)
        );
        assert!(accept_rewrite(
            source,
            "Pointers:\n* Keep 222 checks.\n* Customize teams.\n* Host the files."
        ));
        assert!(accept_rewrite(source, source));
    }

    #[test]
    fn conversational_ordinals_keep_alternatives_in_their_own_item() {
        let source = "Okay, so here's something I'm trying to build and here are few points that I want to include: So first, can you please add a mode where the user can select a custom lighting. Or they can select an auto RGB mode. The second one is let the user enter a prompt and based on that it will select the colours. And the third is if the user clicks a button then it should update it real time.";
        let expected = "Okay, so here's something I'm trying to build and here are few points that I want to include:\n- Can you please add a mode where the user can select a custom lighting. Or they can select an auto RGB mode.\n- Let the user enter a prompt and based on that it will select the colours.\n- If the user clicks a button then it should update it real time.";
        assert_eq!(clean(source, &options()), expected);
        assert_eq!(format_numbers(expected), expected);
        assert_eq!(clean("A few pointers. And so first, use custom lighting. Or use RGB. And the second is enter a prompt. And the third is update on click.", &options()),
            "A few pointers:\n- Use custom lighting. Or use RGB.\n- Enter a prompt.\n- Update on click.");
    }

    #[test]
    fn ordinal_detection_does_not_merge_declared_alternative_options_or_plain_prose() {
        assert_eq!(
            clean(
                "Here are two options. Use custom lighting. Or use RGB.",
                &options()
            ),
            "Here are two options:\n- Use custom lighting.\n- Or use RGB."
        );
        for source in [
            "So first, check it. Then we can decide.",
            "The first car won. And the second car lost.",
            "And so the first car won. The second car lost.",
            "The third is ready. We can continue.",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
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
            "Can't we just use VS Code instead of X code?"
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

    #[test]
    fn full_schedule_corrections_keep_only_the_final_choice() {
        let o = options();
        for (source, expected) in [
            (
                "Meet Thursday at 4 pm actually make that Friday at 10 30 am before lunch.",
                "Meet Friday at 10 30 am before lunch.",
            ),
            (
                "Book Monday at 3 pm no wait Wednesday at 9:45 am for the review.",
                "Book Wednesday at 9:45 am for the review.",
            ),
            (
                "Book Monday at 3 pm no wait Wednesday.",
                "Book Wednesday at 3 pm.",
            ),
            (
                "Start at 6 pm no wait 7:15 pm and keep the room.",
                "Start at 7:15 pm and keep the room.",
            ),
            (
                "Use Monday, no wait Tuesday, no wait Friday.",
                "Use Friday.",
            ),
        ] {
            assert_eq!(clean(source, &o), expected);
        }
    }

    #[test]
    fn corrections_do_not_eat_the_next_clause_or_lowercase_names() {
        let o = options();
        for (source, expected) in [
            (
                "Email it to Leonie no sorry to Darius before noon.",
                "Email it to Darius before noon.",
            ),
            (
                "Send it to Petra, sorry I mean Theo after the review.",
                "Send it to Theo after the review.",
            ),
            (
                "Set the amount to 3200 rupees no wait 2800 including tax.",
                "Set the amount to 2800 rupees including tax.",
            ),
            (
                "It took 4.5 hours sorry I mean 3.25 hours at 900 rupees per hour.",
                "It took 3.25 hours at 900 rupees per hour.",
            ),
            (
                "Test the login issue I mean the logout issue before release.",
                "Test the logout issue before release.",
            ),
            (
                "Make it blue, I mean green before Friday.",
                "Make it green before Friday.",
            ),
        ] {
            assert_eq!(clean(source, &o), expected);
        }
    }

    #[test]
    fn punctuated_apologies_replace_complete_spoken_counts() {
        for (source, expected) in [
            ("Run two hundred and fourteen checks. No, I'm sorry, two hundred and ten checks.",
             "Run two hundred and ten checks."),
            ("Use two hundred fourteen cores. No, I am sorry, two hundred ten cores. Keep six passages.",
             "Use two hundred ten cores. Keep six passages."),
            ("Run twenty-seven checks, no, I\u{2019}m sorry, twenty-nine checks before Friday.",
             "Run twenty-nine checks before Friday."),
            ("Use two hundred and fourteen codes. No, I'm sorry, two hundred and ten cores.",
             "Use two hundred and ten cores."),
            ("Set the budget to two thousand three hundred rupees, no wait two thousand one hundred rupees including tax.",
             "Set the budget to two thousand one hundred rupees including tax."),
        ] {
            assert_eq!(clean(source, &options()), expected);
        }
        for source in [
            "No, I'm sorry you missed two hundred checks.",
            "I am sorry we lost twenty files.",
            "We ran two hundred checks and six passages.",
            "The old count was two hundred and fourteen and the new count is two hundred and ten.",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn spoken_counts_accept_digit_formatting_but_reject_changed_facts() {
        for (source, expected) in [
            (
                "Run two hundred and ten checks and six passages.",
                "Run 210 checks and 6 passages.",
            ),
            ("Run twenty-seven checks.", "Run 27 checks."),
            (
                "Set the total to one thousand two hundred and fifty rupees.",
                "Set the total to 1250 rupees.",
            ),
            (
                "Keep one million two hundred thousand files.",
                "Keep 1200000 files.",
            ),
            ("Keep four and six separate.", "Keep 4 and 6 separate."),
            ("Keep the price at $five.", "Keep the price at 5 dollars."),
            (
                "Set it to minus twenty-five percent.",
                "Set it to -25 percent.",
            ),
        ] {
            assert!(
                accept_rewrite(source, expected),
                "{:?}",
                rewrite_rejection(source, expected)
            );
        }
        for (source, wrong) in [
            (
                "Run two hundred and ten checks and six passages.",
                "Run 214 checks and 6 passages.",
            ),
            ("Keep four and six separate.", "Keep 46 separate."),
            ("Keep the price at $five.", "Keep the price at 5 rupees."),
            (
                "Set it to minus twenty-five percent.",
                "Set it to 25 percent.",
            ),
            (
                "Keep the original two hundred and fourteen and the new two hundred and ten.",
                "Keep the new 210.",
            ),
        ] {
            assert_eq!(
                rewrite_rejection(source, wrong),
                Some(RewriteRejection::Quantity)
            );
        }
    }

    #[test]
    fn explicit_spoken_points_and_clock_minutes_are_written_before_corrections() {
        for (source, expected) in [
            (
                "Meet Thursday at nine thirty AM.",
                "Meet Thursday at 9:30 AM.",
            ),
            (
                "Meet Monday at two PM actually make that Thursday at nine thirty five AM.",
                "Meet Thursday at 9:35 AM.",
            ),
            (
                "Testing took four point five hours, sorry I mean three point seven five hours.",
                "Testing took 3.75 hours.",
            ),
            (
                "Set drift to zero point oh five percent.",
                "Set drift to 0.05 percent.",
            ),
            ("Keep point seven five hours.", "Keep 0.75 hours."),
            (
                "Use version seven point two point four.",
                "Use version 7.2.4.",
            ),
            ("Use version seven point two four.", "Use version 7.24."),
            (
                "Discuss point three in the agenda.",
                "Discuss point three in the agenda.",
            ),
            (
                "Point the camera at four birds.",
                "Point the camera at four birds.",
            ),
        ] {
            assert_eq!(clean(source, &options()), expected);
        }
        let mut o = options();
        o.spoken_punctuation = false;
        assert_eq!(
            clean("Keep three point seven five hours.", &o),
            "Keep three point seven five hours."
        );
    }

    #[test]
    fn guard_compares_spoken_decimals_and_times_without_inventing_missing_separators() {
        for (source, expected) in [
            ("Meet at nine thirty AM.", "Meet at 9:30 AM."),
            ("Meet at nine thirty five PM.", "Meet at 21:35."),
            (
                "Testing took three point seven five hours.",
                "Testing took 3.75 hours.",
            ),
            ("Keep point seven five hours.", "Keep 0.75 hours."),
            (
                "Use version seven point two point four.",
                "Use version 7.2.4.",
            ),
            (
                "Set drift to zero point oh five percent.",
                "Set drift to 0.05 percent.",
            ),
            (
                "Discuss point three in the agenda.",
                "Discuss point 3 in the agenda.",
            ),
        ] {
            assert!(
                accept_rewrite(source, expected),
                "{:?}",
                rewrite_rejection(source, expected)
            );
        }
        for (source, wrong) in [
            ("Meet at nine thirty AM.", "Meet at 9:35 AM."),
            (
                "Testing took three point seven five hours.",
                "Testing took 3.5 hours.",
            ),
            ("Keep point seven five hours.", "Keep 3.75 hours."),
            ("Use version seven point two four.", "Use version 7.2.4."),
        ] {
            assert_eq!(
                rewrite_rejection(source, wrong),
                Some(RewriteRejection::Quantity)
            );
        }
    }

    #[test]
    fn final_number_presentation_uses_digits_after_rewrites_or_fallbacks() {
        for (source, expected) in [
            ("Send the report to Daniel. We completed two hundred and ten checks.",
             "Send the report to Daniel. We completed 210 checks."),
            ("Keep twenty-one checks, twenty two files and six passages.",
             "Keep 21 checks, 22 files and 6 passages."),
            ("The original estimate was twenty four thousand five hundred rupees and the revised estimate is twenty two thousand six hundred rupees.",
             "The original estimate was 24500 rupees and the revised estimate is 22600 rupees."),
            ("Keep four and six separate.", "Keep 4 and 6 separate."),
            ("Use version seven point two point four at nine thirty AM.",
             "Use version 7.2.4 at 9:30 AM."),
            ("Keep three point seven five hours at minus twenty-five percent.",
             "Keep 3.75 hours at minus 25 percent."),
            ("There are zero issues and twelve tests.", "There are 0 issues and 12 tests."),
        ] {
            assert_eq!(format_numbers(source), expected);
            assert!(accept_rewrite(source, expected), "{:?}", rewrite_rejection(source, expected));
            assert_eq!(format_numbers(expected), expected);
        }
        let cleaned = clean("Send the report to Maya, no, I'm sorry, to Daniel. We completed two hundred and fourteen checks, no, I'm sorry, two hundred and ten checks.", &options());
        assert_eq!(
            format_numbers(&cleaned),
            "Send the report to Daniel. We completed 210 checks."
        );
    }

    #[test]
    fn numeric_presentation_preserves_layout_quotes_names_and_pronouns() {
        for (source, expected) in [
            (
                "  - two hundred checks\n\n- six passages\t",
                "  - 200 checks\n\n- 6 passages\t",
            ),
            (
                "Keep the words \"two hundred\" and count twenty files.",
                "Keep the words \"two hundred\" and count 20 files.",
            ),
            (
                "Keep 'twenty one' as written.",
                "Keep 'twenty one' as written.",
            ),
            (
                "The band Twenty One Pilots has three albums.",
                "The band Twenty One Pilots has 3 albums.",
            ),
            (
                "Use Studio One and OneDrive.",
                "Use Studio One and OneDrive.",
            ),
            (
                "One of them completed one file.",
                "One of them completed 1 file.",
            ),
            ("No one was there.", "No one was there."),
            (
                "Keep reports_Q4_2026.csv and version 7.2.4.",
                "Keep reports_Q4_2026.csv and version 7.2.4.",
            ),
        ] {
            assert_eq!(format_numbers(source), expected);
        }
    }

    #[test]
    fn spoken_integer_identifiers_do_not_cancel_other_numeric_formatting() {
        for (source, expected) in [
            ("Keep twenty six samples and version eight.",
             "Keep 26 samples and version 8."),
            ("Keep twenty six samples. Publish version eight. 0.35.",
             "Keep 26 samples. Publish version 8. 0.35."),
            ("Use code one and keep forty two files.",
             "Use code 1 and keep 42 files."),
            ("Keep build twelve and pin thirty four.",
             "Keep build 12 and pin 34."),
        ] {
            assert_eq!(format_numbers(source), expected);
            assert!(accept_rewrite(source, expected));
            assert_eq!(format_numbers(expected), expected);
        }
        assert!(!accept_rewrite("Use version eight.", "Use version nine."));
        assert!(!accept_rewrite("Use version eight. 0.35.", "Use version 8.35."));
    }

    #[test]
    fn sentence_punctuation_does_not_block_typed_scoped_corrections() {
        for (source, expected) in [
            ("Send the summary before Tuesday at three. Actually at Friday at 10:15 AM. Keep 8 logs.",
             "Send the summary before Friday at 10:15 AM. Keep 8 logs."),
            ("Send the summary before Tuesday at 3 PM. Actually on Friday at ten fifteen AM.",
             "Send the summary on Friday at 10:15 AM."),
            ("We recorded three hundred and twelve samples. Sorry, three hundred and nine samples. Keep both quotes.",
             "We recorded 309 samples. Keep both quotes."),
            ("The visit took 3 hours. Sorry, 4 hours. Keep 8 receipts.",
             "The visit took 4 hours. Keep 8 receipts."),
            ("Send the summary before Tuesday at three. Actually at Friday at 10:15 AM we completed twelve samples.",
             "Send the summary before Friday at 10:15 AM. We completed 12 samples."),
            ("We recorded three hundred and thir twelve samples. Sorry, three hundred and nine samples. Keep 8 receipts.",
             "We recorded 309 samples. Keep 8 receipts."),
        ] {
            assert_eq!(format_numbers(&clean(source, &options())), expected);
        }
    }

    #[test]
    fn scoped_sentence_corrections_keep_unrelated_or_incomplete_facts() {
        for source in [
            "We recorded 312 samples. Sorry, 309 receipts.",
            "We recorded 312 samples. Sorry, three hundred and nine.",
            "The visit took 3 hours. Sorry, 4 days.",
            "Send it before Tuesday at 3 PM. Actually Friday works better.",
            "Send it before Tuesday at 3 PM. Actually at 10:15 AM we left Friday.",
            "We recorded \"312 samples\". Sorry, 309 samples.",
            "We recorded 312 samples. Sorry, I lost 309 samples.",
            "Meet Tuesday at 3 PM. Actually, Friday at 10:15 AM is unavailable.",
            "We recorded three hundred and final twelve samples. Sorry, 309 samples.",
            "We recorded three hundred and thirty twelve samples. Sorry, 309 samples.",
            "We recorded three hundred and thir twelve samples. Sorry, 309 receipts.",
            "Are there 5 files? Sorry, 7 files.",
            "There are 5 files. No, 7 files.",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn explicit_denied_count_replaces_only_the_matching_preceding_value() {
        for (source, expected) in [
            ("We have completed 214 checks. No, it's not 214, it's 210 checks.",
             "We have completed 210 checks."),
            ("We completed two hundred and fourteen checks. No, it is not two hundred and fourteen, it is two hundred and ten checks. Keep six compatibility tests.",
             "We completed 210 checks. Keep 6 compatibility tests."),
            ("We completed 214 checks. No, it\u{2019}s not 214 checks, it\u{2019}s 210 checks before Friday.",
             "We completed 210 checks before Friday."),
            ("We completed 214 checks. No, not 214 but 210 checks.",
             "We completed 210 checks."),
            ("We completed 214 checks. No, it isn't 214, it's 210.",
             "We completed 210 checks."),
            ("It took 3 hours. No, it was not 3, it was 4. Keep the logs.",
             "It took 4 hours. Keep the logs."),
            ("Use code 214. No, it's not 214, it's 210.", "Use code 210."),
            ("We completed 214 checks. No, it's not 214, it's 210 checks. No, it's not 210, it's 209 checks.",
             "We completed 209 checks."),
        ] {
            assert_eq!(format_numbers(&clean(source, &options())), expected);
        }
        assert_eq!(correction_cue_count("No, it's not 214, it's 210."), 1);
    }

    #[test]
    fn quantity_contrasts_preserve_other_facts_negations_and_quoted_values() {
        for source in [
            "We completed 214 checks. No, it's not 999, it's 210 checks.",
            "We completed 214 checks. No, it's not 214, it's 210 files.",
            "We completed 214 checks. No, it's not ready, it's broken.",
            "There are 214 files. No, it's not 214 hours, it's 210 hours.",
            "Do not delete 214 files or overwrite 210 logs.",
            "We completed 214 checks and the other team completed 210 checks.",
            "The label is \"214\". No, it's not 214, it's 210.",
            "The label says \"No, it's not 214, it's 210\".",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn recipient_contrasts_and_modifiers_resolve_verified_old_values() {
        for (source, expected) in [
            ("Send the report to Daniel. No, not Daniel but Maya. We have completed 214 checks. No, it's not actually 214, it's 210 checks.",
             "Send the report to Maya. We have completed 210 checks."),
            ("Email it to Daniel Reed. No, not Daniel Reed but Maya Patel before Friday. Keep 214 logs.",
             "Email it to Maya Patel before Friday. Keep 214 logs."),
            ("Send it to Daniel. No, not actually Daniel but really Maya.", "Send it to Maya."),
            ("Send it to Daniel. No, not to Daniel but to Maya.", "Send it to Maya."),
            ("Send it to daniel. No, not daniel but maya.", "Send it to Maya."),
            ("Send it to Daniel. No, not Daniel but Maya de Silva.", "Send it to Maya de Silva."),
            ("Send it to De Daniel. No, not De Daniel but Maya.", "Send it to Maya."),
            ("We completed 214 checks. No, it's not really 214, it's actually 210 checks.",
             "We completed 210 checks."),
            ("We completed 214 checks. No, not 214 but actually 210 checks.", "We completed 210 checks."),
        ] {
            assert_eq!(format_numbers(&clean(source, &options())), expected);
        }
    }

    #[test]
    fn recipient_contrasts_preserve_mismatches_full_names_and_prohibitions() {
        for source in [
            "Send it to Daniel. No, not Alex but Maya.",
            "Send it to Daniel Reed. No, not Daniel but Maya.",
            "Daniel and Maya are both invited. No, not Daniel but Maya.",
            "Send it to Daniel and Maya.",
            "Send it to Daniel. Do not send it to Maya.",
            "Send it to Daniel. No, not actually ready but Maya can help.",
            "Send it to \"Daniel\". No, not Daniel but Maya.",
            "The label says \"No, not Daniel but Maya\".",
            "We completed 214 checks. No, it's not actually 999, it's 210 checks.",
            "We completed 214 checks. No, it's not \"actually\" 214, it's 210 checks.",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn direct_you_requests_fix_an_asr_period_without_changing_indirect_or_quoted_text() {
        assert_eq!(
            clean("Could you review the panel. Keep the backup.", &options()),
            "Could you review the panel? Keep the backup."
        );
        for source in [
            "I asked could you review the panel.",
            "Could you believe it!",
            "The note says \"Could you review the panel.\".",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn quoted_content_remains_literal_while_outer_grammar_can_change() {
        let mut custom = options();
        custom.dictionary.push(Replacement {
            from: ":".into(),
            to: "Marker".into(),
        });
        assert_eq!(clean(": and \"comma\"", &custom), "Marker and \"comma\"");
        assert_eq!(
            clean(
                "The label says \"Here are the tasks: First, save. Second, close.\".",
                &options()
            ),
            "The label says \"Here are the tasks: First, save. Second, close.\"."
        );
        assert_eq!(
            clean(
                "Um she said \"No sorry I mean three point five, new line\" and we we kept it.",
                &options()
            ),
            "She said \"No sorry I mean three point five, new line\" and we kept it."
        );
        assert!(!accept_rewrite(
            "She said \"put it on the shelf\".",
            "She said \"put it in the shelf\"."
        ));
        assert!(!accept_rewrite(
            "She said \"Keep the logs\".",
            "She said keep the logs."
        ));
        assert!(accept_rewrite(
            "She dont know who said \"Keep the logs\".",
            "She does not know who said \"Keep the logs\"."
        ));
        assert!(accept_rewrite(
            "She said \u{201c}Keep the logs\u{201d}.",
            "She said \"Keep the logs\"."
        ));
    }

    #[test]
    fn reverse_recipient_corrections_verify_the_full_denied_name() {
        for (source,expected) in [
            ("Send the checklist to Ingrid Cole. Wait, I meant Karim Shah, not Ingrid Cole. Keep 12 receipts.",
             "Send the checklist to Karim Shah. Keep 12 receipts."),
            ("Email it to Ingrid. Sorry I meant Karim, not Ingrid before Thursday.",
             "Email it to Karim before Thursday."),
            ("Send it to Ingrid Wait I meant Karim not Ingrid Keep 12 records",
             "Send it to Karim Keep 12 records"),
            ("we recorded 72 samples let me correct that we recorded 68 samples the original quote is 720 rupees and the revised quote is 680 rupees",
             "We recorded 68 samples the original quote is 720 rupees and the revised quote is 680 rupees"),
        ] { assert_eq!(clean(source,&options()),expected); }
        for source in [
            "Send it to Ingrid Cole. Wait, I meant Karim Shah, not Ingrid.",
            "Send it to Ingrid. Wait, I meant Karim, not Tessa.",
            "Send it to Ingrid. Wait, I meant Karim, not Ingrid Cole.",
            "Send it to \"Ingrid\". Wait, I meant Karim, not Ingrid.",
            "I am sorry I mean we are not ready.",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn explicit_start_next_finally_lists_keep_alternatives_and_require_the_request() {
        assert_eq!(clean("Put these into bullet points, start with checking the sensors or asking Ingrid, next, save 12 receipts, finally, keep the logs.",&options()),
            "Put these into bullet points:\n- Checking the sensors or asking Ingrid.\n- Save 12 receipts.\n- Keep the logs.");
        assert_eq!(clean("Put these into bullet points. Start with checking the sensors or asking Ingrid. Next, save 12 receipts. Finally, keep the logs.",&options()),
            "Put these into bullet points:\n- Checking the sensors or asking Ingrid.\n- Save 12 receipts.\n- Keep the logs.");
        assert_eq!(clean("put these into bullet points start with checking the sensors or asking Ingrid next save 12 receipts finally keep the logs",&options()),
            "Put these into bullet points:\n- Checking the sensors or asking Ingrid\n- Save 12 receipts\n- Keep the logs");
        for source in [
            "Start with a question. Next, consider it. Finally, decide.",
            "The label says \"start with red next blue finally green\".",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
    }

    #[test]
    fn full_repeated_corrections_change_only_a_verified_value() {
        for (source, expected) in [
            ("Please give Ingrid Cole the survey. No, that is wrong, please give Karim Shah the survey.",
             "Please give Karim Shah the survey."),
            ("Send it to Ingrid Cole. Let me correct that: send it to Karim Shah. Keep 12 receipts.",
             "Send it to Karim Shah. Keep 12 receipts."),
            ("We recorded 321 samples. Let me correct that: we recorded 319 samples. Do not delete 4 logs.",
             "We recorded 319 samples. Do not delete 4 logs."),
            ("The visit took 3 hours. Let me correct that: the visit took 4 hours.",
             "The visit took 4 hours."),
            ("Keep 2 copies. We counted 72 inspections. No, not 72 inspections, 68 inspections. Preserve archive_v4.csv.",
             "Keep 2 copies. We counted 68 inspections. Preserve archive_v4.csv."),
        ] {
            assert_eq!(clean(source, &options()), expected);
            assert!(accept_rewrite(expected, expected));
        }
    }

    #[test]
    fn repeated_corrections_require_matching_context_and_preserve_prohibitions() {
        for source in [
            "We counted 72 inspections. No, not 71 inspections, 68 inspections.",
            "We counted 72 inspections. No, not 72 inspections, 68 receipts.",
            "We counted 72 inspections. No, not 72 inspections, 68.",
            "Please give Ingrid Cole the survey. No, that is wrong, please give Karim Shah the invoice.",
            "We counted 72 inspections. Let me correct that: we deleted 68 inspections.",
            "Do not upload the archive. Let me correct that: upload the archive.",
            "The label says \"Let me correct that: we counted 68 inspections\".",
            "Send it to \"Ingrid Cole\". Let me correct that: send it to Karim Shah.",
            "The visit took 3 hours. Let me correct that: the visit took 4 days.",
        ] {
            assert_eq!(clean(source, &options()), source);
        }
        let source = clean("We counted 72 inspections. No, not 72 inspections, 68 inspections. Do not upload archive_v4.csv.", &options());
        assert!(!accept_rewrite(
            &source,
            "We counted 69 inspections. Do not upload archive_v4.csv."
        ));
        assert!(!accept_rewrite(
            &source,
            "We counted 68 inspections. Upload archive_v4.csv."
        ));
    }

    #[test]
    fn ordinary_apologies_actual_facts_and_quoted_cues_are_not_replacements() {
        let o = options();
        for source in [
            "It actually works.",
            "I am sorry you missed the meeting.",
            "We sent it to Leonie. Actually it arrived.",
            "I mean, it works.",
            "The original amount was 2400 and the new amount is 1800.",
            "The label says \"no wait\" next to the button.",
            "He wrote 'I mean' in the notes.",
        ] {
            assert_eq!(clean(source, &o), source);
        }
    }

    #[test]
    fn direct_questions_receive_punctuation_before_the_following_instruction() {
        let o = options();
        assert_eq!(
            clean("can you check the invoice", &o),
            "can you check the invoice?"
        );
        assert_eq!(
            clean("Please check whether the invoice is correct.", &o),
            "Please check whether the invoice is correct."
        );
        assert_eq!(
            clean("I wonder if you can check the invoice.", &o),
            "I wonder if you can check the invoice."
        );
        assert_eq!(
            clean(
                "can you check the invoice and one more thing dont delete the backup",
                &o
            ),
            "can you check the invoice? And one more thing dont delete the backup"
        );
    }

    #[test]
    fn rewrite_guard_preserves_numbers_units_dates_versions_and_signs() {
        for (source, bad) in [
            (
                "Keep the total at 7250 rupees.",
                "Keep the total at 7520 rupees.",
            ),
            ("It took 3.25 hours.", "It took 3.25 days."),
            ("Set it to -5 percent.", "Set it to 5 percent."),
            (
                "Keep version 3.8.2 installed.",
                "Keep version 3.8.3 installed.",
            ),
            (
                "The deadline is Wednesday at 7:45 pm.",
                "The deadline is Thursday at 7:45 pm.",
            ),
            (
                "The deadline is Wednesday at 7:45 pm.",
                "The deadline is Wednesday at 7:45 am.",
            ),
            ("Use code 0042.", "Use code 42."),
            ("Use 1,5 liters.", "Use 15 liters."),
            ("There are five files.", "There are six files."),
            ("Use .5 kilograms.", "Use 5 kilograms."),
            ("The amount is -$25.", "The amount is $25."),
        ] {
            assert!(
                !accept_rewrite(source, bad),
                "unsafe fact change was accepted"
            );
        }
        for (source, good) in [
            (
                "The total is 7,250.50 rupees.",
                "The total is 7250.50 rupees.",
            ),
            ("Use 2.50 kilograms.", "Use 2.5 kg."),
            ("Use 2.5kg.", "Use 2.5 kg."),
            ("The fee is USD 25.", "The fee is $25."),
            ("Meet Friday at 10 30 am.", "Meet Friday at 10:30 am."),
            ("Meet at three pm.", "Meet at 3 pm."),
            ("There are five files.", "There are 5 files."),
            ("Use .50 kilograms.", "Use 0.5 kilograms."),
        ] {
            assert!(
                accept_rewrite(source, good),
                "equivalent fact formatting was refused"
            );
        }
    }

    #[test]
    fn rewrite_guard_preserves_named_recipients_and_negation_scope() {
        for (source, bad) in [
            ("Send the draft to Leonie.", "Send the draft to Darius."),
            ("Email the draft to leonie.", "Email the draft to darius."),
            ("Ask Ingrid Cole to review it.", "Ask Ingrid to review it."),
            ("Ask ingrid cole to review it.", "Ask ingrid to review it."),
            (
                "Give Ingrid Cole the survey.",
                "Give Karim Shah the survey.",
            ),
            ("Omar has the latest draft.", "Neha has the latest draft."),
            ("Use invoice INV-2048.", "Use invoice INV-2049."),
            ("Keep Archive_Q4_2026.csv.", "Keep archive_q4_2026.csv."),
            ("Open Report.csv.", "Open report.csv."),
            ("Do not delete the backup.", "Delete the backup."),
            (
                "Do not send the invoice; delete the draft.",
                "Send the invoice; do not delete the draft.",
            ),
            (
                "Do not change the amount or delete the backup.",
                "Do not change the amount. Delete the backup.",
            ),
            ("We haven't sent the draft.", "We have sent the draft."),
            (
                "Do not cancel the booking.",
                "We cannot cancel the booking.",
            ),
            (
                "Do not cancel the booking.",
                "We will not cancel the booking.",
            ),
            ("Ship the release in May.", "Ship the release soon."),
        ] {
            assert!(
                !accept_rewrite(source, bad),
                "unsafe entity or negation change was accepted"
            );
        }
        assert!(accept_rewrite(
            "Omar dont have the draft and i hasnt received it.",
            "Omar does not have the draft and I have not received it."
        ));
        assert!(accept_rewrite(
            "Please don't delete the backup.",
            "Please do not delete the backup."
        ));
    }

    #[test]
    fn rewrite_guard_preserves_action_direction_and_does_not_append_answers() {
        for (source, bad) in [
            ("Fix the logout issue.", "Fix the login issue."),
            ("The user signs out.", "The user logs in."),
            ("Do not increase the limit.", "Do not decrease the limit."),
            ("Disable this option.", "Enable this option."),
            ("Upload the report.", "Download the report."),
            (
                "What is the capital of Canada?",
                "What is the capital of Canada? Ottawa.",
            ),
            ("can you check the invoice", "The invoice looks correct."),
            ("Can you check the invoice?", "Can you check the invoice."),
        ] {
            assert!(
                !accept_rewrite(source, bad),
                "unsafe action change or answer was accepted"
            );
        }
        assert!(accept_rewrite("The user signs out.", "The user logs out."));
        assert!(accept_rewrite(
            "can you check the invoice",
            "Can you check the invoice?"
        ));
        assert!(!accept_rewrite(
            "Can't we use this option?",
            "Can we use this option?"
        ));
    }

    #[test]
    fn ordinary_verb_capitalization_does_not_become_a_protected_person() {
        assert!(accept_rewrite(
            "send it to Petra before Friday",
            "Send it to Petra before Friday."
        ));
        assert!(accept_rewrite(
            "keep the pressure at -5 percent",
            "Keep the pressure at -5 percent."
        ));
        assert!(accept_rewrite(
            "May has the latest draft.",
            "May has the latest draft."
        ));
        assert!(!accept_rewrite(
            "May has the latest draft.",
            "She has the latest draft."
        ));
        assert!(accept_rewrite(
            "petra has the latest draft",
            "Petra has the latest draft."
        ));
        assert!(accept_rewrite(
            "who has the latest draft?",
            "Who has the latest draft?"
        ));
        assert!(accept_rewrite(
            "the review is in july",
            "The review is in July."
        ));
        assert!(!accept_rewrite(
            "Send it to Leonie Hart.",
            "Send it to Leonie Quinn."
        ));
    }

    #[test]
    fn rewrites_do_not_drop_the_auxiliary_from_negative_instructions() {
        assert!(accept_rewrite(
            "I havent uploaded the files yet please dont delete the backup or overwrite the logs.",
            "I have not uploaded the files yet. Please do not delete the backup or overwrite the logs."
        ));
        assert!(!accept_rewrite(
            "Do not cancel the booking.",
            "Not cancel the booking."
        ));
        assert!(!accept_rewrite(
            "Don't delete the backup.",
            "Please not delete the backup."
        ));
        assert!(accept_rewrite(
            "Please do not cancel the booking.",
            "Please don't cancel the booking."
        ));
        assert!(accept_rewrite(
            "The app isn't responding.",
            "The app is currently not responding."
        ));
        assert!(accept_rewrite(
            "Try not to close it.",
            "Try not to close it."
        ));
        assert!(accept_rewrite(
            "Not only the first file.",
            "Not only the first file."
        ));
    }

    #[test]
    fn an_unpunctuated_question_restart_is_not_a_negative_instruction() {
        assert!(accept_rewrite(
            "which year no when was it released?",
            "When was it released?"
        ));
        assert!(!accept_rewrite("Are there no errors?", "Are there errors?"));
        assert!(!accept_rewrite(
            "No, why should I leave?",
            "Why should I leave?"
        ));
        assert_eq!(
            clean("Set the limit to 45 no 54 before saving.", &options()),
            "Set the limit to 54 before saving."
        );
        assert_eq!(
            clean("Are there 5 files? No, 7 files.", &options()),
            "Are there 5 files? No, 7 files."
        );
        assert_eq!(
            clean("The issue is that the app crashes.", &options()),
            "The issue is that the app crashes."
        );
        assert_eq!(
            clean("What is important is the result.", &options()),
            "What is important is the result."
        );
        assert!(accept_rewrite(
            "What is important is the result.",
            "What is important is the result."
        ));
    }
}



#[cfg(test)]
mod list_layout_tests {
    use super::{clean, TextOptions};

    fn options() -> TextOptions {
        TextOptions { filler_removal: true, spoken_punctuation: true, dictionary: Vec::new(), snippets: Vec::new() }
    }

    #[test]
    fn steps_after_a_lead_in_become_a_list() {
        assert_eq!(
            clean("To set it up, step one, download the installer. Step two, run it as admin. Step three, restart your PC.", &options()),
            "To set it up:
- Download the installer.
- Run it as admin.
- Restart your PC."
        );
        // Without a second step it is just a sentence.
        let text = "To be fair, first, we should ask. Then we decide.";
        assert_eq!(clean(text, &options()), text);
    }

    #[test]
    fn a_requested_list_of_comma_items_becomes_bullets() {
        assert_eq!(
            clean("Make a list of things to pack: passport, charger, headphones and a jacket.", &options()),
            "Make a list of things to pack:
- Passport
- Charger
- Headphones
- A jacket"
        );
        // Not asked for: an ordinary sentence.
        for text in [
            "We bought apples, pears and plums.",
            "Pack these: passport, charger and a jacket.",
        ] {
            assert_eq!(clean(text, &options()), text);
        }
    }
}


#[cfg(test)]
mod dictated_list_tests {
    #[test]
    fn small_words_lose_a_capital_left_over_from_a_pause() {
        use super::finish_layout;
        assert_eq!(
            finish_layout("Can you increase the hover On hover time?"),
            "Can you increase the hover on hover time?"
        );
        assert_eq!(finish_layout("I paused here, And then went on."), "I paused here, and then went on.");
        // "I" is always a capital, so it doesn't make "And I" a name.
        assert_eq!(
            finish_layout("It's performance centric, And I want better graphics, But I'm not sure."),
            "It's performance centric, and I want better graphics, but I'm not sure."
        );
        // Sentence starts, names, titles and "I" keep their capitals.
        for kept in [
            "It works. On Monday we ship.",
            "We read The Lord Of The Rings.",
            "Open Spotify and play The Weeknd.",
            "I think I can.",
            "Plan A is fine.",
            "Notes:\n- On Monday, ship it.",
        ] {
            assert_eq!(finish_layout(kept), kept);
        }
    }

    #[test]
    fn a_counted_lead_in_makes_a_list_when_the_count_fits() {
        let bullets = "I need 3 things:\n- Milk\n- Eggs\n- Bread";
        for said in [
            "I need 3 things: milk, eggs, and bread.",
            "I need three things: milk, eggs and bread.",
            "I need three things, milk, eggs, and bread.",
            // Nothing heard after the count, or a full stop there.
            "I need three things milk, eggs, and bread.",
            "I need three things milk eggs and bread.",
            "I need three things. Milk, eggs, and bread.",
        ] {
            assert_eq!(cleaned(said), bullets, "{said}");
        }
        // The count must match, and the noun must be a list word.
        for prose in [
            "I need 3 things: milk and eggs.",
            "I have three kids: Anna, Ben and Carl.",
            "We tried three things, none of them worked, sadly.",
            "I tried two things today and yesterday.",
            "I need three things. Milk and eggs are two of them.",
        ] {
            let out = cleaned(prose);
            assert!(!out.contains("\n- "), "{prose} -> {out}");
        }
    }

    use super::{accept_rewrite, clean, format_numbers, TextOptions};

    fn options() -> TextOptions {
        TextOptions { filler_removal: true, spoken_punctuation: true, dictionary: Vec::new(), snippets: Vec::new() }
    }

    fn cleaned(text: &str) -> String {
        format_numbers(&clean(text, &options()))
    }

    /// Each of these came back from the speech model exactly like this in a
    /// real test and was laid out wrong before.
    #[test]
    fn lists_as_the_speech_model_punctuates_them() {
        assert_eq!(
            cleaned("I need three things for the launch. We need a landing page, we need a demo video. We need a press kit."),
            "I need 3 things for the launch:\n- We need a landing page.\n- We need a demo video.\n- We need a press kit."
        );
        for text in [
            "Firstly, the app crashes on startup, secondly, the dark mode is broken, lastly, the settings don't save.",
            "Firstly, the app crashes on startup. Secondly, the dark mode is broken, lastly, the settings don't save.",
        ] {
            assert_eq!(
                cleaned(text),
                "- The app crashes on startup.\n- The dark mode is broken.\n- The settings don't save.",
                "{text}"
            );
        }
        assert_eq!(
            cleaned("Number 1. Faster search. Number 2. Better offline mode. Number 3. A cleaner settings page."),
            "- Faster search.\n- Better offline mode.\n- A cleaner settings page."
        );
        assert_eq!(
            cleaned("I have a few pointers for the next release. The first one is to add voice commands. The second feature will be to support dark mode. And the third one is. Improving battery life."),
            "I have a few pointers for the next release:\n- Add voice commands.\n- Support dark mode.\n- Improving battery life."
        );
        assert_eq!(
            cleaned("Set it up. Step 1: Download the installer. Step 2: Run it as an admin. Step 3: Restart your PC."),
            "Set it up:\n- Download the installer.\n- Run it as an admin.\n- Restart your PC."
        );
        assert_eq!(
            cleaned("Make a list of things to pack for a passport, charger, headphones and a jacket."),
            "Make a list of things to pack:\n- Passport\n- Charger\n- Headphones\n- A jacket"
        );
        // A step's own "then" stays inside it.
        assert_eq!(
            cleaned("First, cook the rice, then add salt. Second, fry the onions."),
            "- Cook the rice, then add salt.\n- Fry the onions."
        );
    }

    #[test]
    fn a_list_asked_for_after_steps_is_its_own_list() {
        assert_eq!(
            cleaned("Okay so to fix the build, step one, delete the target folder, step two, run cargo clean, and step three, rebuild the release. Also make a list of things to check after that: the logs, the installer, the icons and the settings page."),
            "Okay so to fix the build:\n- Delete the target folder.\n- Run cargo clean.\n- Rebuild the release.\nAlso make a list of things to check after that:\n- The logs\n- The installer\n- The icons\n- The settings page"
        );
    }

    /// Round three, said with deliberate mistakes.
    #[test]
    fn a_denied_value_is_replaced_where_it_was_first_said() {
        assert_eq!(
            cleaned("Finalize the budget, which is around 25,000 dollars. No, no, not $25,000. Sorry, $30,000."),
            "Finalize the budget, which is around $30,000."
        );
        assert_eq!(cleaned("Order 12 chairs, no, not 12, I mean 15."), "Order 15 chairs.");
        // A plain "no, not now" is no correction.
        let text = "Can we start? No, not now. Sorry, I am busy.";
        assert_eq!(cleaned(text), text);
    }

    #[test]
    fn a_misheard_thanks_is_thanks() {
        assert_eq!(cleaned("Can you send it before 5 PM? Tanks."), "Can you send it before 5 PM? Thanks.");
        assert_eq!(cleaned("Tanks for the update, I will review it today."), "Thanks for the update, I will review it today.");
        assert_eq!(cleaned("Tanks a lot, Maya."), "Thanks a lot, Maya.");
        for text in ["The army moved the tanks closer.", "Tanks are expensive to run.", "Clean the fish tanks. Tanks need water."] {
            assert_eq!(clean(text, &options()), text);
        }
    }

    #[test]
    fn a_stop_before_a_lowercase_word_was_only_a_pause() {
        assert_eq!(
            cleaned("Can you send the contract to Daniel? before 5pm. The new contract the contract should include the new pricing. and the delivery dates."),
            "Can you send the contract to Daniel before 5pm? The contract should include the new pricing and the delivery dates."
        );
        // A question that went on, then ran into a new "I want…" sentence
        // with no stop: the question ends before it.
        assert_eq!(
            cleaned("Can you create a PDF on how we train the AI model? like the Gemma model for dictation I want to explain it to a developer."),
            "Can you create a PDF on how we train the AI model like the Gemma model for dictation? I want to explain it to a developer."
        );
        // But not inside the question's own clause ("…that I want…").
        assert_eq!(
            cleaned("Can you find the report? the one from last week that I want to send."),
            "Can you find the report the one from last week that I want to send?"
        );
        // Real sentence ends and abbreviations stay.
        for text in ["It works. Then we ship.", "Talk to Dr. smith today.", "Use e.g. the logs."] {
            assert_eq!(clean(text, &options()), text);
        }
    }

    #[test]
    fn a_requested_list_after_a_full_stop() {
        assert_eq!(
            cleaned("Also make a list of things to check after that. Logs, installer, the icons and the settings page."),
            "Also make a list of things to check after that:\n- Logs\n- Installer\n- The icons\n- The settings page"
        );
    }

    #[test]
    fn the_final_layout_capitalizes_bullets_and_spaces_currency() {
        assert_eq!(super::finish_layout("Steps:\n- rebuild the release.\n- Ship"), "Steps:\n- Rebuild the release.\n- Ship.");
        assert_eq!(super::finish_layout("No, not$25,000. Sorry,$30,000 and $ 40"), "No, not $25,000. Sorry, $30,000 and $40");
        assert_eq!(super::finish_layout("Pay ($5) now"), "Pay ($5) now");
    }

    /// Round four: long dictations with stutters, fillers, corrections,
    /// lists and paragraphs, as the rules see them.
    #[test]
    fn complex_dictations() {
        assert_eq!(
            cleaned("Um so so I was thinking about the the launch plan and uh I have a few pointers. The first one is to to finalize the landing page by by Friday, no sorry, by Thursday. The second one is, um, we need to hire two two designers. And the third one is, like, to set up the the analytics dashboard. And finally, we should, you know, send the press kit to Maya, I mean to Daniel."),
            "So I was thinking about the launch plan and I have a few pointers:\n- Finalize the landing page by Thursday.\n- We need to hire 2 designers.\n- Set up the analytics dashboard.\n- We should send the press kit to Daniel."
        );
        assert_eq!(
            cleaned("First, book the flights. Second, reserve the the hotel near the near the conference center. Third, buy train tickets."),
            "- Book the flights.\n- Reserve the hotel near the conference center.\n- Buy train tickets."
        );
        assert_eq!(
            cleaned("Hey Priya new paragraph Thanks for the the update on the the budget. I I think we we're around twenty two thousand dollars, no, no, not twenty two thousand, sorry, twenty five thousand dollars. Can you, uh, can you check the numbers before before Monday? new paragraph So the plan. First, uh, review the invoices. Second, flag anything above five hundred dollars. Finally, send me a summary. new paragraph Thanks"),
            "Hey Priya\n\nThanks for the update on the budget. I think we're around 25000 dollars. Can you check the numbers before Monday?\n\nSo the plan:\n- Review the invoices.\n- Flag anything above 500 dollars.\n- Send me a summary.\n\nThanks"
        );
        assert_eq!(
            cleaned("So for the app I want three things. We need a dark mode, we need offline sync. We need faster search. Also this settings page should be the settings page should be cleaner."),
            "So for the app I want 3 things:\n- We need a dark mode.\n- We need offline sync.\n- We need faster search.\nAlso the settings page should be cleaner."
        );
        // Ordinary speech stays.
        for text in [
            "Put the book on the table near the door.",
            "I like it like that.",
            "Do you know the answer?",
            "Send it to the team that built the team page.",
            "What it is is a better way to work.",
        ] {
            assert_eq!(clean(text, &options()), text, "{text}");
        }
        assert_eq!(cleaned("The settings page is is kind of messy."), "The settings page is kind of messy.");
        assert_eq!(super::finish_layout("Check:\n- The logs\n- The installer\n- The settings page."), "Check:\n- The logs\n- The installer\n- The settings page");
    }

    #[test]
    fn the_model_may_not_flatten_requested_lines() {
        let rules = clean("Hi Sarah new paragraph Thanks for the update new line I will review it today new paragraph Best regards", &options());
        assert_eq!(rules, "Hi Sarah\n\nThanks for the update\nI will review it today\n\nBest regards");
        assert!(!accept_rewrite(&rules, "Hi Sarah, Thanks for the update. I will review it today."));
    }
}
