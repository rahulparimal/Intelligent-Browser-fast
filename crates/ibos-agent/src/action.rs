//! The action vocabulary: one small JSON object per step.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "a", rename_all = "lowercase")]
pub enum Action {
    Go {
        url: String,
    },
    Click {
        h: u32,
    },
    Type {
        h: u32,
        t: String,
        #[serde(default = "yes")]
        clear: bool,
    },
    Scroll {
        d: Dir,
    },
    More {
        h: u32,
        /// List only hidden items whose text contains this (case-insensitive).
        #[serde(default)]
        q: Option<String>,
        /// Skip this many hidden items first (for paging).
        #[serde(default)]
        skip: usize,
        /// How many to list (default 25, at most 200).
        #[serde(default)]
        n: Option<usize>,
        /// With `q`: also list this many elements after each match (default 3), so a matching table row or product card
        /// comes with its other cells.
        #[serde(default)]
        ctx: Option<usize>,
    },
    Back,
    Done {
        #[serde(default)]
        t: String,
        #[serde(default = "yes")]
        ok: bool,
    },
}

fn yes() -> bool {
    true
}

impl Action {
    /// A short description for the history. Typed text is never stored, only its length: it may be a password.
    pub fn describe(&self) -> String {
        match self {
            Self::Go { url } => format!("go {url}"),
            Self::Click { h } => format!("click {h}"),
            Self::Type { h, t, .. } => format!("type {h} [{} chars]", t.chars().count()),
            Self::Scroll { d } => format!("scroll {}", if *d == Dir::Down { "down" } else { "up" }),
            Self::More { h, q, skip, .. } => match q {
                Some(q) => format!("more {h} q={q:?}"),
                None if *skip > 0 => format!("more {h} skip={skip}"),
                None => format!("more {h}"),
            },
            Self::Back => "back".into(),
            Self::Done { ok, .. } => format!("done ok={ok}"),
        }
    }
}

/// The first balanced `{...}` in `text`, respecting strings and escapes, that parses as an [`Action`]. Models often wrap the
/// object in prose or a code fence; that is tolerated, a missing or invalid object is not.
pub fn parse_action(reply: &str) -> Result<Action, String> {
    let bytes = reply.as_bytes();
    let mut start = 0;
    let mut last_error = None;
    while let Some(off) = reply[start..].find('{') {
        let begin = start + off;
        let (mut depth, mut in_str, mut esc) = (0usize, false, false);
        let mut end = None;
        for (i, &b) in bytes.iter().enumerate().skip(begin) {
            if in_str {
                match (esc, b) {
                    (true, _) => esc = false,
                    (false, b'\\') => esc = true,
                    (false, b'"') => in_str = false,
                    _ => {}
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        match serde_json::from_str::<Action>(&reply[begin..end]) {
            Ok(a) => return Ok(a),
            Err(e) => last_error = Some(e.to_string()),
        }
        start = begin + 1;
    }
    Err(last_error.map_or_else(
        || "no JSON object found; reply with exactly one JSON action".to_owned(),
        |e| format!("invalid action: {e}; reply with exactly one JSON action"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_action() {
        assert_eq!(
            parse_action(r#"{"a":"go","url":"https://x.test"}"#).unwrap(),
            Action::Go { url: "https://x.test".into() }
        );
        assert_eq!(parse_action(r#"{"a":"click","h":12}"#).unwrap(), Action::Click { h: 12 });
        assert_eq!(
            parse_action(r#"{"a":"type","h":5,"t":"hi"}"#).unwrap(),
            Action::Type { h: 5, t: "hi".into(), clear: true }
        );
        assert_eq!(
            parse_action(r#"{"a":"type","h":5,"t":"hi","clear":false}"#).unwrap(),
            Action::Type { h: 5, t: "hi".into(), clear: false }
        );
        assert_eq!(parse_action(r#"{"a":"scroll","d":"up"}"#).unwrap(), Action::Scroll { d: Dir::Up });
        assert_eq!(
            parse_action(r#"{"a":"more","h":40}"#).unwrap(),
            Action::More { h: 40, q: None, skip: 0, n: None, ctx: None }
        );
        assert_eq!(
            parse_action(r#"{"a":"more","h":40,"q":"model 50","skip":10,"n":5}"#).unwrap(),
            Action::More { h: 40, q: Some("model 50".into()), skip: 10, n: Some(5), ctx: None }
        );
        assert_eq!(parse_action(r#"{"a":"more","h":40,"q":"x"}"#).unwrap().describe(), "more 40 q=\"x\"");
        assert_eq!(parse_action(r#"{"a":"back"}"#).unwrap(), Action::Back);
        assert_eq!(
            parse_action(r#"{"a":"done","t":"42","ok":false}"#).unwrap(),
            Action::Done { t: "42".into(), ok: false }
        );
        assert_eq!(parse_action(r#"{"a":"done"}"#).unwrap(), Action::Done { t: String::new(), ok: true });
    }

    #[test]
    fn tolerates_prose_fences_and_braces_in_strings() {
        let a = parse_action("Sure! I will click.\n```json\n{\"a\":\"click\",\"h\":7}\n```\nDone.").unwrap();
        assert_eq!(a, Action::Click { h: 7 });
        let b = parse_action(r#"{"a":"done","t":"use {braces} and \"quotes\" } here","ok":true}"#).unwrap();
        assert_eq!(b, Action::Done { t: "use {braces} and \"quotes\" } here".into(), ok: true });
        assert_eq!(
            parse_action(r#"thinking {not json} then {"a":"back"}"#).unwrap(),
            Action::Back,
            "skips earlier objects that are not actions"
        );
    }

    #[test]
    fn rejects_garbage_with_a_helpful_message() {
        for bad in [
            "",
            "click 5",
            "{",
            r#"{"a":"fly"}"#,
            r#"{"a":"click"}"#,
            r#"{"a":"click","h":-1}"#,
            r#"{"a":"click","h":"x"}"#,
            "}{",
        ] {
            let e = parse_action(bad).unwrap_err();
            assert!(e.contains("JSON"), "{bad:?} -> {e}");
        }
    }

    #[test]
    fn history_description_never_contains_typed_text() {
        let a = Action::Type { h: 3, t: "hunter2-secret".into(), clear: true };
        assert_eq!(a.describe(), "type 3 [14 chars]");
        assert!(!a.describe().contains("hunter2"));
        assert_eq!(Action::Scroll { d: Dir::Down }.describe(), "scroll down");
    }
}
