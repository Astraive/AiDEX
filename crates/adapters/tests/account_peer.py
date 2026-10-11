"""Official-command-shaped local account peer. No network or real credentials."""
import json, os, sys, time
home = os.environ['CODEX_HOME']
assert home != os.path.expanduser('~/.codex')
path = os.path.join(home, 'auth.json')
if 'status' in sys.argv:
    if os.path.isfile(path):
        print('Logged in using ChatGPT', file=sys.stderr)
        sys.exit(0)
    print('Not logged in', file=sys.stderr)
    sys.exit(1)
if 'logout' in sys.argv:
    if os.path.isfile(path): os.remove(path)
    sys.exit(0)
assert 'login' in sys.argv and 'cli_auth_credentials_store="file"' in sys.argv
if 'pending' in sys.argv:
    json.dump({'tokens': {'access_token':'temporary-opaque-secret'}}, open(path,'w'))
    print('https://auth.openai.com/oauth/authorize?state=fixture&code_challenge=fixture', flush=True)
    time.sleep(60)
else:
    json.dump({'tokens': {'access_token':'fixture-opaque-secret'}}, open(path,'w'))
    print('credential-bearing arbitrary-secret-that-must-never-be-returned', file=sys.stderr)
