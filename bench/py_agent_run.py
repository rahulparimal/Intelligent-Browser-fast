#!/usr/bin/env python3
"""Run ONE task with the Python agent, driven by a deterministic simulated model. Prints `RESULT {json}`. Used by agent_bench.py."""
import asyncio, json, re, sys, time
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]  # this repository
ROOT = Path(__import__('os').environ.get('IBOS_PY_REPO', HERE.parent))  # checkout of the Python Intelli Browser OS (for the comparison)
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT / 'tests'))
from e2e_support import ScriptedLLM  # noqa: E402
from intelli_browser_os.runner import run_task  # noqa: E402

est = lambda s: -(-len(s) // 4)  # noqa: E731


def elements(text: str):
	"""Parse browser-use's `[N]<tag attrs />` lines plus the indented text that follows each element."""
	out, cur = [], None
	for line in text.splitlines():
		m = re.search(r'\[(\d+)\]<(\w+)([^\n]*?)\s*/?>?\s*$', line)
		if m and '[' in line and re.match(r'^(\s*|.*\|SHADOW\([a-z]+\)\|)\[\d+\]<', line):
			cur = {'idx': int(m[1]), 'tag': m[2], 'attrs': m[3], 'text': ''}
			out.append(cur)
		elif cur is not None and line.startswith('\t'):
			cur['text'] += ' ' + line.strip()
		else:
			cur = None
	return out


def find(text, tag=None, attr=None, contains=None):
	for e in elements(text):
		if (tag is None or e['tag'] == tag) and (attr is None or attr in e['attrs']) and (contains is None or contains in e['text'] or contains in e['attrs']):
			return e['idx']
	return None


def policy(task, base, n, state):
	nav = lambda p: {'navigate': {'url': base + p, 'new_tab': False}}  # noqa: E731
	scroll = {'scroll': {'down': True, 'pages': 1.0}}
	if task == 'login':
		if n == 0:
			return nav('/login')
		if n == 1:
			return {'input': {'index': find(state, 'input', 'id=email'), 'text': 'me@example.com', 'clear': True}}
		if n == 2:
			return {'click': {'index': find(state, 'button', 'id=go')}}
		m = re.search(r'(Welcome me@example\.com)', state)
		return {'done': {'text': m[1] if m else 'no welcome', 'success': True}}
	if task == 'form':
		if n == 0:
			return nav('/apply')
		if n == 1:
			return {'input': {'index': find(state, 'input', 'id=name'), 'text': 'Ada Lovelace', 'clear': True}}
		if n == 2:
			return {'input': {'index': find(state, 'input', 'id=mail'), 'text': 'ada@example.com', 'clear': True}}
		if n == 3:
			return {'click': {'index': find(state, 'button', 'id=send')}}
		return {'done': {'text': 'submitted', 'success': True}}
	if task == 'product':
		if n == 0:
			return nav('/products')
		idx = find(state, 'a', contains='model 50')
		if idx is not None and n < 40 and 'Detail page' not in state:
			return {'click': {'index': idx}}
		if 'Detail page 50' in state:
			return {'done': {'text': 'opened model 50', 'success': True}}
		return scroll
	if n == 0:  # lookup
		return nav('/table')
	flat = re.sub(r'\s+', ' ', re.sub(r'\[\d+\]<[^>]*>', ' ', state))  # drop the [n]<td /> wrappers, keep the cell text
	m = re.search(r'Employee 45 (?:\S+ ){2}(\d+ days)', flat)
	if m:
		return {'done': {'text': m[1], 'success': True}}
	return scroll


async def main():
	task, text, base = sys.argv[1:4]
	calls, state_box = [], {}

	class Oracle(ScriptedLLM):
		async def ainvoke(self, messages, output_format=None, **k):
			if output_format is None or 'verdict' in output_format.model_fields:
				return await super().ainvoke(messages, output_format, **k)
			system = ''.join(m.text if hasattr(m, 'text') else str(m.content) for m in messages if type(m).__name__ == 'SystemMessage')
			user = ''.join(m.text if hasattr(m, 'text') else str(m.content) for m in messages if type(m).__name__ != 'SystemMessage')
			m = re.search(r'<browser_state>(.*?)</browser_state>', user, re.S)
			action = policy(task, base, len(calls), m.group(1) if m else user)
			self.steps = [lambda t, a=action: a]
			resp = await super().ainvoke(messages, output_format, **k)
			calls.append({'prefix': est(system), 'tail': est(user), 'completion': est(resp.completion.model_dump_json())})
			return resp

	t0 = time.perf_counter()
	report = await run_task(text, Oracle([]), headless=True, vision=False, optimize_images=False, max_steps=40)
	print('RESULT ' + json.dumps({'calls': calls, 'answer': report.answer, 'success': report.success, 'steps': len(calls), 'agent_s': time.perf_counter() - t0}))


asyncio.run(main())
