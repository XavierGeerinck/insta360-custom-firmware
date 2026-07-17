#!/usr/bin/env python3
"""Small bounded receiver for testing Wave's raw multipart webhook."""

import argparse
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from tempfile import NamedTemporaryFile

CHUNK_BYTES = 64 * 1024
MAX_BYTES = int(os.environ.get("WAVE_WEBHOOK_MAX_BYTES", str(4 * 1024**3)))
EXPECTED_TOKEN = os.environ.get("WAVE_WEBHOOK_TOKEN", "")


class Handler(BaseHTTPRequestHandler):
    server_version = "WaveWebhookReceiver/1"

    def do_POST(self):
        if EXPECTED_TOKEN and self.headers.get("Authorization") != f"Bearer {EXPECTED_TOKEN}":
            self.send_error(401)
            return
        try:
            length = int(self.headers.get("Content-Length", ""))
        except ValueError:
            self.send_error(411, "Content-Length required")
            return
        if length <= 0 or length > MAX_BYTES:
            self.send_error(413, "Request outside configured size bound")
            return

        output = self.server.output
        output.mkdir(parents=True, exist_ok=True)
        key = self.headers.get("Idempotency-Key", "unknown")
        safe_key = "".join(c for c in key if c.isalnum() or c in "-_")[:80] or "unknown"
        with NamedTemporaryFile(prefix=f"{safe_key}-", suffix=".new", dir=output, delete=False) as tmp:
            remaining = length
            while remaining:
                chunk = self.rfile.read(min(CHUNK_BYTES, remaining))
                if not chunk:
                    Path(tmp.name).unlink(missing_ok=True)
                    self.send_error(400, "Request body ended early")
                    return
                tmp.write(chunk)
                remaining -= len(chunk)
            tmp.flush()
            os.fsync(tmp.fileno())
            final = output / f"{safe_key}.multipart"
            os.replace(tmp.name, final)

        payload = json.dumps({"accepted": True, "idempotency_key": key}).encode()
        self.send_response(202)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)
        print(f"accepted {length} bytes as {final}")

    def log_message(self, pattern, *args):
        print(f"{self.client_address[0]} - {pattern % args}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bind", default="0.0.0.0")
    parser.add_argument("--port", type=int, default=8787)
    parser.add_argument("--output", type=Path, default=Path("./webhook-inbox"))
    args = parser.parse_args()
    server = ThreadingHTTPServer((args.bind, args.port), Handler)
    server.output = args.output
    print(f"listening on http://{args.bind}:{args.port}; output={args.output}")
    server.serve_forever()


if __name__ == "__main__":
    main()
