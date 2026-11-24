#![allow(clippy::many_single_char_names)] // short names are clearer in the scoring maths
//! Hybrid retrieval on top of the `ibos-search` FTS5 index.
//!
//! ```text
//! query ──► BM25 (FTS5) ─────────────┐
//!      └──► embed ─► dense (int8) ───┴─► RRF fusion ─► rerank ─► dedupe ─► top-k passages ─► evidence pack ─► grounding check
//!                 ACL filter applied inside both retrievers, before any ranking
//! ```
//!
//! Everything is additive: the Python-compatible tables are untouched; vectors and ACLs live in two extra tables
//! (`vec_chunks`, `doc_acl`) in the same database file.
//!
//! Semantic quality is decided by the [`Embedder`]. The built-in [`HashingEmbedder`] needs no model and finds typos and word
//! forms, not synonyms. Use [`RemoteEmbedder`] for meaning.

mod dedupe;
pub mod embed;
pub mod eval;
mod fusion;
pub mod ground;
pub mod pack;
pub mod rerank;
mod vectors;

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ibos_mem::MemoryBudget;
use ibos_search::LiteStore;
use ibos_search::text::query_terms;
use rusqlite::{Connection, params, params_from_iter};
use serde::Serialize;

pub use embed::{EmbedError, Embedder, HashingEmbedder, RemoteEmbedder};
pub use ibos_search::acl::Policy;
pub use pack::Evidence;
pub use rerank::{HeuristicReranker, Reranker};

#[derive(Debug, thiserror::Error)]
pub enum RetrievalError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Embed(#[from] EmbedError),
    #[error(transparent)]
    Search(#[from] ibos_search::SearchError),
}

/// Which retrievers to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Lexical,
    Dense,
    #[default]
    Hybrid,
}

impl std::str::FromStr for Mode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "lexical" | "bm25" => Ok(Self::Lexical),
            "dense" | "vector" => Ok(Self::Dense),
            "hybrid" | "auto" => Ok(Self::Hybrid),
            other => Err(format!("unknown mode `{other}` (lexical, dense or hybrid)")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Query {
    pub text: String,
    /// Verified identities of the caller. Documents with ACL rows are visible only to a matching principal.
    pub principals: Vec<String>,
    pub mode: Mode,
    pub limit: usize,
    /// How many candidates each retriever contributes before fusion.
    pub candidates: usize,
    pub rerank: bool,
    pub dedupe: bool,
    /// Replace query words that match nothing by the closest indexed words (one or two letters off).
    pub fuzzy: bool,
}

impl Query {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            principals: Vec::new(),
            mode: Mode::Hybrid,
            limit: 10,
            candidates: 50,
            rerank: true,
            dedupe: true,
            fuzzy: true,
        }
    }

    #[must_use]
    pub fn mode(mut self, m: Mode) -> Self {
        self.mode = m;
        self
    }

    #[must_use]
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = n.clamp(1, 100);
        self
    }

    #[must_use]
    pub fn principals<I: IntoIterator<Item = S>, S: Into<String>>(mut self, p: I) -> Self {
        self.principals = p.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn fuzzy(mut self, on: bool) -> Self {
        self.fuzzy = on;
        self
    }

    #[must_use]
    pub fn rerank(mut self, on: bool) -> Self {
        self.rerank = on;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Passage {
    pub chunk_id: i64,
    pub doc_id: i64,
    pub title: String,
    pub path: String,
    pub page: i64,
    pub text: String,
    /// Final score in `[0, 1]` (fused, then reranked when enabled).
    pub score: f32,
    pub lexical_rank: Option<usize>,
    pub dense_rank: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub passages: Vec<Passage>,
    /// What actually ran. Differs from the request when vectors are missing (`Hybrid` falls back to `Lexical`).
    pub mode_used: Mode,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyncStats {
    pub embedded: usize,
    pub kept: usize,
    pub removed: usize,
}

/// Where the vectors are searched from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VectorMode {
    /// Held in RAM, paid for from the memory budget.
    Resident,
    /// Read from SQLite on every query; constant memory.
    Streaming,
    /// No vectors indexed.
    None,
}

struct Cache {
    generation: i64,
    resident: Option<Arc<vectors::Resident>>,
    mode: VectorMode,
}

#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cache").field("generation", &self.generation).field("mode", &self.mode).finish()
    }
}

#[derive(Debug)]
pub struct Retriever {
    db: Mutex<Connection>,
    embedder: Option<Arc<dyn Embedder>>,
    reranker: Box<dyn Reranker>,
    budget: MemoryBudget,
    policy: Policy,
    cache: Mutex<Cache>,
    vocab: Mutex<Option<Vocab>>,
}

/// Principal stored for an empty ACL. Callers cannot present it: principals containing NUL are dropped in `search`.
const NOBODY: &str = "\u{0}nobody";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn chunk_hash(text: &str) -> Vec<u8> {
    blake3::hash(text.as_bytes()).as_bytes()[..8].to_vec()
}

impl Retriever {
    /// Open next to an existing [`LiteStore`] (its database file is shared). `embedder` may be `None` for lexical-only use.
    pub fn open(
        store: &LiteStore,
        embedder: Option<Arc<dyn Embedder>>,
        budget: MemoryBudget,
    ) -> Result<Self, RetrievalError> {
        Self::open_path(store.path(), embedder, budget)
    }

    pub fn open_path(
        db: &Path,
        embedder: Option<Arc<dyn Embedder>>,
        budget: MemoryBudget,
    ) -> Result<Self, RetrievalError> {
        let conn = Connection::open(db)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS vec_chunks(chunk_id INTEGER PRIMARY KEY, doc_id INTEGER NOT NULL, h BLOB NOT NULL, scale REAL NOT NULL, v BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS meta(k TEXT PRIMARY KEY, v TEXT);
             CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vocab USING fts5vocab(chunks, 'row');
             {}",
            ibos_search::acl::SCHEMA
        ))?;
        Ok(Self {
            db: Mutex::new(conn),
            embedder,
            reranker: Box::new(HeuristicReranker),
            budget,
            policy: Policy::Open,
            cache: Mutex::new(Cache { generation: -1, resident: None, mode: VectorMode::None }),
            vocab: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn with_policy(mut self, p: Policy) -> Self {
        self.policy = p;
        self
    }

    #[must_use]
    pub fn with_reranker(mut self, r: Box<dyn Reranker>) -> Self {
        self.reranker = r;
        self
    }

    fn meta(db: &Connection, k: &str) -> Option<String> {
        db.query_row("SELECT v FROM meta WHERE k=?1", [k], |r| r.get(0)).ok()
    }

    fn set_meta(db: &Connection, k: &str, v: &str) -> Result<(), RetrievalError> {
        db.execute("INSERT INTO meta(k,v) VALUES(?1,?2) ON CONFLICT(k) DO UPDATE SET v=excluded.v", params![k, v])?;
        Ok(())
    }

    // ---- access control -------------------------------------------------------------------------------------------
    /// Restrict a document to the given principals (replaces any earlier list). An empty list makes it visible to nobody.
    pub fn set_acl(&self, path: &str, principals: &[&str]) -> Result<(), RetrievalError> {
        let mut db = lock(&self.db);
        let tx = db.transaction()?;
        tx.execute("DELETE FROM doc_acl WHERE path=?1", [path])?;
        for p in principals {
            tx.execute("INSERT OR IGNORE INTO doc_acl(path, principal) VALUES(?1, ?2)", params![path, p])?;
        }
        if principals.is_empty() {
            // a row nobody can match keeps the document hidden under the Open policy too
            tx.execute("INSERT INTO doc_acl(path, principal) VALUES(?1, ?2)", params![path, NOBODY])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn clear_acl(&self, path: &str) -> Result<(), RetrievalError> {
        lock(&self.db).execute("DELETE FROM doc_acl WHERE path=?1", [path])?;
        Ok(())
    }

    fn hidden_docs(&self, db: &Connection, principals: &[String]) -> Result<HashSet<i64>, RetrievalError> {
        let sql = format!(
            "SELECT d.id FROM docs d WHERE NOT {}",
            ibos_search::acl::visible_sql(self.policy, 1, principals.len())
        );
        let mut st = db.prepare(&sql)?;
        let ids = st
            .query_map(params_from_iter(principals.iter()), |r| r.get::<_, i64>(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        Ok(ids)
    }

    // ---- vector index ---------------------------------------------------------------------------------------------
    /// Bring the vector table in line with the FTS index: embed new or changed passages, drop vanished ones.
    /// Passages are matched by a hash of their text, so a changed file is re-embedded and a reused row id can never keep a
    /// stale vector. Does not hold the database lock while the embedder runs.
    pub fn sync_vectors(&self, progress: Option<&dyn Fn(usize, usize)>) -> Result<SyncStats, RetrievalError> {
        let emb = self.embedder.clone().ok_or_else(|| EmbedError::Request("no embedder configured".into()))?;
        let model = emb.id();
        let (todo, mut stats) = {
            let db = lock(&self.db);
            let mut stats = SyncStats::default();
            if Self::meta(&db, "vec_model").as_deref() != Some(model.as_str()) {
                stats.removed += db.execute("DELETE FROM vec_chunks", [])?;
                Self::set_meta(&db, "vec_model", &model)?;
            }
            stats.removed +=
                db.execute("DELETE FROM vec_chunks WHERE chunk_id NOT IN (SELECT rowid FROM chunks)", [])?;
            let mut have = std::collections::HashMap::new();
            {
                let mut st = db.prepare("SELECT chunk_id, doc_id, h FROM vec_chunks")?;
                for r in
                    st.query_map([], |r| Ok((r.get::<_, i64>(0)?, (r.get::<_, i64>(1)?, r.get::<_, Vec<u8>>(2)?))))?
                {
                    let (id, v) = r?;
                    have.insert(id, v);
                }
            }
            let mut todo: Vec<(i64, i64, String, Vec<u8>)> = Vec::new();
            let mut st = db.prepare("SELECT rowid, doc_id, text FROM chunks")?;
            for r in st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))? {
                let (id, doc, text) = r?;
                let h = chunk_hash(&text);
                match have.get(&id) {
                    Some((d, old)) if *old == h && *d == doc => stats.kept += 1,
                    _ => todo.push((id, doc, text, h)),
                }
            }
            (todo, stats)
        };
        let total = todo.len();
        for (done, batch) in todo.chunks(64).enumerate() {
            let texts: Vec<&str> = batch.iter().map(|t| t.2.as_str()).collect();
            let vs = emb.embed(&texts)?;
            let db = lock(&self.db);
            let tx = db.unchecked_transaction()?;
            for ((id, doc, _, h), mut v) in batch.iter().zip(vs) {
                embed::normalise(&mut v);
                let (scale, q) = vectors::quantise(&v);
                tx.execute(
                    "INSERT OR REPLACE INTO vec_chunks(chunk_id, doc_id, h, scale, v) VALUES(?1,?2,?3,?4,?5)",
                    params![id, doc, h, f64::from(scale), vectors::i8_to_bytes(&q)],
                )?;
            }
            tx.commit()?;
            stats.embedded += batch.len();
            if let Some(p) = progress {
                p(((done + 1) * 64).min(total), total);
            }
        }
        let db = lock(&self.db);
        let gen_: i64 = Self::meta(&db, "vec_gen").and_then(|s| s.parse().ok()).unwrap_or(0);
        Self::set_meta(&db, "vec_gen", &(gen_ + 1).to_string())?;
        Self::set_meta(&db, "vec_dim", &emb.dim().to_string())?;
        Ok(stats)
    }

    pub fn vector_count(&self) -> Result<i64, RetrievalError> {
        Ok(lock(&self.db).query_row("SELECT COUNT(*) FROM vec_chunks", [], |r| r.get(0))?)
    }

    /// How vectors are currently served (loads them if needed).
    pub fn vector_mode(&self) -> Result<VectorMode, RetrievalError> {
        self.refresh()?;
        Ok(lock(&self.cache).mode)
    }

    fn refresh(&self) -> Result<(), RetrievalError> {
        let db = lock(&self.db);
        let gen_: i64 = Self::meta(&db, "vec_gen").and_then(|s| s.parse().ok()).unwrap_or(0);
        let mut cache = lock(&self.cache);
        if cache.generation == gen_ {
            return Ok(());
        }
        let n: i64 = db.query_row("SELECT COUNT(*) FROM vec_chunks", [], |r| r.get(0))?;
        let dim: usize = Self::meta(&db, "vec_dim").and_then(|s| s.parse().ok()).unwrap_or(0);
        cache.generation = gen_;
        cache.resident = None;
        cache.mode = VectorMode::None;
        if n == 0 || dim == 0 {
            return Ok(());
        }
        if let Some(mut b) = vectors::Resident::try_new(&self.budget, usize::try_from(n).unwrap_or(0), dim) {
            let mut st = db.prepare("SELECT chunk_id, doc_id, scale, v FROM vec_chunks")?;
            for r in st.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?, r.get::<_, Vec<u8>>(3)?))
            })? {
                let (c, d, s, v) = r?;
                b.push(c, d, s as f32, &vectors::bytes_to_i8(&v));
            }
            cache.resident = Some(Arc::new(b.done()));
            cache.mode = VectorMode::Resident;
        } else {
            tracing::warn!("memory budget too small for {n} vectors; searching them from disk");
            cache.mode = VectorMode::Streaming;
        }
        Ok(())
    }

    fn dense_candidates(&self, q: &Query, hidden: &HashSet<i64>) -> Result<Option<Vec<i64>>, RetrievalError> {
        let Some(emb) = &self.embedder else { return Ok(None) };
        self.refresh()?;
        let (mode, resident) = {
            let c = lock(&self.cache);
            (c.mode, c.resident.clone())
        };
        if mode == VectorMode::None {
            return Ok(None);
        }
        let mut qv = emb.embed(&[q.text.as_str()])?.into_iter().next().unwrap_or_default();
        embed::normalise(&mut qv);
        let top = match resident {
            Some(r) if r.dim == qv.len() => r.search(&qv, q.candidates, hidden),
            Some(_) => return Ok(None),
            None => {
                let db = lock(&self.db);
                let mut st = db.prepare("SELECT chunk_id, doc_id, scale, v FROM vec_chunks")?;
                let mut top = vectors::TopK::new(q.candidates);
                for r in st.query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, f64>(2)?, r.get::<_, Vec<u8>>(3)?))
                })? {
                    let (c, d, s, v) = r?;
                    if hidden.contains(&d) || v.len() != qv.len() {
                        continue;
                    }
                    top.push(vectors::dot(&qv, &vectors::bytes_to_i8(&v), s as f32), c);
                }
                top.finish()
            }
        };
        Ok(Some(top.into_iter().filter(|(s, _)| *s > 0.0).map(|(_, c)| c).collect()))
    }

    fn lexical_candidates(&self, q: &Query) -> Result<Vec<i64>, RetrievalError> {
        let terms = query_terms(&q.text);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let vis = ibos_search::acl::visible_sql(self.policy, 3, q.principals.len());
        let sql = format!(
            "SELECT c.rowid FROM chunks c JOIN docs d ON d.id = c.doc_id WHERE chunks MATCH ?1 AND {vis} ORDER BY bm25(chunks, 1.0, 2.0, 1.0) LIMIT ?2"
        );
        let db = lock(&self.db);
        let clauses: Vec<String> = terms
            .iter()
            .map(|t| {
                let quoted = format!("\"{}\"", t.replace('"', ""));
                if !q.fuzzy || term_matches(&db, &quoted) {
                    return quoted;
                }
                let near = self.nearest_terms(&db, t);
                if near.is_empty() {
                    quoted
                } else {
                    format!(
                        "({})",
                        near.iter().map(|n| format!("\"{}\"", n.replace('"', ""))).collect::<Vec<_>>().join(" OR ")
                    )
                }
            })
            .collect();
        let mut st = db.prepare_cached(&sql)?;
        for joiner in [" AND ", " OR "] {
            let m = clauses.join(joiner);
            let mut args: Vec<rusqlite::types::Value> = vec![m.into(), (q.candidates as i64).into()];
            args.extend(q.principals.iter().map(|p| rusqlite::types::Value::from(p.clone())));
            let ids = st.query_map(params_from_iter(args), |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
            if !ids.is_empty() || terms.len() == 1 {
                return Ok(ids);
            }
        }
        Ok(Vec::new())
    }

    fn passage(&self, id: i64) -> Result<Option<Passage>, RetrievalError> {
        let db = lock(&self.db);
        let mut st = db.prepare_cached(
            "SELECT c.doc_id, d.title, d.path, c.page, c.text FROM chunks c JOIN docs d ON d.id=c.doc_id WHERE c.rowid=?1",
        )?;
        let mut rows = st.query_map([id], |r| {
            Ok(Passage {
                chunk_id: id,
                doc_id: r.get(0)?,
                title: r.get(1)?,
                path: r.get(2)?,
                page: r.get(3)?,
                text: r.get(4)?,
                score: 0.0,
                lexical_rank: None,
                dense_rank: None,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    // ---- search ---------------------------------------------------------------------------------------------------
    pub fn search(&self, q: &Query) -> Result<Outcome, RetrievalError> {
        let mut q = q.clone();
        q.principals.retain(|p| !p.contains('\0'));
        let q = &q;
        let mut notes = Vec::new();
        let hidden = {
            let db = lock(&self.db);
            self.hidden_docs(&db, &q.principals)?
        };
        let lexical = if q.mode == Mode::Dense { Vec::new() } else { self.lexical_candidates(q)? };
        let dense = if q.mode == Mode::Lexical {
            None
        } else {
            let d = self.dense_candidates(q, &hidden)?;
            if d.is_none() {
                notes.push("no vectors available: fell back to keyword search (run `index --embed`)".to_string());
            }
            d
        };
        let (mode_used, ranked): (Mode, Vec<(i64, f32)>) = match (&dense, q.mode) {
            (Some(d), Mode::Hybrid) => (Mode::Hybrid, fusion::rrf(&[(&lexical, 1.0), (d, 1.0)])),
            (Some(d), Mode::Dense) => (Mode::Dense, fusion::rrf(&[(d, 1.0)])),
            (None, Mode::Dense) => (Mode::Dense, Vec::new()),
            _ => (Mode::Lexical, fusion::rrf(&[(&lexical, 1.0)])),
        };
        let top = ranked.first().map_or(1.0, |r| r.1).max(f32::EPSILON);
        let mut passages = Vec::new();
        for (id, s) in ranked.into_iter().take(q.candidates) {
            if let Some(mut p) = self.passage(id)? {
                p.score = s / top;
                p.lexical_rank = lexical.iter().position(|x| *x == id).map(|i| i + 1);
                p.dense_rank = dense.as_ref().and_then(|d| d.iter().position(|x| *x == id)).map(|i| i + 1);
                passages.push(p);
            }
        }
        if q.rerank {
            for p in &mut passages {
                p.score = self.reranker.score(&q.text, &p.title, &p.text, p.score);
            }
            passages.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.chunk_id.cmp(&b.chunk_id)));
        }
        if q.dedupe {
            let texts: Vec<&str> = passages.iter().map(|p| p.text.as_str()).collect();
            let keep = dedupe::keep(&texts, 0.8);
            let mut i = 0;
            passages.retain(|_| {
                let k = keep.contains(&i);
                i += 1;
                k
            });
        }
        passages.truncate(q.limit);
        Ok(Outcome { passages, mode_used, notes })
    }

    /// Search, then pack the best passages into `budget_tokens` for a model prompt.
    pub fn context(
        &self,
        q: &Query,
        budget_tokens: usize,
        max_per_doc: usize,
    ) -> Result<(Outcome, Vec<Evidence>), RetrievalError> {
        let out = self.search(q)?;
        let ev = pack::pack(&out.passages, budget_tokens, max_per_doc, &ibos_ctx::ApproxTokenizer);
        Ok((out, ev))
    }
}

/// Does the index contain at least one passage matching this (already quoted) term?
fn term_matches(db: &Connection, quoted: &str) -> bool {
    db.query_row("SELECT 1 FROM chunks WHERE chunks MATCH ?1 LIMIT 1", [quoted], |_| Ok(())).is_ok()
}

/// Edit distance, giving up (returning `max + 1`) once it must exceed `max`.
pub(crate) fn edit_distance(a: &str, b: &str, max: usize) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > max {
        return max + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        if cur.iter().all(|d| *d > max) {
            return max + 1;
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The index's words, grouped by length, kept in memory so a misspelled word costs a few hundred distance checks, not a table scan.
/// Rebuilt when another connection (the indexer) commits, detected with `PRAGMA data_version`.
#[derive(Debug)]
struct Vocab {
    version: i64,
    by_len: std::collections::HashMap<usize, Vec<(String, i64)>>,
}

impl Retriever {
    /// Up to three indexed words close to `term`: one letter off for words of 5-7 letters, two for longer ones; none for
    /// shorter words (too many false matches). The most frequent candidates win.
    fn nearest_terms(&self, db: &Connection, term: &str) -> Vec<String> {
        let len = term.chars().count();
        let max = match len {
            0..=4 => return Vec::new(),
            5..=7 => 1,
            _ => 2,
        };
        let version: i64 = db.query_row("PRAGMA data_version", [], |r| r.get(0)).unwrap_or(-1);
        let mut cache = lock(&self.vocab);
        if cache.as_ref().is_none_or(|v| v.version != version) {
            let mut by_len: std::collections::HashMap<usize, Vec<(String, i64)>> = std::collections::HashMap::new();
            if let Ok(mut st) = db.prepare("SELECT term, doc FROM chunks_vocab")
                && let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            {
                for (w, docs) in rows.flatten() {
                    by_len.entry(w.chars().count()).or_default().push((w, docs));
                }
            }
            *cache = Some(Vocab { version, by_len });
        }
        let Some(vocab) = cache.as_ref() else { return Vec::new() };
        let mut found: Vec<(usize, i64, &str)> = Vec::new();
        for l in len.saturating_sub(max)..=len + max {
            for (cand, docs) in vocab.by_len.get(&l).into_iter().flatten() {
                let d = edit_distance(term, cand, max);
                if d <= max {
                    found.push((d, -docs, cand));
                }
            }
        }
        found.sort_unstable();
        found.into_iter().take(3).map(|f| f.2.to_owned()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::edit_distance;

    #[test]
    fn edit_distance_counts_insertions_deletions_and_substitutions() {
        assert_eq!(edit_distance("invoice", "invoise", 2), 1);
        assert_eq!(edit_distance("payment", "paymnt", 2), 1);
        assert_eq!(edit_distance("abc", "xyz", 1), 2);
        assert_eq!(edit_distance("a", "abcdef", 2), 3);
    }
}
