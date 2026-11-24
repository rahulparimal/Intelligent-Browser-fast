//! Safety layer: domain limits, approval for risky actions, an audit log. Heuristics, a seatbelt rather than a sandbox.
//!
//! Risky actions (buy, pay, delete, send, a card number typed into a page) need approval. With no way to ask, they are denied.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use url::Url;

use crate::action::Action;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfirmMode {
    /// Ask before risky actions; deny when nobody can answer.
    #[default]
    Risky,
    /// Never ask (domain limits and the audit log still apply).
    Off,
}

#[derive(Debug, Clone, Default)]
pub struct GuardConfig {
    pub allowed_domains: Vec<String>,
    pub blocked_domains: Vec<String>,
    pub confirm_mode: ConfirmMode,
    pub audit_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow {
        /// A person approved a risky action.
        approved: bool,
    },
    Deny(String),
}

type Confirm = Box<dyn Fn(&str) -> bool + Send + Sync>;

pub struct Guard {
    cfg: GuardConfig,
    confirm: Confirm,
    denied: AtomicUsize,
    audit: Mutex<Option<File>>,
}

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guard").field("cfg", &self.cfg).field("denied", &self.denied).finish_non_exhaustive()
    }
}

const RISKY: [&str; 29] = [
    "buy",
    "purchase",
    "pay",
    "checkout",
    "check out",
    "place order",
    "place your order",
    "confirm order",
    "confirm payment",
    "confirm purchase",
    "complete order",
    "complete purchase",
    "subscribe",
    "delete",
    "remove",
    "erase",
    "close account",
    "deactivate",
    "transfer",
    "send",
    "wire",
    "withdraw",
    "unsubscribe",
    "publish",
    "post",
    "submit order",
    "submit payment",
    "empty cart",
    "cancel subscription",
];

/// `phrase` appears in `text` as whole words (case-insensitive).
fn has_phrase(text: &str, phrase: &str) -> bool {
    let (t, p) = (text.to_lowercase(), phrase.to_lowercase());
    t.match_indices(&p).any(|(i, m)| {
        let before = t[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = t[i + m.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

fn luhn(digits: &str) -> bool {
    let (mut sum, mut alt) = (0u32, false);
    for c in digits.chars().rev() {
        let Some(mut d) = c.to_digit(10) else { return false };
        if alt {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        alt = !alt;
    }
    sum % 10 == 0
}

/// A payment card number (13-19 digits, optional spaces or dashes, valid Luhn checksum) appears in `text`.
pub fn has_card_number(text: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_digit() || c == ' ' || c == '-')).any(|run| {
        let digits: String = run.chars().filter(char::is_ascii_digit).collect();
        (13..=19).contains(&digits.len()) && luhn(&digits)
    })
}

fn mask_cards(text: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if has_card_number(run) {
            out.push_str("[card]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if c.is_ascii_digit() || ((c == ' ' || c == '-') && !run.is_empty()) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn host_matches(host: &str, pattern: &str) -> bool {
    let p = pattern.trim().to_lowercase();
    match p.strip_prefix("*.") {
        Some(base) => host == base || host.ends_with(&format!(".{base}")),
        None => host == p,
    }
}

/// Whether `url` may be opened: http(s) only (plus `about:blank`), not blocked, and allowed if an allow-list exists.
pub fn host_allowed(url: &str, allowed: &[String], blocked: &[String]) -> Result<(), String> {
    if url == "about:blank" {
        return Ok(());
    }
    let parsed = Url::parse(url).map_err(|_| format!("{url} is not a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("only http and https pages may be opened, not {}:", parsed.scheme()));
    }
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    if blocked.iter().any(|b| host_matches(&host, b)) {
        return Err(format!("{host} is on the blocked list"));
    }
    if !allowed.is_empty() && !allowed.iter().any(|a| host_matches(&host, a)) {
        return Err(format!("{host} is not in the allowed domains"));
    }
    Ok(())
}

impl Guard {
    /// `confirm` is asked about each risky action and returns whether to allow it. In `Risky` mode with no terminal, pass a
    /// closure that returns `false`.
    pub fn new(cfg: GuardConfig, confirm: Confirm) -> Self {
        let audit = cfg.audit_path.as_ref().and_then(|p| {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let mut opts = OpenOptions::new();
            opts.create(true).append(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600); // the log can name sites and actions: owner only
            opts.open(p).ok()
        });
        Self { cfg, confirm, denied: AtomicUsize::new(0), audit: Mutex::new(audit) }
    }

    /// Denies every risky action. The safe default when nobody is there to ask.
    pub fn deny_risky(cfg: GuardConfig) -> Self {
        Self::new(cfg, Box::new(|_| false))
    }

    pub fn denied(&self) -> usize {
        self.denied.load(Ordering::Relaxed)
    }

    pub fn check_navigation(&self, url: &str) -> Result<(), String> {
        host_allowed(url, &self.cfg.allowed_domains, &self.cfg.blocked_domains)
    }

    /// Why this action needs approval, if it does.
    fn risk(action: &Action, element_text: Option<&str>) -> Option<String> {
        match action {
            Action::Click { .. } => {
                let text = element_text?;
                RISKY.iter().find(|p| has_phrase(text, p)).map(|p| {
                    format!(
                        "clicks \"{}\", which looks like a purchase, deletion or send ({p})",
                        text.chars().take(60).collect::<String>().trim()
                    )
                })
            }
            Action::Type { t, .. } if has_card_number(t) => {
                Some("types something that looks like a payment card number".into())
            }
            _ => None,
        }
    }

    pub fn check_action(&self, action: &Action, element_text: Option<&str>) -> Verdict {
        if let Action::Go { url } = action
            && let Err(reason) = self.check_navigation(url)
        {
            self.denied.fetch_add(1, Ordering::Relaxed);
            return Verdict::Deny(format!("Blocked by guardrail: {reason}."));
        }
        // Fail closed: a click or typing target the agent never saw (an invented or stale handle) cannot be risk-checked.
        if let Action::Click { h } | Action::Type { h, .. } = action
            && element_text.is_none()
        {
            self.denied.fetch_add(1, Ordering::Relaxed);
            return Verdict::Deny(format!(
                "Blocked by guardrail: element {h} does not exist on this page. Use handles exactly as shown."
            ));
        }
        let Some(reason) = Self::risk(action, element_text).filter(|_| self.cfg.confirm_mode == ConfirmMode::Risky)
        else {
            return Verdict::Allow { approved: false };
        };
        if (self.confirm)(&reason) {
            Verdict::Allow { approved: true }
        } else {
            self.denied.fetch_add(1, Ordering::Relaxed);
            Verdict::Deny(format!(
                "Blocked by guardrail: the agent {reason}. The user did not approve. Choose a different approach or finish."
            ))
        }
    }

    /// Append one JSON line to the audit log (if configured). Card numbers are masked.
    pub fn audit(&self, step: u32, entry: &Value) {
        let mut guard = self.audit.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(f) = guard.as_mut() else { return };
        let ts = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let mut line = json!({"ts": ts, "step": step});
        if let (Some(l), Value::Object(e)) = (line.as_object_mut(), entry) {
            l.extend(e.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        let _ = writeln!(f, "{}", mask_cards(&line.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard(allowed: &[&str], blocked: &[&str], confirm: bool) -> Guard {
        let cfg = GuardConfig {
            allowed_domains: allowed.iter().map(ToString::to_string).collect(),
            blocked_domains: blocked.iter().map(ToString::to_string).collect(),
            ..GuardConfig::default()
        };
        Guard::new(cfg, Box::new(move |_| confirm))
    }

    #[test]
    fn domain_rules() {
        let a = |v: &[&str]| v.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(host_allowed("https://example.com/x", &[], &[]).is_ok());
        assert!(host_allowed("https://shop.example.com", &a(&["*.example.com"]), &[]).is_ok());
        assert!(host_allowed("https://example.com", &a(&["*.example.com"]), &[]).is_ok(), "*.x also matches x itself");
        assert!(
            host_allowed("https://evilexample.com", &a(&["*.example.com"]), &[]).is_err(),
            "suffix tricks do not match"
        );
        assert!(host_allowed("https://example.com.evil.io", &a(&["example.com"]), &[]).is_err());
        assert!(host_allowed("https://sub.example.com", &a(&["example.com"]), &[]).is_err(), "a bare domain is exact");
        assert!(
            host_allowed("https://ads.example.com", &a(&["*.example.com"]), &a(&["ads.example.com"])).is_err(),
            "blocked wins"
        );
        assert!(host_allowed("HTTPS://Example.COM/", &a(&["example.com"]), &[]).is_ok(), "case-insensitive");
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "data:text/html,x", "ftp://x.test", "not a url", ""] {
            assert!(host_allowed(bad, &[], &[]).is_err(), "{bad}");
        }
        assert!(host_allowed("about:blank", &a(&["example.com"]), &[]).is_ok());
    }

    #[test]
    fn risky_clicks_need_approval_and_safe_ones_never_ask() {
        let click = Action::Click { h: 1 };
        for risky in ["Place order", "Delete account", "Send message", "Buy now", "Pay $20", "CHECK OUT", "Unsubscribe"]
        {
            assert!(matches!(guard(&[], &[], false).check_action(&click, Some(risky)), Verdict::Deny(_)), "{risky}");
            assert_eq!(guard(&[], &[], true).check_action(&click, Some(risky)), Verdict::Allow { approved: true });
        }
        for safe in ["Read more", "Sign in", "Search", "Sender info", "Compost", "Display options", "Prepay guide"] {
            assert_eq!(
                guard(&[], &[], false).check_action(&click, Some(safe)),
                Verdict::Allow { approved: false },
                "{safe}"
            );
        }
        assert!(
            matches!(guard(&[], &[], false).check_action(&click, None), Verdict::Deny(_)),
            "an element nobody has seen is refused, not allowed"
        );
        assert!(
            matches!(guard(&[], &[], true).check_action(&click, None), Verdict::Deny(_)),
            "approval cannot rescue a handle that does not exist"
        );
    }

    #[test]
    fn card_numbers_are_detected_by_luhn_not_by_digit_count() {
        assert!(
            has_card_number("4242 4242 4242 4242")
                && has_card_number("pay 4111-1111-1111-1111 now")
                && has_card_number("4242424242424242")
        );
        assert!(!has_card_number("4242 4242 4242 4241"), "fails Luhn: not a card");
        assert!(!has_card_number("order 12345") && !has_card_number("") && !has_card_number("+1 555 123 4567"));
        let typed = Action::Type { h: 1, t: "4242 4242 4242 4242".into(), clear: true };
        assert!(matches!(guard(&[], &[], false).check_action(&typed, Some("input card")), Verdict::Deny(_)));
        // the old "<secret>" prefix exemption was a bypass: nothing substitutes it, so it must not skip the card check
        let smuggled = Action::Type { h: 1, t: "<secret> 4242 4242 4242 4242".into(), clear: true };
        assert!(matches!(guard(&[], &[], false).check_action(&smuggled, Some("input card")), Verdict::Deny(_)));
        let harmless = Action::Type { h: 1, t: "hello".into(), clear: true };
        assert_eq!(
            guard(&[], &[], false).check_action(&harmless, Some("input name")),
            Verdict::Allow { approved: false }
        );
    }

    #[test]
    fn confirm_off_skips_questions_but_domain_limits_still_apply() {
        let g = Guard::new(
            GuardConfig {
                confirm_mode: ConfirmMode::Off,
                allowed_domains: vec!["example.com".into()],
                ..GuardConfig::default()
            },
            Box::new(|_| false),
        );
        assert_eq!(g.check_action(&Action::Click { h: 1 }, Some("Place order")), Verdict::Allow { approved: false });
        assert!(matches!(g.check_action(&Action::Go { url: "https://other.test".into() }, None), Verdict::Deny(_)));
        assert_eq!(g.denied(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn audit_log_is_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jsonl");
        let g = Guard::deny_risky(GuardConfig { audit_path: Some(p.clone()), ..GuardConfig::default() });
        g.audit(1, &json!({"action": "click 4"}));
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn audit_log_is_json_lines_with_cards_masked() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("logs").join("a.jsonl");
        let g = Guard::deny_risky(GuardConfig { audit_path: Some(p.clone()), ..GuardConfig::default() });
        g.audit(1, &json!({"action": "type 3", "note": "card 4242 4242 4242 4242 typed", "ok": true}));
        g.audit(2, &json!({"action": "click 4"}));
        let lines: Vec<Value> =
            std::fs::read_to_string(&p).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(
            (lines.len(), lines[0]["step"].clone(), lines[1]["action"].clone()),
            (2, json!(1), json!("click 4"))
        );
        assert!(
            !std::fs::read_to_string(&p).unwrap().contains("4242")
                && lines[0]["note"].as_str().unwrap().contains("[card]")
        );
        assert!(lines[0]["ts"].as_f64().unwrap() > 1.0e9);
    }
}
