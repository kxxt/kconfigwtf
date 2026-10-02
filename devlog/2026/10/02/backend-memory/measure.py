"""Measure a real server process, including startup and concurrent API traffic."""
import concurrent.futures
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import threading
import time
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary")
parser.add_argument("destination")
parser.add_argument("--cache-dir")
parser.add_argument("--concurrency", type=int, default=8)
parser.add_argument("--requests", type=int, default=80)
parser.add_argument("--max-rss-mib", type=float)
args = parser.parse_args()
binary, destination = args.binary, args.destination
base = "http://127.0.0.1:3187"
samples = []
stop = threading.Event()

with open(destination + ".log", "w") as log:
    command = [binary, "serve", "--data-dir", "data", "--listen", "127.0.0.1:3187"]
    if args.cache_dir:
        command += ["--cache-dir", args.cache_dir]
    process = subprocess.Popen(
        command,
        stdout=log, stderr=log,
    )
    def memory():
        while not stop.is_set():
            try:
                fields = dict(line.split(":", 1) for line in Path(f"/proc/{process.pid}/status").read_text().splitlines())
                samples.append({k: int(fields[k].split()[0]) / 1024 for k in ("VmRSS", "VmHWM")})
            except (FileNotFoundError, KeyError):
                pass
            stop.wait(.02)
    monitor = threading.Thread(target=memory)
    monitor.start()
    def fetch(path):
        start = time.monotonic()
        with urllib.request.urlopen(base + path, timeout=90) as response:
            body = response.read()
            return {"path": path, "ms": (time.monotonic() - start) * 1000,
                    "sha256": hashlib.sha256(body).hexdigest(), "bytes": len(body)}, body
    try:
        start = time.monotonic()
        while True:
            if process.poll() is not None:
                raise RuntimeError(f"Server exited: {process.returncode}; see {destination}.log")
            try:
                fetch("/healthz")
                break
            except OSError:
                if time.monotonic() - start > 300:
                    raise TimeoutError("Server startup exceeded 300 seconds")
                time.sleep(.1)
        result = {"binary": binary, "command": command, "startup_seconds": time.monotonic() - start,
                  "idle_rss_mib": samples[-1]["VmRSS"],
                  "startup_peak_mib": max(s["VmHWM"] for s in samples)}
        paths = ["/api/v1/configs", "/api/v1/configs/BPF", "/api/v1/configs/EXT4_FS", "/api/v1/configs/LOCALVERSION", "/api/v1/configs/ARM64", "/api/v1/configs/X86"]
        responses = [fetch(path) for path in paths]
        result["responses"] = [r for r, _ in responses]
        raw_path = json.loads(responses[1][1])["records"][0]["config_url"]
        result["raw"] = fetch(raw_path)[0]
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
            timings = list(pool.map(lambda n: fetch(paths[1 + n % (len(paths) - 1)])[0]["ms"], range(args.requests)))
        result["load"] = {"requests": args.requests, "concurrency": args.concurrency, "mean_ms": sum(timings) / len(timings), "max_ms": max(timings)}
        time.sleep(.1)
        result["peak_rss_mib"] = max(s["VmHWM"] for s in samples)
        result["after_load_rss_mib"] = samples[-1]["VmRSS"]
        Path(destination).write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2), flush=True)
        if args.max_rss_mib is not None and result["peak_rss_mib"] >= args.max_rss_mib:
            raise RuntimeError(f"Peak RSS {result['peak_rss_mib']} MiB exceeds target {args.max_rss_mib} MiB")
    finally:
        process.terminate()
        process.wait(timeout=15)
        stop.set()
        monitor.join()
