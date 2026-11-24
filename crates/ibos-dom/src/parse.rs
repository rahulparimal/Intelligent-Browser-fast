//! Borrowing parsers for the two CDP payloads. Strings are `Cow<'a, str>`: borrowed from the response unless they contain
//! JSON escapes.

use std::borrow::Cow;
use std::collections::HashMap;

use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Deserialize)]
pub(crate) struct AxResult<'a> {
    #[serde(borrow)]
    pub nodes: Vec<AxNode<'a>>,
}

#[derive(Deserialize)]
pub(crate) struct AxStr<'a> {
    #[serde(borrow, default)]
    pub value: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
pub(crate) struct AxProp<'a> {
    pub name: &'a str,
    #[serde(borrow)]
    pub value: AxPropValue<'a>,
}

#[derive(Deserialize)]
pub(crate) struct AxPropValue<'a> {
    #[serde(borrow, default)]
    pub value: Option<&'a RawValue>,
}

#[derive(Deserialize)]
pub(crate) struct AxNode<'a> {
    #[serde(rename = "nodeId", borrow)]
    pub node_id: Cow<'a, str>,
    #[serde(default)]
    pub ignored: bool,
    #[serde(borrow, default)]
    pub role: Option<AxStr<'a>>,
    #[serde(borrow, default)]
    pub name: Option<AxStr<'a>>,
    #[serde(borrow, default)]
    pub value: Option<AxStr<'a>>,
    #[serde(borrow, default)]
    pub properties: Vec<AxProp<'a>>,
    #[serde(rename = "childIds", borrow, default)]
    pub child_ids: Vec<Cow<'a, str>>,
    #[serde(rename = "backendDOMNodeId", default)]
    pub backend: Option<i64>,
}

impl AxNode<'_> {
    pub(crate) fn role(&self) -> &str {
        self.role.as_ref().and_then(|r| r.value.as_deref()).unwrap_or("")
    }

    pub(crate) fn name(&self) -> &str {
        self.name.as_ref().and_then(|r| r.value.as_deref()).unwrap_or("")
    }

    pub(crate) fn value(&self) -> &str {
        self.value.as_ref().and_then(|r| r.value.as_deref()).unwrap_or("")
    }

    /// Raw JSON of a property value (`true`, `"text"`, `3`), if present.
    pub(crate) fn prop(&self, name: &str) -> Option<&str> {
        self.properties.iter().find(|p| p.name == name).and_then(|p| p.value.value).map(RawValue::get)
    }

    /// A property value without JSON quotes: Chrome sends tri-state values (`checked`, `pressed`) as the string `"true"` but
    /// plain booleans (`disabled`, `required`) as `true`.
    pub(crate) fn prop_str(&self, name: &str) -> Option<&str> {
        self.prop(name).map(|v| v.trim_matches('"'))
    }

    pub(crate) fn flag(&self, name: &str) -> bool {
        self.prop_str(name) == Some("true")
    }
}

#[derive(Deserialize)]
pub(crate) struct DomResult {
    #[serde(default)]
    pub documents: Vec<DomDoc>,
}

#[derive(Deserialize)]
pub(crate) struct DomDoc {
    #[serde(default)]
    pub nodes: DomNodes,
    #[serde(default)]
    pub layout: DomLayout,
    #[serde(rename = "scrollOffsetX", default)]
    pub scroll_x: f64,
    #[serde(rename = "scrollOffsetY", default)]
    pub scroll_y: f64,
}

#[derive(Deserialize, Default)]
pub(crate) struct DomNodes {
    #[serde(rename = "backendNodeId", default)]
    pub backend_node_id: Vec<i64>,
}

#[derive(Deserialize, Default)]
pub(crate) struct DomLayout {
    #[serde(rename = "nodeIndex", default)]
    pub node_index: Vec<usize>,
    #[serde(default)]
    pub bounds: Vec<[f64; 4]>,
}

/// Rectangle in viewport coordinates (CSS pixels), top-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// `backend node id -> rectangle relative to the viewport`. The first rectangle seen for a node wins.
pub(crate) fn rects(dom_json: &str) -> Result<HashMap<i64, Rect>, serde_json::Error> {
    let dom: DomResult = serde_json::from_str(dom_json)?;
    let mut out = HashMap::new();
    for doc in &dom.documents {
        for (layout_i, &node_i) in doc.layout.node_index.iter().enumerate() {
            let (Some(&backend), Some(b)) = (doc.nodes.backend_node_id.get(node_i), doc.layout.bounds.get(layout_i))
            else {
                continue;
            };
            out.entry(backend).or_insert(Rect { x: b[0] - doc.scroll_x, y: b[1] - doc.scroll_y, w: b[2], h: b[3] });
        }
    }
    Ok(out)
}
