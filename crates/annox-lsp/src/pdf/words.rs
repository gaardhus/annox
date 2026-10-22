//! Which words a suggestion changes, to tint them as the editors do.

/// A range `[start, end)` of a string's characters.
type Span = (usize, usize);

/// Whether `c` is part of a word for [`word_changes`]: as in the Neovim
/// plugin, letters, digits, underscores and anything outside ASCII.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii()
}

/// `s` split into words, runs of spaces and tabs, and single other
/// characters, as character ranges.
fn tokens(s: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let same = |c: char| {
            if is_word(s[i]) {
                is_word(c)
            } else if matches!(s[i], ' ' | '\t') {
                matches!(c, ' ' | '\t')
            } else {
                false
            }
        };
        let mut j = i + 1;
        while j < s.len() && same(s[j]) {
            j += 1;
        }
        out.push((i, j));
        i = j;
    }
    out
}

/// The words that differ between `old` and `new`, diff-so-fancy style, as
/// character ranges of each, the same as the Neovim plugin's: none when either
/// is empty, or when the two share no word, as then everything changed.
pub(super) fn word_changes(old: &str, new: &str) -> (Vec<Span>, Vec<Span>) {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let (ta, tb) = (tokens(&a), tokens(&b));
    if ta.is_empty() || tb.is_empty() || ta.len() * tb.len() > 1_000_000 {
        return (Vec::new(), Vec::new());
    }
    fn word(s: &[char], (i, j): (usize, usize)) -> &[char] {
        &s[i..j]
    }
    // The longest common subsequence of tokens; what's not in it changed.
    let mut lcs = vec![vec![0u32; tb.len() + 1]; ta.len() + 1];
    for i in (0..ta.len()).rev() {
        for j in (0..tb.len()).rev() {
            lcs[i][j] = if word(&a, ta[i]) == word(&b, tb[j]) {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut changed_a, mut changed_b) = (vec![true; ta.len()], vec![true; tb.len()]);
    let (mut i, mut j) = (0, 0);
    while i < ta.len() && j < tb.len() {
        if word(&a, ta[i]) == word(&b, tb[j]) {
            (changed_a[i], changed_b[j]) = (false, false);
            (i, j) = (i + 1, j + 1);
        } else if lcs[i][j + 1] >= lcs[i + 1][j] {
            j += 1;
        } else {
            i += 1;
        }
    }
    let shared = ta.iter().zip(&changed_a).any(|(&t, &changed)| !changed && is_word(a[t.0]));
    if !shared {
        return (Vec::new(), Vec::new());
    }
    // Runs of changed tokens, joined when only spaces separate them.
    let ranges = |s: &[char], toks: &[(usize, usize)], changed: &[bool]| {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (&(t0, t1), _) in toks.iter().zip(changed).filter(|(_, &c)| c) {
            match out.last_mut() {
                Some(last) if s[last.1..t0].iter().all(|c| matches!(c, ' ' | '\t')) => last.1 = t1,
                _ => out.push((t0, t1)),
            }
        }
        // Which of two equal spaces counts as changed is arbitrary; neither is.
        for (i, j) in &mut out {
            while *i < *j && matches!(s[*i], ' ' | '\t') {
                *i += 1;
            }
            while *j > *i && matches!(s[*j - 1], ' ' | '\t') {
                *j -= 1;
            }
        }
        out.retain(|(i, j)| j > i);
        out
    };
    (ranges(&a, &ta, &changed_a), ranges(&b, &tb, &changed_b))
}

/// `s` with its whitespace collapsed to single spaces and trimmed, as it's
/// drawn, and `ranges` of its characters moved to match.
pub(super) fn collapse(s: &str, ranges: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
    let mut out = String::new();
    let mut len = 0;
    let mut at = Vec::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
            at.push(len);
            continue;
        }
        if space && len > 0 {
            out.push(' ');
            len += 1;
        }
        space = false;
        at.push(len);
        out.push(c);
        len += 1;
    }
    at.push(len);
    let moved = ranges.iter().map(|&(a, b)| (at[a], at[b])).filter(|(a, b)| b > a).collect();
    (out, moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_words_like_the_neovim_plugin() {
        let diff = |a: &str, b: &str| {
            let (del, add) = word_changes(a, b);
            let slice = |s: &str, (i, j): (usize, usize)| s.chars().skip(i).take(j - i).collect::<String>();
            (
                del.into_iter().map(|r| slice(a, r)).collect::<Vec<_>>(),
                add.into_iter().map(|r| slice(b, r)).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            diff("the code solves all three.", "the code addresses all three."),
            (vec!["solves".into()], vec!["addresses".into()])
        );
        // Punctuation is a token of its own.
        assert_eq!(diff("all three.", "all three!"), (vec![".".into()], vec!["!".into()]));
        // Changes only spaces apart are one.
        assert_eq!(diff("a b c d", "a x y d"), (vec!["b c".into()], vec!["x y".into()]));
        assert_eq!(
            diff("prove the old bound", "prove that the new bound"),
            (vec!["old".into()], vec!["that".into(), "new".into()])
        );
        // Nothing in common, or nothing on one side: everything changed, so nothing's marked.
        assert_eq!(diff("one two", "three four"), (vec![], vec![]));
        assert_eq!(diff("one two", ""), (vec![], vec![]));
    }

    #[test]
    fn collapses_whitespace_and_moves_ranges() {
        assert_eq!(collapse("  a\n  bc d ", &[(6, 8), (9, 10)]), ("a bc d".to_owned(), vec![(2, 4), (5, 6)]));
    }
}
