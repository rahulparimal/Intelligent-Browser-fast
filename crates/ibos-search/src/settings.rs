//! Search settings: validated and clamped on every load and update, written atomically with owner-only permissions.
//! The API key is never part of [`Settings::public`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use url::Url;

pub const SUPPORTED_EXTENSIONS: [&str; 10] =
    [".txt", ".md", ".markdown", ".rst", ".html", ".htm", ".pdf", ".docx", ".csv", ".log"];

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("{0}")]
    Invalid(String),
    #[error("could not write settings: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub result_limit: i64,
    pub group_by_document: bool,
    pub snippet_words: i64,
    pub chunk_chars: i64,
    pub max_file_mb: i64,
    pub extensions: Vec<String>,
    pub auto_reindex_on_start: bool,
    pub answers_enabled: bool,
    pub llm_base_url: String,
    pub llm_model: String,
    pub llm_api_key: String,
    pub answer_passages: i64,
    pub llm_timeout_s: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            result_limit: 10,
            group_by_document: true,
            snippet_words: 32,
            chunk_chars: 900,
            max_file_mb: 20,
            extensions: SUPPORTED_EXTENSIONS.iter().map(|s| (*s).to_owned()).collect(),
            auto_reindex_on_start: false,
            answers_enabled: false,
            llm_base_url: String::new(),
            llm_model: String::new(),
            llm_api_key: String::new(),
            answer_passages: 5,
            llm_timeout_s: 30,
        }
    }
}

/// An http(s) URL with a host and no embedded credentials, trailing slash removed. Empty stays empty.
pub fn validate_base_url(url: &str) -> Result<String, SettingsError> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Ok(String::new());
    }
    let parsed = Url::parse(url).ok().filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
    let Some(parsed) = parsed else {
        return Err(SettingsError::Invalid(
            "Endpoint must be an http(s) URL, e.g. https://api.openai.com/v1 or http://localhost:11434/v1".into(),
        ));
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(SettingsError::Invalid("Do not put credentials in the URL; use the API key field".into()));
    }
    Ok(url.to_owned())
}

/// Whether the endpoint is on this computer, so passages do not leave it.
pub fn is_local_url(url: &str) -> bool {
    Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).is_some_and(|h| {
        let h = h.trim_matches(|c| c == '[' || c == ']');
        h == "localhost" || h == "127.0.0.1" || h == "::1" || h.ends_with(".localhost")
    })
}

impl Settings {
    /// Clamp numbers, drop unknown extensions, validate the endpoint.
    pub fn validated(mut self) -> Result<Self, SettingsError> {
        let d = Self::default();
        let clamp = |v: i64, lo: i64, hi: i64| v.clamp(lo, hi);
        self.result_limit = clamp(self.result_limit, 1, 50);
        self.snippet_words = clamp(self.snippet_words, 8, 64);
        self.chunk_chars = clamp(self.chunk_chars, 300, 3000);
        self.max_file_mb = clamp(self.max_file_mb, 1, 200);
        self.answer_passages = clamp(self.answer_passages, 1, 10);
        self.llm_timeout_s = clamp(self.llm_timeout_s, 5, 120);
        let wanted: Vec<String> = self.extensions.iter().map(|e| e.to_lowercase()).collect();
        self.extensions =
            SUPPORTED_EXTENSIONS.iter().filter(|e| wanted.iter().any(|w| w == **e)).map(|e| (*e).to_owned()).collect();
        if self.extensions.is_empty() {
            self.extensions = d.extensions;
        }
        self.llm_base_url = validate_base_url(&self.llm_base_url)?;
        self.llm_model = self.llm_model.trim().chars().take(200).collect();
        self.llm_api_key = self.llm_api_key.trim().chars().take(500).collect();
        Ok(self)
    }

    /// What the index was built with. Format matches the Python implementation (`json.dumps` separators), so a shared
    /// index is not reported stale just because the other implementation wrote it.
    pub fn index_signature(&self) -> String {
        let exts = self.extensions.iter().map(|e| format!("\"{e}\"")).collect::<Vec<_>>().join(", ");
        format!("[{}, {}, [{exts}]]", self.chunk_chars, self.max_file_mb)
    }

    /// Safe to send to a browser page: no secret values, only whether they are set.
    pub fn public(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or_else(|_| json!({}));
        if let Some(o) = v.as_object_mut() {
            o.remove("llm_api_key");
            o.insert("has_api_key".into(), json!(!self.llm_api_key.is_empty()));
            o.insert(
                "llm_is_local".into(),
                if self.llm_base_url.is_empty() { Value::Null } else { json!(is_local_url(&self.llm_base_url)) },
            );
        }
        v
    }
}

/// Settings persisted as JSON.
#[derive(Debug)]
pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    /// Load from `path`; a missing or corrupt file yields defaults.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let current = Self::load(&path);
        Self { path, current: Mutex::new(current) }
    }

    fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().map(lenient))
            .and_then(|s| s.validated().ok())
            .unwrap_or_default()
    }

    pub fn get(&self) -> Settings {
        self.current.lock().map_or_else(|p| p.into_inner().clone(), |g| g.clone())
    }

    /// Apply a partial update. Unknown keys are ignored; an empty API key keeps the saved one.
    pub fn update(&self, changes: &Map<String, Value>, clear_api_key: bool) -> Result<Settings, SettingsError> {
        let mut guard = self.current.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut merged = serde_json::to_value(&*guard).map_err(|e| SettingsError::Invalid(e.to_string()))?;
        let obj = merged.as_object_mut().ok_or_else(|| SettingsError::Invalid("internal".into()))?;
        // A saved key belongs to the endpoint it was saved for. Pointing the app at a different endpoint without supplying a new
        // key must not send the old key there.
        let endpoint_changed = changes
            .get("llm_base_url")
            .and_then(Value::as_str)
            .is_some_and(|u| obj.get("llm_base_url").and_then(Value::as_str) != Some(u));
        let new_key = changes.get("llm_api_key").and_then(Value::as_str).is_some_and(|k| !k.is_empty());
        let clear_api_key = clear_api_key || (endpoint_changed && !new_key);
        for (k, v) in changes {
            if !obj.contains_key(k) || (k == "llm_api_key" && v.as_str().is_none_or(str::is_empty)) {
                continue;
            }
            obj.insert(k.clone(), v.clone());
        }
        if clear_api_key {
            obj.insert("llm_api_key".into(), json!(""));
        }
        let new = lenient(obj).validated()?;
        if new.answers_enabled && (new.llm_base_url.is_empty() || new.llm_model.is_empty()) {
            return Err(SettingsError::Invalid("To enable AI answers, set the endpoint and model first".into()));
        }
        write_atomic(&self.path, &new)?;
        *guard = new.clone();
        Ok(new)
    }
}

/// Build settings from a JSON object, tolerating wrong types by falling back to the default for that field.
fn lenient(o: &Map<String, Value>) -> Settings {
    let d = Settings::default();
    let int = |k: &str, dv: i64| {
        o.get(k)
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_f64().map(|f| f as i64))
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            })
            .unwrap_or(dv)
    };
    let boolean = |k: &str, dv: bool| {
        o.get(k).and_then(|v| v.as_bool().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(dv)
    };
    let string = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    Settings {
        result_limit: int("result_limit", d.result_limit),
        group_by_document: boolean("group_by_document", d.group_by_document),
        snippet_words: int("snippet_words", d.snippet_words),
        chunk_chars: int("chunk_chars", d.chunk_chars),
        max_file_mb: int("max_file_mb", d.max_file_mb),
        extensions: o
            .get("extensions")
            .and_then(Value::as_array)
            .map_or(d.extensions.clone(), |a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()),
        auto_reindex_on_start: boolean("auto_reindex_on_start", d.auto_reindex_on_start),
        answers_enabled: boolean("answers_enabled", d.answers_enabled),
        llm_base_url: string("llm_base_url"),
        llm_model: string("llm_model"),
        llm_api_key: string("llm_api_key"),
        answer_passages: int("answer_passages", d.answer_passages),
        llm_timeout_s: int("llm_timeout_s", d.llm_timeout_s),
    }
}

fn write_atomic(path: &Path, s: &Settings) -> Result<(), SettingsError> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?; // the API key is inside: owner-only
    }
    f.write_all(serde_json::to_string_pretty(s).map_err(|e| SettingsError::Invalid(e.to_string()))?.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?; // atomic: a crash never leaves a half-written file
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: &Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn clamps_sanitises_and_never_empties_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path().join("s.json"));
        let n = s.update(&obj(&json!({"result_limit": 9999, "chunk_chars": "abc", "snippet_words": -5, "extensions": [".exe", ".PDF"], "unknown": 1})), false).unwrap();
        assert_eq!((n.result_limit, n.chunk_chars, n.snippet_words), (50, 900, 8));
        assert_eq!(n.extensions, [".pdf"]);
        assert_eq!(
            s.update(&obj(&json!({"extensions": []})), false).unwrap().extensions.len(),
            SUPPORTED_EXTENSIONS.len()
        );
    }

    #[test]
    fn url_validation_and_locality() {
        assert_eq!(validate_base_url(" https://api.openai.com/v1/ ").unwrap(), "https://api.openai.com/v1");
        assert_eq!(validate_base_url("").unwrap(), "");
        for bad in ["ftp://x", "localhost:11434", "http://", "javascript:alert(1)", "https://user:pw@host/v1"] {
            assert!(validate_base_url(bad).is_err(), "{bad}");
        }
        assert!(
            is_local_url("http://localhost:11434/v1")
                && is_local_url("http://127.0.0.1/v1")
                && is_local_url("http://[::1]:8080/v1")
        );
        assert!(!is_local_url("https://api.openai.com/v1") && !is_local_url("http://localhost.evil.com/v1"));
    }

    #[test]
    fn answers_need_endpoint_and_model() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path().join("s.json"));
        assert!(s.update(&obj(&json!({"answers_enabled": true})), false).is_err());
        assert!(
            s.update(
                &obj(&json!({"answers_enabled": true, "llm_base_url": "http://localhost:1/v1", "llm_model": "m"})),
                false
            )
            .unwrap()
            .answers_enabled
        );
    }

    #[test]
    fn a_saved_key_does_not_follow_a_changed_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path().join("s.json"));
        s.update(
            &obj(&json!({"llm_api_key": "sk-secret-123", "llm_base_url": "https://good.test/v1", "llm_model": "m"})),
            false,
        )
        .unwrap();
        // same endpoint: the key stays
        s.update(&obj(&json!({"llm_base_url": "https://good.test/v1", "llm_model": "m2"})), false).unwrap();
        assert_eq!(s.get().llm_api_key, "sk-secret-123");
        // different endpoint, no new key: the old key is dropped, never sent to the new host
        s.update(&obj(&json!({"llm_base_url": "https://evil.test/v1"})), false).unwrap();
        assert_eq!(s.get().llm_api_key, "");
        // different endpoint with a new key: that key is kept
        s.update(&obj(&json!({"llm_base_url": "https://other.test/v1", "llm_api_key": "sk-new"})), false).unwrap();
        assert_eq!(s.get().llm_api_key, "sk-new");
    }

    #[test]
    fn api_key_is_never_public_and_blank_update_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path().join("s.json"));
        s.update(
            &obj(&json!({"llm_api_key": "sk-secret-123", "llm_base_url": "https://x.test/v1", "llm_model": "m"})),
            false,
        )
        .unwrap();
        let public = s.get().public();
        assert!(public.get("llm_api_key").is_none() && public["has_api_key"] == json!(true));
        assert!(!public.to_string().contains("sk-secret-123"));
        s.update(&obj(&json!({"llm_model": "m2", "llm_api_key": ""})), false).unwrap();
        assert_eq!(s.get().llm_api_key, "sk-secret-123");
        s.update(&Map::new(), true).unwrap();
        assert_eq!(s.get().llm_api_key, "");
    }

    #[test]
    fn persistence_corruption_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        SettingsStore::open(&p)
            .update(&obj(&json!({"result_limit": 3, "group_by_document": false, "llm_api_key": "k"})), false)
            .unwrap();
        let again = SettingsStore::open(&p).get();
        assert_eq!((again.result_limit, again.group_by_document), (3, false));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(SettingsStore::open(&p).get(), Settings::default());
        std::fs::write(&p, r#"{"result_limit": "7", "evil": 1, "extensions": "nope"}"#).unwrap();
        assert_eq!(SettingsStore::open(&p).get().result_limit, 7);
    }

    #[test]
    fn signature_matches_the_python_format() {
        let s = Settings::default();
        assert!(s.index_signature().starts_with(r#"[900, 20, [".txt", ".md", ".markdown""#), "{}", s.index_signature());
    }

    #[test]
    fn reads_a_settings_file_written_by_the_python_implementation() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        std::fs::write(&p, r#"{"result_limit": 5, "group_by_document": true, "snippet_words": 32, "chunk_chars": 900, "max_file_mb": 20,
            "extensions": [".txt", ".md"], "auto_reindex_on_start": false, "answers_enabled": false, "llm_base_url": "",
            "llm_model": "", "llm_api_key": "", "answer_passages": 5, "llm_timeout_s": 30}"#).unwrap();
        let s = SettingsStore::open(&p).get();
        assert_eq!((s.result_limit, s.extensions.len()), (5, 2));
    }
}
