"""A tiny MCP stdio server for the aidex-mcp tests.

Newline-delimited JSON-RPC 2.0 on stdin/stdout, diagnostics on stderr.
`argv[1]` selects a scenario: `normal`, `old-version` (answers initialize with
an unsupported protocol version), `exit` (exits during initialize), `silent`
(never answers initialize), `flood-init` (never answers initialize, sends
pings instead, and stops reading its input), `noisy` (prints a non-JSON line
first), or `chatty` (also lists the `flood` and `unsolicited` tools, which every
scenario answers).
"""

import json
import os
import sys
import time

SCENARIO = sys.argv[1] if len(sys.argv) > 1 else "normal"
PAGE_SIZE = 2
TOOLS = [
    {
        "name": "echo",
        "description": "Echo the given text",
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    },
    {
        "name": "add",
        "description": "Add two numbers",
        "inputSchema": {
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}}
        },
    },
    {"name": "fail", "description": "Always reports a tool error", "inputSchema": {"type": "object"}},
    {"name": "env", "description": "Report environment variables", "inputSchema": {"type": "object"}},
    {"name": "image", "description": "Return an image", "inputSchema": {"type": "object"}},
    {"name": "slow", "description": "Answer after a delay", "inputSchema": {"type": "object"}},
    {"name": "crash", "description": "Exit the server", "inputSchema": {"type": "object"}},
]
CHATTY_TOOLS = [
    {
        "name": "flood",
        "description": "Send ping requests, then stop reading input for a while",
        "inputSchema": {"type": "object"},
    },
    {
        "name": "unsolicited",
        "description": "Send responses to requests the client never made",
        "inputSchema": {"type": "object"},
    },
]
if SCENARIO == "chatty":
    TOOLS = TOOLS + CHATTY_TOOLS


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def log(text):
    sys.stderr.write(text + "\n")
    sys.stderr.flush()


def result(request_id, value):
    send({"jsonrpc": "2.0", "id": request_id, "result": value})


def error(request_id, code, message):
    send({"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}})


def text(value, is_error=False):
    return {"content": [{"type": "text", "text": value}], "isError": is_error}


def call(request_id, params):
    name = params.get("name")
    arguments = params.get("arguments") or {}
    if name == "echo":
        result(request_id, text(arguments.get("text", "")))
    elif name == "add":
        result(request_id, text(str(arguments.get("a", 0) + arguments.get("b", 0))))
    elif name == "fail":
        result(request_id, text("the fixture tool failed", is_error=True))
    elif name == "env":
        names = arguments.get("names", [])
        result(request_id, text(json.dumps({key: os.environ.get(key) for key in names})))
    elif name == "image":
        result(
            request_id,
            {"content": [{"type": "image", "data": "aGVsbG8=", "mimeType": "image/png"}]},
        )
    elif name == "slow":
        time.sleep(float(arguments.get("seconds", 5)))
        result(request_id, text("late"))
    elif name == "crash":
        log("fixture crashing on request")
        sys.exit(3)
    elif name == "flood":
        result(request_id, text("flooding"))
        for n in range(int(arguments.get("pings", 20000))):
            send({"jsonrpc": "2.0", "id": "flood-%d" % n, "method": "ping"})
        # Input is not read meanwhile, so the client's answers fill the pipe.
        time.sleep(float(arguments.get("seconds", 30)))
    elif name == "unsolicited":
        result(request_id, text("sent"))
        pad = "u" * int(arguments.get("bytes", 1000))
        for n in range(int(arguments.get("count", 50))):
            send({"jsonrpc": "2.0", "id": 900000 + n, "result": {"pad": pad}})
        # A second answer to the request already answered.
        result(request_id, text("again"))
    else:
        error(request_id, -32602, "Unknown tool: %s" % name)


def main():
    log("fixture starting (%s)" % SCENARIO)
    if SCENARIO == "noisy":
        sys.stdout.write("booting the fixture server\n")
        sys.stdout.flush()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        method = message.get("method")
        request_id = message.get("id")
        if method is None:
            continue  # a reply to one of our requests
        if request_id is None:
            if method == "notifications/initialized":
                log("fixture initialized")
            continue
        if method == "initialize":
            if SCENARIO == "exit":
                log("fixture refusing to start")
                sys.exit(2)
            if SCENARIO == "silent":
                continue
            if SCENARIO == "flood-init":
                for n in range(20000):
                    send({"jsonrpc": "2.0", "id": "init-%d" % n, "method": "ping"})
                # Input is not read again, so the client's answers fill the pipe.
                time.sleep(120)
                continue
            version = message["params"]["protocolVersion"]
            if SCENARIO == "old-version":
                version = "1999-01-01"
            result(
                request_id,
                {
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": True}},
                    "serverInfo": {"name": "fixture", "version": "1.0.0"},
                },
            )
        elif method == "tools/list":
            cursor = (message.get("params") or {}).get("cursor")
            start = int(cursor) if cursor else 0
            page = {"tools": TOOLS[start : start + PAGE_SIZE]}
            if start + PAGE_SIZE < len(TOOLS):
                page["nextCursor"] = str(start + PAGE_SIZE)
            send(
                {
                    "jsonrpc": "2.0",
                    "method": "notifications/message",
                    "params": {"level": "info", "data": "listing from %d" % start},
                }
            )
            if start == 0:
                # A server request the client must answer (it declines it).
                send({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"})
            result(request_id, page)
        elif method == "tools/call":
            call(request_id, message.get("params") or {})
        elif method == "ping":
            result(request_id, {})
        else:
            error(request_id, -32601, "Method not found: %s" % method)
    log("fixture input closed")


main()
