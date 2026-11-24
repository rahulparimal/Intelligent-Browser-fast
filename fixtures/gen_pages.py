#!/usr/bin/env python3
"""Generate deterministic test pages. Output: fixtures/pages/*.html (checked in; regenerate with `python3 fixtures/gen_pages.py`)."""
from pathlib import Path
import random

out = Path(__file__).parent / 'pages'
out.mkdir(exist_ok=True)
rnd = random.Random(7)
WORDS = 'quarterly budget planning travel policy security laptop onboarding benefits holiday remote expense review hiring forecast team schedule project report client contract invoice'.split()


def sentence(n=14):
	return ' '.join(rnd.choice(WORDS) for _ in range(n)).capitalize() + '.'


def page(title, body, extra_css=''):
	return f'<!doctype html><html lang="en"><head><meta charset="utf-8"><title>{title}</title><style>body{{font:16px sans-serif;margin:0;padding:12px}}{extra_css}</style></head><body>{body}</body></html>'


(out / 'login.html').write_text(page('Login', '''
<h1>Sign in</h1>
<label>Email <input id="email" type="email" required></label>
<label>Password <input id="pw" type="password"></label>
<label><input type="checkbox" id="remember"> Remember me</label>
<button id="go">Sign in</button> <button id="buy">Place order</button>
<a href="/forgot">Forgot password?</a>
<div id="result"></div>'''))

nav = ''.join(f'<a href="/c/{i}">Category {i}</a> ' for i in range(1, 9))
cards = ''.join(
	f'<li class="card"><a href="/p/{i}">{rnd.choice(WORDS).title()} {rnd.choice(WORDS)} model {i}</a> <span>${rnd.randint(5, 500)}.99</span> <button>Add to cart</button></li>'
	for i in range(1, 121))
foot = ''.join(f'<a href="/f/{i}">Footer link {i}</a> ' for i in range(1, 11))
(out / 'products.html').write_text(page('Shop: all products', f'''
<header><nav>{nav}</nav><form role="search"><input type="search" placeholder="Search products" aria-label="Search"><button>Search</button></form></header>
<h1>All products</h1><p>{sentence(20)}</p><ul style="list-style:none;padding:0">{cards}</ul><footer>{foot}</footer>'''))

paras = ''.join(f'<h2>Section {i}</h2><p>{sentence(40)} <strong>{rnd.choice(WORDS)}</strong> {sentence(25)} <a href="/ref/{i}">reference {i}</a> {sentence(30)}</p><p>{sentence(60)}</p>' for i in range(1, 13))
(out / 'article.html').write_text(page('A long article about planning', f'<article><h1>Planning in practice</h1><p class="lead">{sentence(30)}</p>{paras}</article>'))

rows = ''.join(f'<tr><td><a href="/emp/{i}">Employee {i}</a></td><td>{rnd.choice(["Sales","Eng","Ops","HR"])}</td><td>{rnd.randint(30,120)}k</td><td>{rnd.randint(1,28)} days</td></tr>' for i in range(1, 61))
(out / 'table.html').write_text(page('Directory', f'<h1>Staff directory</h1><table><caption>All staff</caption><thead><tr><th>Name</th><th>Team</th><th>Salary</th><th>Leave</th></tr></thead><tbody>{rows}</tbody></table>'))

menus = ''.join(f'<div class="menu"><h3>Department {c}</h3><ul>' + ''.join(f'<li><a href="/d/{c}/{i}">Item {c}-{i}</a></li>' for i in range(1, 13)) + '</ul></div>' for c in range(1, 13))
(out / 'menu.html').write_text(page('Mega menu', f'<nav aria-label="Main">{menus}</nav><main><h1>Welcome</h1><p>{sentence(30)}</p><button>Get started</button></main>'))

(out / 'form.html').write_text(page('Application form', '''
<h1>Job application</h1>
<form><label>Full name <input name="n" required></label>
<label>Email <input type="email" name="e" required placeholder="you@example.com"></label>
<label>Position <select name="p"><option>Engineer</option><option selected>Designer</option><option>Manager</option></select></label>
<fieldset><legend>Work mode</legend><label><input type="radio" name="m" value="r" checked> Remote</label> <label><input type="radio" name="m" value="o"> Office</label></fieldset>
<label><input type="checkbox" name="t" checked> I accept the terms</label>
<label>Cover letter <textarea name="c" rows="4">Hello</textarea></label>
<button type="submit" disabled>Submit application</button> <button type="reset">Reset</button></form>'''))
print('pages written:', sorted(p.name for p in out.iterdir()))
