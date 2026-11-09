"""Drives `<command> mcp` over stdio and checks that every stdout line is a
JSON-RPC 2.0 message and that tools/list lists `apply`. Run with python3 -I.

    python3 -I scripts/tests/mcp_stdout.py <command> [args...]
"""
import json
import subprocess
import sys
import threading
import time

msgs = [
    {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {},
        "clientInfo": {"name": "packaging-test", "version": "0"}}},
    {"jsonrpc": "2.0", "method": "notifications/initialized"},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
]
proc = subprocess.Popen(sys.argv[1:], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL)
lines = []
reader = threading.Thread(target=lambda: lines.extend(proc.stdout), daemon=True)
reader.start()
for m in msgs:
    proc.stdin.write((json.dumps(m) + "\n").encode())
    proc.stdin.flush()
    time.sleep(0.5)
# Keep stdin open for a while: a late banner or log line would show up here.
time.sleep(10)
proc.stdin.close()
try:
    proc.wait(timeout=10)
except subprocess.TimeoutExpired:
    proc.kill()
    sys.exit("the server did not exit after stdin closed")
reader.join(timeout=5)

tools = None
for raw in lines:
    try:
        msg = json.loads(raw)
    except ValueError:
        sys.exit("not JSON on stdout: %r" % raw[:200])
    if msg.get("jsonrpc") != "2.0":
        sys.exit("not JSON-RPC 2.0 on stdout: %r" % raw[:200])
    if msg.get("id") == 2:
        tools = [t["name"] for t in msg["result"]["tools"]]
if tools is None:
    sys.exit("no tools/list response (%d stdout lines)" % len(lines))
if "apply" not in tools:
    sys.exit("tools/list lacks apply: %s" % tools)
print("ok: %d lines, tools %s" % (len(lines), ", ".join(tools)))
