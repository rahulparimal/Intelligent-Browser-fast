use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};

use bumpalo::collections::Vec as BVec;
use ibos_ctx::{Elem, Snapshot};
use ibos_mem::StepArena;

use crate::parse::{AxNode, AxResult, Rect, rects};
use crate::roles::{Class, classify};

#[derive(Debug, thiserror::Error)]
pub enum DistillError {
    #[error("the accessibility tree is not valid: {0}")]
    Ax(serde_json::Error),
    #[error("the layout snapshot is not valid: {0}")]
    Layout(serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct DistillOptions {
    pub max_elements: usize,
    /// How many items of a repeated structure to show before collapsing the rest.
    pub keep_repeat: usize,
    pub max_name_chars: usize,
    pub max_text_chars: usize,
    pub max_title_chars: usize,
    /// List elements outside the viewport too (instead of counting them).
    pub include_offscreen: bool,
}

impl Default for DistillOptions {
    fn default() -> Self {
        Self {
            max_elements: 400,
            keep_repeat: 6,
            max_name_chars: 100,
            max_text_chars: 240,
            max_title_chars: 120,
            include_offscreen: false,
        }
    }
}

/// What the distiller did, for logging and benchmarks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub input_bytes: usize,
    pub ax_nodes: usize,
    pub candidates: usize,
    pub kept: usize,
    /// Items hidden by collapsing repeated structures.
    pub collapsed_items: usize,
    pub below_viewport: usize,
    pub above_viewport: usize,
}

#[derive(Debug, Clone)]
pub struct Distilled {
    pub snapshot: Snapshot,
    pub stats: Stats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Interactive,
    Heading,
    Text,
    Image,
}

#[derive(Debug)]
struct Cand {
    backend: i64,
    role: String,
    text: String,
    rect: Option<Rect>,
}

enum Frame {
    Visit { node: usize, suppress_text: bool },
    EndBlock,
    Marker { backend: i64, items: usize, visible: bool },
}

fn clip(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

fn path_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.find('/').map_or("/", |i| &rest[i..])
}

fn unquote(raw: &str) -> &str {
    raw.trim_matches('"')
}

/// What a node contributes, if anything. Shared by the repeat detector and the emitter so they always agree.
fn emit_kind(n: &AxNode<'_>, class: Class) -> Option<Kind> {
    if n.ignored {
        return None;
    }
    match class {
        Class::Interactive(role) => {
            // an icon link has no text but still has somewhere to go; a control without a label is still a control
            let unlabeled_ok = matches!(role, "chk" | "radio" | "switch" | "input" | "select" | "range")
                || (role == "a" && n.prop("url").is_some());
            (!n.name().trim().is_empty() || !n.value().trim().is_empty() || unlabeled_ok).then_some(Kind::Interactive)
        }
        Class::Heading => (!n.name().trim().is_empty()).then_some(Kind::Heading),
        Class::Text => (!n.name().trim().is_empty()).then_some(Kind::Text),
        Class::Image => (!n.name().trim().is_empty()).then_some(Kind::Image),
        _ => None,
    }
}

fn state_flags(n: &AxNode<'_>) -> String {
    let mut f: Vec<&str> = Vec::new();
    match n.prop_str("checked") {
        Some("true") => f.push("checked"),
        Some("mixed") => f.push("mixed"),
        _ => {}
    }
    for (prop, label) in [
        ("expanded", "expanded"),
        ("disabled", "disabled"),
        ("required", "required"),
        ("selected", "selected"),
        ("focused", "focused"),
        ("readonly", "readonly"),
        ("pressed", "pressed"),
    ] {
        if n.flag(prop) {
            f.push(label);
        }
    }
    if f.is_empty() { String::new() } else { format!(" [{}]", f.join(", ")) }
}

fn describe(n: &AxNode<'_>, class: Class, opts: &DistillOptions) -> Option<(String, String)> {
    match class {
        Class::Interactive(role) => {
            let mut label = clip(n.name(), opts.max_name_chars);
            if label.is_empty()
                && role == "a"
                && let Some(u) = n.prop("url")
            {
                label = clip(path_of(unquote(u)), 60);
            }
            let mut text = label;
            let value = n.value().trim();
            if !value.is_empty() && matches!(role, "input" | "select" | "range") {
                if !text.is_empty() {
                    text.push(' ');
                }
                if role == "input" && sensitive_field(n.name(), value) {
                    // what the user typed here (a password, a card number) must never reach the model
                    text.push_str("= [hidden]");
                } else {
                    let _ = write!(text, "= \"{}\"", clip(value, 60));
                }
            }
            text.push_str(&state_flags(n));
            Some((role.to_owned(), text))
        }
        Class::Heading => {
            let level = n.prop("level").and_then(|l| l.parse::<u8>().ok()).unwrap_or(2).clamp(1, 6);
            Some((format!("h{level}"), clip(n.name(), opts.max_name_chars)))
        }
        Class::Image => Some(("img".to_owned(), clip(n.name(), opts.max_name_chars))),
        _ => None,
    }
}

fn hash_of(parts: &[u64]) -> u64 {
    let mut h = DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// Distil the page. `dom_json` is optional: without layout, everything is treated as visible.
pub fn distill(
    url: &str,
    title: &str,
    ax_json: &str,
    dom_json: Option<&str>,
    viewport: (f64, f64),
    opts: &DistillOptions,
    arena: &StepArena,
) -> Result<Distilled, DistillError> {
    let ax: AxResult<'_> = serde_json::from_str(ax_json).map_err(DistillError::Ax)?;
    let nodes = &ax.nodes;
    let rects = match dom_json {
        Some(j) => rects(j).map_err(DistillError::Layout)?,
        None => HashMap::new(),
    };
    let have_layout = !rects.is_empty();
    let mut stats =
        Stats { input_bytes: ax_json.len() + dom_json.map_or(0, str::len), ax_nodes: nodes.len(), ..Stats::default() };

    // adjacency in the step arena: freed in one operation when the step ends
    let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.node_id.as_ref(), i)).collect();
    let mut is_child = vec![false; nodes.len()];
    let kids: Vec<&[usize]> = nodes
        .iter()
        .map(|n| {
            let mut v = BVec::new_in(arena.bump());
            for c in &n.child_ids {
                if let Some(&ci) = index.get(c.as_ref()) {
                    is_child[ci] = true;
                    v.push(ci);
                }
            }
            v.into_bump_slice()
        })
        .collect();
    let roots: Vec<usize> = (0..nodes.len()).filter(|i| !is_child[*i]).collect();
    let class: Vec<Class> = nodes.iter().map(|n| classify(n.role())).collect();
    let kind: Vec<Option<Kind>> = nodes.iter().zip(&class).map(|(n, c)| emit_kind(n, *c)).collect();

    // structural signature of every subtree (post-order), so repeated structures can be recognised without comparing text
    let mut order = Vec::with_capacity(nodes.len());
    let mut seen = vec![false; nodes.len()];
    let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        order.push(i);
        stack.extend(kids[i].iter().rev().copied());
    }
    let mut sig = vec![0u64; nodes.len()];
    let mut count = vec![0usize; nodes.len()];
    for &i in order.iter().rev() {
        let own = kind[i].map_or(0, |k| 1 + k as u64);
        let mut parts = vec![own, class_tag(class[i])];
        let mut c = usize::from(kind[i].is_some());
        for &k in kids[i] {
            if count[k] > 0 {
                parts.push(sig[k]);
                c += count[k];
            }
        }
        count[i] = c;
        sig[i] = if c == 0 { 0 } else { hash_of(&parts) };
    }

    // emission, depth-first on an explicit stack (a page can be arbitrarily deep)
    let mut cands: Vec<Cand> = Vec::new();
    let mut run: Option<(i64, String, Option<Rect>)> = None;
    let mut frames: Vec<Frame> = roots.iter().rev().map(|&node| Frame::Visit { node, suppress_text: false }).collect();
    let mut visited = vec![false; nodes.len()];
    let flush = |run: &mut Option<(i64, String, Option<Rect>)>, cands: &mut Vec<Cand>| {
        if let Some((backend, text, rect)) = run.take() {
            let text = clip(&text, opts.max_text_chars);
            if !text.is_empty() {
                cands.push(Cand { backend, role: "text".into(), text, rect });
            }
        }
    };
    loop {
        while let Some(frame) = frames.pop() {
            match frame {
                Frame::EndBlock => flush(&mut run, &mut cands),
                Frame::Marker { backend, items, visible } => {
                    flush(&mut run, &mut cands);
                    stats.collapsed_items += items;
                    if visible {
                        cands.push(Cand {
                            backend,
                            role: "+".into(),
                            text: format!("{items} more similar items omitted"),
                            rect: None,
                        });
                    }
                }
                Frame::Visit { node: i, suppress_text } => {
                    if std::mem::replace(&mut visited[i], true) {
                        continue;
                    }
                    let n = &nodes[i];
                    let rect = n.backend.and_then(|b| rects.get(&b)).copied();
                    let mut child_suppress = suppress_text;
                    match (class[i], kind[i]) {
                        (Class::Drop, _) => continue,
                        (Class::Text, Some(_)) => {
                            if !suppress_text {
                                let piece = n.name().trim();
                                match &mut run {
                                    Some((_, text, _)) => {
                                        text.push(' ');
                                        text.push_str(piece);
                                    }
                                    None => run = Some((n.backend.unwrap_or(-1), piece.to_owned(), rect)),
                                }
                            }
                            continue;
                        }
                        (c, Some(k)) => {
                            flush(&mut run, &mut cands);
                            if let (Some((role, text)), Some(backend)) = (describe(n, c, opts), n.backend) {
                                cands.push(Cand { backend, role, text, rect });
                            }
                            child_suppress =
                                matches!(k, Kind::Interactive | Kind::Heading | Kind::Image) || suppress_text;
                        }
                        (Class::Label, None) => child_suppress = true,
                        (Class::Block, None) => {
                            flush(&mut run, &mut cands);
                            frames.push(Frame::EndBlock);
                        }
                        _ => {}
                    }
                    // children, with repeated sibling structures collapsed after `keep_repeat`
                    let list = kids[i];
                    let mut plan: Vec<Frame> = Vec::with_capacity(list.len());
                    let mut j = 0;
                    while j < list.len() {
                        let k = list[j];
                        let run_len = if count[k] > 0 {
                            list[j..].iter().take_while(|&&o| count[o] > 0 && sig[o] == sig[k]).count()
                        } else {
                            1
                        };
                        if run_len > opts.keep_repeat {
                            for &keep in &list[j..j + opts.keep_repeat] {
                                plan.push(Frame::Visit { node: keep, suppress_text: child_suppress });
                            }
                            let skipped = &list[j + opts.keep_repeat..j + run_len];
                            let anchor = first_backend(skipped[0], &kids, nodes, &kind).unwrap_or(-1);
                            let visible = !have_layout || rects.get(&anchor).is_none_or(|r| in_view(r, viewport));
                            plan.push(Frame::Marker { backend: anchor, items: skipped.len(), visible });
                            for &s in skipped {
                                mark_visited(s, &kids, &mut visited);
                            }
                            j += run_len;
                        } else {
                            plan.push(Frame::Visit { node: k, suppress_text: child_suppress });
                            j += 1;
                        }
                    }
                    frames.extend(plan.into_iter().rev());
                }
            }
        }
        // nodes unreachable from any root (a cycle, or ids that point nowhere) are still part of the page
        match (0..nodes.len()).find(|&i| !visited[i]) {
            Some(i) => frames.push(Frame::Visit { node: i, suppress_text: false }),
            None => break,
        }
    }
    flush(&mut run, &mut cands);
    stats.candidates = cands.len();

    // visibility, then the element cap
    let mut kept: Vec<&Cand> = Vec::new();
    for c in &cands {
        match c.rect.filter(|_| !opts.include_offscreen) {
            Some(r) if r.y + r.h <= 0.0 => stats.above_viewport += 1,
            Some(r) if r.y >= viewport.1 => stats.below_viewport += 1,
            Some(r) if r.x + r.w <= 0.0 || r.x >= viewport.0 || (r.w <= 0.0 && r.h <= 0.0) => {}
            _ => kept.push(c),
        }
    }
    if kept.len() > opts.max_elements {
        stats.below_viewport += kept.len() - opts.max_elements;
        kept.truncate(opts.max_elements);
    }
    let mut snapshot = Snapshot {
        url: clip(url, 200),
        title: clip(title, opts.max_title_chars),
        more_below: 0,
        ..Snapshot::default()
    };
    for c in kept {
        if let Ok(h) = u32::try_from(c.backend)
            && !snapshot.elems.contains_key(&h)
        {
            snapshot.elems.insert(h, Elem { role: c.role.clone(), text: c.text.clone() });
        }
    }
    stats.kept = snapshot.elems.len();
    snapshot.more_below = u32::try_from(stats.below_viewport).unwrap_or(u32::MAX);
    Ok(Distilled { snapshot, stats })
}

fn class_tag(c: Class) -> u64 {
    match c {
        Class::Interactive(r) => 10 + r.len() as u64 * 7 + u64::from(r.as_bytes()[0]),
        Class::Heading => 2,
        Class::Text => 3,
        Class::Image => 4,
        Class::Block => 5,
        Class::Transparent => 6,
        Class::Label => 8,
        Class::Drop => 7,
    }
}

fn in_view(r: &Rect, vp: (f64, f64)) -> bool {
    !(r.y + r.h <= 0.0 || r.y >= vp.1 || r.x + r.w <= 0.0 || r.x >= vp.0)
}

/// Backend id of the first element a subtree would emit.
fn first_backend(root: usize, kids: &[&[usize]], nodes: &[AxNode<'_>], kind: &[Option<Kind>]) -> Option<i64> {
    let mut stack = vec![root];
    while let Some(i) = stack.pop() {
        if kind[i].is_some()
            && let Some(b) = nodes[i].backend
        {
            return Some(b);
        }
        stack.extend(kids[i].iter().rev());
    }
    None
}

fn mark_visited(root: usize, kids: &[&[usize]], visited: &mut [bool]) {
    let mut stack = vec![root];
    while let Some(i) = stack.pop() {
        if !std::mem::replace(&mut visited[i], true) {
            stack.extend(kids[i].iter());
        }
    }
}

/// Words in a field's label that mark its content as secret.
const SECRET_LABELS: &[&str] = &[
    "password",
    "passcode",
    "passwd",
    "card number",
    "credit card",
    "debit card",
    "cvv",
    "cvc",
    "security code",
    "ssn",
    "social security",
    "iban",
    "account number",
    "routing number",
    "pin code",
    "secret",
    "token",
    "api key",
];

/// True if an input's value should be hidden from the model: its label names a secret, the value is only bullets, or the value
/// is a payment card number (Luhn-valid, 13 to 19 digits).
fn sensitive_field(label: &str, value: &str) -> bool {
    let label = label.to_lowercase();
    if SECRET_LABELS.iter().any(|w| label.contains(w))
        || label.split(|c: char| !c.is_alphanumeric()).any(|w| w == "pin")
    {
        return true;
    }
    if !value.is_empty() && value.chars().all(|c| matches!(c, '\u{2022}' | '*' | '\u{25cf}')) {
        return true;
    }
    let digits: Vec<u32> = value
        .chars()
        .filter(|c| !matches!(c, ' ' | '-'))
        .map(|c| c.to_digit(10))
        .collect::<Option<_>>()
        .unwrap_or_default();
    if !(13..=19).contains(&digits.len()) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, d)| {
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                *d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}
