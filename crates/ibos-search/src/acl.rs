//! Per-document access control, applied **before** ranking so a restricted passage can neither appear nor influence order.
//!
//! A document is keyed by path. A principal is any string the caller trusts (a user id, a group). Rules:
//! * a document with ACL rows is visible only to callers holding at least one listed principal;
//! * a document without rows follows [`Policy`]: `Open` (visible to everyone) or `Closed` (visible to nobody).
//!
//! The library enforces the rule; it does not authenticate anyone. The caller must supply principals it has verified.

/// What happens to documents that have no ACL rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    #[default]
    Open,
    Closed,
}

/// Who is asking. `Access::anonymous()` holds no principals: it sees only documents that have no access list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    pub principals: Vec<String>,
    pub policy: Policy,
}

impl Access {
    pub fn anonymous() -> Self {
        Self::default()
    }

    pub fn new<I: IntoIterator<Item = S>, S: Into<String>>(principals: I) -> Self {
        Self { principals: principals.into_iter().map(Into::into).collect(), policy: Policy::Open }
    }

    #[must_use]
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// Principals with NUL removed: NUL is reserved for the "nobody" marker, so a caller cannot present it.
    pub(crate) fn clean(&self) -> Vec<String> {
        self.principals.iter().filter(|p| !p.contains('\0')).cloned().collect()
    }
}

pub const SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS doc_acl(path TEXT NOT NULL, principal TEXT NOT NULL, PRIMARY KEY(path, principal));";

/// Principal stored for an empty list, so the document stays hidden under the `Open` policy too.
pub(crate) const NOBODY: &str = "\u{0}nobody";

/// SQL predicate over a `docs` row aliased `d`, binding principals as `?{first}`..`?{first+n-1}`.
pub fn visible_sql(policy: Policy, first: usize, n: usize) -> String {
    let marks = (first..first + n).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    let granted = format!("EXISTS (SELECT 1 FROM doc_acl a WHERE a.path = d.path AND a.principal IN ({marks}))");
    match policy {
        Policy::Open => format!("(NOT EXISTS (SELECT 1 FROM doc_acl a WHERE a.path = d.path) OR {granted})"),
        Policy::Closed => format!("({granted})"),
    }
}
