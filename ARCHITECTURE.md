# Architecture

## One step of the agent

```
observe  ->  distill  ->  build prompt  ->  model  ->  guard  ->  act
(CDP)       (ibos-dom)    (ibos-ctx)       (ibos-llm)  (ibos-agent)
```

1. **Observe** (`ibos-cdp`): fetch the accessibility tree and layout rectangles in parallel, plus URL, title and viewport. The raw
   JSON is kept as `Box<RawValue>` so it is parsed once, by borrowing.
2. **Distil** (`ibos-dom`): keep interactive elements, headings, images with names and visible text; merge adjacent text into one
   element; drop what is hidden or irrelevant; collapse repeated sibling structures (recognised by a structural hash, not by
   text); count offscreen content instead of listing it. Scratch data lives in a per-step arena (`ibos-mem::StepArena`) released
   in one operation.
3. **Build the prompt** (`ibos-ctx`): three tiers, least to most volatile: *prefix* (system prompt), *task block* (goal and
   facts ledger), *tail* (history window and the page). The prefix and task block are byte-identical on every step, so providers
   serve them from cache; provider renderers place cache breakpoints (`cache_control` for Anthropic).
4. **Model** (`ibos-llm`): one call, bounded by a timeout, retried on 429/5xx/network errors with exponential backoff.
5. **Guard** (`ibos-agent`): domain allow and block lists, approval for risky actions, card-number detection (Luhn), audit log.
   A redirect to a disallowed domain is caught after the page loads.
6. **Act** (`ibos-cdp`): click and type are addressed by the browser's backend node id.

## Memory

- Every large buffer is charged to a `MemoryBudget` (soft and hard limit, RAII reservations, never over the hard limit).
- Screenshots are kept **encoded** in a bounded ring and de-duplicated by a 64-bit perceptual hash.
- Channels and queues are bounded; a silent browser connection becomes a timeout error, not a hang.
- The browser runs in its own process group (a job object on Windows) and the whole tree is killed on drop. A test scans for
  leftover processes.
- No `unsafe` in any crate of this workspace (`unsafe_code = "forbid"`). Dependencies contain some.

## Tokens

The static prompt is 242 tokens. The page is distilled to 51 to 694 tokens on the six test pages (Python agent: 167 to 3,699).
History is a facts ledger plus the last few steps as one line each; older steps shrink to summaries; large results arrive as a
one-off note. A per-task token budget degrades the prompt in a fixed order (drop screenshots, compact history, summary only) and
then stops with an explanation.

## Compatibility with the Python implementation

Same config directory, same SQLite schema and content hashes, same settings file and signature format, same HTML for the home and
admin pages. `tests/python_compat.rs` proves an index written by either side is read by the other without re-indexing, and the
server test compares the HTML files byte for byte with the Python copies.

## Decisions that changed during the build

| initial idea | what the tests showed | now |
|---|---|---|
| send only page *changes* each step | a stateless prompt cannot act on elements it saw earlier | full distilled page each step, plus an "unchanged" hint; deltas only for conversation-style callers |
| a `more` action listing the next items | only 25 elements (about 8 products), always the same ones | `more` with `q` (filter), `skip` and `n` |
| drop `LabelText` nodes to avoid duplicate text | Chrome nests the input *inside* the label, so inputs vanished | descend into labels, suppress only their text |
| handles ordered by node id | document order was lost | an order-preserving `Page` |
| `wait_ready` bounded per poll | an endless script hung the wait for 30 s | one deadline around the whole wait |
| calls after a closed socket waited for the timeout | a request registered after the reader exited was never answered | a closed flag checked after registering |
