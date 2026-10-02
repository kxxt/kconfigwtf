"""Compare a deterministic config sample and HTTP semantics, sequential servers."""
import hashlib
import argparse
import json
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent
BASE = "http://127.0.0.1:3187"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("baseline_binary")
parser.add_argument("candidate_binary")
parser.add_argument("--cache-dir")
parser.add_argument("--output", default=str(ROOT / "final-compatibility.json"))
args = parser.parse_args()

def fetch(path, method="GET", headers=None):
    request = urllib.request.Request(BASE + path, method=method, headers=headers or {})
    try:
        response = urllib.request.urlopen(request, timeout=30)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        body = response.read()
        return {"status": response.status, "bytes": len(body), "sha256": hashlib.sha256(body).hexdigest(),
                "headers": {key: response.headers.get(key) for key in ["Content-Type", "Content-Length", "Cache-Control", "CDN-Cache-Control", "Cloudflare-CDN-Cache-Control", "ETag", "Allow"]}}, body

def snapshot(binary, paths=None, cache_dir=None):
    with (ROOT / "compare.log").open("w") as log:
        command = [binary, "serve", "--data-dir", "data", "--listen", "127.0.0.1:3187"]
        if cache_dir:
            command += ["--cache-dir", cache_dir]
        process = subprocess.Popen(command, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 180
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f"Server exited: {process.returncode}")
                try:
                    fetch("/healthz")
                    break
                except OSError:
                    if time.monotonic() > deadline:
                        raise TimeoutError("Startup timeout")
                    time.sleep(.1)
            if paths is None:
                manifest = json.loads(fetch("/api/v1/configs")[1])["configs"]
                names = sorted(set(manifest[index * (len(manifest) - 1) // 127] for index in range(128)))
                paths = ["/api/v1/configs/" + name for name in names]
                paths += ["/api/v1/configs", "/api/v1/configs/BPF", "/api/v1/configs/CONFIG_BPF", "/api/v1/configs/DOES_NOT_EXIST_987654321", "/api/v1/configs/a%2Fb", "/api/v1/configs/%FF", "/healthz", "/", "/app.js", "/styles.css", "/CONFIG_/BPF/"]
            results = {path: fetch(path)[0] for path in paths}
            etag = results["/api/v1/configs/BPF"]["headers"]["ETag"]
            results["HEAD BPF"] = fetch("/api/v1/configs/BPF", "HEAD")[0]
            results["conditional BPF"] = fetch("/api/v1/configs/BPF", headers={"If-None-Match": etag})[0]
            results["POST BPF"] = fetch("/api/v1/configs/BPF", "POST")[0]
            return paths, results
        finally:
            process.terminate()
            process.wait(timeout=15)

paths, baseline = snapshot(args.baseline_binary)
print(f"Captured {len(baseline)} baseline cases", flush=True)
_, candidate = snapshot(args.candidate_binary, paths, args.cache_dir)
differences = [key for key in baseline if baseline[key] != candidate[key]]
result = {"cases": len(baseline), "different_cases": differences, "baseline": baseline, "disk_prototype": candidate}
Path(args.output).write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps({"cases": len(baseline), "different_cases": differences}), flush=True)
raise SystemExit(bool(differences))
