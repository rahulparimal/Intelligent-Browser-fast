const fs = require('fs');
const { d, C, FONT, P, H1, H2, H3, bullets, table, gap, callout, code, figure, W } = require('./lib');
const { Document, Packer, Paragraph, TextRun, Header, Footer, PageNumber, AlignmentType, LevelFormat, TabStopType, BorderStyle, PageBreak } = d;
const F = (n) => __dirname + '/' + n;
const bullet = (ref) => ({ reference: ref, levels: [{ level: 0, format: LevelFormat.BULLET, text: '•', alignment: AlignmentType.LEFT, style: { paragraph: { indent: { left: 540, hanging: 270 } } } }] });

const cover = [
  new Paragraph({ spacing: { before: 2200, after: 120 }, children: [new TextRun({ text: 'INTELLI BROWSER OS', font: FONT, size: 22, bold: true, color: C.blue, characterSpacing: 60 })] }),
  new Paragraph({ spacing: { after: 160 }, children: [new TextRun({ text: 'intelli-rust: technical architecture and implementation approach', font: FONT, size: 56, bold: true, color: C.ink })] }),
  new Paragraph({ spacing: { after: 320 }, children: [new TextRun({ text: 'A local Rust browser agent and document search engine: how it is built, how it was tested, and what the measurements show', font: FONT, size: 26, color: C.muted })] }),
  new Paragraph({ border: { top: { style: BorderStyle.SINGLE, size: 8, color: C.blue, space: 10 } }, spacing: { before: 200, after: 80 }, children: [new TextRun({ text: 'Technical document  |  23 November 2026', font: FONT, size: 20, color: C.ink })] }),
  P('Status: implemented and tested on Linux. Every number in section 7 was measured by the scripts in the repository. Real-model task success, real embedding quality, and Windows and macOS behaviour have not been measured; section 8 lists what that means.', { size: 20, color: C.muted }),
  new Paragraph({ spacing: { before: 400, after: 80 }, children: [new TextRun({ text: 'Contents', font: FONT, size: 22, bold: true })] }),
  ...['1  Summary', '2  System architecture', '3  The browser agent', '4  The search engine', '5  Implementation approach', '6  Quality and testing', '7  Measured results', '8  Limits and what is not verified', '9  Roadmap', 'Appendix  CLI, configuration, storage'].map((t) => new Paragraph({ spacing: { after: 30 }, children: [new TextRun({ text: t, font: FONT, size: 21, color: C.muted })] })),
  new Paragraph({ children: [new PageBreak()] }),
];

const body = [
  H1('1  Summary'),
  P('`intelli-rust` is a single small binary (15 MB release build) that does two jobs: it drives a real Chromium browser from plain-English tasks, and it searches your own documents. It reimplements the Python Intelli Browser OS core in Rust, keeps the on-disk search format compatible with the Python version, and adds a hybrid search engine on top.'),
  P('Design goals, in priority order:'),
  ...bullets(['**Few tokens per task.** The model sees a compact distillation of the page, not raw HTML, and a short fixed system prompt.', '**Bounded memory.** Large buffers are paid for from one `MemoryBudget`; when it says no, the code degrades instead of allocating.', '**Safe by default.** Domain rules, risky-click detection, card-number masking, an audit log, and no orphaned browser processes.', '**Search that respects access.** Per-document access lists are applied before ranking, so hidden text cannot appear or influence order.', '**Honest measurement.** Each claim in this document is tied to a script and a stated limit.']),
  gap(60),
  callout('What was measured', [
    'On four browser tasks with the same website, browser and a scripted model, `intelli-rust` sent **94% fewer prompt tokens** than the Python agent (5,841 vs 95,259, sum of medians over 10 runs) and finished **6.1x faster** (2.3 s vs 13.8 s). Both succeeded in 40 of 40 runs.',
    'On a synthetic corpus of 11,000 passages, fuzzy keyword matching raised recall on misspelled queries from **0.3% to 99.7%**, and **0 of 200** access-control probes leaked a restricted document in any mode.',
  ], 'green'),
  gap(60),
  callout('What was not measured', [
    'How well a real model completes tasks with either agent, real dollar cost, search quality on real documents or with a real embedding model, and behaviour on Windows or macOS. The vector search with the built-in embedder matches word forms and typos, not meaning.',
  ], 'amber'),

  H1('2  System architecture'),
  P('The workspace has ten crates. Dependencies point downward: the two front ends use the agent and search crates, which use the infrastructure crates. No crate uses `unsafe` (the workspace forbids it).'),
  ...figure(F('fig_arch.png'), 600, 'Figure 1. Crates and their dependencies'),
  table([1700, 5560, 2100], ['Crate', 'Responsibility', 'Lines of Rust'], [
    ['`ibos-mem`', 'Memory budget with RAII reservations, arenas, per-step frame slabs, string interning, process memory probe', '547'],
    ['`ibos-ctx`', 'Token counting, three-tier prompt layout, history compaction, artifacts, usage accounting', '1,441'],
    ['`ibos-dom`', 'Turns the accessibility tree and DOM snapshot into compact numbered page text', '653'],
    ['`ibos-cdp`', 'Chrome DevTools Protocol client: bounded channels, timeouts, process-group kill', '861'],
    ['`ibos-llm`', 'OpenAI-compatible and Anthropic clients, cache-aware requests, retries, usage', '567'],
    ['`ibos-agent`', 'The step loop, action parsing, guardrails, audit log, `more` expansion', '1,003'],
    ['`ibos-search`', 'Extraction (txt, md, html, docx, pdf), chunking, FTS5 index, settings; format-compatible with Python', '1,295'],
    ['`ibos-retrieval`', 'Fuzzy words, vectors, fusion, reranking, access control, evidence packing, grounding, evaluation', '1,753'],
    ['`ibos-server`', 'Local web UI: search home page, settings, admin (axum)', '859'],
    ['`ibos-cli`', 'The `intelli-rust` binary and its commands', '689'],
  ]),
  P('Lines are counted in each crate\'s `src/` directory and include in-file unit tests; integration tests are extra.', { size: 18, color: C.muted }),

  H1('3  The browser agent'),
  H2('3.1  The step loop'),
  P('Each step: read the page, build a prompt, ask the model for one action, check the action against the guardrails, perform it, record it. The loop stops on a final answer, on the step limit, or when the user declines a risky action. The model is stateless between steps: nothing is carried except a short history and a short note about the previous result.'),
  H2('3.2  What the model sees'),
  P('The page is read through Chrome\'s accessibility tree plus a DOM snapshot. The result is a list of numbered interactive and text elements; the number is the browser\'s own `backendNodeId`, so a handle stays valid as long as the element does.'),
  ...bullets([
    '**Compact text.** Names, roles, values and visible text only. Repeated sibling structures (product cards, table rows) are collapsed by a structural hash and summarised.',
    '**Offscreen content is counted, not dumped.** The page says how many items are hidden. The `more` action brings them in on request: by position, or with a `q` filter that returns the matching items together with their neighbouring row (`ctx`).',
    '**Unchanged pages cost almost nothing.** If the page is the same as after the last action, the model is told so instead of receiving it again. Deltas are available but off by default, because a full page is more robust than a patch.',
    '**Three-tier prompt.** A fixed prefix (system prompt and action schema, 242 tokens), the task block, and a changing tail. The fixed prefix is identical on every step, so provider-side prompt caching applies; the Anthropic renderer marks it with `cache_control`.',
  ]),
  H2('3.3  Browser control'),
  P('`ibos-cdp` speaks the DevTools protocol over a WebSocket. Every call has a timeout, the channels are bounded (a slow reader cannot grow memory without limit), and a closed flag stops new calls once the connection drops. The browser is started in its own process group (a job object on Windows) and killed as a group on exit, including on panic and on Ctrl+C; tests check that no orphan is left.'),
  H2('3.4  Guardrails'),
  table([2500, 6860], ['Guardrail', 'Behaviour'], [
    ['Domain rules', 'Allow-list and block-list checked on every navigation, including redirects'],
    ['Risky clicks', 'Words such as buy, pay, delete, send are matched on word boundaries; the user must confirm unless `--confirm off`'],
    ['No approver, no action', 'If a risky action needs approval and nobody can give it (no terminal), it is refused rather than allowed'],
    ['Card numbers', 'Luhn-validated detection; digits are masked in the audit log'],
    ['Audit log', 'One JSON line per step: action, target, result, usage'],
  ]),
  H2('3.5  Memory'),
  P('`MemoryBudget` has a soft and a hard limit. A subsystem calls `reserve(bytes)` before a large allocation and gets a guard that releases the bytes when dropped, on every path. If the hard limit would be exceeded, `reserve` returns `None` and the caller degrades: the search engine, for example, streams vectors from disk instead of holding them. Per-step work uses an arena that is freed at the end of the step. A dhat test in `ibos-dom` guards the heap use of one page distillation against regressions.'),

  H1('4  The search engine'),
  H2('4.1  Storage and compatibility'),
  P('The index is one SQLite file. `docs`, `folders`, `meta` and the FTS5 table `chunks` (porter stemmer, unicode61) have the same schema, hashing (SHA-256 of content) and settings format as the Python implementation, so an existing index and settings file work as they are. `ibos-retrieval` only adds tables: `vec_chunks` (int8 vectors), `doc_acl` (access lists) and the virtual table `chunks_vocab`.'),
  H2('4.2  Indexing'),
  P('Files are walked (hidden directories skipped), extracted by type, split into paragraph-aware chunks of about 900 characters, and stored with a hash so unchanged files are skipped. Files that disappeared are pruned. Questions are searched with stopwords removed, and hyphenated or dotted words are also indexed in joined form (`e-mail` finds `email`).'),
  H2('4.3  Hybrid retrieval'),
  ...figure(F('fig_retrieval.png'), 600, 'Figure 2. Retrieval pipeline'),
  table([1900, 7460], ['Stage', 'How it works'], [
    ['Keyword', 'FTS5 BM25. The query is tried with AND first (precision), then OR (recall). Terms are quoted, so user text can never inject FTS syntax.'],
    ['Fuzzy words', 'A query word that matches nothing is replaced by the nearest indexed words, read from `fts5vocab`: one letter off for words of 5 to 7 letters, two for longer ones, none for shorter. Up to three candidates, most frequent first.'],
    ['Vectors', 'The query is embedded; vectors are int8 with one scale each (4 times smaller than f32). Exact search by dot product, top-k kept in a bounded buffer.'],
    ['Fusion', 'Reciprocal rank fusion, `score = sum of 1 / (60 + rank)`. It compares ranks, so BM25 and cosine scores need no calibration. Ties break on id for stable output.'],
    ['Rerank', 'Heuristic: share of query terms present, closeness of the terms, adjacent query words appearing as a phrase, title match. Deterministic and takes microseconds. A model reranker can implement the same trait.'],
    ['De-duplication', '3-word shingles, Jaccard 0.8 or more; the better-ranked copy stays.'],
    ['Evidence packing', 'Best passages that fit a token budget, at most N per document; the last one is trimmed to fit.'],
    ['Answer check', 'Per sentence, content words and every number must appear in the cited evidence; citations to blocks that do not exist are flagged. Lexical only.'],
  ]),
  H2('4.4  Access control'),
  P('A document is identified by path. A caller presents principals it has verified (user ids, groups). A document with ACL rows is visible only to a caller holding one of them. A document with no rows follows the policy: `Open` (visible to all) or `Closed` (visible to nobody). The rule is a SQL predicate inside the keyword query and a hidden-document set inside the vector scan, so a hidden passage is never scored and cannot move another result. An empty list hides a document from everyone. The library enforces the rule; it does not authenticate callers.'),
  H2('4.5  Vectors and memory'),
  P('Vectors are matched to passages by a hash of the passage text. A changed file is re-embedded; a reused row id can never keep a stale vector; changing the embedder discards the old vectors. At 384 dimensions each vector costs about 404 bytes in memory (4.2 MiB for 11,000). They are loaded under the `MemoryBudget`; if the reservation is refused they are streamed from SQLite on each query. A test checks both paths give identical answers and that the reservation is released on drop.'),
  H2('4.6  Embedders'),
  table([2400, 6960], ['Embedder', 'What it does and does not do'], [
    ['`HashingEmbedder` (built in)', 'No model, no network. Hashes stemmed words, word pairs and character trigrams into 384 dimensions. Matches word forms and shared phrasing. Does **not** know synonyms: a test asserts "car" and "automobile" are not close.'],
    ['`RemoteEmbedder`', 'Any OpenAI-compatible `/embeddings` endpoint (OpenAI, Ollama, vLLM and similar). Batches of 64, 60 s timeout, key sent only in the Authorization header and hidden from `Debug`, error text never includes the server body. Tested against a local fake server for protocol, batching, key handling and errors; not tested against a real model.'],
    ['`Embedder` trait', 'Implement `id`, `dim`, `embed` to plug in a local model (ONNX, candle) later.'],
  ]),

  H1('5  Implementation approach'),
  H2('5.1  Principles'),
  ...bullets([
    '**Bound everything.** Channels, timeouts, memory, candidates per query, tokens per prompt. An unbounded resource is a bug.',
    '**Degrade, do not fail.** Missing vectors fall back to keyword search and say so in the result notes. A refused reservation falls back to streaming.',
    '**Additive storage.** New features add tables; they never rewrite the Python-compatible ones.',
    '**Small trait seams** where the real world varies: `Embedder`, `Reranker`, `Tokenizer`, the model client. Everything else is concrete.',
    '**Measure before claiming.** Each performance statement has a script that produces it.',
  ]),
  H2('5.2  Build order'),
  table([900, 3300, 5160], ['Step', 'What', 'Gate before moving on'], [
    ['1', '`ibos-mem`, `ibos-ctx`', 'Budget property tests; prompt layout stable'],
    ['2', '`ibos-cdp`, `ibos-dom`', 'Orphan-process tests; distillation on fixture pages; dhat heap guard; offscreen counts correct'],
    ['3', '`ibos-llm`, `ibos-agent`', 'End-to-end tests against a real headless Chromium and a scripted model (11 tests)'],
    ['4', '`ibos-search`, `ibos-server`', 'Index format read by and from the Python implementation; settings validation tests'],
    ['5', '`ibos-retrieval`', 'ACL no-leak tests in every mode; streaming equals resident; fake-server embedder tests'],
    ['6', 'Benchmarks and documentation', 'Scripts reproduce every table in section 7'],
  ]),
  H2('5.3  Toolchain'),
  ...code('cargo fmt --all --check\ncargo clippy --workspace --all-targets -- -D warnings   # all=deny, pedantic=warn\ncargo test --workspace                                    # 227 passed, 0 failed\ncargo deny check                                          # advisories, licenses, bans, sources: ok\nIBOS_REQUIRE_BROWSER=1 cargo test -p ibos-agent --test e2e   # needs Chromium'),
  P('Edition 2024, resolver 3. TLS is rustls only (OpenSSL and native-tls are banned in `deny.toml`). The CI workflow runs the same commands.'),

  H1('6  Quality and testing'),
  table([2500, 6860], ['Layer', 'What it checks'], [
    ['Unit tests', 'Scoring maths, fusion, quantisation, edit distance, stemming, packing, grounding, settings validation'],
    ['Property tests', 'Memory budget never exceeds the hard limit; top-k equals a full sort; chunking invariants'],
    ['Integration tests', 'Real FTS5 index in a temp directory: fuzzy words, fallback notes, ACLs in all modes and both policies, de-duplication, incremental sync, switching embedder, evidence budget, answer check'],
    ['End-to-end tests', 'Real headless Chromium and a local website: login, form, product search, table lookup, filtered `more`, guardrail refusals'],
    ['Process tests', 'No orphan browser after normal exit, error and kill'],
    ['Fake-server tests', 'Embedding protocol and error handling without network access'],
  ]),
  P('Several bugs were found by these tests and fixed before the measurements: a pack function that could overflow its token budget after trimming; a SQL literal containing NUL in the empty-ACL path; a stemmer that did not join `invoice` and `invoicing`; a benchmark Python regex that missed table cells.'),

  H1('7  Measured results'),
  H2('7.1  Agent: Rust vs Python'),
  P('**Method.** Four tasks (login, form, product lookup, table lookup) on one local website and one Chromium. Both agents are driven by a deterministic scripted model, so the comparison isolates what the agent controls: what it sends, how many calls it needs, and its own cost. Success is checked from what the website received. Tokens are characters divided by four for both agents. "Cache-adjusted" assumes the static prefix is read from a provider cache at 10% of price after the first call (an assumption, not a measurement). 10 runs per cell; medians shown.'),
  table([1200, 1100, 1000, 1000, 1500, 1500, 1060], ['Task', 'Agent', 'Success', 'Calls', 'Prompt tokens', 'Cache-adjusted', 'Wall time'], [
    ['Login', 'Python', '10/10', '4', '25,316', '9,057', '3.6 s'], ['', 'Rust', '10/10', '4', '1,304', '651', '0.6 s'],
    ['Form', 'Python', '10/10', '5', '31,580', '9,901', '3.9 s'], ['', 'Rust', '10/10', '5', '1,638', '767', '0.6 s'],
    ['Product', 'Python', '10/10', '3', '22,418', '11,578', '3.7 s'], ['', 'Rust', '10/10', '4', '1,710', '1,057', '0.7 s'],
    ['Lookup', 'Python', '10/10', '2', '15,945', '10,525', '2.6 s'], ['', 'Rust', '10/10', '3', '1,189', '753', '0.4 s'],
  ]),
  ...figure(F('fig_tokens.png'), 440, 'Figure 3. Prompt tokens per task (median of 10 runs)'),
  ...bullets([
    '**Totals across the four tasks:** prompt tokens 95,259 vs 5,841 (94% fewer); cache-adjusted 41,061 vs 3,228 (92% fewer); wall time 13.8 s vs 2.3 s (6.1x faster).',
    '**System prompt:** 6,022 tokens in Python, 242 in Rust. Most of the difference per step is this fixed cost plus the size of the page state.',
    '**Python needed fewer calls on two tasks** (3 vs 4 on product, 2 vs 3 on lookup) because its state already lists elements far below the fold. Rust needs an extra `more` call to reach them. Fewer tokens per call outweighs this by a wide margin here.',
    '**Memory:** agent process about 200 MB for both (dominated by the harness baseline); the browser used about 1.0 GB with the Rust run and 0.85 GB with the Python run. There is no memory advantage for Rust in this test.',
  ]),
  callout('Limits of this comparison', [
    'The model is scripted, so task success says the agent can carry the task out, not that a real model would choose the same actions. Completion tokens are a lower bound and equal for both agents. Dollar cost depends on the provider and is not computed. The wall-time gap was not broken down by cause.',
  ], 'amber'),
  gap(80),
  H2('7.2  Search: keyword, fuzzy, vector, hybrid'),
  P('**Method.** 3,000 generated documents (11,000 passages), each with three rare key words. 300 queries per category ask for a document by its words: exact, with one letter dropped or swapped per word, and with a different word ending. Relevant is the one source document. One document in ten is restricted to a group and probed 200 times by a caller outside it. Built-in hashing embedder, 384 dimensions.'),
  ...figure(F('fig_search.png'), 470, 'Figure 4. nDCG@10 by query type and configuration'),
  table([2800, 1500, 1500, 1800, 1760], ['Configuration', 'Exact nDCG', 'Typos nDCG', 'Typos recall@10', 'p50 latency (typos)'], [
    ['Keyword (BM25)', '1.000', '0.003', '0.003', '0.55 ms'],
    ['Keyword + fuzzy words', '1.000', '0.987', '0.997', '29.9 ms'],
    ['Dense (hashing)', '0.536', '0.024', '0.050', '4.9 ms'],
    ['Hybrid (RRF)', '0.982', '0.027', '0.053', '5.2 ms'],
    ['Hybrid + fuzzy + rerank', '1.000', '0.928', '0.993', '35.7 ms'],
  ]),
  gap(60),
  ...bullets([
    '**Fuzzy words are the real gain** for misspellings, at about 30 ms per query here because each unmatched word scans the vocabulary.',
    '**Dense search with the hashing embedder is weak on long passages** (0.54 on exact words): 60 common words per paragraph drown three key words. Hybrid fusion therefore slightly lowers exact-query quality (0.982) until the reranker restores it (1.000), and it does not beat keyword plus fuzzy on typos (0.928 vs 0.987).',
    '**Access control:** 0 leaks in 200 probes for each of the five configurations.',
    '**Index cost:** FTS5 index 1.1 to 1.4 s for 3,000 documents; embedding 11,000 passages 1.2 s; vectors 4.2 MiB in memory.',
    '**Against the Python search:** query latency is about the same (both use SQLite FTS5); indexing a test set took 786 ms vs 1,100 ms and a no-change re-scan 21 ms vs 100 ms. Search is not a reason to move to Rust on its own.',
  ]),
  callout('Limits of this evaluation', [
    'Synthetic documents and queries; no synonym or paraphrase test (the built-in embedder has none, and no real embedding model was available offline); corpora beyond 11,000 passages not tried. Expect a real embedder to change the dense and hybrid rows. Do not infer them from this table.',
  ], 'amber'),

  H1('8  Limits and what is not verified'),
  table([3000, 6360], ['Area', 'Status'], [
    ['Real-model task success', 'Not measured. The benchmark uses a scripted model. Run the agent against a real model on your own tasks before relying on the token figures for cost.'],
    ['Real embedding quality', 'Not measured. The remote embedder is protocol-tested only. Semantic search needs one.'],
    ['Windows and macOS', 'Code paths exist (job objects, per-OS config dirs) but were not run. Linux only.'],
    ['Web UI and server search', '`ibos-server` still uses keyword search only; hybrid is available from the CLI.'],
    ['Access control', 'Enforced in the library, not authenticated. The caller must supply verified principals. Documents without an ACL are public under the default `Open` policy.'],
    ['Answer grounding check', 'Lexical. Catches invented numbers, names and citations; passes a sentence that reuses the right words to say the opposite.'],
    ['Scale', 'Exact vector search in one process; fine to hundreds of thousands of passages, not tested beyond 11,000.'],
    ['Fuzzy cost', 'A misspelled word scans the vocabulary (about 30 ms here); a cached vocabulary or n-gram index would cut it.'],
  ]),

  H1('9  Roadmap'),
  table([600, 5200, 3560], ['#', 'Work', 'Why'], [
    ['1', 'Run the agent and the Python agent against a real model on 20 to 50 tasks; record success, tokens billed, cost', 'Turns the token savings into a cost and quality statement'],
    ['2', 'Evaluate a real embedder (hosted or local) on a judged set of real documents, including paraphrase queries', 'Decides whether vectors earn their place'],
    ['3', 'Add hybrid search to `ibos-server` and the admin page (mode, ACL editor, embed button)', 'Closes the gap between CLI and UI'],
    ['4', 'CI matrix for Windows and macOS with the browser tests', 'Makes the cross-platform claim testable'],
    ['5', 'Cache the vocabulary or add a trigram index for fuzzy words', 'Brings typo queries under 5 ms'],
    ['6', 'Local embedding model behind the `Embedder` trait', 'Semantic search without a network call'],
  ]),

  H1('Appendix  CLI, configuration, storage'),
  H3('Commands'),
  ...code('intelli-rust index ~/Documents --embed          # index files and compute vectors\nintelli-rust search "invoise paymnt" --mode hybrid   # keyword | fuzzy | dense | hybrid\nintelli-rust search "refund rules" --mode hybrid --as hr --context-tokens 800\nintelli-rust eval judgments.json                    # compare modes on judged queries\nintelli-rust run "log in and read the balance" --model openai:gpt-4o --headless\nintelli-rust ui | admin | distill | doctor'),
  H3('Configuration'),
  table([3300, 6060], ['Setting', 'Meaning'], [
    ['`INTELLI_HOME`', 'Directory for the index, settings and audit logs'],
    ['`INTELLI_EMBED_URL`, `INTELLI_EMBED_MODEL`', 'Use a remote embedder (OpenAI-compatible `/embeddings`); otherwise the built-in hashing embedder'],
    ['`INTELLI_EMBED_KEY` or `OPENAI_API_KEY`', 'Key for the embedder; sent only as a bearer header'],
    ['`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`', 'Model provider keys for `run`'],
    ['`INTELLI_LOG`', 'Log filter, for example `debug`'],
  ]),
  H3('Search database tables'),
  table([2300, 7060], ['Table', 'Contents'], [
    ['`docs`, `folders`, `meta`', 'Python-compatible: document paths, hashes, titles; indexed folders; key-value (`vec_model`, `vec_dim`, `vec_gen`)'],
    ['`chunks` (FTS5)', 'Passage text, title, joined-word forms, document id, page; porter stemmer'],
    ['`vec_chunks`', 'chunk id, document id, 8-byte text hash, scale, int8 vector'],
    ['`doc_acl`', 'path, principal'],
    ['`chunks_vocab`', 'Virtual `fts5vocab` table over `chunks` used for fuzzy words'],
  ]),
];

const doc = new Document({
  creator: 'Intelli Browser OS', title: 'intelli-rust: technical architecture and implementation approach',
  styles: {
    default: { document: { run: { font: FONT, size: 22, color: C.ink } } },
    paragraphStyles: [
      { id: 'Heading1', name: 'Heading 1', basedOn: 'Normal', next: 'Normal', quickFormat: true, run: { font: FONT, size: 34, bold: true, color: C.ink }, paragraph: { spacing: { before: 420, after: 140 }, keepNext: true, outlineLevel: 0 } },
      { id: 'Heading2', name: 'Heading 2', basedOn: 'Normal', next: 'Normal', quickFormat: true, run: { font: FONT, size: 26, bold: true, color: C.blue }, paragraph: { spacing: { before: 280, after: 100 }, keepNext: true, outlineLevel: 1 } },
      { id: 'Heading3', name: 'Heading 3', basedOn: 'Normal', next: 'Normal', quickFormat: true, run: { font: FONT, size: 22, bold: true, color: C.ink }, paragraph: { spacing: { before: 200, after: 80 }, keepNext: true, outlineLevel: 2 } },
    ],
  },
  numbering: { config: [bullet('bul')] },
  sections: [{
    properties: { page: { size: { width: 12240, height: 15840 }, margin: { top: 1300, right: 1440, bottom: 1200, left: 1440 } } },
    headers: { default: new Header({ children: [new Paragraph({ border: { bottom: { style: BorderStyle.SINGLE, size: 4, color: C.line, space: 6 } }, children: [new TextRun({ text: 'Intelli Browser OS  |  intelli-rust technical architecture', font: FONT, size: 17, color: C.muted })] })] }) },
    footers: { default: new Footer({ children: [new Paragraph({ tabStops: [{ type: TabStopType.RIGHT, position: W }], children: [new TextRun({ text: 'Measured on Linux with a scripted model; see section 8 for limits.', font: FONT, size: 16, color: C.muted }), new TextRun({ children: ['\t', 'Page ', PageNumber.CURRENT, ' of ', PageNumber.TOTAL_PAGES], font: FONT, size: 17, color: C.muted })] })] }) },
    children: [...cover, ...body],
  }],
});
Packer.toBuffer(doc).then((buf) => { fs.writeFileSync(process.argv[2], buf); console.log('wrote', buf.length); });
