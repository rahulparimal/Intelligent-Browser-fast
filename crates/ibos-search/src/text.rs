//! Text processing: chunking, stopwords, and joined-word forms. Behaviour matches the Python implementation.

const STOPWORDS: &str = "a an and are as at be been but by can could did do does for from had has have how i if in into is it its me my of on or our so than that the their then there these they this to was we were what when where which who whom why will with would you your";

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// True for the common words that carry no search meaning (`the`, `of`, ...). Expects lowercase.
pub fn is_stopword(w: &str) -> bool {
    STOPWORDS.split(' ').any(|s| s == w)
}

/// Search terms: lowercase word runs (at most 16), stopwords dropped unless that would leave nothing.
pub fn query_terms(query: &str) -> Vec<String> {
    let all: Vec<String> =
        query.split(|c| !is_word(c)).filter(|w| !w.is_empty()).take(16).map(str::to_lowercase).collect();
    let content: Vec<String> = all.iter().filter(|w| !is_stopword(w)).cloned().collect();
    if content.is_empty() { all } else { content }
}

/// Joined variants of hyphenated, dotted or apostrophised words: `e-mail` -> `email`, `don't` -> `dont`, `A.B.C` -> `ABC`.
/// Indexed next to the text so both spellings match.
pub fn alt_forms(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_word(chars[i]) {
            i += 1;
            continue;
        }
        let mut joined = String::new();
        let mut had_connector = false;
        let mut j = i;
        loop {
            while j < chars.len() && is_word(chars[j]) {
                joined.push(chars[j]);
                j += 1;
            }
            if j + 1 < chars.len() && matches!(chars[j], '-' | '.' | '\'' | '\u{2019}') && is_word(chars[j + 1]) {
                had_connector = true;
                j += 1;
            } else {
                break;
            }
        }
        if had_connector {
            out.push(joined);
        }
        i = j.max(i + 1);
    }
    out.join(" ")
}

/// Paragraph-aware chunks of about `size` characters. An oversized paragraph is split at a sentence or space boundary.
/// Chunks of 20 characters or fewer are dropped.
pub fn chunk(text: &str, size: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for para in paragraphs(text) {
        let mut para = para;
        while para.chars().count() > size {
            let (head, rest) = split_at_boundary(&para, size);
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(head);
            para = rest;
        }
        if para.is_empty() {
            continue;
        }
        if !cur.is_empty() && cur.chars().count() + para.chars().count() + 1 > size {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&para);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.into_iter().filter(|c| c.chars().count() > 20).collect()
}

/// Blank-line separated paragraphs with whitespace collapsed to single spaces.
fn paragraphs(text: &str) -> Vec<String> {
    let mut paras = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                paras.push(cur.join(" ").split_whitespace().collect::<Vec<_>>().join(" "));
                cur.clear();
            }
        } else {
            cur.push(line);
        }
    }
    if !cur.is_empty() {
        paras.push(cur.join(" ").split_whitespace().collect::<Vec<_>>().join(" "));
    }
    paras
}

/// Split `s` after at most `size` characters, preferring the last ". " then the last space in the second half.
fn split_at_boundary(s: &str, size: usize) -> (String, String) {
    let idx: Vec<usize> = s.char_indices().map(|(i, _)| i).chain(std::iter::once(s.len())).collect();
    let window = &s[..idx[size]];
    let cut_char = |byte: usize| window[..byte].chars().count();
    let sentence = window.rfind(". ").map(cut_char);
    let space = window.rfind(' ').map(cut_char);
    let best = [sentence, space].into_iter().flatten().max();
    let cut = match best {
        Some(c) if c > size / 2 => {
            if sentence == Some(c) {
                c + 1
            } else {
                c
            }
        }
        _ => size,
    };
    (s[..idx[cut]].trim().to_owned(), s[idx[cut]..].trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn stopwords_are_dropped_unless_that_leaves_nothing() {
        assert_eq!(query_terms("what is the parental leave policy"), ["parental", "leave", "policy"]);
        assert_eq!(query_terms("the"), ["the"]);
        assert!(query_terms("").is_empty() && query_terms("!!! ???").is_empty());
        assert_eq!(query_terms("Résumé CAFÉ"), ["résumé", "café"]);
        assert_eq!(query_terms(&"word ".repeat(40)).len(), 16);
    }

    #[test]
    fn joined_forms_match_the_python_behaviour() {
        assert_eq!(alt_forms("use e-mail today"), "email");
        assert_eq!(alt_forms("don't stop"), "dont");
        assert_eq!(alt_forms("A.B.C and 30-60-90"), "ABC 306090");
        assert_eq!(alt_forms("plain words only"), "");
        assert_eq!(alt_forms("trailing- dash and .leading"), "");
        assert_eq!(alt_forms("it\u{2019}s fine"), "its");
    }

    #[test]
    fn chunking_keeps_paragraphs_and_splits_long_ones() {
        let long = "Sentence number one is here. ".repeat(60);
        let parts = chunk(&long, 300);
        assert!(
            parts.len() > 4 && parts.iter().all(|p| p.chars().count() <= 300),
            "{:?}",
            parts.iter().map(|p| p.chars().count()).collect::<Vec<_>>()
        );
        assert!(chunk("tiny", 900).is_empty());
        let two =
            chunk("First paragraph with enough words to count.\n\nSecond paragraph, also with enough words.", 900);
        assert_eq!(two.len(), 1, "short paragraphs are packed together");
        let big =
            chunk(&format!("{}\n\n{}", "a b c d e f g h i j ".repeat(20), "z y x w v u t s r q ".repeat(20)), 300);
        assert!(big.len() >= 2);
    }

    #[test]
    fn chunking_is_unicode_safe() {
        let t = "héllo wörld ünïcode ".repeat(100);
        for c in chunk(&t, 300) {
            assert!(c.chars().count() <= 300);
        }
    }

    proptest! {
        #[test]
        fn chunks_never_exceed_size_and_never_panic(s in "\\PC{0,3000}", size in 300usize..1200) {
            for c in chunk(&s, size) { prop_assert!(c.chars().count() <= size); }
        }
    }
}
