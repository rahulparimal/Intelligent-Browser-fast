//! The distiller on hand-built trees (edge cases) and on real Chrome captures (behaviour that matters).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;

use ibos_ctx::Snapshot;
use ibos_ctx::observe::render_full;
use ibos_dom::{DistillError, DistillOptions, Distilled, distill};
use ibos_mem::StepArena;
use serde_json::json;

// ---------------------------------------------------------------- builders for tiny trees

struct N {
    id: usize,
    role: &'static str,
    name: String,
    kids: Vec<usize>,
    props: Vec<(&'static str, serde_json::Value)>,
    value: Option<String>,
    ignored: bool,
}

fn n(id: usize, role: &'static str, name: &str, kids: &[usize]) -> N {
    N { id, role, name: name.into(), kids: kids.to_vec(), props: vec![], value: None, ignored: false }
}

impl N {
    fn prop(mut self, k: &'static str, v: serde_json::Value) -> Self {
        self.props.push((k, v));
        self
    }

    fn value(mut self, v: &str) -> Self {
        self.value = Some(v.into());
        self
    }

    fn ignored(mut self) -> Self {
        self.ignored = true;
        self
    }
}

fn ax(nodes: &[N]) -> String {
    let list: Vec<_> = nodes
        .iter()
        .map(|x| {
            let mut v = json!({
                "nodeId": x.id.to_string(), "ignored": x.ignored, "role": {"type": "role", "value": x.role},
                "name": {"type": "computedString", "value": x.name},
                "childIds": x.kids.iter().map(ToString::to_string).collect::<Vec<_>>(), "backendDOMNodeId": 100 + x.id,
                "properties": x.props.iter().map(|(k, v)| json!({"name": k, "value": {"type": "x", "value": v}})).collect::<Vec<_>>(),
            });
            if let Some(val) = &x.value {
                v["value"] = json!({"type": "string", "value": val});
            }
            v
        })
        .collect();
    json!({"nodes": list}).to_string()
}

/// Layout: `(id, x, y, w, h)` in document coordinates.
fn dom(rects: &[(usize, f64, f64, f64, f64)], scroll_y: f64) -> String {
    let backend: Vec<_> = rects.iter().map(|r| 100 + r.0).collect();
    let idx: Vec<_> = (0..rects.len()).collect();
    let bounds: Vec<_> = rects.iter().map(|r| json!([r.1, r.2, r.3, r.4])).collect();
    json!({"documents": [{"nodes": {"backendNodeId": backend}, "layout": {"nodeIndex": idx, "bounds": bounds}, "scrollOffsetX": 0, "scrollOffsetY": scroll_y}]}).to_string()
}

fn run(ax_json: &str, dom_json: Option<&str>, opts: &DistillOptions) -> Distilled {
    distill("https://x.test/a/b", "Title", ax_json, dom_json, (1280.0, 800.0), opts, &StepArena::new()).unwrap()
}

fn lines(d: &Distilled) -> Vec<String> {
    render_full(&d.snapshot).lines().skip(1).map(str::to_owned).collect()
}

fn root(kids: &[usize]) -> N {
    n(0, "RootWebArea", "Title", kids)
}

// ---------------------------------------------------------------- edge cases

#[test]
fn keeps_what_a_model_acts_on_and_drops_the_rest() {
    let tree = [
        root(&[1, 2, 3, 4, 5]),
        n(1, "banner", "", &[6]),
        n(6, "generic", "", &[]),
        n(2, "heading", "Welcome", &[7]).prop("level", json!(1)),
        n(7, "StaticText", "Welcome", &[]),
        n(3, "button", "Sign in", &[8]),
        n(8, "StaticText", "Sign in", &[]),
        n(4, "link", "", &[]).prop("url", json!("https://x.test/forgot?x=1")),
        n(5, "image", "Company logo", &[]),
    ];
    let d = run(&ax(&tree), None, &DistillOptions::default());
    assert_eq!(
        lines(&d),
        ["[102]<h1> Welcome", "[103]<btn> Sign in", "[104]<a> /forgot?x=1", "[105]<img> Company logo"]
    );
    assert_eq!(d.snapshot.title, "Title");
    assert_eq!(d.stats.ax_nodes, 9);
}

#[test]
fn text_inside_controls_is_not_repeated_and_adjacent_text_is_merged() {
    let tree = [
        root(&[1, 2]),
        n(1, "paragraph", "", &[3, 4, 5]),
        n(3, "StaticText", "Hello", &[]),
        n(4, "strong", "", &[6]),
        n(6, "StaticText", "big", &[]),
        n(5, "StaticText", "world", &[]),
        n(2, "paragraph", "", &[7]),
        n(7, "StaticText", "Second paragraph", &[]),
    ];
    let d = run(&ax(&tree), None, &DistillOptions::default());
    assert_eq!(
        lines(&d),
        ["[103]<text> Hello big world", "[107]<text> Second paragraph"],
        "a paragraph is one element; paragraphs stay separate"
    );
}

#[test]
fn ignored_nodes_are_skipped_but_their_children_are_visited() {
    let tree =
        [root(&[1]), n(1, "generic", "", &[2]).ignored(), n(2, "button", "Deep", &[]), n(3, "button", "Orphan", &[])];
    let d = run(&ax(&tree), None, &DistillOptions::default());
    assert_eq!(lines(&d), ["[102]<btn> Deep", "[103]<btn> Orphan"], "unreferenced nodes are roots too");
    let hidden = [root(&[1]), n(1, "button", "Hidden", &[]).ignored()];
    assert!(lines(&run(&ax(&hidden), None, &DistillOptions::default())).is_empty());
}

#[test]
fn states_values_and_levels_are_reported() {
    let tree = [
        root(&[1, 2, 3, 4, 5, 6]),
        n(1, "textbox", "Email", &[]).value("me@example.com").prop("required", json!(true)),
        n(2, "checkbox", "Remember", &[]).prop("checked", json!("true")),
        n(3, "checkbox", "Maybe", &[]).prop("checked", json!("mixed")),
        n(4, "button", "Go", &[]).prop("disabled", json!(true)).prop("expanded", json!(true)),
        n(5, "heading", "Sub", &[]).prop("level", json!(3)),
        n(6, "textbox", "", &[]),
    ];
    let d = run(&ax(&tree), None, &DistillOptions::default());
    assert_eq!(
        lines(&d),
        [
            "[101]<input> Email = \"me@example.com\" [required]",
            "[102]<chk> Remember [checked]",
            "[103]<chk> Maybe [mixed]",
            "[104]<btn> Go [expanded, disabled]",
            "[105]<h3> Sub",
            "[106]<input> "
        ]
    );
}

#[test]
fn long_text_and_names_are_truncated_with_an_ellipsis() {
    let long = "word ".repeat(200);
    let tree = [root(&[1, 2]), n(1, "button", &long, &[]), n(2, "StaticText", &long, &[])];
    let d = run(&ax(&tree), None, &DistillOptions::default());
    let l = lines(&d);
    assert!(l[0].chars().count() <= 100 + "[101]<btn> ".len() && l[0].ends_with("..."), "{}", l[0]);
    assert!(l[1].chars().count() <= 240 + "[102]<text> ".len() && l[1].ends_with("..."));
}

#[test]
fn json_escapes_and_unicode_survive_the_borrowing_parser() {
    let tree = [root(&[1]), n(1, "button", "Say \"hi\" \\ café \u{1F642} \n tab\t", &[])];
    assert_eq!(
        lines(&run(&ax(&tree), None, &DistillOptions::default())),
        ["[101]<btn> Say \"hi\" \\ café \u{1F642} tab"]
    );
}

#[test]
fn repeated_structures_collapse_after_keep_repeat() {
    let mut tree = vec![root(&[1]), n(1, "list", "", &(2..42).collect::<Vec<_>>())];
    for i in 0..40 {
        let id = 2 + i;
        tree.push(n(id, "listitem", "", &[100 + id]));
        tree.push(n(100 + id, "link", &format!("Item {i}"), &[]));
    }
    let d = run(&ax(&tree), None, &DistillOptions::default());
    let l = lines(&d);
    assert_eq!(l.len(), 7, "6 kept + 1 marker: {l:?}");
    assert!(l[0].ends_with("Item 0") && l[5].ends_with("Item 5"));
    assert!(l[6].contains("34 more similar items omitted") && d.stats.collapsed_items == 34);
    let all = run(&ax(&tree), None, &DistillOptions { keep_repeat: 1000, ..DistillOptions::default() });
    assert_eq!(lines(&all).len(), 40, "collapsing can be switched off");
}

#[test]
fn different_items_are_not_collapsed_even_if_numerous() {
    let mut tree = vec![root(&[1]), n(1, "list", "", &(2..22).collect::<Vec<_>>())];
    for i in 0..20 {
        let id = 2 + i;
        let role = if i % 2 == 0 { "link" } else { "button" };
        tree.push(n(id, role, &format!("Item {i}"), &[]));
    }
    assert_eq!(
        lines(&run(&ax(&tree), None, &DistillOptions::default())).len(),
        20,
        "alternating shapes are not a repeat"
    );
}

#[test]
fn viewport_counts_offscreen_content_instead_of_listing_it() {
    let tree = [
        root(&[1, 2, 3, 4]),
        n(1, "button", "Above", &[]),
        n(2, "button", "Visible", &[]),
        n(3, "button", "Below", &[]),
        n(4, "button", "Aside", &[]),
    ];
    // scrolled down 1000px: Above is at y=100 (above the viewport), Visible at y=1100 (in view), Below at y=2500
    let layout = dom(
        &[
            (1, 0.0, 100.0, 80.0, 20.0),
            (2, 0.0, 1100.0, 80.0, 20.0),
            (3, 0.0, 2500.0, 80.0, 20.0),
            (4, 5000.0, 1100.0, 80.0, 20.0),
        ],
        1000.0,
    );
    let d = run(&ax(&tree), Some(&layout), &DistillOptions::default());
    assert_eq!(lines(&d), ["[102]<btn> Visible", "(+1 more below; scroll to see)"]);
    assert_eq!((d.stats.above_viewport, d.stats.below_viewport, d.snapshot.more_below), (1, 1, 1));
    let all = run(&ax(&tree), Some(&layout), &DistillOptions { include_offscreen: true, ..DistillOptions::default() });
    assert_eq!(all.snapshot.elems.len(), 4);
}

#[test]
fn elements_without_layout_are_kept_when_layout_exists_for_others() {
    let tree = [root(&[1, 2]), n(1, "button", "Has layout", &[]), n(2, "button", "No layout", &[])];
    let layout = dom(&[(1, 0.0, 10.0, 50.0, 20.0)], 0.0);
    assert_eq!(lines(&run(&ax(&tree), Some(&layout), &DistillOptions::default())).len(), 2);
}

#[test]
fn element_cap_is_enforced_and_counted() {
    let mut tree = vec![root(&(1..=50).collect::<Vec<_>>())];
    for i in 1..=50 {
        tree.push(n(i, if i % 2 == 0 { "link" } else { "button" }, &format!("B{i}"), &[]));
    }
    let d = run(&ax(&tree), None, &DistillOptions { max_elements: 10, ..DistillOptions::default() });
    assert_eq!((d.snapshot.elems.len(), d.snapshot.more_below), (10, 40));
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    let o = DistillOptions::default();
    let a = StepArena::new();
    assert!(matches!(distill("u", "t", "not json", None, (1.0, 1.0), &o, &a), Err(DistillError::Ax(_))));
    assert!(matches!(distill("u", "t", "{\"nodes\": 5}", None, (1.0, 1.0), &o, &a), Err(DistillError::Ax(_))));
    assert!(matches!(
        distill("u", "t", "{\"nodes\": []}", Some("{{"), (1.0, 1.0), &o, &a),
        Err(DistillError::Layout(_))
    ));
    assert!(distill("u", "t", "{\"nodes\": []}", None, (1.0, 1.0), &o, &a).unwrap().snapshot.elems.is_empty());
    // dangling child ids, duplicate ids and cycles must not loop or panic
    let weird = r#"{"nodes":[{"nodeId":"1","childIds":["2","9"],"role":{"value":"RootWebArea"},"backendDOMNodeId":1},{"nodeId":"2","childIds":["1"],"role":{"value":"button"},"name":{"value":"Loop"},"backendDOMNodeId":2}]}"#;
    assert_eq!(distill("u", "t", weird, None, (1.0, 1.0), &o, &a).unwrap().snapshot.elems.len(), 1);
}

#[test]
fn very_deep_and_very_wide_trees_do_not_overflow_the_stack_or_crawl() {
    let depth = 50_000;
    let mut deep = vec![root(&[1])];
    for i in 1..depth {
        deep.push(n(i, "generic", "", &[i + 1]));
    }
    deep.push(n(depth, "button", "At the bottom", &[]));
    let t0 = std::time::Instant::now();
    assert_eq!(lines(&run(&ax(&deep), None, &DistillOptions::default())).len(), 1);
    let width = 30_000;
    let mut wide = vec![root(&(1..=width).collect::<Vec<_>>())];
    for i in 1..=width {
        wide.push(n(i, "link", &format!("L{i}"), &[]));
    }
    let d = run(&ax(&wide), None, &DistillOptions { keep_repeat: 5, max_elements: 50, ..DistillOptions::default() });
    assert!(d.snapshot.elems.len() <= 50 && d.stats.ax_nodes == width + 1);
    assert!(t0.elapsed() < std::time::Duration::from_secs(10), "{:?}", t0.elapsed());
}

// ---------------------------------------------------------------- real Chrome captures

fn fixture(name: &str) -> (String, String, serde_json::Value) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let read = |ext: &str| std::fs::read_to_string(format!("{dir}/{name}.{ext}.json")).unwrap();
    (read("ax"), read("dom"), serde_json::from_str(&read("meta")).unwrap())
}

fn real(name: &str, opts: &DistillOptions) -> Distilled {
    let (a, d, meta) = fixture(name);
    let vp = (meta["viewport"][0].as_f64().unwrap(), meta["viewport"][1].as_f64().unwrap());
    distill(meta["url"].as_str().unwrap(), meta["title"].as_str().unwrap(), &a, Some(&d), vp, opts, &StepArena::new())
        .unwrap()
}

fn texts(d: &Distilled) -> Vec<String> {
    d.snapshot.elems.iter().map(|(_, e)| format!("<{}> {}", e.role, e.text)).collect()
}

#[test]
fn real_login_page_is_complete_ordered_and_tiny() {
    let d = real("login", &DistillOptions::default());
    assert_eq!(
        texts(&d),
        [
            "<h1> Sign in",
            "<input> Email [required]",
            "<input> Password",
            "<chk> Remember me",
            "<btn> Sign in",
            "<btn> Place order",
            "<a> Forgot password?"
        ]
    );
    assert!(render_full(&d.snapshot).len() < 350, "a login page is a few dozen tokens, not thousands");
    assert!(d.stats.input_bytes > 15_000);
}

#[test]
fn real_form_keeps_every_field_with_its_state() {
    let t = texts(&real("form", &DistillOptions::default())).join("\n");
    for expect in [
        "<input> Full name [required]",
        "<input> Email [required]",
        "<select> Position = \"Designer\"",
        "<option> Designer [selected]",
        "<radio> Remote [checked]",
        "<radio> Office",
        "<chk> I accept the terms [checked]",
        "<input> Cover letter = \"Hello\"",
        "<btn> Submit application [disabled]",
        "<btn> Reset",
    ] {
        assert!(t.contains(expect), "missing {expect:?} in:\n{t}");
    }
}

#[test]
fn real_product_list_collapses_to_a_few_hundred_tokens() {
    let d = real("products", &DistillOptions::default());
    let out = render_full(&d.snapshot);
    assert!(d.stats.collapsed_items >= 100, "{:?}", d.stats);
    assert!(
        out.contains("more similar items omitted")
            && out.contains("model 1")
            && out.contains("model 6")
            && !out.contains("model 50")
    );
    assert!(out.len() < 2000, "{} chars", out.len());
    assert!(
        d.stats.input_bytes > 600_000 && out.len() * 100 < d.stats.input_bytes,
        "more than 100x smaller than the browser data"
    );
    let full =
        real("products", &DistillOptions { keep_repeat: 1000, include_offscreen: true, ..DistillOptions::default() });
    assert!(render_full(&full.snapshot).contains("model 120"), "nothing is lost when asked for everything");
}

#[test]
fn real_table_menu_and_article_behave() {
    let table = real("table", &DistillOptions::default());
    assert!(table.stats.collapsed_items >= 50 && render_full(&table.snapshot).contains("Staff directory"));
    let menu = real("menu", &DistillOptions::default());
    assert!(
        menu.stats.collapsed_items > 0 && menu.stats.below_viewport > 0,
        "the menu fills the screen, so the button below it is counted, not listed"
    );
    assert!(!render_full(&menu.snapshot).contains("Get started") && render_full(&menu.snapshot).contains("more below"));
    let article = real("article", &DistillOptions::default());
    assert!(article.stats.below_viewport > 20 && render_full(&article.snapshot).contains("Planning in practice"));
    assert!(render_full(&article.snapshot).contains("(+"), "the page says there is more below");
}

#[test]
fn every_real_page_distils_deterministically_and_quickly() {
    for name in ["login", "form", "products", "table", "menu", "article"] {
        let t0 = std::time::Instant::now();
        let (a, b) = (real(name, &DistillOptions::default()), real(name, &DistillOptions::default()));
        assert_eq!(render_full(&a.snapshot), render_full(&b.snapshot), "{name}: same input, same output");
        assert!(t0.elapsed() < std::time::Duration::from_secs(2), "{name}: {:?}", t0.elapsed());
        assert!(a.stats.kept <= 400 && a.stats.kept > 0, "{name}");
    }
}

#[test]
fn handles_are_stable_browser_ids_usable_for_actions() {
    let d = real("login", &DistillOptions::default());
    let go = d.snapshot.elems.iter().find(|(_, e)| e.text == "Sign in" && e.role == "btn").map(|(h, _)| h).unwrap();
    assert_eq!(go, 30, "the handle is the DOM backend node id the CDP click uses");
    let _ = (Snapshot::default(), String::new().write_str(""));
}
