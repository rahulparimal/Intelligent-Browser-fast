//! Grounding check for an answer written by a model.
//!
//! For each sentence it asks: are the content words (and, strictly, every number) present in the evidence the sentence cites?
//! This is **lexical**. It catches invented numbers, invented names and citations to evidence that does not exist. It
//! cannot catch a sentence that reuses the right words to say the wrong thing.

use std::collections::HashSet;

use ibos_search::text::is_stopword;
use serde::Serialize;

use crate::embed::{stem, words};
use crate::pack::Evidence;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SentenceCheck {
    pub text: String,
    pub cited: Vec<usize>,
    pub overlap: f32,
    pub missing_numbers: Vec<String>,
    pub supported: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Grounding {
    pub sentences: Vec<SentenceCheck>,
    /// Citation numbers that point at no evidence block.
    pub invalid_citations: Vec<usize>,
    pub supported_ratio: f32,
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        cur.push(*c);
        let end = matches!(c, '.' | '!' | '?' | '\n') && chars.get(i + 1).is_none_or(|n| n.is_whitespace());
        if end && !cur.trim().is_empty() {
            out.push(std::mem::take(&mut cur).trim().to_string());
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Remove `[n]` markers, returning the cleaned text and the numbers found.
fn citations(s: &str) -> (String, Vec<usize>) {
    let (mut clean, mut found) = (String::new(), Vec::new());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        clean.push_str(&rest[..open]);
        match rest[open..].find(']') {
            Some(close) if rest[open + 1..open + close].trim().parse::<usize>().is_ok() => {
                found.push(rest[open + 1..open + close].trim().parse().unwrap_or(0));
                rest = &rest[open + close + 1..];
            }
            _ => {
                clean.push('[');
                rest = &rest[open + 1..];
            }
        }
    }
    clean.push_str(rest);
    (clean, found)
}

fn terms(text: &str) -> HashSet<String> {
    words(text).into_iter().filter(|w| !is_stopword(w)).map(|w| stem(&w)).collect()
}

/// `min_overlap`: share of a sentence's content words that must appear in its evidence (0.6 is a reasonable start).
pub fn check(answer: &str, evidence: &[Evidence], min_overlap: f32) -> Grounding {
    let mut invalid: Vec<usize> = Vec::new();
    let mut sentences = Vec::new();
    for raw in split_sentences(answer) {
        let (clean, cited) = citations(&raw);
        for c in &cited {
            if !evidence.iter().any(|e| e.n == *c) && !invalid.contains(c) {
                invalid.push(*c);
            }
        }
        let source: String = if cited.is_empty() {
            evidence.iter().map(|e| e.text.as_str()).collect::<Vec<_>>().join(" ")
        } else {
            evidence.iter().filter(|e| cited.contains(&e.n)).map(|e| e.text.as_str()).collect::<Vec<_>>().join(" ")
        };
        let have = terms(&source);
        let mine = terms(&clean);
        let overlap = if mine.is_empty() {
            1.0
        } else {
            mine.iter().filter(|t| have.contains(*t)).count() as f32 / mine.len() as f32
        };
        let src_words: HashSet<String> = words(&source).into_iter().collect();
        let missing_numbers: Vec<String> = words(&clean)
            .into_iter()
            .filter(|w| w.chars().any(|c| c.is_ascii_digit()) && !src_words.contains(w))
            .collect();
        let supported = overlap >= min_overlap && missing_numbers.is_empty() && (!source.is_empty() || mine.is_empty());
        sentences.push(SentenceCheck { text: clean.trim().to_string(), cited, overlap, missing_numbers, supported });
    }
    let ratio = if sentences.is_empty() {
        1.0
    } else {
        sentences.iter().filter(|s| s.supported).count() as f32 / sentences.len() as f32
    };
    Grounding { sentences, invalid_citations: invalid, supported_ratio: ratio }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(n: usize, text: &str) -> Evidence {
        Evidence { n, title: "t".into(), path: "p".into(), page: 1, text: text.into(), tokens: 0 }
    }

    #[test]
    fn supported_and_invented_sentences_are_told_apart() {
        let e = [ev(1, "Returns are accepted within 30 days of delivery. Refunds go to the original payment method.")];
        let g = check("Returns are accepted within 30 days [1]. Refunds are paid in 5 business days [1].", &e, 0.6);
        assert!(g.sentences[0].supported);
        assert!(!g.sentences[1].supported);
        assert_eq!(g.sentences[1].missing_numbers, vec!["5"]);
        assert!((g.supported_ratio - 0.5).abs() < 1e-6);
    }

    #[test]
    fn citing_a_missing_block_is_flagged() {
        let g = check("Returns are accepted [3].", &[ev(1, "Returns are accepted.")], 0.6);
        assert_eq!(g.invalid_citations, vec![3]);
        assert!(!g.sentences[0].supported);
    }

    #[test]
    fn a_sentence_cites_only_its_own_evidence() {
        let e = [ev(1, "Cats sleep a lot."), ev(2, "Refunds take thirty days.")];
        let g = check("Refunds take thirty days [1].", &e, 0.6);
        assert!(!g.sentences[0].supported);
    }

    #[test]
    fn no_evidence_means_nothing_is_supported() {
        let g = check("The moon is cheese.", &[], 0.6);
        assert!(!g.sentences[0].supported);
    }

    #[test]
    fn it_is_lexical_and_misses_reversed_meaning() {
        // documented limit: same words, opposite claim, still "supported"
        let e = [ev(1, "Refunds are never available for opened software.")];
        let g = check("Refunds are available for opened software [1].", &e, 0.6);
        assert!(g.sentences[0].supported);
    }

    #[test]
    fn markers_and_brackets_are_parsed_safely() {
        assert_eq!(citations("see [1] and [x] and [2"), ("see  and [x] and [2".to_string(), vec![1]));
    }
}
