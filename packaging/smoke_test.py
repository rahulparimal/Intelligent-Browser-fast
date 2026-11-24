"""Smoke-test a built app: the CLI works, index + search works, the web page needs its launch token, and the program exits.

Usage: smoke_test.py CLI_PATH LAUNCHER_PATH      (run on the machine that built it; Windows, macOS and Linux)
"""

import http.cookiejar
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path


def run(cmd, env, timeout=120):
	r = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=timeout, check=False)
	print(f'$ {" ".join(map(str, cmd))} -> {r.returncode}\n{(r.stdout + r.stderr).strip()[:500]}')
	return r


def main(cli: str, launcher: str) -> int:
	tmp = Path(tempfile.mkdtemp())
	env = {**os.environ, 'INTELLI_HOME': str(tmp / 'cfg')}
	docs = tmp / 'docs'
	docs.mkdir()
	(docs / 'leave.md').write_text('Employees receive twenty five days of annual leave per year.\n\nCarry over is capped at five days.')
	failures = []

	def check(ok, what):
		print(('PASS ' if ok else 'FAIL ') + what)
		if not ok:
			failures.append(what)

	check(run([cli, '--help'], env).returncode == 0, 'CLI --help')
	run([cli, 'doctor'], env)  # informational: a CI machine may lack a browser or API key
	check(run([cli, 'index', str(docs)], env).returncode == 0, 'index a folder')
	r = run([cli, 'search', 'annual leave'], env)
	check(r.returncode == 0 and 'leave' in r.stdout.lower(), 'search finds the document')
	r = run([cli, 'search', 'anual leev'], env)
	check(r.returncode == 0 and 'leave' in r.stdout.lower(), 'a misspelled search still finds it')
	check(Path(launcher).exists() and os.access(launcher, os.X_OK), 'the launcher is present and executable')

	# the web page: start it, read the printed link, check the token is required, then stop it
	proc = subprocess.Popen([cli, 'ui', '--no-open', '--port', '0'], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
	try:
		url = None
		deadline = time.time() + 30
		while time.time() < deadline and url is None:
			line = proc.stderr.readline()
			if not line and proc.poll() is not None:
				break
			m = re.search(r'(http://localhost:\d+/\?t=[0-9a-f]+)', line)
			url = m.group(1) if m else None
		check(url is not None, 'the page starts and prints a link with a launch token')
		if url:
			base = url.split('/?t=')[0]
			try:
				urllib.request.urlopen(base + '/api/search?q=leave', timeout=5)
				check(False, 'a request without the token is refused')
			except urllib.error.HTTPError as e:
				check(e.code == 403, 'a request without the token is refused')
			opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
			page = opener.open(url, timeout=5).read().decode()
			check('Intelli Search' in page, 'the link with the token opens the page')
			hits = json.loads(opener.open(base + '/api/search?q=annual+leave', timeout=5).read())
			check(len(hits['results']) >= 1, 'the page can search')
	finally:
		proc.terminate()
		try:
			proc.wait(timeout=10)
		except subprocess.TimeoutExpired:
			proc.kill()
	print('\nFAILED: ' + ', '.join(failures) if failures else '\nall checks passed')
	return 1 if failures else 0


if __name__ == '__main__':
	sys.exit(main(sys.argv[1], sys.argv[2]))
