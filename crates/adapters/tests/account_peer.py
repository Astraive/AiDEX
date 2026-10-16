"""Official-command-shaped local account peer. No network or real credentials."""
import json, os, sys, time
home = os.environ['CODEX_HOME']
assert home != os.path.expanduser('~/.codex')
assert 'cli_auth_credentials_store="file"' in sys.argv
path = os.path.join(home, 'auth.json')
AUTH_URL = 'https://auth.openai.com/oauth/authorize?response_type=code&client_id=fixture&code_challenge=fixture&code_challenge_method=S256&state=app-server-fixture'
if 'status' in sys.argv:
    if os.path.isfile(path):
        print('Logged in using ChatGPT', file=sys.stderr)
        sys.exit(0)
    print('Not logged in', file=sys.stderr)
    sys.exit(1)
if 'logout' in sys.argv:
    if os.path.isfile(path): os.remove(path)
    sys.exit(0)
# `codex login` opens the user's browser itself; on Windows that browser
# would join the login's kill-on-close job. AiDEX must never run it.
assert 'login' not in sys.argv, 'the browser-opening login command was used'
assert 'app-server' in sys.argv
def send(v):
    print(json.dumps(v), flush=True)
def read():
    line = sys.stdin.readline()
    if not line: sys.exit(0)
    return json.loads(line)
r = read(); assert r['method'] == 'initialize' and r['params']['clientInfo']['name'] == 'AiDEX'
send({'id': r['id'], 'result': {}})
r = read(); assert r['method'] == 'initialized'
r = read(); assert r['method'] == 'account/login/start' and r['params'] == {'type': 'chatgpt'}
if 'unsupported' in sys.argv:
    send({'id': r['id'], 'error': {'code': -32601, 'message': 'credential-bearing arbitrary-secret-that-must-never-be-returned'}})
    while True: read()
# The app-server returns the page to open and opens no browser.
send({'id': r['id'], 'result': {'type': 'chatgpt', 'loginId': 'fixture-login', 'authUrl': AUTH_URL}})
if 'pending' in sys.argv:
    json.dump({'tokens': {'access_token': 'temporary-opaque-secret'}}, open(path, 'w'))
    time.sleep(60)
elif 'fail' in sys.argv:
    print('credential-bearing arbitrary-secret-that-must-never-be-returned', file=sys.stderr, flush=True)
    send({'method': 'account/login/completed', 'params': {'loginId': 'fixture-login', 'success': False, 'error': 'credential-bearing arbitrary-secret-that-must-never-be-returned'}})
else:
    json.dump({'tokens': {'access_token': 'fixture-opaque-secret'}}, open(path, 'w'))
    print('credential-bearing arbitrary-secret-that-must-never-be-returned', file=sys.stderr, flush=True)
    # A completion of another login is not this one's.
    send({'method': 'account/login/completed', 'params': {'loginId': 'other-login', 'success': False, 'error': None}})
    send({'method': 'account/login/completed', 'params': {'loginId': 'fixture-login', 'success': True, 'error': None}})
# Like the app-server, it runs until its input closes.
while True: read()
