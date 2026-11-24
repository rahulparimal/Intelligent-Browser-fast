import sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch, FancyArrowPatch
OUT = sys.argv[1]
INK, MUTED, LINE = '#1a1d21', '#5f6b7a', '#c9d1db'
BLUE, TEAL, AMBER, GREY, GREEN = '#2563eb', '#0f766e', '#b45309', '#eef2f6', '#15803d'
plt.rcParams['font.family'] = 'DejaVu Sans'

def box(ax, x, y, w, h, title, sub='', fc='#ffffff', ec=BLUE, fs=9.5):
    ax.add_patch(FancyBboxPatch((x, y), w, h, boxstyle='round,pad=0.02,rounding_size=0.08', fc=fc, ec=ec, lw=1.4))
    ax.text(x + w / 2, y + h / 2 + (0.13 if sub else 0), title, ha='center', va='center', fontsize=fs, fontweight='bold', color=INK)
    if sub:
        ax.text(x + w / 2, y + h / 2 - 0.17, sub, ha='center', va='center', fontsize=7.6, color=MUTED)

def arrow(ax, x1, y1, x2, y2, color=MUTED):
    ax.add_patch(FancyArrowPatch((x1, y1), (x2, y2), arrowstyle='-|>', mutation_scale=11, color=color, lw=1.3))

# 1. system architecture
fig, ax = plt.subplots(figsize=(10, 6.2)); ax.set_xlim(0, 10); ax.set_ylim(0, 6.2); ax.axis('off')
box(ax, 0.2, 5.3, 4.4, 0.75, 'intelli-rust CLI', 'run | search | index | eval | ui | admin | doctor', fc=GREY, ec=LINE)
box(ax, 5.4, 5.3, 4.4, 0.75, 'ibos-server (web UI)', 'search home page, settings and admin (axum)', fc=GREY, ec=LINE)
arrow(ax, 2.4, 5.3, 2.4, 4.85); arrow(ax, 7.6, 5.3, 7.6, 4.85)
ax.add_patch(FancyBboxPatch((0.15, 2.9), 4.75, 1.95, boxstyle='round,pad=0.02,rounding_size=0.1', fc='#f5f9ff', ec=BLUE, lw=1.6))
ax.text(0.35, 4.6, 'Browser agent', fontsize=10, fontweight='bold', color=BLUE)
box(ax, 0.35, 3.1, 1.4, 1.05, 'ibos-agent', 'loop, actions,\nguardrails, audit', fs=8.5)
box(ax, 1.9, 3.1, 1.4, 1.05, 'ibos-dom', 'AX tree to compact\npage text', fs=8.5)
box(ax, 3.45, 3.1, 1.3, 1.05, 'ibos-llm', 'OpenAI and\nAnthropic', fs=8.5)
ax.add_patch(FancyBboxPatch((5.1, 2.9), 4.75, 1.95, boxstyle='round,pad=0.02,rounding_size=0.1', fc='#f0fdf9', ec=TEAL, lw=1.6))
ax.text(5.3, 4.6, 'Search', fontsize=10, fontweight='bold', color=TEAL)
box(ax, 5.3, 3.1, 2.1, 1.05, 'ibos-search', 'extract, chunk, FTS5,\nsettings', ec=TEAL, fs=8.5)
box(ax, 7.55, 3.1, 2.15, 1.05, 'ibos-retrieval', 'fuzzy, vectors, RRF, rerank,\nACL, packing, grounding', ec=TEAL, fs=8.5)
arrow(ax, 7.4, 3.62, 7.55, 3.62, TEAL)
box(ax, 0.2, 1.45, 2.3, 0.9, 'ibos-cdp', 'Chrome DevTools client,\nprocess-group kill', fs=8.5)
box(ax, 2.7, 1.45, 2.2, 0.9, 'ibos-ctx', 'prompt tiers, history,\ntoken budgets', fs=8.5)
box(ax, 5.1, 1.45, 2.3, 0.9, 'ibos-mem', 'MemoryBudget, arenas,\nframe slabs, interning', fs=8.5)
box(ax, 7.6, 1.45, 2.2, 0.9, 'SQLite (one file)', 'docs, chunks (FTS5),\nvectors, ACLs', ec=AMBER, fs=8.5)
for x in (1.0, 2.5, 4.1):
    arrow(ax, x, 3.1, x if x < 3 else 3.8, 2.38)
arrow(ax, 6.3, 3.1, 8.6, 2.38, TEAL); arrow(ax, 8.6, 3.1, 8.7, 2.38, TEAL)
box(ax, 0.2, 0.2, 4.7, 0.75, 'Chromium (headless or visible)', 'driven over CDP; killed as a process group on exit', fc=GREY, ec=LINE)
box(ax, 5.1, 0.2, 4.7, 0.75, 'Model provider / embedding service', 'only reachable endpoints you configure', fc=GREY, ec=LINE)
arrow(ax, 1.3, 1.45, 1.3, 0.97)
fig.savefig(OUT + '/fig_arch.png', dpi=200, bbox_inches='tight'); plt.close(fig)

# 2. retrieval pipeline
fig, ax = plt.subplots(figsize=(10, 3.9)); ax.set_xlim(0, 10); ax.set_ylim(0, 3.9); ax.axis('off')
box(ax, 0.1, 1.55, 1.1, 0.8, 'Query', '', fc=GREY, ec=LINE)
box(ax, 1.7, 2.55, 1.9, 0.9, 'Keyword', 'FTS5 BM25 + fuzzy words', ec=TEAL, fs=8.8)
box(ax, 1.7, 0.45, 1.9, 0.9, 'Vectors', 'embed, int8 cosine', ec=TEAL, fs=8.8)
box(ax, 4.1, 1.5, 1.2, 0.9, 'RRF', 'fuse ranks', fs=9)
box(ax, 5.6, 1.5, 1.2, 0.9, 'Rerank', 'coverage, phrase', fs=9)
box(ax, 7.1, 1.5, 1.2, 0.9, 'De-dupe', '3-word shingles', fs=9)
box(ax, 8.6, 1.5, 1.3, 0.9, 'Pack', 'token budget', fs=9)
arrow(ax, 1.2, 2.1, 1.7, 2.8); arrow(ax, 1.2, 1.8, 1.7, 1.0)
arrow(ax, 3.6, 3.0, 4.2, 2.35); arrow(ax, 3.6, 0.9, 4.2, 1.55)
for x in (5.3, 6.8, 8.3):
    arrow(ax, x, 1.95, x + 0.3, 1.95)
ax.add_patch(FancyBboxPatch((1.55, 0.2), 2.2, 3.4, boxstyle='round,pad=0.02,rounding_size=0.1', fc='none', ec=AMBER, lw=1.3, ls='--'))
ax.text(2.65, 3.72, 'access control applied inside both', ha='center', fontsize=8.2, color=AMBER)
box(ax, 8.6, 0.2, 1.3, 0.8, 'Answer check', 'lexical grounding', ec=GREEN, fs=8.2)
arrow(ax, 9.25, 1.5, 9.25, 1.02, GREEN)
fig.savefig(OUT + '/fig_retrieval.png', dpi=200, bbox_inches='tight'); plt.close(fig)

# 3. tokens per task
tasks = ['Login', 'Form', 'Product', 'Lookup']
py = [25316, 31580, 22418, 15945]; rs = [1304, 1638, 1710, 1189]
fig, ax = plt.subplots(figsize=(8, 3.8))
import numpy as np
x = np.arange(4); w = 0.36
b1 = ax.bar(x - w/2, py, w, color='#94a3b8', label='Python agent'); b2 = ax.bar(x + w/2, rs, w, color=BLUE, label='intelli-rust')
for b in list(b1) + list(b2):
    ax.text(b.get_x() + b.get_width()/2, b.get_height() + 400, f'{int(b.get_height()):,}', ha='center', fontsize=8, color=INK)
ax.set_xticks(x); ax.set_xticklabels(tasks); ax.set_ylabel('prompt tokens (median of 10 runs)', fontsize=9, color=MUTED)
for s in ('top', 'right'): ax.spines[s].set_visible(False)
ax.legend(frameon=False, fontsize=9); ax.set_ylim(0, 36000); ax.tick_params(colors=MUTED)
fig.savefig(OUT + '/fig_tokens.png', dpi=200, bbox_inches='tight'); plt.close(fig)

# 4. search quality
cats = ['Exact words', 'Typos', 'Word endings']
data = {'Keyword': [1.0, 0.003, 1.0], 'Keyword + fuzzy': [1.0, 0.987, 1.0], 'Dense (hashing)': [0.536, 0.024, 0.536], 'Hybrid + fuzzy + rerank': [1.0, 0.928, 1.0]}
cols = ['#94a3b8', TEAL, '#cbd5e1', BLUE]
fig, ax = plt.subplots(figsize=(8, 3.8)); w = 0.2
for i, (k, v) in enumerate(data.items()):
    bars = ax.bar(np.arange(3) + (i - 1.5) * w, v, w, color=cols[i], label=k)
    for b in bars:
        ax.text(b.get_x() + b.get_width()/2, b.get_height() + 0.02, f'{b.get_height():.2f}', ha='center', fontsize=7, color=INK)
ax.set_xticks(np.arange(3)); ax.set_xticklabels(cats); ax.set_ylabel('nDCG@10', fontsize=9, color=MUTED); ax.set_ylim(0, 1.2)
for s in ('top', 'right'): ax.spines[s].set_visible(False)
ax.legend(frameon=False, fontsize=8, ncol=2, loc='upper center', bbox_to_anchor=(0.5, 1.18)); ax.tick_params(colors=MUTED)
fig.savefig(OUT + '/fig_search.png', dpi=200, bbox_inches='tight'); plt.close(fig)
