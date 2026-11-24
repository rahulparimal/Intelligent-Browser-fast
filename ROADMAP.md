# Roadmap

Everything below is **planned, not built**, unless marked done. Order is a proposal; change it.

## Done
- Browser agent in Rust: compact page text, `more` expansion, guardrails, audit log, bounded memory, no orphan browsers
- Search: SQLite FTS5 index (compatible with the Python version), web UI and admin page
- Hybrid retrieval (`ibos-retrieval`): fuzzy words, vectors, rank fusion, rerank, per-document access control, evidence packing, answer check, `eval`
- Benchmarks: [agent](bench/RESULTS.md) and [search](bench/retrieval_results.md), with their limits stated

## Next: trust and privacy
| Feature | What it gives you |
|---|---|
| **Secret vault** (`ibos-vault`), [design with diagrams](docs/security/VAULT-DESIGN.md) | Fill forms with passwords and cards that the model never sees. OS keychain (encrypted-file fallback), placeholders like `{{cred:bank.password}}`, bound to one domain, confirmation before filling, never indexed, never logged |
| **Fully local mode** | One switch that refuses any network call except to the browser and a local model, with a test that proves it |
| **Prompt-injection shield** | Page text is marked as untrusted data; actions that came from page text need confirmation |

## Next: local models (your custom quantized LLM)
| Feature | What it gives you |
|---|---|
| **Local LLM backend** | Run your CPU/GPU-quantized model as the agent's brain with no data leaving the machine. See [docs/LOCAL-LLM.md](docs/LOCAL-LLM.md) for what is needed |
| **Local embedder** | Semantic search without a network call (the `Embedder` trait is ready) |
| **Local reranker** | A small cross-encoder behind the `Reranker` trait |
| **Model-aware prompts** | Shorter prompts and stricter action formats for small models; measured, not assumed |
| **Task benchmark with real models** | Success rate, tokens and cost per task: your model vs hosted models |

## Next: search
| Feature | What it gives you |
|---|---|
| **Qdrant backend (optional)** | Vectors behind a trait; built-in store stays the default, Qdrant for team servers and millions of passages |
| **Hybrid search in the web UI** | Mode switch, access-list editor, "embed" button in the admin page |
| **Faster typo matching** | Cached vocabulary or trigram index: under 5 ms instead of about 30 ms |
| **More formats** | pptx, xlsx, email, images with OCR |
| **Watch folders** | Re-index on change |

## Later: agent
| Feature | What it gives you |
|---|---|
| **Skills** | Record a task once, replay it without the model, fall back to the model when the page changed |
| **MCP server** | Use the agent and the search from other AI tools |
| **Live view** | Watch the agent work; take over and hand back |
| **Scheduled tasks and page watchers** | "Tell me when the price drops" |
| **Windows and macOS** | CI matrix with browser tests; installers |

## Principles for every item
Measured before claimed. A feature that cannot be tested honestly stays on this list.
