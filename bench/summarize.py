#!/usr/bin/env python3
"""Turn agent_results.json into the tables in RESULTS.md. Usage: python3 summarize.py [agent_results.json]"""
import json, statistics, sys
from pathlib import Path

rows = json.loads(Path(sys.argv[1] if len(sys.argv) > 1 else Path(__file__).with_name('agent_results.json')).read_text())
tasks = list(dict.fromkeys(r['task'] for r in rows))
med = lambda rs, k: statistics.median(r[k] for r in rs)  # noqa: E731
cell = lambda agent, task: [r for r in rows if r['agent'] == agent and r['task'] == task and 'calls' in r]  # noqa: E731
n = len([r for r in rows if r['agent'] == 'rust' and r['task'] == tasks[0]])
print(f'Median of {n} runs per cell. Success = the website received the right thing.\n')
print('| task | agent | success | model calls | prompt tokens | prompt tokens, cache-adjusted | wall time | agent memory | browser memory |')
print('|---|---|---|---|---|---|---|---|---|')
tot = {'python': [0, 0, 0.0], 'rust': [0, 0, 0.0]}
for t in tasks:
	for a in ('python', 'rust'):
		rs = cell(a, t)
		ok = sum(r['ok'] for r in rs)
		print(f'| {t} | {a} | {ok}/{len(rs)} | {med(rs, "calls"):.0f} | {med(rs, "prompt_tokens"):,.0f} | {med(rs, "cache_adjusted_tokens"):,.0f} | {med(rs, "wall_s"):.1f} s | {med(rs, "agent_rss_mb"):.0f} MB | {med(rs, "browser_rss_mb"):.0f} MB |')
		tot[a][0] += med(rs, 'prompt_tokens')
		tot[a][1] += med(rs, 'cache_adjusted_tokens')
		tot[a][2] += med(rs, 'wall_s')
p, r = tot['python'], tot['rust']
print(f'\nAcross the four tasks (sum of medians): prompt tokens {p[0]:,.0f} vs {r[0]:,.0f} ({1 - r[0] / p[0]:.0%} fewer); cache-adjusted {p[1]:,.0f} vs {r[1]:,.0f} ({1 - r[1] / p[1]:.0%} fewer); wall time {p[2]:.1f} s vs {r[2]:.1f} s ({p[2] / r[2]:.1f}x faster)')
print('\nSystem prompt (static prefix): python', cell('python', tasks[0])[0]['prefix_tokens'], 'tokens; rust', cell('rust', tasks[0])[0]['prefix_tokens'], 'tokens')
