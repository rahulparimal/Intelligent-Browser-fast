#!/usr/bin/env python3
"""Compare the page representation sent to the model by the Python agent (browser-use) and by intelli-rust, on the same pages.

Both sides are measured with the same crude estimate (characters / 4), so the ratio is meaningful even though the absolute
token counts are estimates. Usage: python3 intelli-rust/bench/compare_tokens.py   (needs the repo venv and a built intelli-rust)
"""
import asyncio, json, os, re, socket, subprocess, sys, threading
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]  # this repository
ROOT = Path(__import__('os').environ.get('IBOS_PY_REPO', HERE.parent))  # checkout of the Python Intelli Browser OS (for the comparison)
sys.path.insert(0, str(ROOT)); sys.path.insert(0, str(ROOT / 'tests'))
os.environ.setdefault('ANONYMIZED_TELEMETRY', 'false')
if hasattr(os, 'geteuid') and os.geteuid() == 0:
	os.environ.setdefault('IN_DOCKER', 'true')
from e2e_support import ScriptedLLM  # noqa: E402
from intelli_browser_os.runner import run_task  # noqa: E402

PAGES = ['login', 'form', 'products', 'table', 'menu', 'article']
RUST = HERE / 'target' / 'release' / 'intelli-rust'
page_marker = {'login': 'Sign in', 'form': 'Job application', 'products': 'All products', 'table': 'Staff directory', 'menu': 'Department 1', 'article': 'Planning in practice'}
est = lambda s: -(-len(s) // 4)  # noqa: E731
PAGES_DIR = HERE / 'fixtures' / 'pages'


class Quiet(SimpleHTTPRequestHandler):
	def log_message(self, *a):
		pass


def serve_pages() -> str:
	"""Both agents load the same http:// URLs (the Python agent does not browse file:// pages)."""
	srv = ThreadingHTTPServer(('127.0.0.1', 0), partial(Quiet, directory=str(PAGES_DIR)))
	threading.Thread(target=srv.serve_forever, daemon=True).start()
	return f'http://127.0.0.1:{srv.server_port}'


BASE = ''


async def python_view(page: str) -> dict:
	url = f'{BASE}/{page}.html'
	seen = []

	class Rec(ScriptedLLM):
		async def ainvoke(self, messages, output_format=None, **k):
			if output_format is not None and 'verdict' not in output_format.model_fields:
				seen.append([(type(m).__name__, m.text if hasattr(m, 'text') else str(m.content)) for m in messages])
			return await super().ainvoke(messages, output_format, **k)

	llm = Rec([lambda t: {'navigate': {'url': url, 'new_tab': False}}, lambda t: {'done': {'text': 'x', 'success': True}}])
	await run_task('look', llm, headless=True, vision=False, optimize_images=False, max_steps=4)
	user = [t for n, t in seen[1] if n != 'SystemMessage'][0]
	system = ''.join(t for n, t in seen[1] if n == 'SystemMessage')
	state = re.search(r'<browser_state>(.*?)</browser_state>', user, re.S).group(1)
	assert page_marker[page] in state, f'the Python agent did not load {page}: {state[:300]!r}'  # a wrong measurement is worse than none
	return {'page_tokens': est(state), 'user_message_tokens': est(user), 'system_tokens': est(system)}


def rust_view(page: str) -> dict:
	out = subprocess.run([str(RUST), 'distill', f'{BASE}/{page}.html', '--json'], capture_output=True, text=True, check=True, env={**os.environ, 'INTELLI_NO_SANDBOX': '1'})
	s = json.loads(out.stdout)['stats']
	return {'page_tokens': s['tokens'], 'chars': s['chars'], 'input_bytes': s['input_bytes'], 'distill_ms': s['distill_micros'] / 1000}


async def main():
	global BASE
	BASE = serve_pages()
	rows = []
	for p in PAGES:
		py, rs = await python_view(p), rust_view(p)
		rows.append((p, py, rs))
	print(f'{"page":9} {"python page":>12} {"rust page":>10} {"reduction":>10}   {"browser data":>13} {"distill":>9}')
	tp = tr = 0
	for p, py, rs in rows:
		tp += py['page_tokens']
		tr += rs['page_tokens']
		print(f'{p:9} {py["page_tokens"]:>12} {rs["page_tokens"]:>10} {1 - rs["page_tokens"] / py["page_tokens"]:>9.0%}   {rs["input_bytes"] / 1024:>10.0f} KB {rs["distill_ms"]:>7.1f}ms')
	print(f'{"TOTAL":9} {tp:>12} {tr:>10} {1 - tr / tp:>9.0%}')
	print(f'\nstatic system prompt: python {rows[0][1]["system_tokens"]} tokens; rust: see `cargo test -p ibos-agent --test e2e -- --nocapture` ("static prefix")')


asyncio.run(main())
