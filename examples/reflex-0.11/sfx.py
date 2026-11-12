"""Adds the reel's sound effects through the motion MCP server (`sfx_add`).

Usage: sfx.py <lahza-motion binary> <project dir>
Effects are anchored to the layers they belong to, so they follow the edit.
"""
import json
import subprocess
import sys

SOUNDS = [  # (sound, at, gain dB)
    ("type", [0.5, 1.0, 1.5, 2.0, 2.5, 3.0], -14),
    ("whoosh", "#m-stem.start - 4f", -9),
    ("type", [8.0, 8.5, 9.0, 9.5, 10.0, 10.75], -14),
    ("whoosh", 11.6, -10),
    ("click", [12.0 + 0.5 * i for i in range(8)], -9),
    ("click", [20.25, 22.0], -8),
    ("swipe", [20.5, 22.5], -11),
    ("thud", 21.0, -9),
    ("ding", 23.3, -12),
    ("swipe", [24.5, 25.0], -12),
    ("click", [25.5, 26.0, 26.5, 27.0, 27.5], -8),
    ("pop", [29.0, 30.0, 31.0], -10),
    ("thud", [33.0, 34.0], -8),
    ("whoosh-out", 35.0, -10),
    ("thud", 37.0, -5),
    ("glitch", 37.2, -10),
    ("whoosh", 38.75, -10),
    ("whoosh", 43.72, -8),
    ("type", 46.1, -12),
    ("ding", 47.0, -12),
]


def main(binary, project):
    p = subprocess.Popen([binary, "mcp", "--cpu"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    n = 0

    def send(method, params=None, notify=False):
        nonlocal n
        msg = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            msg["params"] = params
        if not notify:
            n += 1
            msg["id"] = n
        p.stdin.write(json.dumps(msg) + "\n")
        p.stdin.flush()
        if notify:
            return None
        while True:
            reply = json.loads(p.stdout.readline())
            if reply.get("id") == n:
                return reply

    send("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "reel-sfx", "version": "1"}})
    send("notifications/initialized", notify=True)
    for sound, at, gain in SOUNDS:
        r = send("tools/call", {"name": "sfx_add", "arguments": {"project": project, "sound": sound, "at": at, "gain_db": gain}})
        if "error" in r or r["result"].get("isError"):
            sys.exit(f"sfx_add {sound} {at}: {json.dumps(r)[:400]}")
    p.stdin.close()
    p.wait()
    print("added", sum(len(a) if isinstance(a, list) else 1 for _, a, _ in SOUNDS), "sound effects")


if __name__ == "__main__":
    main(*sys.argv[1:3])
