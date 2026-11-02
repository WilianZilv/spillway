#!/usr/bin/env python3
"""Hook receiver for the demo video: answers validate with readable labels
(camera / empresa / stream) and refuses segments named drop-* with a
realistic reason; acknowledges moved batches. Usage: demohook.py [port]"""
import hashlib
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CAMERAS = ["Portaria", "Garagem", "Recepção", "Doca 2", "Estoque", "Corredor B", "Pátio", "Caixa 1", "Caixa 2", "Refeitório", "Escada", "Elevador"]
EMPRESAS = ["Loja Centro", "Matriz", "CD Norte", "Filial Sul"]
REASONS = ["sem detecção de movimento", "câmera fora do horário de gravação", "contrato sem gravação contínua"]


def pick(seq, key):
    return seq[int(hashlib.md5(key.encode()).hexdigest(), 16) % len(seq)]


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, obj):
        b = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))) or b"{}")
        if self.path.endswith("/validate"):
            out = []
            for f in body.get("files", []):
                top = f["path"].split("/")[0]
                cam = top.removesuffix("-sub")
                labels = {"camera": pick(CAMERAS, cam), "empresa": pick(EMPRESAS, cam), "stream": "sub" if top.endswith("-sub") else "principal"}
                if "/drop-" in f["path"]:
                    out.append({"id": f["id"], "action": "drop", "reason": pick(REASONS, f["path"]), "labels": labels})
                else:
                    out.append({"id": f["id"], "action": "move", "labels": labels})
            self.reply({"decisions": out})
        else:
            self.reply({"ok": True})


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 7091), H).serve_forever()
