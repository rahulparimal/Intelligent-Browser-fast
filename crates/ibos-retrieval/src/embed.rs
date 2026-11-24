//! Turning text into vectors.
//!
//! [`HashingEmbedder`] is built in and needs no model: it hashes words, word pairs and character trigrams into a fixed-size
//! vector. It finds typos, word endings and shared phrasing. It does **not** know that "car" and "automobile" are related.
//! For meaning, use [`RemoteEmbedder`] (any OpenAI-compatible `/embeddings` endpoint) or implement [`Embedder`].

use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding request failed: {0}")]
    Request(String),
    #[error("embedding service returned an unexpected answer: {0}")]
    Response(String),
}

/// Anything that maps text to fixed-length vectors. Implementations must return one vector per input, in order, all of
/// length [`dim`](Self::dim). Vectors need not be normalised.
pub trait Embedder: Send + Sync + fmt::Debug {
    /// Identifies the model. Vectors from different ids are never mixed in one index.
    fn id(&self) -> String;
    fn dim(&self) -> usize;
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// Model-free embedder (feature hashing). See the module note for what it can and cannot do.
#[derive(Debug, Clone, Copy)]
pub struct HashingEmbedder {
    dim: usize,
}

impl Default for HashingEmbedder {
    fn default() -> Self {
        Self { dim: 384 }
    }
}

impl HashingEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.clamp(32, 4096) }
    }

    fn add(self, v: &mut [f32], feature: &[u8], weight: f32) {
        let h = blake3::hash(feature);
        let b = h.as_bytes();
        let idx = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize % self.dim;
        let sign = if b[4] & 1 == 0 { 1.0 } else { -1.0 };
        v[idx] += sign * weight;
    }
}

/// Light stemming so "invoices", "invoiced" and "invoicing" share features. Not linguistic; only trims common endings.
pub(crate) fn stem(w: &str) -> String {
    let n = w.chars().count();
    let mut out = w;
    for suf in ["ing", "ed", "es", "s"] {
        if n > suf.len() + 3 && w.ends_with(suf) {
            out = &w[..w.len() - suf.len()];
            break;
        }
    }
    // "invoice" and "invoic(ing)" meet at the same stem
    if out.len() > 4 {
        out = out.strip_suffix('e').unwrap_or(out);
    }
    out.to_string()
}

pub(crate) fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

impl Embedder for HashingEmbedder {
    fn id(&self) -> String {
        format!("hashing-v1-{}", self.dim)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0.0_f32; self.dim];
                let ws: Vec<String> = words(t).iter().map(|w| stem(w)).collect();
                for (i, w) in ws.iter().enumerate() {
                    self.add(&mut v, format!("w:{w}").as_bytes(), 1.0);
                    if let Some(n) = ws.get(i + 1) {
                        self.add(&mut v, format!("b:{w} {n}").as_bytes(), 0.7);
                    }
                    let padded: Vec<char> = format!("^{w}$").chars().collect();
                    for tri in padded.windows(3) {
                        let s: String = tri.iter().collect();
                        self.add(&mut v, format!("c:{s}").as_bytes(), 0.35);
                    }
                }
                normalise(&mut v);
                v
            })
            .collect())
    }
}

pub(crate) fn normalise(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

/// Client for an OpenAI-compatible `POST {base_url}/embeddings` endpoint (OpenAI, Azure-style gateways, Ollama, vLLM...).
///
/// The API key is sent only in the `Authorization` header and is never printed (`Debug` hides it).
pub struct RemoteEmbedder {
    base_url: String,
    model: String,
    key: Option<String>,
    dim: usize,
    batch: usize,
}

#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for RemoteEmbedder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteEmbedder")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("key", &self.key.as_ref().map(|_| "<hidden>"))
            .field("dim", &self.dim)
            .finish()
    }
}

impl RemoteEmbedder {
    /// Probes the endpoint once to learn the vector length.
    pub fn connect(base_url: &str, model: &str, key: Option<String>) -> Result<Self, EmbedError> {
        let mut e = Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            key,
            dim: 0,
            batch: 64,
        };
        let probe = e.call(&["dimension probe"])?;
        e.dim = probe
            .first()
            .map(Vec::len)
            .filter(|d| *d > 0)
            .ok_or_else(|| EmbedError::Response("empty vector".into()))?;
        Ok(e)
    }

    fn call(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let body = serde_json::json!({ "model": self.model, "input": texts });
        let url = format!("{}/embeddings", self.base_url);
        // reqwest's blocking client must not run on an async runtime thread, so use a short-lived plain thread.
        let reply: Result<serde_json::Value, String> = std::thread::scope(|s| {
            s.spawn(|| {
                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_mins(1))
                    .build()
                    .map_err(|e| e.to_string())?;
                let mut req = client.post(&url).json(&body);
                if let Some(k) = &self.key {
                    req = req.bearer_auth(k);
                }
                let r = req.send().map_err(|e| e.without_url().to_string())?;
                let status = r.status();
                if !status.is_success() {
                    return Err(format!("HTTP {status}"));
                }
                r.json::<serde_json::Value>().map_err(|e| e.to_string())
            })
            .join()
            .unwrap_or_else(|_| Err("worker thread panicked".into()))
        });
        let v = reply.map_err(EmbedError::Request)?;
        let data = v["data"].as_array().ok_or_else(|| EmbedError::Response("no `data` array".into()))?;
        let mut rows: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());
        for (i, d) in data.iter().enumerate() {
            let idx = d["index"].as_u64().map_or(i, |x| x as usize);
            let emb = d["embedding"]
                .as_array()
                .ok_or_else(|| EmbedError::Response("missing `embedding`".into()))?
                .iter()
                .map(|x| x.as_f64().map(|f| f as f32).ok_or_else(|| EmbedError::Response("non-numeric value".into())))
                .collect::<Result<Vec<f32>, _>>()?;
            rows.push((idx, emb));
        }
        rows.sort_by_key(|r| r.0);
        if rows.len() != texts.len() {
            return Err(EmbedError::Response(format!("asked for {} vectors, got {}", texts.len(), rows.len())));
        }
        Ok(rows.into_iter().map(|r| r.1).collect())
    }
}

impl Embedder for RemoteEmbedder {
    fn id(&self) -> String {
        format!("remote:{}:{}", self.model, self.dim)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut out = Vec::with_capacity(texts.len());
        for part in texts.chunks(self.batch) {
            let vs = self.call(part)?;
            if vs.iter().any(|v| v.len() != self.dim) {
                return Err(EmbedError::Response("vector length changed".into()));
            }
            out.extend(vs);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cos(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn vectors_are_unit_length_and_deterministic() {
        let e = HashingEmbedder::default();
        let a = e.embed(&["Refund policy for damaged goods"]).unwrap();
        let b = e.embed(&["Refund policy for damaged goods"]).unwrap();
        assert_eq!(a, b);
        assert!((cos(&a[0], &a[0]) - 1.0).abs() < 1e-4);
        assert_eq!(a[0].len(), e.dim());
    }

    #[test]
    fn word_endings_and_typos_stay_close_but_unrelated_text_does_not() {
        let e = HashingEmbedder::default();
        let v = e
            .embed(&[
                "invoice payment terms",
                "invoicing payments term",
                "invoise paymnt terms",
                "volcano eruption history",
            ])
            .unwrap();
        assert!(cos(&v[0], &v[1]) > 0.8, "endings: {}", cos(&v[0], &v[1]));
        assert!(cos(&v[0], &v[2]) > 0.3, "typos: {}", cos(&v[0], &v[2]));
        assert!(cos(&v[0], &v[3]) < 0.15, "unrelated: {}", cos(&v[0], &v[3]));
    }

    #[test]
    fn it_does_not_pretend_to_know_synonyms() {
        let e = HashingEmbedder::default();
        let v = e.embed(&["car", "automobile"]).unwrap();
        assert!(cos(&v[0], &v[1]) < 0.3);
    }

    #[test]
    fn empty_text_gives_a_zero_vector_not_nan() {
        let v = HashingEmbedder::default().embed(&[""]).unwrap();
        assert!(v[0].iter().all(|x| *x == 0.0));
    }

    #[test]
    fn debug_hides_the_key() {
        let e = RemoteEmbedder {
            base_url: "http://x".into(),
            model: "m".into(),
            key: Some("sk-secret".into()),
            dim: 3,
            batch: 8,
        };
        assert!(!format!("{e:?}").contains("sk-secret"));
    }
}
