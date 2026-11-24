//! Content-addressed store for large tool outputs. The prompt keeps a short handle; the model asks for the part it needs.

use std::collections::HashMap;

use ibos_mem::{MemoryBudget, Reservation};

/// Short content hash, e.g. `art:3fa91c02`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(String);

impl Handle {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parse `art:xxxxxxxx`.
    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix("art:")?;
        (hex.len() == 8 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| Self(s.to_ascii_lowercase()))
    }
}

#[derive(Debug)]
struct Entry {
    text: String,
    _reservation: Option<Reservation>,
}

/// In-memory artifact store with optional budget accounting. Identical content is stored once.
#[derive(Debug, Default)]
pub struct ArtifactStore {
    items: HashMap<Handle, Entry>,
    budget: Option<MemoryBudget>,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_budget(mut self, budget: MemoryBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Store `text`. Returns `None` when the memory budget refuses it (callers should truncate or drop instead).
    pub fn put(&mut self, text: &str) -> Option<Handle> {
        let hash = blake3::hash(text.as_bytes());
        let handle = Handle(format!("art:{}", &hash.to_hex()[..8]));
        if self.items.contains_key(&handle) {
            return Some(handle);
        }
        let reservation = match &self.budget {
            Some(b) => Some(b.reserve(text.len())?),
            None => None,
        };
        self.items.insert(handle.clone(), Entry { text: text.to_owned(), _reservation: reservation });
        Some(handle)
    }

    pub fn get(&self, handle: &Handle) -> Option<&str> {
        self.items.get(handle).map(|e| e.text.as_str())
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The part of an artifact most relevant to `query`: paragraphs ranked by query-term overlap, returned in document order
    /// up to `max_chars`. This is the "recall instead of re-read" primitive; a search index can replace the scorer later.
    pub fn recall(&self, handle: &Handle, query: &str, max_chars: usize) -> Option<String> {
        let text = self.get(handle)?;
        let terms: Vec<String> =
            query.split(|c: char| !c.is_alphanumeric()).filter(|t| t.len() > 2).map(str::to_lowercase).collect();
        let paras: Vec<&str> = text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).collect();
        let mut scored: Vec<(usize, usize)> = paras
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let lower = p.to_lowercase();
                (terms.iter().filter(|t| lower.contains(t.as_str())).count(), i)
            })
            .filter(|(s, _)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut chosen = Vec::new();
        let mut used = 0;
        for (_, i) in scored {
            let len = paras[i].len() + 2;
            if used + len > max_chars && !chosen.is_empty() {
                continue;
            }
            used += len;
            chosen.push(i);
        }
        chosen.sort_unstable();
        let out = chosen.iter().map(|&i| paras[i]).collect::<Vec<_>>().join("\n\n");
        Some(if out.len() > max_chars { out.chars().take(max_chars).collect() } else { out })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "Introduction to the policy.\n\nEmployees receive 25 days of annual leave.\n\nTravel must be approved by a manager.\n\nParental leave is 16 weeks.";

    #[test]
    fn put_is_content_addressed_and_deduplicated() {
        let mut s = ArtifactStore::new();
        let a = s.put(DOC).unwrap();
        assert_eq!(a, s.put(DOC).unwrap());
        assert_eq!((s.len(), s.get(&a).unwrap()), (1, DOC));
        assert_ne!(a, s.put("other").unwrap());
        assert_eq!(Handle::parse(a.as_str()), Some(a));
        assert!(Handle::parse("art:zzzzzzzz").is_none() && Handle::parse("x").is_none());
    }

    #[test]
    fn recall_returns_only_relevant_paragraphs_in_order() {
        let mut s = ArtifactStore::new();
        let h = s.put(DOC).unwrap();
        let r = s.recall(&h, "how much annual leave and parental leave", 200).unwrap();
        assert!(r.contains("25 days") && r.contains("16 weeks") && !r.contains("Travel"));
        assert!(r.find("25 days") < r.find("16 weeks"), "document order is preserved");
        assert_eq!(s.recall(&h, "zzz", 100).unwrap(), "", "nothing relevant: empty, not the whole document");
        assert!(s.recall(&Handle("art:00000000".into()), "x", 10).is_none());
    }

    #[test]
    fn recall_respects_size_cap() {
        let mut s = ArtifactStore::new();
        let h = s.put(&"leave policy details. ".repeat(100)).unwrap();
        assert!(s.recall(&h, "leave", 50).unwrap().chars().count() <= 50);
    }

    #[test]
    fn budget_refuses_and_releases() {
        let b = MemoryBudget::new(100, 100);
        let mut s = ArtifactStore::new().with_budget(b.clone());
        assert!(s.put(&"x".repeat(80)).is_some());
        assert!(s.put(&"y".repeat(80)).is_none());
        drop(s);
        assert_eq!(b.used(), 0);
    }
}
