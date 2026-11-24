//! Evidence packing: choose the passages that go to the model so the prompt stays inside a token budget.
//! At most `max_per_doc` passages per document (diversity), best first, with the last one trimmed to fit.

use std::collections::HashMap;

use ibos_ctx::Tokenizer;
use ibos_ctx::tokens::truncate_to_tokens;
use serde::Serialize;

use crate::Passage;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    /// 1-based number the model cites as `[n]`.
    pub n: usize,
    pub title: String,
    pub path: String,
    pub page: i64,
    pub text: String,
    pub tokens: usize,
}

impl Evidence {
    /// The block as it appears in the prompt.
    pub fn render(&self) -> String {
        format!("[{}] {} (page {})\n{}", self.n, self.title, self.page, self.text)
    }
}

pub fn pack(passages: &[Passage], budget_tokens: usize, max_per_doc: usize, tok: &dyn Tokenizer) -> Vec<Evidence> {
    let mut out: Vec<Evidence> = Vec::new();
    let mut per_doc: HashMap<i64, usize> = HashMap::new();
    let mut left = budget_tokens;
    for p in passages {
        if per_doc.get(&p.doc_id).copied().unwrap_or(0) >= max_per_doc.max(1) {
            continue;
        }
        let n = out.len() + 1;
        let header = tok.count(&format!("[{n}] {} (page {})\n", p.title, p.page));
        if header + 8 > left {
            break;
        }
        let full = tok.count(&p.text);
        let (text, tokens) = if header + full <= left {
            (p.text.clone(), full)
        } else {
            // trimming may add a marker, so shrink the target until the result really fits
            let mut target = left - header;
            loop {
                let t = truncate_to_tokens(&p.text, target, tok);
                let c = tok.count(&t);
                if header + c <= left || target <= 1 {
                    break (t, c.min(left - header));
                }
                target -= 1;
            }
        };
        left -= header + tokens;
        *per_doc.entry(p.doc_id).or_default() += 1;
        out.push(Evidence {
            n,
            title: p.title.clone(),
            path: p.path.clone(),
            page: p.page,
            text,
            tokens: header + tokens,
        });
        if left < 24 {
            break;
        }
    }
    out
}
