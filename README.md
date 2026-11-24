# Intelligent Browser Fast

A fast, local browser agent and document search engine in Rust. One 15 MB executable.

- **Browser agent**: do tasks in a real Chromium from plain English, sending the model compact page text instead of HTML.
- **Hybrid document search**: keyword (BM25) plus vectors, typo tolerance, per-document access control, answer checking.
- **Local first**: the index is one SQLite file on your machine. Only what you configure (a model endpoint, an embedding endpoint) leaves it.

> **Status: working prototype, tested on Linux.** 227 tests pass. Benchmarks use a scripted model and synthetic documents, so
> real-model success, real-embedding quality, and Windows and macOS are **not measured**. See [Honest limits](#honest-limits).

## What the measurements show

| | Python agent | This (Rust) |
|---|---|---|
| Prompt tokens, 4 tasks (sum of medians of 10 runs) | 95,259 | **5,841** (94% fewer) |
| Wall time, same tasks | 13.8 s | **2.3 s** (6.1x faster) |
| System prompt | 6,022 tokens | **242 tokens** |
| Typo queries, recall@10 (11,000 passages) | 0.3% | **99.7%** (fuzzy words) |
| Access-control leaks in 200 probes | n/a | **0** |

Method and caveats: [`bench/RESULTS.md`](bench/RESULTS.md). The scripted-model comparison isolates what the agent controls; it
does not show how a real model performs.

## Try it

```bash
cargo build --release                      # about 2 minutes; produces target/release/intelli-rust

intelli-rust index ~/Documents --embed                     # index files and compute vectors
intelli-rust search "invoise paymnt" --mode hybrid          # keyword | fuzzy | dense | hybrid
intelli-rust search "refund rules" --mode hybrid --as hr --context-tokens 800
intelli-rust eval judgments.json                            # compare modes on your own judged queries
intelli-rust                                                # search home page (web UI)
intelli-rust admin                                          # folders, indexing, AI-answer settings

intelli-rust distill https://example.com                    # what the model would see, in tokens
export OPENAI_API_KEY=...                                   # or ANTHROPIC_API_KEY, or a local model (docs/LOCAL-LLM.md)
intelli-rust run "open news.ycombinator.com and tell me the top story" --headless
intelli-rust doctor
```

Real semantic search needs a real embedding model:
`INTELLI_EMBED_URL=https://api.openai.com/v1 INTELLI_EMBED_MODEL=text-embedding-3-small INTELLI_EMBED_KEY=...`.
Without it the built-in hashing embedder is used: it finds typos and word forms, not synonyms.

The search index and settings use the **same files as the Python Intelli Browser OS**, so an index built by one is read by the other.

## Crates

| crate | purpose |
|---|---|
| `ibos-mem` | memory budget with RAII reservations, arenas, frame slabs, interning, process memory probe |
| `ibos-ctx` | token budgets, three-tier cache-friendly prompts, history compaction, artifacts, usage |
| `ibos-dom` | page distillation from the accessibility tree: numbered elements, repeats collapsed, offscreen counted |
| `ibos-cdp` | Chrome DevTools client: timeouts, bounded channels, process-group kill |
| `ibos-llm` | OpenAI-compatible and Anthropic clients, retries, key redaction, cache accounting |
| `ibos-agent` | agent loop, actions, guardrails (domains, risky clicks, card numbers), audit log |
| `ibos-search` | SQLite FTS5 index, extraction (txt, md, html, pdf, docx), settings |
| `ibos-retrieval` | fuzzy words, vectors, RRF, rerank, de-dupe, access control, evidence packing, grounding check, eval |
| `ibos-server` | local web UI: home page, admin, optional AI answers |
| `ibos-cli` | the `intelli-rust` binary |

Architecture and implementation approach: [`docs/IntelliRust-Technical-Architecture.docx`](docs/IntelliRust-Technical-Architecture.docx)
and [`.pptx`](docs/IntelliRust-Technical-Architecture.pptx). Crate-level notes: [`ARCHITECTURE.md`](ARCHITECTURE.md),
[`crates/ibos-retrieval/README.md`](crates/ibos-retrieval/README.md).

## Coming next

See [`ROADMAP.md`](ROADMAP.md). Highlights, all **planned, not built**: a secret vault so the model never sees passwords or cards ([design with diagrams](docs/security/VAULT-DESIGN.md)),
a fully-local mode, running your own CPU/GPU-quantized model as the agent's brain ([`docs/LOCAL-LLM.md`](docs/LOCAL-LLM.md)),
a local embedder and reranker, an optional Qdrant backend, hybrid search in the web UI, skills (record and replay), an MCP server.

## Design decisions worth knowing

- **Every prompt is stateless and contains the whole distilled page.** An earlier design sent only *changes* between steps. That
  is wrong when the prompt is rebuilt each step: the model cannot act on an element it was shown only in a previous prompt. It is
  now off by default (`ContextConfig::deltas`) and the engine instead adds a one-line "page unchanged" hint. The distilled page is
  small enough (see results) that this costs little.
- **Collapsing is lossy, so there is a way back.** Repeated structures (a product grid, a table, a menu) show the first six
  items and a `[N]<+>` marker. The `more` action lists the hidden items, optionally filtered (`"q"`) or paged (`"skip"`), as a
  one-off note in the next prompt.
- **Handles are the browser's own node ids.** Stable across steps and used directly by actions; no mapping table.
- **Typed text is never stored in history**, only its length; the audit log masks card numbers.
- **Risky actions are denied when nobody can answer.** Running unattended fails safe.

## Testing

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
IBOS_REQUIRE_BROWSER=1 cargo test --workspace --all-features
cargo deny check
```

Tests that need Chromium skip themselves when it is missing, unless `IBOS_REQUIRE_BROWSER=1` (CI sets it). The Python-compatibility
tests run only when `IBOS_PY_REPO` points at a checkout of the Python project. Highlights: real Chromium end to end with a local
website; no orphan browser processes; `dhat` heap guard; access control tested in every search mode; fake-server tests for the
embedding client.

Reproduce the benchmarks: `cargo run --release -p ibos-retrieval --example bench -- 3000` (search), and for the agent comparison
set `IBOS_PY_REPO` to the Python project and run `python3 bench/agent_bench.py --runs 10` then `python3 bench/summarize.py`.

## Honest limits

- Agent benchmark uses a scripted model: no real-model success rate or cost yet.
- No real embedding model was tested; the vector leg with the built-in embedder is weak on long passages. Hybrid did not beat keyword plus fuzzy in the synthetic test.
- Linux only so far. Windows and macOS are configured in CI but never run.
- The web UI still uses keyword search only; hybrid is CLI-only for now.
- Access control is enforced, not authenticated: pass only principals you have verified.
- Not ported from Python: the encrypted vault, skills, MCP server, live view, installers.
- Distillation rules are tuned on synthetic pages; clickable `div`s without an accessibility role are not seen yet.

## License

MIT. See [`LICENSE`](LICENSE).
