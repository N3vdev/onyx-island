//! Learning from your corrections (opt-in, local only).
//!
//! After Onyx types a dictation, the app may read the same text box again a
//! little later. If you changed a few of the words it typed, that edit says
//! what you meant: `corrected` finds the dictated passage in the box and
//! returns your version of it, and `word_swaps` lists the single-word
//! changes ("tanks" -> "thanks") that the dictionary can learn.
//!
//! Only edits *inside* the dictated passage count. A passage you rewrote
//! wholesale, deleted, or that can't be found again is ignored.

/// Words in a passage, with surrounding punctuation dropped for matching.
fn key(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

fn keys(text: &str) -> Vec<String> {
    text.split_whitespace().map(key).collect()
}

/// Your edited version of the dictated passage `typed`, found in `field`
/// (the text box contents later), or `None` when it is unchanged, gone, or
/// changed too much to be a correction.
#[must_use]
pub fn corrected(typed: &str, field: &str) -> Option<String> {
    let field_words: Vec<&str> = field.split_whitespace().collect();
    let (t, f) = (keys(typed), keys(field));
    let n = t.len();
    if n < 3 || f.is_empty() {
        return None;
    }
    // The stretch of the box closest to what was typed. Only the end of a
    // very long box is searched; a dictation is usually near where you are.
    let from = f.len().saturating_sub(4000);
    // (start, end, words changed, length difference): fewest changes, then
    // the stretch closest in length to what was typed.
    let mut best: Option<(usize, usize, usize, usize)> = None;
    for start in from..f.len() {
        // Cheap filter: the stretch must begin near one of the first words.
        if !t[..3.min(n)].contains(&f[start]) && start > from && !t[..3.min(n)].contains(&f[start - 1]) {
            continue;
        }
        for len in n.saturating_sub(3).max(1)..=(n + 3) {
            let end = start + len;
            if end > f.len() {
                break;
            }
            let changed = distance(&t, &f[start..end]);
            let gap = len.abs_diff(n);
            if best.is_none_or(|(_, _, d, g)| (changed, gap) < (d, g)) {
                best = Some((start, end, changed, gap));
            }
        }
    }
    let (start, end, changed, _) = best?;
    // Text typed straight after (or before) the dictation, with no space,
    // sticks to its edge words ("Tanks.Can"). That is not a correction.
    let mut region: Vec<String> = f[start..end].to_vec();
    let raw: Vec<&str> = field_words[start..end].to_vec();
    if let (Some(first), Some(word)) = (region.first_mut(), raw.first()) {
        let glued = word.to_lowercase();
        if *first != t[0] && glued.ends_with(&typed_edge(typed, false)) {
            first.clone_from(&t[0]);
        }
    }
    if let (Some(last), Some(word)) = (region.last_mut(), raw.last()) {
        let glued = word.to_lowercase();
        if *last != t[n - 1] && glued.starts_with(&typed_edge(typed, true)) {
            last.clone_from(&t[n - 1]);
        }
    }
    let changed = changed.min(distance(&t, &region));
    // Unchanged, or changed too much to be a correction of it.
    if changed == 0 || changed * 10 > n * 4 || changed > 8 || distance(&t, &region) == 0 {
        return None;
    }
    Some(field_words[start..end].join(" "))
}

/// The first or last word of what was typed, as written (lower case).
fn typed_edge(typed: &str, last: bool) -> String {
    let words = typed.split_whitespace();
    let word = if last { words.last() } else { words.into_iter().next() };
    word.unwrap_or_default().to_lowercase()
}

/// Single-word swaps between the typed passage and your version, lower
/// case: the mishearings a dictionary entry can fix.
#[must_use]
pub fn word_swaps(typed: &str, corrected: &str) -> Vec<(String, String)> {
    let (a, b) = (keys(typed), keys(corrected));
    let mut swaps = Vec::new();
    let ops = alignment(&a, &b);
    for (index, op) in ops.iter().enumerate() {
        if let Op::Change(i, j) = *op {
            // Only a lone swap with unchanged words on both sides, not part
            // of a longer rewrite.
            let calm = |other: Option<&Op>| other.is_none_or(|op| *op == Op::Same);
            let lone = calm(index.checked_sub(1).and_then(|at| ops.get(at))) && calm(ops.get(index + 1));
            if lone && !a[i].is_empty() && !b[j].is_empty() && a[i] != b[j] {
                swaps.push((a[i].clone(), b[j].clone()));
            }
        }
    }
    swaps
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Same,
    Change(usize, usize),
    Insert(usize),
    Delete(usize),
}

fn table(a: &[String], b: &[String]) -> Vec<Vec<usize>> {
    let mut d = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
        }
    }
    d
}

fn distance(a: &[String], b: &[String]) -> usize {
    table(a, b)[a.len()][b.len()]
}

fn alignment(a: &[String], b: &[String]) -> Vec<Op> {
    let d = table(a, b);
    let (mut i, mut j) = (a.len(), b.len());
    let mut ops = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && d[i][j] == d[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]) {
            ops.push(if a[i - 1] == b[j - 1] { Op::Same } else { Op::Change(i - 1, j - 1) });
            i -= 1;
            j -= 1;
        } else if i > 0 && d[i][j] == d[i - 1][j] + 1 {
            ops.push(Op::Delete(i - 1));
            i -= 1;
        } else {
            ops.push(Op::Insert(j - 1));
            j -= 1;
        }
    }
    ops.reverse();
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixed_word_inside_the_dictation_is_a_correction() {
        let typed = "Can you send the contract to Daniel before 5 PM? Tanks.";
        let field = "Hi team,\nCan you send the contract to Daniel before 5 PM? Thanks.\nAnd one more thing";
        let fixed = corrected(typed, field).unwrap();
        assert_eq!(fixed, "Can you send the contract to Daniel before 5 PM? Thanks.");
        assert_eq!(word_swaps(typed, &fixed), [("tanks".to_owned(), "thanks".to_owned())]);
    }

    #[test]
    fn several_fixes_are_kept_and_paired() {
        let typed = "Please set the itinerary to Sarah by Friday afternoon.";
        let field = "Please send the itinerary to Sara by Friday afternoon.";
        let fixed = corrected(typed, field).unwrap();
        assert_eq!(word_swaps(typed, &fixed), [("set".to_owned(), "send".to_owned()), ("sarah".to_owned(), "sara".to_owned())]);
    }

    #[test]
    fn unchanged_rewritten_or_missing_text_is_not_a_correction() {
        let typed = "We should move the launch to Friday morning.";
        assert_eq!(corrected(typed, "Notes: We should move the launch to Friday morning. Done"), None);
        assert_eq!(corrected(typed, "Completely different words were written in here instead."), None);
        assert_eq!(corrected(typed, ""), None);
        // Most of it rewritten: not a correction.
        assert_eq!(corrected(typed, "We should cancel everything and start again on Friday morning."), None);
    }

    #[test]
    fn text_typed_straight_after_is_not_a_correction() {
        let typed = "Can you send the contract to Daniel before 5 PM? Tanks.";
        assert_eq!(corrected(typed, "Can you send the contract to Daniel before 5 PM? Tanks.Can we meet"), None);
        assert_eq!(corrected(typed, "Note:Can you send the contract to Daniel before 5 PM? Tanks."), None);
    }

    #[test]
    fn a_rewrite_of_several_words_in_a_row_is_no_word_swap() {
        assert!(word_swaps("send it to the design team today", "send it to finance tomorrow morning").is_empty());
    }
}
