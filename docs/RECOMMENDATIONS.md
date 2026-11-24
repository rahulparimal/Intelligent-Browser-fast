# Recommendations: harmonized after review

**How this list was made.** I wrote a 29-item draft. Three independent reviewers (Claude Opus, read-only, each reading the code)
then criticised it from three angles: security and privacy, architecture and search quality, and product and agent reliability.
This is the merged result. Where they disagreed with the draft or with each other, the decision and the reason are stated.

## What all three reviewers agreed on

1. **Trust fixes come before new features.** The "safe by default" claim had real holes (below). A vault built on top of them would inherit them.
2. **Measure before building.** Both benchmarks are weak evidence: the agent one uses a scripted model, the search one uses nonsense words that make keyword search trivially perfect. Most of the draft's search and agent items cannot be judged until real evaluation exists.
3. **Pick a wedge.** The credible one is: *compact page text lets cheap or local models drive a browser.* Search and agent read as two products; the README leads with the agent.

## Holes the reviewers found in the code (verified, then fixed or queued)

| # | Hole | Status |
|---|---|---|
| G1 | A typed card number bypassed the card check if the text contained the literal `<secret>` | **Fixed**, test added |
| G2 | Risk check failed open: a click on a handle the agent had not been shown (revealed through `more`, or invented) got no check at all | **Fixed**: the name is looked up in the full page; unknown handles are refused. Three e2e tests fail without the fix |
| G3 | Allow-list enforced only on `go`; a link click could leave the allowed sites | **Partly fixed**: after click and back the browser is reset to a blank page if it left. Not fixed: sub-requests and the request that triggered the move are not blocked (needs CDP request interception) |
| - | Values typed into form fields (passwords, card numbers) were sent to the model in the next page snapshot | **Fixed**: fields labelled as secrets, bullet-only values and Luhn-valid numbers are shown as `[hidden]`. Not yet done: a full-run canary test over prompts, history, logs and stderr |
| G9 | Index database, data directory and audit log were created with default permissions | **Fixed** (0600 / 0700) |
| G6 | A saved API key followed a changed endpoint to the new host | **Fixed** |
| G4 | The web UI search and AI-answer routes ignore access lists; restricted passages can reach the remote LLM | **Queued (P0)**. "0 leaks in 200 probes" is true of the library, not of the web UI |
| G5 | The local web UI has no authentication | Queued (P0) |
| G7 | Chromium DevTools listens on a loopback TCP port without authentication | Queued (P0): use a pipe |
| G8 | `--no-sandbox` is used when running as root or when `INTELLI_NO_SANDBOX=1` | Queued (P0): refuse without an explicit flag |
| - | PDF/docx extraction runs in-process with only `catch_unwind`; no timeout or memory limit | Queued (P0): subprocess sandbox and fuzzing |

## Harmonized priorities

### P0: trust and safety (finish before anything user-visible is added)
1. ~~Fail-closed guard; `<secret>` bypass; origin check after click/back; field masking; file permissions; key vs endpoint~~ **done**
2. One search service used by the CLI, the web UI and later MCP, with access lists enforced everywhere; the AI-answer route must filter by access list. *Accept:* the web UI returns no restricted document; one code path.
3. Local web UI: per-launch token. DevTools over a pipe. Refuse `--no-sandbox` without a flag. *Accept:* request without the token gets 403; no DevTools TCP listener.
4. Document extraction in a sandboxed subprocess (timeout, memory limit, no network) plus fuzzing. *Accept:* an infinite-loop PDF, a zip bomb and a huge sparse file are each recorded as an error within the timeout.
5. Fully-local mode with *external* proof: run a whole task and index under `unshare -n` (loopback only) or a recording proxy. *Accept:* zero non-loopback connections from the process.
6. Page content marked as untrusted data in the prompt; domain policy enforced on every navigation and (via CDP interception) on sub-requests.
7. Canary-secret test over a full run (prompts, history, audit log, stderr). Then the vault ([design](security/VAULT-DESIGN.md)), in that order.

### P1: measurement (decides everything below)
8. **Real-model eval harness.** 50 tasks x 3 runs, success judged from what the site received (not the model's own "done"), against browser-use and Playwright MCP on the *same* model; report success with confidence interval, dollars per success, p50 time, failure taxonomy, pass^k. Replace the README headline with it. Target to test: match browser-use's success at 30% or less of its tokens.
9. Independent completion check; report the false-"done ok" rate (today success is the model's own claim).
10. **Real retrieval eval.** At least two public BEIR subsets plus 300 graded queries over a realistic local corpus, with typo, synonym and paraphrase slices and confidence intervals. Needed before any embedder or HNSW decision.
11. Distillation fidelity: about 100 real pages, annotate the target element, measure "present in the distilled output or reachable with `more`".
12. CI regression gates: fail on a 15% worse retrieval p95, distill time or heap peak.

### P2: agent reliability
13. Missing actions: key press (Enter), select option, hover, wait, file upload; follow new tabs; handle JavaScript dialogs (an `alert()` can stall a run today); iframe accessibility trees.
14. Postconditions (a click must change the page; read typed values back), loop detector, event-driven settle instead of a fixed 150 ms wait.
15. Per-step trace (page, prompt, reply, screenshot) with a viewer; `--max-cost` in dollars; an "ask the user" action.
16. Minimum session support: `--user-data-dir` and attaching to an existing Chrome. Persistent logins opt-in per site only.

### P3: search quality and speed
17. Default to keyword + fuzzy unless a real embedding model is configured. **Done in the CLI** (`--mode auto`).
18. In-process local embedder (ONNX) and tuned fusion weights, chosen by the eval in item 10. *Accept:* beats keyword + fuzzy by 5 nDCG points on paraphrases, loses under 0.5 on exact queries, no network.
19. Faster fuzzy: **partly done** (vocabulary cached: about 30 ms to about 10 ms, same quality). Next: FST or SymSpell for under 3 ms.
20. Batch the passage fetch, cache the access-list per caller, connection pool, avoid loading all vectors on every CLI start.
21. Structure-aware chunking with overlap and heading prefix; OCR and more formats; 1-bit prefilter before HNSW; HNSW only past about 200,000 passages; an entailment check for answer grounding.

### P4: product
22. MCP server with authentication and the same guard profiles, plus stable JSON output.
23. First run under 5 minutes; `doctor` checks Chromium, the key and that the model answers.
24. Local small-model path: constrained decoding through an OpenAI-compatible local server first; in-process only if latency data justifies it. (Waiting for your model details.)
25. macOS binary and Homebrew first; installers and auto-update later (signed manifests, opt-in, no telemetry).

## Decisions where the review changed my draft

| Draft said | Decision | Why |
|---|---|---|
| Qdrant as an optional vector backend | **Dropped.** If more is needed, use `sqlite-vec` | A second datastore breaks the one-file design and the rule that access lists are applied before ranking |
| Vault first | Guard fixes and field masking first, then vault | The vault would inherit the holes; masking was leaking typed values already |
| Hybrid as default | Keyword + fuzzy by default | Measured: the built-in vectors lowered exact-query quality |
| Hash-chained audit log | Do file permissions and secret scanning first; chain only with an external anchor | A chain written by the same process only catches careless edits |
| Real embedder first | Real eval first | Without judged data nobody can tell whether it helps |
| Fully-local mode as a unit test | Prove it from outside the process | Chromium is itself the network |
| Skills, parallel agents, plugin signing, installers | **Deferred or cut** | The agent is not yet proven; there are no users to share with |

## Status of this list
Phase 1 (item 1, 17, part of 19) is implemented and tested: 235 tests pass including real-browser end-to-end tests, `clippy -D warnings`
and `cargo deny` are clean. The Word document and deck in this folder were built before phase 1: they describe a fuzzy latency of
about 30 ms (now about 10 ms) and do not mention the guard fixes. Everything else above is planned, not built.
