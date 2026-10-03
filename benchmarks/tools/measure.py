"""Reproducible local measurements; no frontier token/task-success claims.

Python is a contributor tool only. End-user VEXA binaries do not require it.
"""
import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import queue
import shutil
import subprocess
import tempfile
import threading
import time


def percentile(values, fraction):
    return sorted(values)[max(0, math.ceil(len(values) * fraction) - 1)]


def peak_memory(pid):
    if os.name == "nt":
        from ctypes import wintypes
        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("faults", wintypes.DWORD)] + [
                (name, ctypes.c_size_t) for name in (
                    "peak", "working", "paged_peak", "paged", "nonpaged_peak",
                    "nonpaged", "pagefile", "pagefile_peak")]
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
        kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
        psapi.GetProcessMemoryInfo.argtypes = (wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD)
        handle = kernel.OpenProcess(0x0400 | 0x0010, False, pid)
        if not handle:
            return None
        try:
            counters = Counters()
            counters.cb = ctypes.sizeof(counters)
            if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
                return counters.peak
        finally:
            kernel.CloseHandle(handle)
        return None
    status = Path(f"/proc/{pid}/status")
    if status.exists():
        for line in status.read_text().splitlines():
            if line.startswith("VmHWM:"):
                return int(line.split()[1]) * 1024
    return None


def process_cpu_seconds(pid):
    if os.name == "nt":
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
        kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
        kernel.GetProcessTimes.argtypes = (wintypes.HANDLE,) + (ctypes.POINTER(wintypes.FILETIME),) * 4
        handle = kernel.OpenProcess(0x0400, False, pid)
        if not handle:
            return None
        try:
            creation, exit_time, system, user = (wintypes.FILETIME() for _ in range(4))
            if kernel.GetProcessTimes(handle, ctypes.byref(creation), ctypes.byref(exit_time),
                                      ctypes.byref(system), ctypes.byref(user)):
                return sum((part.dwHighDateTime << 32) + part.dwLowDateTime for part in (system, user)) / 10_000_000
        finally:
            kernel.CloseHandle(handle)
    else:
        stat = Path(f"/proc/{pid}/stat")
        if stat.exists():
            # The parenthesized command may itself contain spaces or parentheses.
            fields = stat.read_text().rsplit(")", 1)[1].split()
            return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--repo", type=Path, help="Optional real repository; default is isolated polyglot fixture")
    parser.add_argument("--prompt", default="Find NormalizeModulePath and its implementation. MUST preserve the public API.")
    parser.add_argument("--skip-curves", action="store_true", help="Measure the repository workload only")
    args = parser.parse_args()
    if args.samples < 5:
        parser.error("at least five samples required")
    binary = args.binary.resolve(strict=True)
    digest = hashlib.sha256()
    with binary.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    project = Path(__file__).resolve().parents[2]
    result = {"schema_version": 1, "recorded_utc": datetime.now(timezone.utc).isoformat(),
              "binary_sha256": digest.hexdigest(), "platform": platform.platform(), "architecture": platform.machine(),
              "logical_cpus": os.cpu_count(), "binary_bytes": binary.stat().st_size,
              "token_estimator": "UTF-8 bytes / 4, rounded up; not provider tokens",
              "frontier_task_success": "NOT MEASURED", "samples": args.samples,
              "workload": str(args.repo) if args.repo else "isolated authored polyglot fixture",
              "prompt": args.prompt}
    with tempfile.TemporaryDirectory(prefix="vexa-measure-") as temp:
        root = Path(temp)
        env = os.environ.copy()
        env.update(VEXA_CACHE_ROOT=str(root / "cache"), VEXA_CONFIG_ROOT=str(root / "config"),
                   VEXA_INSTALL_ROOT=str(root / "install"))
        if args.repo:
            repo = args.repo.resolve(strict=True)
        else:
            repo = root / "repo"
            shutil.copytree(project / "benchmarks/fixtures/polyglot", repo)
            (repo / ".git").mkdir()
        prompt = args.prompt
        request = {"vexa_api": "1", "op": "prepare", "cwd": str(repo), "prompt": prompt,
                   "mode": "lite", "max_context_tokens": 1800, "model": "unknown-future-model"}
        cold = []
        for index in range(5):
            # Distinct cache measures process spawn plus genuinely cold indexing.
            env["VEXA_CACHE_ROOT"] = str(root / f"cold-{index}")
            start = time.perf_counter()
            run = subprocess.run([str(binary), "prepare", "--cwd", str(repo), "--prompt", prompt,
                                  "--mode", "lite", "--json"], env=env, capture_output=True, text=True,
                                 encoding="utf-8", check=True, timeout=120)
            json.loads(run.stdout)
            cold.append((time.perf_counter() - start) * 1000)
        env["VEXA_CACHE_ROOT"] = str(root / "warm-cache")
        child = subprocess.Popen([str(binary), "protocol"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, env=env, text=True, encoding="utf-8")
        messages = queue.Queue()
        reader = threading.Thread(target=lambda: [messages.put(line) for line in child.stdout], daemon=True)
        reader.start()
        warm, reported = [], []
        try:
            for index in range(args.samples + 1):
                start = time.perf_counter()
                child.stdin.write(json.dumps(request) + "\n")
                child.stdin.flush()
                response = json.loads(messages.get(timeout=120))
                elapsed = (time.perf_counter() - start) * 1000
                if response.get("status") != "ok":
                    raise RuntimeError(response)
                if index:
                    warm.append(elapsed)
                    reported.append(response["timings_ms"]["total"])
            result["peak_working_set_or_rss_bytes"] = peak_memory(child.pid)
            result["warm_process_total_cpu_seconds_including_warmup"] = process_cpu_seconds(child.pid)
            result["indexed_source_token_estimate"] = response["estimated_source_tokens"]
            result["packet_token_estimate"] = response["estimated_packet_tokens"]
            result["packet_paths"] = [item["path"] for item in response["evidence"]]
        finally:
            child.stdin.close()
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        result.update(cold_spawn_and_index_ms=cold, cold_p95_ms=percentile(cold, .95),
                      warm_protocol_roundtrip_ms=warm, warm_p95_ms=percentile(warm, .95),
                      warm_core_ms=reported, warm_core_p95_ms=percentile(reported, .95))
        curves = []
        # Per-task fractional source budgets, with immutable prompt/metadata overhead explicitly retained.
        for fraction in (() if args.skip_curves else (1, .75, .5, .3, .2, .1, .05)):
            out = root / f"curve-{fraction}.json"
            run = subprocess.run([str(binary), "benchmark", "--source-fraction", str(fraction),
                                  "--output", str(out)], cwd=project, env=env,
                                 capture_output=True, text=True, encoding="utf-8", timeout=120)
            if not out.exists():
                raise RuntimeError(run.stderr)
            report = json.loads(out.read_text(encoding="utf-8"))
            curves.append({"requested_source_fraction": fraction, "exit_code": run.returncode,
                           "aggregate": report["aggregate"]})
        result["retrieval_compression_curve"] = curves
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: value for key, value in result.items() if not isinstance(value, list)}, indent=2))


if __name__ == "__main__":
    main()
