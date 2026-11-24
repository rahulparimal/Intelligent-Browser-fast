//! Token counting. Budgets are in tokens, not characters.

/// Counts tokens for budgeting. Implementations must be cheap enough to run on every step.
pub trait Tokenizer: Send + Sync {
    fn count(&self, text: &str) -> usize;
    fn name(&self) -> &'static str;
}

/// Model-agnostic estimate: one token per four characters, rounded up.
///
/// Deliberately simple. It is the fallback when no exact tokenizer is available, and the thing exact counting replaces.
#[derive(Debug, Default, Clone, Copy)]
pub struct ApproxTokenizer;

impl Tokenizer for ApproxTokenizer {
    fn count(&self, text: &str) -> usize {
        text.chars().count().div_ceil(4)
    }

    fn name(&self) -> &'static str {
        "approx-chars/4"
    }
}

/// Exact counts for OpenAI-family models (`o200k_base`).
#[cfg(feature = "openai-tokenizer")]
pub struct OpenAiTokenizer {
    bpe: tiktoken_rs::CoreBPE,
}

#[cfg(feature = "openai-tokenizer")]
impl std::fmt::Debug for OpenAiTokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiTokenizer").field("encoding", &"o200k_base").finish()
    }
}

#[cfg(feature = "openai-tokenizer")]
impl OpenAiTokenizer {
    /// # Errors
    /// If the bundled tables fail to load.
    pub fn o200k() -> Result<Self, String> {
        tiktoken_rs::o200k_base().map(|bpe| Self { bpe }).map_err(|e| e.to_string())
    }
}

#[cfg(feature = "openai-tokenizer")]
impl Tokenizer for OpenAiTokenizer {
    fn count(&self, text: &str) -> usize {
        self.bpe.encode_ordinary(text).len()
    }

    fn name(&self) -> &'static str {
        "openai-o200k"
    }
}

/// Truncate `text` to at most `max_tokens`, cutting on a character boundary and appending a marker that says how much was
/// omitted, so the model knows the text is incomplete. Returns the text unchanged if it already fits.
pub fn truncate_to_tokens(text: &str, max_tokens: usize, tok: &dyn Tokenizer) -> String {
    if tok.count(text) <= max_tokens {
        return text.to_owned();
    }
    let boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    let (mut lo, mut hi) = (0usize, boundaries.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if tok.count(&text[..boundaries[mid]]) <= max_tokens {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let kept = &text[..boundaries[lo]];
    let omitted = tok.count(text).saturating_sub(tok.count(kept));
    format!("{kept}... [+{omitted} tokens omitted]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn approx_counts_chars_not_bytes() {
        let t = ApproxTokenizer;
        assert_eq!(t.count(""), 0);
        assert_eq!(t.count("abcd"), 1);
        assert_eq!(t.count("abcde"), 2);
        assert_eq!(t.count("日本語日"), 1, "four characters, twelve bytes");
    }

    #[test]
    fn truncation_marks_omission_and_respects_unicode() {
        let t = ApproxTokenizer;
        let text = "héllo wörld ".repeat(50);
        let out = truncate_to_tokens(&text, 20, &t);
        assert!(out.contains("tokens omitted]"));
        assert!(t.count(out.split("... [").next().unwrap()) <= 20);
        assert_eq!(truncate_to_tokens("short", 20, &t), "short");
    }

    proptest! {
        #[test]
        fn truncated_text_never_exceeds_budget(s in "\\PC{0,400}", max in 1usize..60) {
            let t = ApproxTokenizer;
            let out = truncate_to_tokens(&s, max, &t);
            let body = out.split("... [+").next().unwrap();
            prop_assert!(t.count(body) <= max);
        }
    }

    #[cfg(feature = "openai-tokenizer")]
    #[test]
    fn exact_tokenizer_counts_known_strings() {
        let t = OpenAiTokenizer::o200k().unwrap();
        assert_eq!(t.count("hello world"), 2);
        assert!(t.count("The quick brown fox jumps over the lazy dog.") >= 9);
        assert_eq!(t.name(), "openai-o200k");
    }
}
