# Measured results

Everything here was measured on one Linux machine (a container) with the code in this repository. Re-run the commands to check
them. Where a number is an estimate it says so.

## Page representation: intelli-rust vs the Python agent (browser-use)

`python3 intelli-rust/bench/compare_tokens.py` loads six local pages over HTTP in both agents and measures what each would send
the model for the page. Both sides use the same estimate (characters / 4), so the ratio is meaningful. The script asserts that
the Python agent really loaded each page.

| page | what it is | Python | Rust | smaller by | browser data |
|---|---|---|---|---|---|
| login | 3 fields, 3 buttons, a link | 167 | 51 | 69% | 17 KB |
| form | select, radios, checkbox, textarea | 273 | 114 | 58% | 32 KB |
| products | 120 product cards | 1,897 | 245 | 87% | 666 KB |
| table | 60-row staff table | 1,798 | 155 | 91% | 390 KB |
| menu | 144-link mega menu | 574 | 148 | 74% | 429 KB |
| article | long article with headings | 3,699 | 694 | 81% | 175 KB |
| **total** | | **8,408** | **1,407** | **83%** | |

Distilling took 0.1 to 2.5 ms per page (release build).

**Read this honestly.** The Rust page text is smaller partly because it is *less detailed* (no element attributes or ids) and
because repeated items are collapsed behind a `more` action. Whether a real model completes tasks as reliably from the smaller
page has **not been measured**: that needs real model calls (`intelli bench` on the Python side; a Rust equivalent is not
written). Treat 83% as the size of the input, not as a proven cost saving.

## Static prompt

| | tokens (chars / 4) |
|---|---|
| Python agent system prompt | 6,022 |
| intelli-rust system prompt | 242 |

The Rust prompt is a small action vocabulary. It is smaller because it is simpler, which may also make a weaker model less
reliable. Unmeasured with real models.

## Footprint (release build)

| | intelli-rust | Python (packaged) |
|---|---|---|
| Executable | 14.8 MB (one file, includes SQLite) | 151 MB folder |
| `--help` start-up (median of 10) | 2.1 ms | about 180 ms packaged, about 59 ms from a normal install |
| Resident memory, `ui` idle | about 10 MB | about 36 MB (launcher) |

## Memory behaviour (regression-tested)

`cargo test -p ibos-dom --test mem -- --nocapture` measures heap use with `dhat` while distilling the 680 KB products page:

- peak heap growth 840 KB (1.24x the input)
- after 30 steps, live heap grew by less than 4 KB between step 3 and step 30 (no leak)
- what stays allocated (69.9 KB) is the per-step arena's reusable chunk (69.6 KB)

## Search

Ranking: same corpus and gold queries as the Python tests, 20 of 20 correct at rank 1, mean reciprocal rank 1.0.

Speed: `cargo run --release -p ibos-search --example perf` (2,000 generated documents, 6.2 MB), compared with the earlier Python
measurement on the same workload:

| | Rust | Python |
|---|---|---|
| first index | 786 ms | 1,100 ms |
| re-scan, nothing changed | 21 ms | 100 ms |
| one query | 13.2 ms | 15.1 ms |

Both use the same SQLite FTS5 engine, so queries are about the same speed; the gain is in the surrounding code (file walking,
hashing, parsing). Do not expect search to be the reason to move to Rust.

## End-to-end agent comparison (measured, simulated model)

`agent_bench.py` runs the same four tasks (login, form, product lookup, table lookup) on the same local website and the same
Chromium, once with the Python agent and once with `intelli-rust`. The model is a deterministic scripted "oracle" for both, so
the comparison isolates what the agent controls: what it sends to the model, how many calls it needs, and its own cost.
Success is checked from what the website actually received. Tokens are estimated as chars/4 for both agents.
"Cache-adjusted" assumes the static prefix is served from a provider cache at 10% of price after the first call (an assumption).

Median of 10 runs per cell. Success = the website received the right thing.

| task | agent | success | model calls | prompt tokens | prompt tokens, cache-adjusted | wall time | agent memory | browser memory |
|---|---|---|---|---|---|---|---|---|
| login | python | 10/10 | 4 | 25,316 | 9,057 | 3.6 s | 203 MB | 833 MB |
| login | rust | 10/10 | 4 | 1,304 | 651 | 0.6 s | 201 MB | 1047 MB |
| form | python | 10/10 | 5 | 31,580 | 9,901 | 3.9 s | 202 MB | 887 MB |
| form | rust | 10/10 | 5 | 1,638 | 767 | 0.6 s | 200 MB | 1045 MB |
| product | python | 10/10 | 3 | 22,418 | 11,578 | 3.7 s | 202 MB | 881 MB |
| product | rust | 10/10 | 4 | 1,710 | 1,057 | 0.7 s | 203 MB | 1061 MB |
| lookup | python | 10/10 | 2 | 15,945 | 10,525 | 2.6 s | 201 MB | 903 MB |
| lookup | rust | 10/10 | 3 | 1,189 | 753 | 0.4 s | 199 MB | 1034 MB |

Across the four tasks (sum of medians): prompt tokens 95,259 vs 5,841 (94% fewer); cache-adjusted 41,061 vs 3,228 (92% fewer); wall time 13.8 s vs 2.3 s (6.1x faster)

System prompt (static prefix): python 6022 tokens; rust 242 tokens

Caveats, stated plainly:
- This does **not** measure real-model accuracy, latency, or dollar prices; a real model may take more or fewer steps.
- Completion tokens are a lower bound (the oracle emits short, ideal actions) and are the same for both agents.
- Python's state already includes elements far below the fold, so it needs fewer calls on product and lookup (3 and 2 vs 4 and 3).
- Browser memory is higher with the Rust run here (about 1.0 GB vs 0.85 GB); agent memory is the same (~200 MB, dominated by
  the process baseline in this harness). Rust shows no memory advantage in this test.
- The wall-time gap was not broken down by cause (process startup, state building, per-step overhead).

## Hybrid retrieval (measured, synthetic corpus)

`cargo run --release -p ibos-retrieval --example bench -- 3000` generates 3,000 documents, each with three rare key words, and
asks for them exactly, with a letter dropped or swapped, and with different word endings. One in ten documents is restricted
and probed by a caller without access. Relevant = the one document the words came from.

```text
corpus: 3000 documents, 11000 passages; FTS5 index built in 2.99 s
embedded 11000 passages (hashing, 384 dims, int8) in 3.09 s
vector memory held: 4.2 MiB for 11000 vectors

### exact words (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 1.000 | 1.000 | 1.000 | 0 | 0.87 ms | 1.45 ms |
| keyword + fuzzy words | 1.000 | 1.000 | 1.000 | 0 | 1.00 ms | 1.85 ms |
| dense (hashing) | 0.536 | 0.488 | 0.690 | 0 | 7.69 ms | 10.45 ms |
| hybrid (RRF) | 0.982 | 0.975 | 1.000 | 0 | 7.29 ms | 9.60 ms |
| hybrid + fuzzy + rerank | 1.000 | 1.000 | 1.000 | 0 | 9.65 ms | 14.10 ms |

### typos (one letter dropped or swapped per word) (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 0.003 | 0.003 | 0.003 | 0 | 0.93 ms | 1.96 ms |
| keyword + fuzzy words | 0.987 | 0.983 | 0.997 | 0 | 10.68 ms | 16.02 ms |
| dense (hashing) | 0.024 | 0.016 | 0.050 | 0 | 6.69 ms | 9.09 ms |
| hybrid (RRF) | 0.027 | 0.020 | 0.053 | 0 | 7.40 ms | 10.25 ms |
| hybrid + fuzzy + rerank | 0.928 | 0.906 | 0.993 | 0 | 18.22 ms | 27.21 ms |

### different word endings (300 queries)

| configuration | nDCG@10 | MRR | recall@10 | ACL leaks | p50 | p95 |
|---|---|---|---|---|---|---|
| keyword (BM25) | 1.000 | 1.000 | 1.000 | 0 | 0.90 ms | 1.21 ms |
| keyword + fuzzy words | 1.000 | 1.000 | 1.000 | 0 | 1.04 ms | 1.51 ms |
| dense (hashing) | 0.536 | 0.488 | 0.690 | 0 | 6.46 ms | 8.43 ms |
| hybrid (RRF) | 0.982 | 0.975 | 1.000 | 0 | 6.95 ms | 9.57 ms |
| hybrid + fuzzy + rerank | 1.000 | 1.000 | 1.000 | 0 | 7.63 ms | 9.66 ms |

### access control: 200 probes for restricted documents by a caller without access

- keyword (BM25): 0 leaks
- keyword + fuzzy words: 0 leaks
- dense (hashing): 0 leaks
- hybrid (RRF): 0 leaks
- hybrid + fuzzy + rerank: 0 leaks
```

How to read this:
- **Typos:** fuzzy words take keyword search from 0.3% to 99.7% recall. This is the real gain. It costs about 10 ms per query here (it was about 30 ms before the vocabulary was cached in memory; the target is under 5 ms).
- **Dense search with the built-in hashing embedder is weak on long passages** (0.54 nDCG@10 on exact words): 60 common
  words per paragraph drown three key words. Hybrid fusion therefore slightly *hurts* exact queries (0.982 vs 1.000) until the
  reranker restores them (1.000), and it does not beat keyword + fuzzy on typos (0.928 vs 0.987).
- **No access-control leaks** in any mode across 200 probes (also covered by unit tests).
- **Not measured:** synonyms, paraphrase, real embedding models, real documents, corpora beyond 11,000 passages.
  Expect a real embedder to change the dense and hybrid rows; do not infer them from these.

## Why these search numbers are weak evidence

Review found that this benchmark cannot tell good ranking from bad: queries are random nonsense syllables that also appear in the
document title, so keyword search is trivially perfect; "different word endings" is solved by the FTS5 stemmer, not by anything
added here; and nonsense words hide the real failure mode of fuzzy matching (in English, one edit away often reaches another real
word). Treat the typo result as "fuzzy works on typos of rare words", not as a quality claim. A real judged corpus is the first
item in [`../ROADMAP.md`](../ROADMAP.md).

## Not measured

Real-model task success, cost per successful task, cache hit rates against real providers, Windows and macOS behaviour, and
memory of a full browser run (Chromium itself dominates).
