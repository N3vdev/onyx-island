//! Coherent, bounded pieces of already rule-cleaned text.
use std::collections::HashMap;

pub(crate) struct Piece<'a> {
    pub text: &'a str,
    pub separator: &'a str,
    pub infer: bool,
}

fn boundary(words: &[&str], at: usize) -> i32 {
    let previous = words[at - 1];
    let word = words[at].to_lowercase();
    let next = words
        .get(at + 1)
        .map(|w| w.to_lowercase())
        .unwrap_or_default();
    let third = words
        .get(at + 2)
        .map(|w| w.to_lowercase())
        .unwrap_or_default();
    if previous.ends_with(['?', '!', ';'])
        || (previous.ends_with('.')
            && !previous.contains(['/', '@', '_'])
            && !previous.trim_end_matches('.').contains('.'))
    {
        return 4;
    }
    if matches!(word.as_str(), "please" | "also")
        || (matches!(word.as_str(), "can" | "could" | "would") && next == "you")
    {
        return 3;
    }
    if (word == "do" && next == "not") || matches!(word.as_str(), "don't" | "dont") {
        return if matches!(
            previous.to_lowercase().as_str(),
            "please" | "and" | "but" | "also"
        ) {
            0
        } else {
            3
        };
    }
    if matches!(word.as_str(), "and" | "but")
        && (matches!(
            next.as_str(),
            "i" | "we"
                | "you"
                | "they"
                | "he"
                | "she"
                | "it"
                | "do"
                | "don't"
                | "dont"
                | "please"
                | "keep"
                | "download"
                | "upload"
        ) || (words
            .get(at + 1)
            .is_some_and(|w| w.chars().next().is_some_and(char::is_uppercase))
            && matches!(
                third.as_str(),
                "is" | "has" | "have" | "does" | "dont" | "doesn't"
            )))
    {
        return 3;
    }
    if word == "the"
        && words.iter().skip(at + 1).take(3).any(|w| {
            matches!(
                w.to_lowercase().as_str(),
                "is" | "are" | "was" | "were" | "took" | "has" | "have"
            )
        })
    {
        return 2;
    }
    0
}

pub(crate) fn plan(source: &str) -> Vec<Piece<'_>> {
    let words: Vec<_> = source.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut cursor = 0;
    let spans: Vec<_> = words
        .iter()
        .map(|word| {
            let start = cursor + source[cursor..].find(word).unwrap();
            cursor = start + word.len();
            (start, cursor)
        })
        .collect();
    let mut cuts = vec![0];
    let mut start = 0;
    while words.len() - start > 48 || spans.last().unwrap().1 - spans[start].0 > 1100 {
        let ceiling = words.len().min(start + 48);
        let at = ((start + 10)..=ceiling)
            .filter(|&at| {
                at < words.len() && spans[at].0 - spans[start].0 <= 1100 && boundary(&words, at) > 0
            })
            .max_by_key(|&at| boundary(&words, at) * 8 - (at as i32 - start as i32 - 32).abs())
            .unwrap_or_else(|| {
                ((ceiling + 1)..words.len())
                    .find(|&at| boundary(&words, at) > 0)
                    .unwrap_or(words.len())
            });
        cuts.push(at);
        start = at;
        if start == words.len() {
            break;
        }
    }
    if cuts.last().copied() != Some(words.len()) {
        cuts.push(words.len());
    }
    cuts.windows(2)
        .map(|pair| {
            let (left, right) = (pair[0], pair[1]);
            let text = &source[spans[left].0..spans[right - 1].1];
            Piece {
                text,
                separator: if right < words.len() {
                    &source[spans[right - 1].1..spans[right].0]
                } else {
                    ""
                },
                infer: right - left <= 48 && text.len() <= 1100,
            }
        })
        .collect()
}

pub(crate) fn repeating(source: &str, output: &str) -> bool {
    fn counts(text: &str) -> HashMap<Vec<String>, usize> {
        let words: Vec<_> = text
            .split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect();
        let mut result = HashMap::new();
        for phrase in words.windows(5) {
            *result.entry(phrase.to_vec()).or_insert(0) += 1;
        }
        result
    }
    let before = counts(source);
    counts(output)
        .iter()
        .any(|(phrase, &count)| count >= 3.max(before.get(phrase).copied().unwrap_or(0) + 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_text_and_protected_tokens() {
        let source = format!("Keep version 7.2.4 and reports_Q4_2026.csv.\n\n{} please do not delete the backup or overwrite the logs.", vec!["context"; 35].join(" "));
        let pieces = plan(&source);
        assert_eq!(
            pieces
                .iter()
                .map(|p| format!("{}{}", p.text, p.separator))
                .collect::<String>(),
            source
        );
        assert!(pieces
            .iter()
            .all(|p| !p.infer || p.text.split_whitespace().count() <= 48));
    }
    #[test]
    fn unbroken_clauses_are_not_guessed_at() {
        assert!(!plan(&vec!["word"; 90].join(" "))[0].infer);
    }
    #[test]
    fn loops_are_bounded_without_rejecting_source_repeats() {
        let phrase = "we need to keep this document ";
        assert!(repeating("keep it", &phrase.repeat(3)));
        assert!(!repeating(&phrase.repeat(3), &phrase.repeat(3)));
    }
}
