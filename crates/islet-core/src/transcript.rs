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

const FILLERS: &[&str] = &["um", "umm", "uh", "uhh", "uhm", "erm", "hmm", "mhm"];

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

        let is_filler = !token.verbatim && FILLERS.contains(&token.key().as_str());
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
        tokens = remove_stutters(remove_fillers(tokens));
    }
    render(&tokens)
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
}
