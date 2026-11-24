# ibos-retrieval

Hybrid retrieval on top of the `ibos-search` FTS5 index: typo-tolerant keywords, vector search, rank fusion, reranking,
access control, evidence packing for model prompts, a grounding check for answers, and an evaluation harness.

```
query ─► keyword (BM25 + fuzzy words) ──┐
     └─► embed ─► vectors (int8) ───────┴─► RRF fusion ─► rerank ─► de-duplicate ─► passages ─► evidence pack ─► grounding check
              access control is applied inside both retrievers, before any ranking
```

## What is in it

| Part | What it does | Notes |
|---|---|---|
| `Embedder` trait | text to vectors | `HashingEmbedder` (no model), `RemoteEmbedder` (any OpenAI-compatible `/embeddings`) |
| Vector store | int8 vectors in SQLite table `vec_chunks`, exact search | held in RAM under `MemoryBudget`, else streamed from disk (same answers) |
| Fuzzy words | query words that match nothing are replaced by the nearest indexed words (1 letter off for 5-7 letter words, 2 for longer) | uses `fts5vocab`; fixes typos |
| Fusion | reciprocal rank fusion, K=60 | compares ranks, not scores, so BM25 and cosine need no calibration |
| Reranker | `HeuristicReranker`: term coverage, proximity, phrase, title | deterministic, no model; `Reranker` trait for a model-based one |
| De-duplication | 3-word shingles, Jaccard >= 0.8 | the better-ranked copy stays |
| Access control | `doc_acl(path, principal)`; `Policy::Open` or `Closed` | filtered in SQL / before scoring, so hidden text cannot appear or change the order |
| Evidence packing | best passages inside a token budget, at most N per document | last passage is trimmed to fit |
| Grounding check | per sentence: content words and every number must appear in the cited evidence | lexical only |
| Evaluation | nDCG@10, MRR, recall@10, ACL leaks, p50/p95 latency | `intelli-rust eval`, `examples/bench.rs` |

Storage is additive: the Python-compatible tables are untouched. New tables: `vec_chunks`, `doc_acl`, and the virtual table
`chunks_vocab`. Vectors are matched to passages by a hash of the passage text, so a changed file is re-embedded and a reused
row id can never keep a stale vector. Changing the embedder discards the old vectors.

## Use

```bash
intelli-rust index ~/Documents --embed                 # also computes vectors
intelli-rust search "invoise paymnt" --mode hybrid     # keyword, fuzzy, dense or hybrid
intelli-rust search "refund rules" --mode hybrid --as hr --context-tokens 800
intelli-rust eval judgments.json                       # compare modes on your own judged queries
```

Real semantic search needs a real embedding model:

```bash
export INTELLI_EMBED_URL=https://api.openai.com/v1 INTELLI_EMBED_MODEL=text-embedding-3-small INTELLI_EMBED_KEY=...
```

## What it does not do

- **Synonyms and paraphrase need a real embedder.** The built-in hashing embedder matches word forms and character patterns;
  it does not know that "car" and "automobile" are related (a test asserts this). Remote embedders are tested against a fake
  server for protocol, batching, key handling and errors, not against a real model for quality.
- **Access control is enforced, not authenticated.** Pass only principals you have verified. Documents with no ACL rows are
  visible under `Policy::Open`; use `Policy::Closed` for deny-by-default.
- **The grounding check is lexical.** It catches invented numbers, names and citations; a sentence that reuses the right words
  to say the opposite still passes (a test documents this).
- **Search is exact and in-process.** Fine to hundreds of thousands of passages; beyond that an approximate index would be
  needed. Not built.
- The admin web page and HTTP API in `ibos-server` still use the keyword search only.

Measured results on a synthetic corpus: `bench/retrieval_results.md` (and `bench/RESULTS.md`).
