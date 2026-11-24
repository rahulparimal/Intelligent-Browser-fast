//! Page snapshots and step-to-step deltas.

use std::collections::HashMap;
use std::fmt::Write as _;

/// One distilled element: a stable handle (the map key) plus what the model sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Elem {
    pub role: String,
    pub text: String,
}

/// Handle -> element, in **document order** (the order elements were inserted), with O(1) lookup by handle.
///
/// Handles are browser node ids, which do not follow document order once the page changes dynamically, so the order is kept
/// explicitly rather than derived from the handle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    order: Vec<u32>,
    map: HashMap<u32, Elem>,
}

impl Page {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace. A new handle is appended; an existing one keeps its position.
    pub fn insert(&mut self, handle: u32, elem: Elem) -> Option<Elem> {
        if !self.map.contains_key(&handle) {
            self.order.push(handle);
        }
        self.map.insert(handle, elem)
    }

    pub fn get(&self, handle: &u32) -> Option<&Elem> {
        self.map.get(handle)
    }

    pub fn get_mut(&mut self, handle: &u32) -> Option<&mut Elem> {
        self.map.get_mut(handle)
    }

    pub fn contains_key(&self, handle: &u32) -> bool {
        self.map.contains_key(handle)
    }

    pub fn remove(&mut self, handle: &u32) -> Option<Elem> {
        let e = self.map.remove(handle)?;
        self.order.retain(|h| h != handle);
        Some(e)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Handles in document order.
    pub fn keys(&self) -> impl Iterator<Item = u32> + '_ {
        self.order.iter().copied()
    }

    /// `(handle, element)` in document order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Elem)> + '_ {
        self.order.iter().filter_map(|h| self.map.get(h).map(|e| (*h, e)))
    }
}

impl FromIterator<(u32, Elem)> for Page {
    fn from_iter<I: IntoIterator<Item = (u32, Elem)>>(iter: I) -> Self {
        let mut p = Self::new();
        for (h, e) in iter {
            p.insert(h, e);
        }
        p
    }
}

/// A distilled view of the current page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub url: String,
    pub title: String,
    pub elems: Page,
    /// Elements known to exist below the viewport that are not listed.
    pub more_below: u32,
}

/// What to send the model for this step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// The whole page.
    Full(String),
    /// Only what changed since the previous step.
    Delta { added: Vec<u32>, removed: Vec<u32>, changed: Vec<u32>, text: String },
}

impl Observation {
    pub fn text(&self) -> &str {
        match self {
            Self::Full(t) | Self::Delta { text: t, .. } => t,
        }
    }

    pub fn is_delta(&self) -> bool {
        matches!(self, Self::Delta { .. })
    }
}

fn render_elems(page: &Page, ids: impl Iterator<Item = u32>) -> String {
    let mut out = String::new();
    for id in ids {
        if let Some(e) = page.get(&id) {
            let _ = writeln!(out, "[{id}]<{}> {}", e.role, e.text);
        }
    }
    out
}

fn header(s: &Snapshot) -> String {
    format!("page: {} | {}\n", s.url, s.title)
}

/// Render the full snapshot.
pub fn render_full(s: &Snapshot) -> String {
    let mut out = header(s);
    out.push_str(&render_elems(&s.elems, s.elems.keys()));
    if s.more_below > 0 {
        let _ = writeln!(out, "(+{} more below; scroll to see)", s.more_below);
    }
    out
}

/// Send only what changed since `prev`, falling back to the full page when the URL changed, when there is no previous
/// snapshot, or when the delta would not be smaller.
pub fn observe(prev: Option<&Snapshot>, cur: &Snapshot) -> Observation {
    let full = render_full(cur);
    let Some(prev) = prev.filter(|p| p.url == cur.url) else {
        return Observation::Full(full);
    };
    let added: Vec<u32> = cur.elems.keys().filter(|k| !prev.elems.contains_key(k)).collect();
    let removed: Vec<u32> = prev.elems.keys().filter(|k| !cur.elems.contains_key(k)).collect();
    let changed: Vec<u32> =
        cur.elems.iter().filter(|(k, v)| prev.elems.get(k).is_some_and(|p| p != *v)).map(|(k, _)| k).collect();
    if added.is_empty() && removed.is_empty() && changed.is_empty() && prev.more_below == cur.more_below {
        let text = format!("page unchanged: {}\n", cur.url);
        return Observation::Delta { added, removed, changed, text };
    }
    let mut text = format!("page changed: {}\n", cur.url);
    text.push_str(&render_elems(&cur.elems, added.iter().chain(changed.iter()).copied()));
    if !removed.is_empty() {
        let _ = writeln!(text, "removed: {removed:?}");
    }
    if prev.more_below != cur.more_below {
        let _ = writeln!(text, "(+{} more below)", cur.more_below);
    }
    if text.len() >= full.len() {
        Observation::Full(full)
    } else {
        Observation::Delta { added, removed, changed, text }
    }
}

/// Fingerprint of a snapshot's rendered text, for detecting drift between what we think the model has seen and the page.
pub fn fingerprint(s: &Snapshot) -> [u8; 8] {
    let h = blake3::hash(render_full(s).as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(&h.as_bytes()[..8]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn snap(url: &str, items: &[(u32, &str, &str)]) -> Snapshot {
        Snapshot {
            url: url.into(),
            title: "t".into(),
            elems: items.iter().map(|(i, r, t)| (*i, Elem { role: (*r).into(), text: (*t).into() })).collect(),
            more_below: 0,
        }
    }

    fn base() -> Snapshot {
        snap(
            "http://x/",
            &[
                (1, "input", "email"),
                (2, "btn", "Sign in"),
                (3, "a", "Forgot password"),
                (4, "a", "Help"),
                (5, "a", "Terms and conditions of service"),
            ],
        )
    }

    #[test]
    fn first_step_is_full_then_delta_then_full_after_navigation() {
        let a = base();
        assert!(matches!(observe(None, &a), Observation::Full(_)));
        let mut b = a.clone();
        b.elems.insert(6, Elem { role: "div".into(), text: "Welcome".into() });
        match observe(Some(&a), &b) {
            Observation::Delta { added, removed, changed, text } => {
                assert_eq!((added, removed, changed), (vec![6], vec![], vec![]));
                assert!(text.len() < render_full(&b).len() / 2);
            }
            other @ Observation::Full(_) => panic!("expected delta, got {other:?}"),
        }
        let c = snap("http://x/next", &[(10, "h1", "Completely different")]);
        assert!(matches!(observe(Some(&a), &c), Observation::Full(_)), "URL change forces a full page");
    }

    #[test]
    fn unchanged_page_costs_one_line() {
        let a = base();
        let o = observe(Some(&a), &a.clone());
        assert!(o.is_delta() && o.text().starts_with("page unchanged"));
        assert!(o.text().len() < 40);
    }

    #[test]
    fn changed_text_is_reported_and_removed_listed() {
        let a = base();
        let mut b = a.clone();
        b.elems.get_mut(&2).unwrap().text = "Signing in...".into();
        b.elems.remove(&4);
        let o = observe(Some(&a), &b);
        assert!(o.text().contains("[2]<btn> Signing in...") && o.text().contains("removed: [4]"));
    }

    #[test]
    fn falls_back_to_full_when_delta_is_not_smaller() {
        let a = snap("u", &[(1, "a", "x")]);
        let b = snap("u", &[(1, "a", "a completely different and much longer text")]);
        assert!(matches!(observe(Some(&a), &b), Observation::Full(_)));
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        let a = base();
        let mut b = a.clone();
        assert_eq!(fingerprint(&a), fingerprint(&b));
        b.elems.get_mut(&1).unwrap().text.push('!');
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn page_keeps_document_order_not_handle_order() {
        let mut p = Page::new();
        for (h, t) in [(30u32, "first"), (5, "second"), (99, "third")] {
            p.insert(h, Elem { role: "a".into(), text: t.into() });
        }
        assert_eq!(p.keys().collect::<Vec<_>>(), [30, 5, 99]);
        p.insert(5, Elem { role: "a".into(), text: "changed".into() });
        assert_eq!(p.keys().collect::<Vec<_>>(), [30, 5, 99], "replacing keeps the position");
        assert!(p.remove(&5).is_some() && p.remove(&5).is_none());
        assert_eq!((p.keys().collect::<Vec<_>>(), p.len(), p.is_empty()), (vec![30, 99], 2, false));
        let text = render_full(&Snapshot { elems: p, ..Snapshot::default() });
        assert!(text.find("first").unwrap() < text.find("third").unwrap());
    }

    fn arb_page() -> impl Strategy<Value = Page> {
        proptest::collection::btree_map(0u32..40, ("[a-z]{1,4}", "[ -~]{0,30}"), 0..30)
            .prop_map(|m| m.into_iter().map(|(k, (role, text))| (k, Elem { role, text })).collect())
    }

    proptest! {
        /// Applying the reported delta to the previous page reproduces the current page: the diff is lossless.
        #[test]
        fn delta_reconstructs_the_current_page(prev in arb_page(), cur in arb_page()) {
            let p = Snapshot { url: "u".into(), title: String::new(), elems: prev.clone(), more_below: 0 };
            let c = Snapshot { url: "u".into(), title: String::new(), elems: cur.clone(), more_below: 0 };
            if let Observation::Delta { added, removed, changed, .. } = observe(Some(&p), &c) {
                let mut rebuilt = prev;
                for r in removed { rebuilt.remove(&r); }
                for k in added.iter().chain(changed.iter()) { rebuilt.insert(*k, cur.get(k).unwrap().clone()); }
                let as_map = |p: &Page| p.iter().map(|(k, e)| (k, e.clone())).collect::<std::collections::BTreeMap<_, _>>();
                prop_assert_eq!(as_map(&rebuilt), as_map(&cur), "same elements (order may differ: added elements are appended)");
            }
        }
    }
}
