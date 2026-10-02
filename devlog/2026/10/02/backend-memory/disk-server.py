"""Local decision UI; run from any directory and open http://127.0.0.1:3189."""
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent

class Handler(BaseHTTPRequestHandler):
    def send(self, status, body, content_type="application/json"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/":
            self.send(200, (ROOT / "disk.html").read_bytes(), "text/html; charset=utf-8")
        elif self.path == "/decision":
            path = ROOT / "decision-v2.json"
            self.send(200, path.read_bytes() if path.exists() else b"null")
        elif self.path == "/measurements":
            self.send(200, (ROOT / "disk-prototype-result.json").read_bytes())
        elif self.path == "/compatibility":
            self.send(200, (ROOT / "disk-compatibility.json").read_bytes())
        elif self.path == "/baseline":
            self.send(200, (ROOT / "baseline.json").read_bytes())
        else:
            self.send(404, b'{}')

    def do_POST(self):
        if self.path != "/decision":
            return self.send(404, b'{}')
        if self.headers.get("Origin") != "http://127.0.0.1:3189":
            return self.send(403, b'{"error":"Open this page at http://127.0.0.1:3189"}')
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 16384:
                raise ValueError("Invalid request length")
            data = json.loads(self.rfile.read(length))
            if data.get("choice") not in ("implement_disk", "persistent", "revise"):
                raise ValueError("Select an option")
            if not isinstance(data.get("notes", ""), str):
                raise ValueError("Notes must be text")
            data = {"choice": data["choice"], "notes": data.get("notes", ""),
                    "proposal_version": 2, "saved_at": datetime.now(timezone.utc).isoformat()}
            temporary = ROOT / "decision-v2.tmp"
            temporary.write_text(json.dumps(data, indent=2) + "\n")
            temporary.replace(ROOT / "decision-v2.json")
            print("Decision saved: " + json.dumps(data), flush=True)
            self.send(200, json.dumps(data).encode())
        except (ValueError, TypeError) as error:
            self.send(400, json.dumps({"error": str(error)}).encode())

if __name__ == "__main__":
    print("Decision page: http://127.0.0.1:3189", flush=True)
    ThreadingHTTPServer(("127.0.0.1", 3189), Handler).serve_forever()
