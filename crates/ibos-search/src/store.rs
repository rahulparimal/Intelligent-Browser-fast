//! SQLite FTS5 index. Lexical (BM25) only: no embeddings, so synonyms and paraphrases are not matched.
//!
//! Thread-safe (one lock around the connection, never held while parsing a file), prunes files that disappeared, finds
//! `email` in `e-mail`, ignores stopwords in questions, and shows each document once unless told otherwise.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::extract::{ExtractError, extract};
use crate::paths::default_db_path;
use crate::settings::{Settings, SettingsError, SettingsStore};
use crate::text::{alt_forms, chunk, query_terms};

const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("path not found: {0}")]
    NotFound(PathBuf),
    #[error(transparent)]
    Settings(#[from] SettingsError),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct IndexStats {
    pub indexed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub failed: usize,
    pub empty: usize,
    pub removed: usize,
}

impl IndexStats {
    fn add(&mut self, o: &Self) {
        self.indexed += o.indexed;
        self.unchanged += o.unchanged;
        self.skipped += o.skipped;
        self.failed += o.failed;
        self.empty += o.empty;
        self.removed += o.removed;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub title: String,
    pub path: String,
    pub page: i64,
    pub snippet: String,
    pub score: f64,
    pub doc_id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocInfo {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub chunks: i64,
    pub bytes: i64,
    pub indexed_at: i64,
    pub exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StoreStats {
    pub documents: i64,
    pub chunks: i64,
    pub bytes: i64,
    pub folders: i64,
}

/// Options for [`LiteStore::index_path`].
#[derive(Debug, Clone, Copy)]
pub struct IndexOptions {
    /// Remove documents under the root whose files no longer exist.
    pub prune: bool,
    /// Re-read every file even if unchanged.
    pub force: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self { prune: true, force: false }
    }
}

#[derive(Debug)]
pub struct LiteStore {
    path: PathBuf,
    db: Mutex<Connection>,
    settings: SettingsStore,
}

type Progress<'a> = Option<&'a (dyn Fn(usize, usize) + Sync)>;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn under(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/")) || path.starts_with(&format!("{root}\\"))
}

impl LiteStore {
    /// Open (creating or migrating) the index. With an explicit `db_path` and no `settings`, settings live next to it.
    pub fn open(db_path: Option<&Path>, settings: Option<SettingsStore>) -> Result<Self, SearchError> {
        let path = db_path.map_or_else(default_db_path, Path::to_path_buf);
        if let Some(dir) = path.parent() {
            let existed = dir.exists();
            std::fs::create_dir_all(dir)?;
            #[cfg(unix)]
            if !existed {
                // a directory we create holds the full text of the user's documents: owner only
                std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
            }
        }
        #[cfg(unix)]
        if !path.exists() {
            // SQLite gives its -wal and -shm files the permissions of the database file
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            let _ = opts.open(&path);
        }
        let settings = settings.unwrap_or_else(|| SettingsStore::open(path.with_file_name("settings.json")));
        let db = Connection::open(&path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let store = Self { path, db: Mutex::new(db), settings };
        store.migrate()?;
        Ok(store)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn settings(&self) -> Settings {
        self.settings.get()
    }

    pub fn settings_store(&self) -> &SettingsStore {
        &self.settings
    }

    fn migrate(&self) -> Result<(), SearchError> {
        let db = self.conn();
        let ver: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let has_docs =
            db.query_row("SELECT 1 FROM sqlite_master WHERE name='docs'", [], |_| Ok(())).optional()?.is_some();
        if ver > 0 && ver < SCHEMA_VERSION {
            // old layout: rebuild the passages; clearing the hashes makes the next index re-read every file
            db.execute_batch("DROP TABLE IF EXISTS chunks")?;
            if has_docs {
                db.execute_batch("UPDATE docs SET sha = ''")?;
            }
        }
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS docs(id INTEGER PRIMARY KEY, path TEXT UNIQUE, title TEXT, sha TEXT, bytes INTEGER DEFAULT 0, indexed_at INTEGER DEFAULT 0);
             CREATE TABLE IF NOT EXISTS folders(path TEXT PRIMARY KEY, added_at INTEGER);
             CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
             CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(text, title, alt, doc_id UNINDEXED, page UNINDEXED, tokenize='porter unicode61 remove_diacritics 2');",
        )?;
        for col in ["bytes", "indexed_at"] {
            let present: bool = db
                .prepare("PRAGMA table_info(docs)")?
                .query_map([], |r| r.get::<_, String>(1))?
                .filter_map(Result::ok)
                .any(|c| c == col);
            if !present {
                db.execute_batch(&format!("ALTER TABLE docs ADD COLUMN {col} INTEGER DEFAULT 0"))?;
            }
        }
        db.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        Ok(())
    }

    // ---- indexing -------------------------------------------------------------------------------------------------------
    /// Index a file or folder (recursive, hidden directories skipped). Unchanged files are skipped by content hash.
    pub fn index_path(
        &self,
        root: &Path,
        opts: IndexOptions,
        progress: Progress<'_>,
    ) -> Result<IndexStats, SearchError> {
        let cfg = self.settings();
        let root = expand(root);
        if !root.exists() {
            return Err(SearchError::NotFound(root));
        }
        let root = root.canonicalize()?;
        let files: Vec<PathBuf> = if root.is_file() {
            vec![root.clone()]
        } else {
            WalkDir::new(&root)
                .follow_links(false)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
                .map(walkdir::DirEntry::into_path)
                .collect()
        };
        let max_bytes = u64::try_from(cfg.max_file_mb).unwrap_or(20) * 1024 * 1024;
        let mut stats = IndexStats::default();
        let mut seen: HashSet<String> = HashSet::new();
        for (n, f) in files.iter().enumerate() {
            if let Some(p) = progress {
                p(n + 1, files.len());
            }
            let ext =
                f.extension().and_then(|e| e.to_str()).map(|e| format!(".{}", e.to_lowercase())).unwrap_or_default();
            let too_big = std::fs::metadata(f).map_or(true, |m| m.len() > max_bytes);
            if !cfg.extensions.contains(&ext) || too_big {
                stats.skipped += 1;
                continue;
            }
            let canonical = f.canonicalize().unwrap_or_else(|_| f.clone());
            seen.insert(canonical.to_string_lossy().into_owned());
            match self.index_file(&canonical, &cfg, opts.force) {
                Ok(FileResult::Indexed) => stats.indexed += 1,
                Ok(FileResult::Unchanged) => stats.unchanged += 1,
                Ok(FileResult::Empty) => stats.empty += 1,
                Err(e) => {
                    tracing::debug!(file = %canonical.display(), error = %e, "index failed");
                    stats.failed += 1;
                }
            }
        }
        if opts.prune && root.is_dir() {
            stats.removed = self.prune(Some(&root.to_string_lossy()), Some(&seen))?;
        }
        self.conn().execute("INSERT OR REPLACE INTO meta(k,v) VALUES('signature', ?1)", [cfg.index_signature()])?;
        Ok(stats)
    }

    fn index_file(&self, f: &Path, cfg: &Settings, force: bool) -> Result<FileResult, IndexError> {
        let raw = std::fs::read(f)?;
        let sha = hex::encode(Sha256::digest(&raw));
        let key = f.to_string_lossy().into_owned();
        let existing: Option<(i64, String)> = self
            .conn()
            .query_row("SELECT id, sha FROM docs WHERE path=?1", [&key], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if !force && existing.as_ref().is_some_and(|(_, s)| *s == sha) {
            return Ok(FileResult::Unchanged);
        }
        let (title, pages) = extract(f)?; // parsing happens outside the lock: it can be slow
        let size = usize::try_from(cfg.chunk_chars).unwrap_or(900);
        let pieces: Vec<(usize, String)> =
            pages.iter().enumerate().flat_map(|(n, p)| chunk(p, size).into_iter().map(move |c| (n + 1, c))).collect();
        let mut db = self.conn();
        let tx = db.transaction()?;
        let doc_id = if let Some((id, _)) = existing {
            tx.execute("DELETE FROM chunks WHERE doc_id=?1", [id])?;
            tx.execute(
                "UPDATE docs SET title=?1, sha=?2, bytes=?3, indexed_at=?4 WHERE id=?5",
                params![title, sha, raw.len() as i64, now(), id],
            )?;
            id
        } else {
            tx.execute(
                "INSERT INTO docs(path,title,sha,bytes,indexed_at) VALUES(?1,?2,?3,?4,?5)",
                params![key, title, sha, raw.len() as i64, now()],
            )?;
            tx.last_insert_rowid()
        };
        {
            let mut ins = tx.prepare("INSERT INTO chunks(text,title,alt,doc_id,page) VALUES(?1,?2,?3,?4,?5)")?;
            for (page, c) in &pieces {
                ins.execute(params![c, title, alt_forms(&format!("{title} {c}")), doc_id, *page as i64])?;
            }
        }
        tx.commit()?;
        Ok(if pieces.is_empty() { FileResult::Empty } else { FileResult::Indexed })
    }

    /// Drop documents whose file no longer exists, optionally only under `root` and never those in `keep`.
    pub fn prune(&self, root: Option<&str>, keep: Option<&HashSet<String>>) -> Result<usize, SearchError> {
        let rows: Vec<(i64, String)> = {
            let db = self.conn();
            let mut st = db.prepare("SELECT id, path FROM docs")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        let mut removed = 0;
        for (id, path) in rows {
            if root.is_some_and(|r| !under(&path, r))
                || keep.is_some_and(|k| k.contains(&path))
                || Path::new(&path).exists()
            {
                continue;
            }
            self.delete_doc(id)?;
            removed += 1;
        }
        Ok(removed)
    }

    fn delete_doc(&self, id: i64) -> Result<(), SearchError> {
        let mut db = self.conn();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM chunks WHERE doc_id=?1", [id])?;
        tx.execute("DELETE FROM docs WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove(&self, path: &Path) -> Result<bool, SearchError> {
        let key = expand(path).canonicalize().unwrap_or_else(|_| expand(path)).to_string_lossy().into_owned();
        let id: Option<i64> =
            self.conn().query_row("SELECT id FROM docs WHERE path=?1", [key], |r| r.get(0)).optional()?;
        id.map(|i| self.delete_doc(i)).transpose().map(|o| o.is_some())
    }

    pub fn remove_id(&self, id: i64) -> Result<bool, SearchError> {
        let exists = self.conn().query_row("SELECT 1 FROM docs WHERE id=?1", [id], |_| Ok(())).optional()?.is_some();
        if exists {
            self.delete_doc(id)?;
        }
        Ok(exists)
    }

    pub fn clear(&self) -> Result<(), SearchError> {
        self.conn().execute_batch("DELETE FROM chunks; DELETE FROM docs; DELETE FROM meta;")?;
        Ok(())
    }

    // ---- folders --------------------------------------------------------------------------------------------------------
    pub fn add_folder(&self, path: &Path) -> Result<String, SearchError> {
        let p = expand(path);
        if !p.exists() {
            return Err(SearchError::NotFound(p));
        }
        let key = p.canonicalize()?.to_string_lossy().into_owned();
        self.conn().execute("INSERT OR IGNORE INTO folders(path, added_at) VALUES(?1, ?2)", params![key, now()])?;
        Ok(key)
    }

    pub fn remove_folder(&self, path: &Path, drop_documents: bool) -> Result<usize, SearchError> {
        let key = expand(path).canonicalize().unwrap_or_else(|_| expand(path)).to_string_lossy().into_owned();
        self.conn().execute("DELETE FROM folders WHERE path=?1", [&key])?;
        if !drop_documents {
            return Ok(0);
        }
        let ids: Vec<i64> = {
            let db = self.conn();
            let mut st = db.prepare("SELECT id, path FROM docs")?;
            st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                .filter_map(Result::ok)
                .filter(|(_, p)| under(p, &key))
                .map(|(i, _)| i)
                .collect()
        };
        for id in &ids {
            self.delete_doc(*id)?;
        }
        Ok(ids.len())
    }

    pub fn folders(&self) -> Result<Vec<String>, SearchError> {
        let db = self.conn();
        let mut st = db.prepare("SELECT path FROM folders ORDER BY added_at, path")?;
        Ok(st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?)
    }

    /// Re-scan every registered folder: picks up changes, additions and deletions.
    pub fn reindex_all(&self, progress: Progress<'_>, force: bool) -> Result<IndexStats, SearchError> {
        let mut total = IndexStats::default();
        let targets: Vec<PathBuf> = self.folders()?.into_iter().map(PathBuf::from).filter(|p| p.exists()).collect();
        for t in &targets {
            total.add(&self.index_path(t, IndexOptions { prune: true, force }, progress)?);
        }
        if targets.is_empty() {
            total.removed += self.prune(None, None)?;
        }
        Ok(total)
    }

    // ---- reading --------------------------------------------------------------------------------------------------------
    pub fn needs_reindex(&self) -> Result<bool, SearchError> {
        let db = self.conn();
        let sig: Option<String> =
            db.query_row("SELECT v FROM meta WHERE k='signature'", [], |r| r.get(0)).optional()?;
        let has_docs = db.query_row("SELECT 1 FROM docs LIMIT 1", [], |_| Ok(())).optional()?.is_some();
        Ok(has_docs && sig.as_deref() != Some(self.settings().index_signature().as_str()))
    }

    pub fn stats(&self) -> Result<StoreStats, SearchError> {
        let db = self.conn();
        let one = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0));
        Ok(StoreStats {
            documents: one("SELECT COUNT(*) FROM docs")?,
            chunks: one("SELECT COUNT(*) FROM chunks")?,
            bytes: one("SELECT COALESCE(SUM(bytes),0) FROM docs")?,
            folders: one("SELECT COUNT(*) FROM folders")?,
        })
    }

    /// Documents whose path or title contains `query` (case-insensitive; `%` and `_` are literal).
    pub fn documents(&self, query: &str, offset: usize, limit: usize) -> Result<(Vec<DocInfo>, i64), SearchError> {
        let like = format!("%{}%", query.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        let db = self.conn();
        let total: i64 = db.query_row(
            "SELECT COUNT(*) FROM docs WHERE lower(path) LIKE ?1 ESCAPE '\\' OR lower(title) LIKE ?1 ESCAPE '\\'",
            [&like],
            |r| r.get(0),
        )?;
        let mut st = db.prepare(
            "SELECT d.id, d.path, d.title, (SELECT COUNT(*) FROM chunks c WHERE c.doc_id=d.id), d.bytes, d.indexed_at FROM docs d \
             WHERE lower(d.path) LIKE ?1 ESCAPE '\\' OR lower(d.title) LIKE ?1 ESCAPE '\\' ORDER BY d.path LIMIT ?2 OFFSET ?3",
        )?;
        let rows = st
            .query_map(params![like, limit.clamp(1, 200) as i64, offset as i64], |r| {
                let path: String = r.get(1)?;
                Ok(DocInfo {
                    id: r.get(0)?,
                    exists: Path::new(&path).exists(),
                    path,
                    title: r.get(2)?,
                    chunks: r.get(3)?,
                    bytes: r.get(4)?,
                    indexed_at: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok((rows, total))
    }

    /// Best passages for `query`. AND first (precision), then OR (recall). Terms are quoted, so user text can never inject
    /// FTS syntax. `group` shows each document once (its best passage); `None` uses the setting.
    pub fn search(&self, query: &str, limit: Option<usize>, group: Option<bool>) -> Result<Vec<Hit>, SearchError> {
        let cfg = self.settings();
        let limit = limit.filter(|l| *l > 0).unwrap_or(usize::try_from(cfg.result_limit).unwrap_or(10)).clamp(1, 50);
        let group = group.unwrap_or(cfg.group_by_document);
        let terms = query_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let fetch = if group { limit * 6 } else { limit };
        let sql = format!(
            "SELECT d.id, d.title, d.path, c.page, snippet(chunks, 0, '<mark>', '</mark>', ' ... ', {}), bm25(chunks, 1.0, 2.0, 1.0) AS s \
             FROM chunks c JOIN docs d ON d.id = c.doc_id WHERE chunks MATCH ?1 ORDER BY s LIMIT ?2",
            cfg.snippet_words
        );
        let mut rows: Vec<(i64, String, String, i64, String, f64)> = Vec::new();
        for joiner in [" AND ", " OR "] {
            let m = terms.iter().map(|t| format!("\"{}\"", t.replace('"', ""))).collect::<Vec<_>>().join(joiner);
            let db = self.conn();
            let mut st = db.prepare_cached(&sql)?;
            rows = st
                .query_map(params![m, fetch as i64], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                })?
                .collect::<Result<_, _>>()?;
            if !rows.is_empty() {
                break;
            }
            if terms.len() == 1 {
                return Ok(Vec::new());
            }
        }
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        for (doc_id, title, path, page, snippet, score) in rows {
            if group && !seen.insert(doc_id) {
                continue;
            }
            hits.push(Hit { title, path, page, snippet, score: -score, doc_id });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }
}

enum FileResult {
    Indexed,
    Unchanged,
    Empty,
}

#[derive(Debug, thiserror::Error)]
enum IndexError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Extract(#[from] ExtractError),
}

fn expand(p: &Path) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix("~/")) {
        Some(rest) => std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or_else(|| p.to_path_buf(), |h| PathBuf::from(h).join(rest)),
        None => p.to_path_buf(),
    }
}

#[cfg(all(test, unix))]
mod permission_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_new_index_is_private_to_the_owner() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let db = data.join("idx.db");
        let store = LiteStore::open(Some(&db), None).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&data), 0o700, "directory");
        assert_eq!(mode(&db), 0o600, "database");
        store.stats().unwrap();
        for ext in ["-wal", "-shm"] {
            let side = PathBuf::from(format!("{}{ext}", db.display()));
            if side.exists() {
                assert_eq!(mode(&side), 0o600, "{ext}");
            }
        }
    }
}
