const pptxgen = require('pptxgenjs');
const K = require('fs').readdirSync('/root/.claude/skills/synced').map((d) => '/root/.claude/skills/synced/' + d + '/pptx/scripts/apply_theme.js').find((p) => require('fs').existsSync(p));
const { applyTheme } = require(K);
const THEME = { name: 'IntelliRust', headFontFace: 'Cambria', bodyFontFace: 'Calibri', colors: { dk1: '1B2430', lt1: 'FFFFFF', dk2: '0F2A43', lt2: 'EEF2F6', accent1: 'C2410C', accent2: '0F766E', accent3: '2563EB', accent4: 'B45309', accent5: '64748B', accent6: '15803D', hlink: '2563EB', folHlink: '64748B' } };
const pres = new pptxgen();
pres.layout = 'LAYOUT_16x9';
pres.theme = { headFontFace: THEME.headFontFace, bodyFontFace: THEME.bodyFontFace };
const C = pres.SchemeColor;
pres.title = 'intelli-rust: technical architecture and implementation approach';

pres.defineSlideMaster({ title: 'TITLE', background: { color: '0F2A43' }, objects: [
  { placeholder: { options: { name: 'title', type: 'title', x: 0.7, y: 1.6, w: 8.6, h: 1.5, fontSize: 40, bold: true, color: 'FFFFFF', align: 'left', valign: 'top', margin: 0 }, text: '' } },
  { placeholder: { options: { name: 'body', type: 'body', x: 0.7, y: 3.25, w: 8.2, h: 1.0, fontSize: 18, color: 'CBD5E1', align: 'left', valign: 'top', margin: 0 }, text: '' } },
] });
pres.defineSlideMaster({ title: 'CONTENT', background: { color: 'FFFFFF' }, margin: [0.5, 0.5, 0.55, 0.5], objects: [
  { placeholder: { options: { name: 'title', type: 'title', x: 0.5, y: 0.3, w: 9.0, h: 0.8, fontSize: 28, bold: true, color: '0F2A43', align: 'left', valign: 'middle', margin: 0 }, text: '' } },
  { text: { text: 'intelli-rust  |  measured on Linux with a scripted model; see "What is not verified"', options: { x: 0.5, y: 5.25, w: 8.0, h: 0.25, fontSize: 10, color: '64748B', margin: 0 } } },
], slideNumber: { x: 9.0, y: 5.25, w: 0.5, h: 0.25, fontSize: 10, color: '64748B', align: 'right' } });

const FIG = __dirname;
const T = (s, o) => ({ isTextBox: true, margin: 0, ...o });
let n = 0;
const slide = (title, section) => { const s = pres.addSlide({ masterName: 'CONTENT', sectionTitle: section }); s.addText(title, { placeholder: 'title' }); return s; };
const card = (s, x, y, w, h, head, text, accent = C.accent2) => {
  s.addShape(pres.ShapeType.roundRect, { x, y, w, h, fill: { color: C.background2 }, line: { color: C.background2 }, rectRadius: 0.08, objectName: 'card ' + head });
  s.addText(head, T(s, { x: x + 0.2, y: y + 0.15, w: w - 0.4, h: 0.4, fontSize: 16, bold: true, color: accent, valign: 'top' }));
  s.addText(text, T(s, { x: x + 0.2, y: y + 0.58, w: w - 0.4, h: h - 0.7, fontSize: 14, color: C.text1, valign: 'top' }));
};
const stat = (s, x, y, w, big, label, color) => {
  s.addText(big, T(s, { x, y, w, h: 0.9, fontSize: 48, bold: true, color, fontFace: 'Cambria', valign: 'top' }));
  s.addText(label, T(s, { x, y: y + 0.95, w, h: 0.9, fontSize: 14, color: C.text1, valign: 'top' }));
};

// 1 title
pres.addSection({ title: 'Overview' });
{ const s = pres.addSlide({ masterName: 'TITLE', sectionTitle: 'Overview' });
  s.addText('intelli-rust: technical architecture and implementation approach', { placeholder: 'title' });
  s.addText('A local Rust browser agent and document search engine: how it is built, tested, and what the measurements show', { placeholder: 'body' });
  s.addText('23 November 2026  |  Linux-tested  |  scripted-model benchmarks', T(s, { x: 0.7, y: 4.8, w: 8, h: 0.3, fontSize: 12, color: '94A3B8' })); }

// 2 numbers
{ const s = slide('What the measurements show', 'Overview');
  stat(s, 0.5, 1.3, 2.1, '94%', 'fewer prompt tokens than the Python agent (5,841 vs 95,259, four tasks)', C.accent1);
  stat(s, 2.75, 1.3, 2.1, '6.1x', 'faster wall time (2.3 s vs 13.8 s); 40 of 40 runs succeeded for both', C.accent1);
  stat(s, 5.0, 1.3, 2.1, '99.7%', 'recall on misspelled queries with fuzzy words, up from 0.3%', C.accent2);
  stat(s, 7.25, 1.3, 2.25, '0 / 200', 'access-control probes that leaked a restricted document', C.accent2);
  s.addShape(pres.ShapeType.roundRect, { x: 0.5, y: 3.7, w: 9.0, h: 1.2, fill: { color: 'FEF3C7' }, line: { color: 'FEF3C7' }, rectRadius: 0.08, objectName: 'caveat' });
  s.addText([{ text: 'Not measured: ', options: { bold: true, color: C.accent4 } }, { text: 'real-model task success and cost, real embedding quality, Windows and macOS. The agent benchmark uses a scripted model; the search benchmark uses synthetic documents.', options: { color: C.text1 } }], T(s, { x: 0.7, y: 3.8, w: 8.6, h: 1.0, fontSize: 14, valign: 'middle' }));
  s.addNotes('Headline numbers. Every figure comes from a script in intelli-rust/bench. The caveat box is part of the result, not a footnote.'); }

// 3 goals
{ const s = slide('One binary, two jobs, five design goals', 'Overview');
  s.addText('A 15 MB binary drives a real Chromium from plain-English tasks and searches your own documents. The search index stays compatible with the Python version.', T(s, { x: 0.5, y: 1.2, w: 9.0, h: 0.7, fontSize: 16, color: C.text1, valign: 'top' }));
  const g = [['Few tokens', 'Compact page text and a short fixed prompt'], ['Bounded memory', 'Large buffers reserved from one budget; degrade, do not overrun'], ['Safe by default', 'Domain rules, risky-click checks, card masking, audit log'], ['Access-aware search', 'Per-document ACLs applied before ranking'], ['Honest numbers', 'Each claim tied to a script and a stated limit']];
  g.forEach((c, i) => card(s, 0.5 + (i % 3) * 3.05, 2.0 + Math.floor(i / 3) * 1.6, 2.9, 1.45, c[0], c[1], i % 2 ? C.accent2 : C.accent1)); }

// 4 architecture
pres.addSection({ title: 'Architecture' });
{ const s = slide('Ten crates, dependencies point downward', 'Architecture');
  s.addImage({ path: FIG + '/fig_arch.png', x: 0.5, y: 1.15, w: 6.0, h: 3.72, objectName: 'architecture diagram', altText: 'Crate dependency diagram: CLI and web server on top of the agent and search crates, which use cdp, ctx, mem and SQLite' });
  s.addText([{ text: 'No unsafe code', options: { bold: true, breakLine: true } }, { text: 'workspace forbids it', options: { breakLine: true, color: C.accent5 } }, { text: ' ', options: { breakLine: true, fontSize: 8 } }, { text: '9,700 lines of Rust', options: { bold: true, breakLine: true } }, { text: 'in the ten crates, tests in-file included', options: { breakLine: true, color: C.accent5 } }, { text: ' ', options: { breakLine: true, fontSize: 8 } }, { text: 'One SQLite file', options: { bold: true, breakLine: true } }, { text: 'documents, passages, vectors, ACLs', options: { color: C.accent5 } }], T(s, { x: 6.8, y: 1.4, w: 2.7, h: 3.2, fontSize: 14, color: C.text1, valign: 'top' })); }

// 5 agent context
{ const s = slide('The agent sends compact text, not HTML', 'Architecture');
  const steps = [['Chromium', 'accessibility tree + DOM snapshot'], ['Distill', 'numbered elements; repeats collapsed'], ['Prompt', 'fixed prefix + task + tail'], ['Model', 'one action per step']];
  steps.forEach((t, i) => { const x = 0.5 + i * 2.3;
    s.addShape(pres.ShapeType.roundRect, { x, y: 1.3, w: 2.0, h: 1.2, fill: { color: i === 1 ? C.accent2 : C.background2 }, line: { color: C.background2 }, rectRadius: 0.08, objectName: 'step ' + t[0] });
    s.addText([{ text: t[0], options: { bold: true, fontSize: 16, breakLine: true } }, { text: t[1], options: { fontSize: 12 } }], T(s, { x: x + 0.12, y: 1.38, w: 1.76, h: 1.04, color: i === 1 ? 'FFFFFF' : C.text1, valign: 'middle' }));
    if (i < 3) s.addText('>', T(s, { x: x + 2.0, y: 1.7, w: 0.3, h: 0.4, fontSize: 20, bold: true, color: C.accent5, align: 'center' })); });
  s.addText([
    { text: 'Offscreen content is counted, not sent; the "more" action fetches it, with a filter and the neighbouring row.', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'An unchanged page is reported as unchanged instead of resent.', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'The fixed prefix is identical every step, so provider prompt caching applies.', options: { bullet: true } },
  ], T(s, { x: 0.5, y: 2.85, w: 5.6, h: 2.1, fontSize: 14, color: C.text1, valign: 'top' }));
  stat(s, 6.5, 2.75, 1.5, '242', 'system-prompt tokens in Rust', C.accent2);
  stat(s, 8.0, 2.75, 1.6, '6,022', 'in the Python agent', C.accent5); }

// 6 safety and memory
{ const s = slide('Safety and resource limits are built in', 'Architecture');
  const g = [['Guardrails', 'Domain allow and block lists, risky-click word list, card numbers detected by Luhn check and masked in the log'], ['No approver, no action', 'If a risky action needs approval and nobody can give it, it is refused'], ['Bounded resources', 'Bounded channels, a timeout on every browser call, one MemoryBudget with soft and hard limits'], ['No orphan browsers', 'Chromium runs in its own process group and is killed as a group, including on panic; tests check it']];
  g.forEach((c, i) => card(s, 0.5 + (i % 2) * 4.6, 1.25 + Math.floor(i / 2) * 1.9, 4.4, 1.75, c[0], c[1], i === 1 ? C.accent1 : C.accent2)); }

// 7 search pipeline
pres.addSection({ title: 'Search' });
{ const s = slide('Search: fuse keyword and vector results', 'Search');
  s.addImage({ path: FIG + '/fig_retrieval.png', x: 0.5, y: 1.2, w: 9.0, h: 3.51, objectName: 'retrieval pipeline', altText: 'Query goes to keyword and vector retrieval inside an access-control boundary, then RRF, rerank, de-duplicate, pack; packed evidence goes to an answer check' });
  s.addText('Index is one SQLite file, format-compatible with the Python version; vectors, ACLs and vocabulary are added tables.', T(s, { x: 0.5, y: 4.75, w: 9.0, h: 0.4, fontSize: 12, color: C.accent5 })); }

// 8 details
{ const s = slide('Fuzzy words, access control, vectors', 'Search');
  card(s, 0.5, 1.25, 2.9, 3.5, 'Fuzzy words', 'A query word that matches nothing is replaced by the nearest indexed words: one letter off for 5 to 7 letters, two for longer. This is what fixes typos.', C.accent2);
  card(s, 3.55, 1.25, 2.9, 3.5, 'Access control', 'A document with an ACL is visible only to listed principals. Applied inside both retrievers, so hidden text is never scored. Open or Closed default for unlisted documents. The caller must authenticate.', C.accent1);
  card(s, 6.6, 1.25, 2.9, 3.5, 'Vectors and memory', 'int8 vectors, 404 bytes each at 384 dimensions (4.2 MiB for 11,000). Held under the memory budget; if refused, streamed from disk with identical results.', C.accent2); }

// 9 method
pres.addSection({ title: 'Results' });
{ const s = slide('How the agent comparison was run', 'Results');
  s.addTable([
    [{ text: 'Held the same', options: { bold: true, color: 'FFFFFF', fill: { color: C.text2 } } }, { text: 'Differs', options: { bold: true, color: 'FFFFFF', fill: { color: C.text2 } } }],
    ['Website, tasks and Chromium', 'Agent implementation (Python vs Rust)'],
    ['Model: a deterministic scripted "oracle"', 'What each agent sends the model per step'],
    ['Success judged by what the website received', 'Number of calls, wall time, memory'],
    ['Tokens counted as characters / 4', 'Static prompt: 6,022 vs 242 tokens'],
    ['10 runs per task per agent, medians', ''],
  ], { x: 0.5, y: 1.25, w: 9.0, colW: [4.5, 4.5], fontSize: 14, color: C.text1, border: { type: 'solid', pt: 0.5, color: 'C9D1DB' }, rowH: 0.42, margin: [0.05, 0.12, 0.05, 0.12] });
  s.addText('This isolates what the agent controls. It does not show how a real model performs, and cost in dollars is not computed.', T(s, { x: 0.5, y: 4.2, w: 9.0, h: 0.7, fontSize: 14, color: C.accent4, bold: true, valign: 'top' })); }

// 10 agent results
{ const s = slide('Agent: 94% fewer prompt tokens on every task', 'Results');
  const cd = [{ name: 'Python agent', labels: ['Login', 'Form', 'Product', 'Lookup'], values: [25316, 31580, 22418, 15945] }, { name: 'intelli-rust', labels: ['Login', 'Form', 'Product', 'Lookup'], values: [1304, 1638, 1710, 1189] }];
  s.addChart(pres.charts.BAR, cd, { x: 0.5, y: 1.2, w: 5.9, h: 3.8, barDir: 'col', barGrouping: 'clustered', chartColors: ['94A3B8', 'C2410C'], showLegend: true, legendPos: 'b', legendFontSize: 11, legendFontFace: '+mn-lt', showValue: true, dataLabelFontSize: 9, dataLabelFontFace: '+mn-lt', dataLabelFormatCode: '#,##0', dataLabelColor: '1B2430', catAxisLabelFontSize: 11, catAxisLabelFontFace: '+mn-lt', valAxisLabelFontSize: 10, valAxisLabelFontFace: '+mn-lt', valAxisLabelColor: '64748B', catAxisLabelColor: '1B2430', valGridLine: { color: 'E2E8F0', size: 0.5 }, catGridLine: { style: 'none' }, showTitle: true, title: 'Prompt tokens per task (median of 10 runs)', titleFontSize: 12, titleFontFace: '+mn-lt', titleColor: '1B2430', valAxisLabelFormatCode: '#,##0' });
  s.addText([
    { text: 'Wall time: 13.8 s vs 2.3 s (6.1x)', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'Cache-adjusted tokens: 41,061 vs 3,228', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'Python used fewer calls on product and lookup (3 vs 4, 2 vs 3): its state lists far-off elements', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'Memory: no Rust advantage here (agent about 200 MB both; browser 1.0 vs 0.85 GB)', options: { bullet: true } },
  ], T(s, { x: 6.6, y: 1.3, w: 2.9, h: 3.6, fontSize: 14, color: C.text1, valign: 'top' })); }

// 11 search results
{ const s = slide('Search: fuzzy words fix typos', 'Results');
  const cats = ['Exact words', 'Typos', 'Word endings'];
  const cd = [{ name: 'Keyword', labels: cats, values: [1.0, 0.003, 1.0] }, { name: 'Keyword + fuzzy', labels: cats, values: [1.0, 0.987, 1.0] }, { name: 'Dense (hashing)', labels: cats, values: [0.536, 0.024, 0.536] }, { name: 'Hybrid + fuzzy + rerank', labels: cats, values: [1.0, 0.928, 1.0] }];
  s.addChart(pres.charts.BAR, cd, { x: 0.5, y: 1.2, w: 5.9, h: 3.8, barDir: 'col', barGrouping: 'clustered', chartColors: ['94A3B8', '0F766E', 'CBD5E1', 'C2410C'], showLegend: true, legendPos: 'b', legendFontSize: 11, legendFontFace: '+mn-lt', showValue: true, dataLabelFontSize: 9, dataLabelFontFace: '+mn-lt', dataLabelFormatCode: '0.00', dataLabelColor: '1B2430', catAxisLabelFontSize: 11, catAxisLabelFontFace: '+mn-lt', valAxisLabelFontSize: 10, valAxisLabelFontFace: '+mn-lt', valAxisLabelColor: '64748B', catAxisLabelColor: '1B2430', valGridLine: { color: 'E2E8F0', size: 0.5 }, catGridLine: { style: 'none' }, valAxisMaxVal: 1.1, valAxisMinVal: 0, showTitle: true, title: 'nDCG@10, 300 queries per type, 11,000 passages', titleFontSize: 12, titleFontFace: '+mn-lt', titleColor: '1B2430', valAxisLabelFormatCode: '0.0' });
  s.addText([
    { text: 'Typo recall 0.3% to 99.7%, at about 30 ms per query', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'Hashing vectors are weak on long text (0.54) and do not know synonyms', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'Hybrid does not beat keyword plus fuzzy here; a real embedder is untested', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: '0 leaks in 200 access probes, every mode', options: { bullet: true } },
  ], T(s, { x: 6.6, y: 1.3, w: 2.9, h: 3.6, fontSize: 14, color: C.text1, valign: 'top' })); }

// 12 quality
pres.addSection({ title: 'Delivery' });
{ const s = slide('Quality gates: what passes today', 'Delivery');
  stat(s, 0.5, 1.3, 2.7, '227', 'tests passed, 0 failed (cargo test --workspace)', C.accent2);
  stat(s, 3.45, 1.3, 2.7, '0', 'clippy warnings with all=deny, pedantic=warn', C.accent2);
  stat(s, 6.4, 1.3, 3.1, 'ok', 'cargo deny: advisories, licenses, bans, sources', C.accent2);
  s.addText([
    { text: 'Real headless Chromium end-to-end tests, orphan-process tests, property tests, fake-server tests for the embedder', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'Bugs the tests found before measurement: token-budget overflow after trimming, NUL in an SQL literal, a stemmer that split invoice and invoicing', options: { bullet: true, breakLine: true, paraSpaceAfter: 6 } },
    { text: 'TLS is rustls only; OpenSSL is banned in the dependency policy', options: { bullet: true } },
  ], T(s, { x: 0.5, y: 3.1, w: 9.0, h: 1.9, fontSize: 14, color: C.text1, valign: 'top' })); }

// 13 build order
{ const s = slide('Build bottom-up, gate each step', 'Delivery');
  const st = [['1', 'mem, ctx', 'budget property tests'], ['2', 'cdp, dom', 'orphan tests, fixtures'], ['3', 'llm, agent', 'e2e on real Chromium'], ['4', 'search, server', 'Python index compatibility'], ['5', 'retrieval', 'ACL no-leak, fake server'], ['6', 'benchmarks', 'scripts reproduce tables']];
  st.forEach((t, i) => { const x = 0.5 + (i % 3) * 3.05, y = 1.3 + Math.floor(i / 3) * 1.85;
    s.addShape(pres.ShapeType.ellipse, { x, y, w: 0.55, h: 0.55, fill: { color: C.accent1 }, line: { color: C.accent1 }, objectName: 'step number ' + t[0] });
    s.addText(t[0], T(s, { x, y, w: 0.55, h: 0.55, fontSize: 16, bold: true, color: 'FFFFFF', align: 'center', valign: 'middle' }));
    s.addText([{ text: t[1], options: { bold: true, fontSize: 16, breakLine: true } }, { text: 'Gate: ' + t[2], options: { fontSize: 14, color: C.accent5 } }], T(s, { x: x + 0.7, y, w: 2.2, h: 1.4, color: C.text1, valign: 'top' })); });
  s.addText('Principles: bound everything, degrade instead of failing, add tables never rewrite, trait seams only where the world varies, measure before claiming.', T(s, { x: 0.5, y: 4.45, w: 9.0, h: 0.6, fontSize: 14, bold: true, color: C.text2 })); }

// 14 limits
pres.addSection({ title: 'Next' });
{ const s = slide('What is not verified', 'Next');
  const g = [['Real models', 'Task success and dollar cost with a real model are unmeasured'], ['Real embeddings', 'Remote embedder is protocol-tested only; synonyms need one'], ['Windows and macOS', 'Code paths exist, never run'], ['Server and UI', 'The web page still uses keyword search only'], ['Access control', 'Enforced, not authenticated; Open policy exposes unlisted documents'], ['Answer check', 'Lexical: catches invented numbers, passes reversed meaning']];
  g.forEach((c, i) => card(s, 0.5 + (i % 3) * 3.05, 1.25 + Math.floor(i / 3) * 1.9, 2.9, 1.75, c[0], c[1], C.accent4)); }

// 15 roadmap
{ const s = slide('Next steps, in order', 'Next');
  s.addTable([
    [{ text: '#', options: { bold: true, color: 'FFFFFF', fill: { color: C.text2 } } }, { text: 'Work', options: { bold: true, color: 'FFFFFF', fill: { color: C.text2 } } }, { text: 'Why', options: { bold: true, color: 'FFFFFF', fill: { color: C.text2 } } }],
    ['1', 'Real-model run, 20 to 50 tasks, both agents', 'Turns token savings into cost and quality'],
    ['2', 'Evaluate a real embedder on real documents', 'Decides if vectors earn their place'],
    ['3', 'Hybrid search in the web UI and admin', 'Closes the CLI and UI gap'],
    ['4', 'Windows and macOS CI with browser tests', 'Makes cross-platform testable'],
    ['5', 'Cache vocabulary or trigram index', 'Typo queries under 5 ms'],
    ['6', 'Local embedding model behind the trait', 'Semantic search offline'],
  ], { x: 0.5, y: 1.25, w: 9.0, colW: [0.5, 5.0, 3.5], fontSize: 14, color: C.text1, border: { type: 'solid', pt: 0.5, color: 'C9D1DB' }, rowH: 0.45, margin: [0.05, 0.12, 0.05, 0.12] });
  s.addText('Decision asked: approve step 1 and 2, which settle the two open questions.', T(s, { x: 0.5, y: 4.6, w: 9.0, h: 0.4, fontSize: 16, bold: true, color: C.accent1 })); }

(async () => { await pres.writeFile({ fileName: 'IntelliRust-Technical-Architecture.pptx' }); await applyTheme('IntelliRust-Technical-Architecture.pptx', THEME); console.log('done'); })();
