#!/usr/bin/env python3
"""Head-to-head: the Python agent (browser-use based) vs intelli-rust on the same tasks, the same website and the same browser.

WHAT THIS MEASURES. Both agents are driven by a *deterministic simulated model* that knows what to do on each page (an "oracle"
written separately against each agent's own prompt format). So differences come from the agents, not from model quality:
  - prompt tokens actually sent (chars / 4, the same estimate for both), steps taken, wall-clock time, memory of the agent
    process and of the browser, and whether the website received the right thing.
WHAT THIS DOES NOT MEASURE. How well a real model reads each prompt; real model latency; real prices. An oracle never misreads a
page. Treat token numbers as input size under equal intelligence, not as a bill.

Usage: python3 intelli-rust/bench/agent_bench.py [--runs 5] [--out bench/agent_results.json]
Needs: the repo venv (browser-use), a release build of intelli-rust, and Chromium.
"""
import argparse, json, os, re, statistics, subprocess, sys, tempfile, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

HERE = Path(__file__).resolve().parents[1]  # this repository
ROOT = Path(__import__('os').environ.get('IBOS_PY_REPO', HERE.parent))  # checkout of the Python Intelli Browser OS (for the comparison)
PAGES = HERE / 'fixtures' / 'pages'
RUST = HERE / 'target' / 'release' / 'intelli-rust'
est = lambda s: -(-len(s) // 4)  # noqa: E731  characters / 4, rounded up: the same crude estimate on both sides

LOGIN = '''<!doctype html><html><head><title>Login</title></head><body><h1>Sign in</h1>
<label>Email <input id="email" type="email" required></label>
<label>Password <input id="pw" type="password"></label>
<label><input type="checkbox" id="remember"> Remember me</label>
<button id="go" onclick="var v=document.getElementById('email').value;fetch('/submit?kind=login&email='+encodeURIComponent(v)).then(function(){document.getElementById('result').textContent='Welcome '+v})">Sign in</button>
<a href="/forgot">Forgot password?</a><div id="result"></div></body></html>'''
APPLY = '''<!doctype html><html><head><title>Apply</title></head><body><h1>Job application</h1>
<label>Full name <input id="name"></label> <label>Email <input id="mail" type="email"></label>
<button id="send" onclick="var n=document.getElementById('name').value,e=document.getElementById('mail').value;fetch('/submit?kind=apply&name='+encodeURIComponent(n)+'&email='+encodeURIComponent(e)).then(function(){document.getElementById('result').textContent='Application received for '+n})">Submit application</button>
<div id="result"></div></body></html>'''
TABLE = (PAGES / 'table.html').read_text()
EXPECTED_LEAVE = re.search(r'/emp/45">Employee 45</a></td><td>[^<]*</td><td>[^<]*</td><td>(\d+ days)', TABLE).group(1)


class Site(BaseHTTPRequestHandler):
	events: list = []

	def log_message(self, *a):
		pass

	def _send(self, body: str, code=200):
		data = body.encode()
		self.send_response(code)
		self.send_header('Content-Type', 'text/html; charset=utf-8')
		self.send_header('Content-Length', str(len(data)))
		self.end_headers()
		self.wfile.write(data)

	def do_GET(self):
		u = urlparse(self.path)
		q = {k: v[0] for k, v in parse_qs(u.query).items()}
		if u.path == '/login':
			return self._send(LOGIN)
		if u.path == '/apply':
			return self._send(APPLY)
		if u.path == '/products':
			return self._send((PAGES / 'products.html').read_text())
		if u.path == '/table':
			return self._send(TABLE)
		if u.path == '/submit':
			Site.events.append(('submit', q))
			return self._send('ok')
		m = re.fullmatch(r'/(p|emp)/(\d+)', u.path)
		if m:
			Site.events.append(('visit', u.path))
			return self._send(f'<html><head><title>Detail {m[2]}</title></head><body><h1>Detail page {m[2]}</h1><button>Back</button></body></html>')
		self._send('not found', 404)


def start_site() -> str:
	srv = ThreadingHTTPServer(('127.0.0.1', 0), Site)
	threading.Thread(target=srv.serve_forever, daemon=True).start()
	return f'http://127.0.0.1:{srv.server_port}'


TASKS = {
	'login': 'Open {base}/login, sign in with the email me@example.com and report the welcome message.',
	'form': 'Open {base}/apply and submit the job application for Ada Lovelace (ada@example.com).',
	'product': "Open {base}/products and open the product page for 'model 50'.",
	'lookup': 'Open {base}/table and tell me how many leave days Employee 45 has.',
}


def check(task: str, answer: str) -> bool:
	ev = Site.events
	if task == 'login':
		return any(k == 'submit' and v.get('kind') == 'login' and v.get('email') == 'me@example.com' for k, v in ev) and 'Welcome me@example.com' in answer
	if task == 'form':
		return any(k == 'submit' and v.get('kind') == 'apply' and v.get('name') == 'Ada Lovelace' and v.get('email') == 'ada@example.com' for k, v in ev)
	if task == 'product':
		return ('visit', '/p/50') in ev
	return EXPECTED_LEAVE in answer


# ---------------------------------------------------------------------------- the simulated model for the RUST agent
def rust_reply(task: str, n: int, tail: str, base: str) -> str:
	def handle(role, needle):
		for line in tail.splitlines():
			m = re.match(r'\[(\d+)\]<([^>]+)> (.*)', line)
			if m and m[2] == role and needle in m[3]:
				return int(m[1])
		raise RuntimeError(f'no <{role}> {needle!r} in prompt:\n{tail}')

	def note_lines():
		return tail.split('hidden items for [', 1)[1].splitlines()[1:]

	go = lambda path: {'a': 'go', 'url': base + path}  # noqa: E731
	if task == 'login':
		welcome = next((m[1] for l in tail.splitlines() if (m := re.match(r'\[\d+\]<text> (Welcome .*)', l))), 'no welcome')
		steps = [lambda: go('/login'), lambda: {'a': 'type', 'h': handle('input', 'Email'), 't': 'me@example.com'}, lambda: {'a': 'click', 'h': handle('btn', 'Sign in')}, lambda: {'a': 'done', 't': welcome, 'ok': True}]
		return json.dumps(steps[min(n, 3)]())
	if task == 'form':
		steps = [lambda: go('/apply'), lambda: {'a': 'type', 'h': handle('input', 'Full name'), 't': 'Ada Lovelace'}, lambda: {'a': 'type', 'h': handle('input', 'Email'), 't': 'ada@example.com'}, lambda: {'a': 'click', 'h': handle('btn', 'Submit application')}, lambda: {'a': 'done', 't': 'submitted', 'ok': True}]
		return json.dumps(steps[min(n, 4)]())
	if task == 'product':
		if n == 0:
			return json.dumps(go('/products'))
		if n == 1:
			return json.dumps({'a': 'more', 'h': handle('+', 'more similar items'), 'q': 'model 50'})
		if n == 2:
			h = next(int(m[1]) for l in note_lines() if (m := re.match(r'\[(\d+)\]<a> .*model 50$', l)))
			return json.dumps({'a': 'click', 'h': h})
		return json.dumps({'a': 'done', 't': 'opened model 50', 'ok': True})
	if n == 0:
		return json.dumps(go('/table'))
	if n == 1:
		return json.dumps({'a': 'more', 'h': handle('+', 'more similar items'), 'q': 'Employee 45'})
	row = note_lines()[:4]
	return json.dumps({'a': 'done', 't': re.sub(r'^\[\d+\]<text> ', '', row[3]), 'ok': True})


class Oracle(BaseHTTPRequestHandler):
	runs: dict = {}  # run id -> dict(task, base, calls=[...])

	def log_message(self, *a):
		pass

	def do_POST(self):
		rid = self.path.split('/')[1]
		run = Oracle.runs[rid]
		body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
		system, user = body['messages'][0]['content'], body['messages'][1]['content']
		reply = rust_reply(run['task'], len(run['calls']), user, run['base'])
		run['calls'].append({'prefix': est(system), 'tail': est(user), 'completion': est(reply)})
		out = json.dumps({'choices': [{'message': {'content': reply}}], 'usage': {'prompt_tokens': est(system) + est(user), 'completion_tokens': est(reply)}}).encode()
		self.send_response(200)
		self.send_header('Content-Type', 'application/json')
		self.send_header('Content-Length', str(len(out)))
		self.end_headers()
		self.wfile.write(out)


def start_oracle() -> str:
	srv = ThreadingHTTPServer(('127.0.0.1', 0), Oracle)
	threading.Thread(target=srv.serve_forever, daemon=True).start()
	return f'http://127.0.0.1:{srv.server_port}'


# ---------------------------------------------------------------------------- measurement of a child process
def rss_kb(pid: int) -> int:
	try:
		for line in Path(f'/proc/{pid}/status').read_text().splitlines():
			if line.startswith('VmRSS:'):
				return int(line.split()[1])
	except OSError:
		pass
	return 0


def browser_rss_kb() -> int:
	total = 0
	for p in Path('/proc').iterdir():
		if p.name.isdigit():
			try:
				cmd = (p / 'cmdline').read_bytes()
			except OSError:
				continue
			if b'remote-debugging' in cmd and b'chrom' in cmd.lower():
				total += rss_kb(int(p.name))
			elif b'--type=' in cmd and b'chrom' in cmd.lower():
				total += rss_kb(int(p.name))
	return total


def measure(cmd: list[str], env: dict) -> dict:
	"""Run `cmd`; return wall seconds, the process's own peak RSS, and the peak summed RSS of the browser processes."""
	peak = {'browser': 0}
	stop = threading.Event()

	def poll():
		while not stop.is_set():
			peak['browser'] = max(peak['browser'], browser_rss_kb())
			time.sleep(0.1)

	t = threading.Thread(target=poll, daemon=True)
	t.start()
	t0 = time.perf_counter()
	p = subprocess.Popen(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
	_, status, ru = os.wait4(p.pid, 0)
	wall = time.perf_counter() - t0
	stop.set()
	out, err = p.stdout.read().decode(), p.stderr.read().decode()
	p.returncode = os.waitstatus_to_exitcode(status)
	return {'wall_s': wall, 'agent_rss_mb': ru.ru_maxrss / 1024, 'browser_rss_mb': peak['browser'] / 1024, 'code': p.returncode, 'stdout': out, 'stderr': err}


def run_rust(task: str, base: str, oracle: str, tmp: Path) -> dict:
	rid = f'r{time.time_ns()}'
	Oracle.runs[rid] = {'task': task, 'base': base, 'calls': []}
	Site.events.clear()
	env = {**os.environ, 'INTELLI_NO_SANDBOX': '1', 'INTELLI_HOME': str(tmp), 'OPENAI_API_KEY': 'x', 'PLAYWRIGHT_BROWSERS_PATH': os.environ.get('PLAYWRIGHT_BROWSERS_PATH', '/opt/pw-browsers')}
	m = measure([str(RUST), 'run', TASKS[task].format(base=base), '--model', 'compat:oracle', '--base-url', f'{oracle}/{rid}/v1', '--headless', '--confirm', 'off', '--max-steps', '40', '--audit-log', str(tmp / 'a.jsonl')], env)
	calls = Oracle.runs[rid]['calls']
	return finish('rust', task, m, calls, m['stdout'].strip(), exit_ok=m['code'] == 0)


def run_python(task: str, base: str) -> dict:
	Site.events.clear()
	env = {**os.environ, 'ANONYMIZED_TELEMETRY': 'false', 'IN_DOCKER': 'true', 'PLAYWRIGHT_BROWSERS_PATH': os.environ.get('PLAYWRIGHT_BROWSERS_PATH', '/opt/pw-browsers')}
	m = measure([sys.executable, str(Path(__file__).with_name('py_agent_run.py')), task, TASKS[task].format(base=base), base], env)
	line = next((l for l in reversed(m['stdout'].splitlines()) if l.startswith('RESULT ')), None)
	if not line:
		return {'agent': 'python', 'task': task, 'ok': False, 'error': (m['stderr'] or m['stdout'])[-400:]}
	r = json.loads(line[7:])
	return finish('python', task, m, r['calls'], r['answer'], exit_ok=r['success'], steps=r['steps'])


def finish(agent, task, m, calls, answer, exit_ok, steps=None) -> dict:
	n = len(calls)
	prefix = calls[0]['prefix'] if calls else 0
	tails = sum(c['tail'] for c in calls)
	raw = sum(c['prefix'] + c['tail'] for c in calls)
	cached = prefix * (1 + 0.1 * max(n - 1, 0)) + tails  # illustrative: cached prefix reads at 10% of the normal price
	return {'agent': agent, 'task': task, 'ok': bool(exit_ok) and check(task, answer), 'calls': n, 'prompt_tokens': raw, 'prefix_tokens': prefix,
		'cache_adjusted_tokens': round(cached), 'completion_tokens_lower_bound': sum(c['completion'] for c in calls), 'wall_s': m['wall_s'],
		'agent_rss_mb': m['agent_rss_mb'], 'browser_rss_mb': m['browser_rss_mb'], 'answer': answer[:80]}


def median(rs, key):
	vals = [r[key] for r in rs if key in r]
	return statistics.median(vals) if vals else float('nan')


def main():
	ap = argparse.ArgumentParser()
	ap.add_argument('--runs', type=int, default=5)
	ap.add_argument('--out', default=str(HERE / 'bench' / 'agent_results.json'))
	a = ap.parse_args()
	base, oracle = start_site(), start_oracle()
	tmp = Path(tempfile.mkdtemp())
	results = []
	for task in TASKS:
		for i in range(a.runs):
			for agent in ('python', 'rust'):
				r = run_rust(task, base, oracle, tmp) if agent == 'rust' else run_python(task, base)
				results.append(r)
				print(f'{agent:6} {task:8} run {i + 1}: ok={r.get("ok")} calls={r.get("calls")} tokens={r.get("prompt_tokens")} wall={r.get("wall_s", 0):.1f}s {r.get("error", "")[:200]}', flush=True)
	Path(a.out).write_text(json.dumps(results, indent=1))
	print('wrote', a.out)


if __name__ == '__main__':
	main()
