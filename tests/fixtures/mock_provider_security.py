#!/usr/bin/env python3
"""Local TLS provider for auth redaction and cross-origin redirect refusal."""
import json
import ssl
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

mode, cert, key, leaked, captured = sys.argv[1:]
TOKEN = "test-only-local-provider-secret"


class Sink(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        Path(leaked).write_text("Unexpected redirected request")
        self.send_response(200)
        self.end_headers()

    do_POST = do_GET


sink = ThreadingHTTPServer(("127.0.0.1", 0), Sink)
threading.Thread(target=sink.serve_forever, daemon=True).start()


class Provider(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        good = self.headers.get("Authorization") == f"Bearer {TOKEN}" and TOKEN.encode() not in body and self.path == "/v1/chat/completions"
        Path(captured).write_text(json.dumps({"bearer_ok": good, "path": self.path}))
        if mode == "redirect":
            self.send_response(303)
            self.send_header("Location", f"http://127.0.0.1:{sink.server_port}/steal")
            self.send_header("Content-Length", "0")
            self.end_headers()
        else:
            data = json.dumps({"error": {"message": f"Invalid key: {TOKEN}"}}).encode()
            self.send_response(403)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)


server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)
server.socket = context.wrap_socket(server.socket, server_side=True)
print(server.server_port, flush=True)
server.serve_forever()
