#!/usr/bin/env python3
"""Test double for spillway hooks (what your application would run).

    POST /spillway/validate   decides per file:
        path contains "/drop-"  -> drop (reason "marked drop")
        mode "hold"             -> first time an id is seen: hold 600 ms, then move
        otherwise               -> move, labels {"group": <first path dir>, "ext": <extension>}
    POST /spillway/moved      records every event (at-least-once: counts duplicates)
    GET  /state               counters, batch sizes, received moved ids
    POST /control             {"validate": "ok|slow|error|hold", "moved": "ok|error", "delay": s, "reset": true}

Token: both endpoints require "Authorization: Bearer hook-token".
"""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TOKEN = "hook-token"
lock = threading.Lock()
state = {}


def reset():
    state.clear()
    state.update(
        validate_mode="ok",
        moved_mode="ok",
        delay=3.0,
        validate_calls=0,
        validate_files=0,
        batch_sizes=[],
        decisions={"move": 0, "drop": 0, "hold": 0},
        held=set(),
        moved_calls=0,
        moved_events=0,
        moved_ids={},
        unauthorized=0,
    )


reset()


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def reply(self, code, obj):
        body = json.dumps(obj, default=list).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def body(self):
        n = int(self.headers.get("content-length") or 0)
        return json.loads(self.rfile.read(n) or b"{}")

    def do_GET(self):
        if self.path == "/state":
            with lock:
                s = dict(state)
                s["held"] = len(state["held"])
                s["moved_unique"] = len(state["moved_ids"])
                s["moved_dupes"] = sum(c - 1 for c in state["moved_ids"].values())
            return self.reply(200, s)
        self.reply(404, {})

    def do_POST(self):
        data = self.body()
        if self.path == "/control":
            with lock:
                if data.get("reset"):
                    reset()
                if "validate" in data:
                    state["validate_mode"] = data["validate"]
                if "moved" in data:
                    state["moved_mode"] = data["moved"]
                if "delay" in data:
                    state["delay"] = float(data["delay"])
            return self.reply(200, {"ok": True})
        if self.headers.get("authorization") != f"Bearer {TOKEN}":
            with lock:
                state["unauthorized"] += 1
            return self.reply(401, {"error": "bad token"})
        if self.path == "/spillway/validate":
            with lock:
                mode = state["validate_mode"]
                delay = state["delay"]
                state["validate_calls"] += 1
                state["validate_files"] += len(data.get("files", []))
                state["batch_sizes"].append(len(data.get("files", [])))
            if mode == "slow":
                time.sleep(delay)
            if mode == "error":
                return self.reply(500, {"error": "boom"})
            out = []
            with lock:
                for f in data.get("files", []):
                    path = f["path"]
                    if "/drop-" in path:
                        out.append({"id": f["id"], "action": "drop", "reason": "marked drop", "labels": {"group": path.split("/")[0]}})
                        state["decisions"]["drop"] += 1
                    elif mode == "hold" and f["id"] not in state["held"]:
                        state["held"].add(f["id"])
                        out.append({"id": f["id"], "action": "hold", "retry_after_ms": 600})
                        state["decisions"]["hold"] += 1
                    else:
                        ext = path.rsplit(".", 1)[-1]
                        out.append({"id": f["id"], "action": "move", "labels": {"group": path.split("/")[0], "ext": ext}})
                        state["decisions"]["move"] += 1
            return self.reply(200, {"decisions": out})
        if self.path == "/spillway/moved":
            with lock:
                state["moved_calls"] += 1
                if state["moved_mode"] == "error":
                    return self.reply(503, {"error": "down"})
                for f in data.get("files", []):
                    state["moved_events"] += 1
                    state["moved_ids"][f["path"]] = state["moved_ids"].get(f["path"], 0) + 1
            return self.reply(200, {"ok": True})
        self.reply(404, {})


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 7090
    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
