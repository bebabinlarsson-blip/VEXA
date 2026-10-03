"""Opt-in matched provider-neutral model A/B runner for single-query fixtures.

This runner measures supplied usage/latency and verifies structured claims. It
does not score whether a model wrote correct code or produced a correct patch.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import re
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[2]
MAX_FIXTURE_BYTES = 16 * 1024 * 1024
MAX_INDEXED_FILE_BYTES = 2 * 1024 * 1024
SENSITIVE_NAMES = {".env", ".env.local", ".env.production", ".env.development", "credentials",
                   "credentials.json", "secrets.json", "id_rsa", "id_ed25519", ".npmrc",
                   ".pypirc", "netrc", ".netrc"}
SKIP_DIRS = {".git", ".hg", ".svn", ".venv", "venv", "__pycache__", "node_modules",
             "vendor", "vendors", "target", "dist", "build"}
TASK_ID = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
IGNORE_CONTROLS = {".gitignore", ".vexaignore"}
SECRET_TEXT = re.compile(r"(?i)(-----BEGIN [A-Z ]*PRIVATE KEY-----|\b(api[_-]?key|access[_-]?token|password)\s*[:=]\s*\S+)")
SENSITIVE_COMPONENT = re.compile(r"(?i)(^|[._-])(secret|credentials?|tokens?|passwords?|private.?keys?)([._-]|$)")


def checked_relative_path(value, prefix):
    if not isinstance(value, str) or not value or "\\" in value or ":" in value:
        raise ValueError(f"unsafe path: {value!r}")
    path = Path(value)
    if path.is_absolute() or any(part in ("", ".", "..") for part in value.split("/")):
        raise ValueError(f"unsafe path: {value!r}")
    if not value.startswith(prefix):
        raise ValueError(f"path must be under {prefix}: {value!r}")
    return path


def resolve_inside(root, relative):
    root = root.resolve(strict=True)
    candidate = root
    for part in relative.parts:
        candidate = candidate / part
        if candidate.is_symlink():
            raise ValueError("symlink path is not allowed")
    resolved = candidate.resolve(strict=True)
    if not resolved.is_relative_to(root):
        raise ValueError("path escapes its allowed root")
    return resolved


def normalized_path(path):
    value = os.path.normcase(os.path.normpath(str(path)))
    if value.startswith("\\\\?\\"):
        value = value[4:]
    return value


def sensitive_fixture_path(rel):
    low = rel.as_posix().lower()
    name = rel.name.lower()
    extension = rel.suffix.lower()
    return (name in SENSITIVE_NAMES or name.startswith(".env.") or extension in {".pem", ".key", ".p12", ".pfx"}
            or any(SENSITIVE_COMPONENT.search(part) for part in rel.parts)
            or any(part.lower() in {".ssh", ".aws", ".config"} for part in rel.parts)
            or "/.config/gcloud/" in f"/{low}/")


def safe_fixture_files(repo):
    """Read a bounded candidate snapshot; actual eligibility comes from VEXA's index."""
    repo = repo.resolve(strict=True)
    files, total = [], 0
    for path in sorted(repo.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"fixture contains a symlink: {path.relative_to(repo)}")
        if path.is_file():
            rel = path.relative_to(repo)
            if sensitive_fixture_path(rel):
                continue
            if any(part.lower() in SKIP_DIRS for part in rel.parts[:-1]):
                continue
            size = path.stat().st_size
            if size > MAX_INDEXED_FILE_BYTES:
                continue
            total += size
            if total > MAX_FIXTURE_BYTES:
                raise ValueError(f"fixture exceeds the {MAX_FIXTURE_BYTES}-byte aggregate limit")
            data = path.read_bytes()
            if len(data) != size:
                raise ValueError("fixture changed while being read")
            try:
                decoded = data.decode("utf-8", errors="strict")
            except UnicodeDecodeError:
                continue
            if SECRET_TEXT.search(decoded):
                raise ValueError("fixture contains secret-like content")
            files.append((rel.as_posix(), data))
    if not files:
        raise ValueError("fixture repository is empty")
    return files


def call_adapter(argv, request, env=None):
    started = time.perf_counter()
    proc = subprocess.run(argv, input=json.dumps(request), text=True, encoding="utf-8", errors="strict",
                          capture_output=True, env=env,
                          timeout=900, check=False)
    elapsed = (time.perf_counter() - started) * 1000
    if proc.returncode:
        raise RuntimeError(f"adapter exited with status {proc.returncode}")
    response = json.loads(proc.stdout)
    if not isinstance(response, dict):
        raise ValueError("adapter response must be a JSON object")
    if response.get("model") != request["model"] or response.get("settings") != request["settings"]:
        raise ValueError("adapter returned a model or settings different from the matched request")
    if not isinstance(response.get("answer"), str) or not isinstance(response.get("claims"), list):
        raise ValueError("adapter response requires string answer and claims array")
    if not all(isinstance(claim, dict) for claim in response["claims"]):
        raise ValueError("adapter claims must be JSON objects")
    response["measured_latency_ms"] = elapsed
    usage = response.get("usage")
    response["usage"] = usage if isinstance(usage, dict) else None
    response["ttft_ms"] = response.get("ttft_ms") if isinstance(response.get("ttft_ms"), (int, float)) else None
    response["correction_packet"] = response.get("correction_packet")
    return response


def verify_claims(binary, cwd, claims, env):
    proc = subprocess.run([str(binary), "verify", "--cwd", str(cwd), "claims"],
                          input=json.dumps(claims), text=True, encoding="utf-8", errors="strict", capture_output=True,
                          env=env, timeout=120)
    data = json.loads(proc.stdout)
    states = [c.get("state", "unknown") for c in data.get("claims", [])]
    if len(states) != len(claims):
        raise ValueError("verifier returned a different number of claim results")
    counts = {state: states.count(state) for state in ("verified", "contradicted", "unknown")}
    by_category = {}
    for source, state in zip(claims, states):
        category = source.get("category", source.get("kind", "other"))
        bucket = by_category.setdefault(category, {key: 0 for key in ("verified", "contradicted", "unknown")})
        bucket[state if state in bucket else "unknown"] += 1
    for bucket in by_category.values():
        checkable = sum(bucket.values())
        bucket["unsupported_claim_rate"] = ((bucket["contradicted"] + bucket["unknown"]) / checkable
                                             if checkable else None)
    return {"claims": data["claims"], "counts": counts,
            "counts_by_category": by_category,
            "unsupported_claim_rate": ((counts["contradicted"] + counts["unknown"]) / len(states)
                                       if states else None)}


def build_eligible_manifest(binary, candidate_files, task_root, env):
    """Ask VEXA's own fresh index which candidate files are eligible."""
    index_root = task_root / "indexer"
    repo = index_root / "repo"
    repo.mkdir(parents=True)
    (repo / ".git").mkdir()
    for relative, data in candidate_files:
        dest = repo / Path(relative)
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(data)
    index_env = env.copy()
    index_env["VEXA_CACHE_ROOT"] = str(index_root / "cache")
    index_env["VEXA_CONFIG_ROOT"] = str(index_root / "config")
    index_env["VEXA_INSTALL_ROOT"] = str(index_root / "install")
    run = subprocess.run([str(binary), "index", "refresh", "--cwd", str(repo), "--json"],
                         text=True, encoding="utf-8", errors="strict", capture_output=True,
                         env=index_env, timeout=120, check=True)
    summary = json.loads(run.stdout)
    cached = list((index_root / "cache" / "indexes").glob("*/index.json"))
    if len(cached) != 1:
        raise ValueError("VEXA did not produce exactly one isolated index manifest")
    snapshot = json.loads(cached[0].read_text(encoding="utf-8", errors="strict"))
    recorded_root = Path(snapshot["repository_root"]).resolve(strict=True)
    if normalized_path(recorded_root) != normalized_path(repo.resolve(strict=True)):
        raise ValueError("VEXA index manifest repository root did not match staged fixture")
    records = snapshot.get("files")
    if not isinstance(records, list) or summary.get("files") != len(records):
        raise ValueError("VEXA index summary and manifest differ")
    indexed, total = [], 0
    for record in records:
        relative = checked_relative_path(record.get("path"), "")
        source = resolve_inside(repo, relative)
        size = source.stat().st_size
        if size > MAX_INDEXED_FILE_BYTES:
            raise ValueError("VEXA indexed source exceeds the per-file limit")
        data = source.read_bytes()
        if len(data) != size or hashlib.sha256(data).hexdigest() != record.get("sha256"):
            raise ValueError("VEXA indexed source no longer matches its manifest")
        try:
            data.decode("utf-8", errors="strict")
        except UnicodeDecodeError as exc:
            raise ValueError("VEXA manifest contained non-UTF-8 source") from exc
        total += len(data)
        if total > MAX_FIXTURE_BYTES:
            raise ValueError("eligible VEXA source exceeds the aggregate fixture limit")
        indexed.append((relative.as_posix(), data))
    return indexed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adapter-argv", required=True, help="JSON string array of executable and arguments")
    parser.add_argument("--model", required=True)
    parser.add_argument("--settings", default="{}", help="JSON object passed identically to both arms")
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--allow-send-fixtures", action="store_true")
    args = parser.parse_args()
    if not args.allow_send_fixtures:
        parser.error("fixture content may be sent to a provider; pass --allow-send-fixtures explicitly")
    try:
        argv = json.loads(args.adapter_argv)
        settings = json.loads(args.settings)
        if not isinstance(argv, list) or not argv or not all(isinstance(x, str) for x in argv):
            raise ValueError("--adapter-argv must be a non-empty JSON string array")
        if not isinstance(settings, dict):
            raise ValueError("--settings must be a JSON object")
        binary = args.binary.resolve(strict=True)
        suite = json.loads((ROOT / "benchmarks/benchmark-suite.v1.json").read_text(encoding="utf-8"))
        rng = random.Random(args.seed)
        env = os.environ.copy()
        env.pop("PYTHONHOME", None)
        env.pop("PYTHONPATH", None)
        with tempfile.TemporaryDirectory(prefix="vexa-model-ab-") as temp:
            temp_root = Path(temp).resolve()
            rows = []
            skipped = []
            for task_rel in suite["tasks"]:
                task_path = checked_relative_path(task_rel, "tasks/")
                task_file = resolve_inside(ROOT / "benchmarks", task_path)
                task_root_allowed = (ROOT / "benchmarks" / "tasks").resolve(strict=True)
                if not task_file.is_relative_to(task_root_allowed):
                    raise ValueError("task manifest path must be under benchmarks/tasks")
                task = json.loads(task_file.read_text(encoding="utf-8", errors="strict"))
                task_id = task.get("id")
                if not isinstance(task_id, str) or not TASK_ID.fullmatch(task_id):
                    raise ValueError("task has an unsafe ID")
                if task["scenario"]["kind"] != "single_query":
                    skipped.append({"task_id": task_id, "reason": "scenario not supported by this runner"})
                    continue
                fixture_rel = checked_relative_path(task.get("fixture_repo"), "fixtures/")
                repo = resolve_inside(ROOT / "benchmarks", fixture_rel)
                fixture_root = (ROOT / "benchmarks" / "fixtures").resolve(strict=True)
                if not repo.is_dir() or not repo.is_relative_to(fixture_root):
                    raise ValueError("fixture repository must be under benchmarks/fixtures")
                files = safe_fixture_files(repo)
                task_root = temp_root / task_id
                task_root.mkdir()
                eligible_files = build_eligible_manifest(binary, files, task_root, env)
                controls = [(rel, data) for rel, data in files if Path(rel).name.lower() in IGNORE_CONTROLS]
                staged_files = dict(controls + eligible_files)
                arms = ["baseline", "vexa"]
                rng.shuffle(arms)
                arms_out = {}
                snapshot_hash = hashlib.sha256(b"".join(p.encode()+b"\0"+hashlib.sha256(d).digest() for p,d in eligible_files)).hexdigest()
                for arm in arms:
                    arm_root = task_root / arm
                    staged = arm_root / "repo"
                    staged.mkdir(parents=True)
                    (staged / ".git").mkdir()
                    for rel, data in staged_files.items():
                        dest = staged / Path(rel)
                        dest.parent.mkdir(parents=True, exist_ok=True)
                        dest.write_bytes(data)
                    arm_env = env.copy()
                    arm_env["VEXA_CACHE_ROOT"] = str(arm_root / "cache")
                    arm_env["VEXA_CONFIG_ROOT"] = str(arm_root / "config")
                    arm_env["VEXA_INSTALL_ROOT"] = str(arm_root / "install")
                    full_source = "\n".join(f"--- {p} ---\n{d.decode('utf-8', errors='strict')}" for p,d in eligible_files)
                    context = full_source
                    if arm == "vexa":
                        prep = subprocess.run([str(binary), "prepare", "--cwd", str(staged), "--prompt", task["prompt"], "--json"],
                                              capture_output=True, text=True, encoding="utf-8", errors="strict",
                                              env=arm_env, timeout=120, check=True)
                        packet = json.loads(prep.stdout)
                        if packet.get("status") != "ok" or not isinstance(packet.get("packet"), str):
                            raise ValueError(f"VEXA prepare failed for {task_id}")
                        context = packet["packet"]
                    req = {"task_id": task_id, "arm": arm, "model": args.model, "settings": settings,
                           "prompt": task["prompt"], "context": context}
                    response = call_adapter(argv, req, arm_env)
                    checked = verify_claims(binary, staged, response["claims"], arm_env)
                    arms_out[arm] = {"model": args.model, "settings": settings, "answer": response["answer"],
                                     "correction_packet": response["correction_packet"],
                                     "usage": response["usage"], "ttft_ms": response["ttft_ms"],
                                     "measured_latency_ms": response["measured_latency_ms"],
                                     "claim_verification": checked}
                rows.append({"task_id": task_id, "fixture_sha256": snapshot_hash,
                             "randomized_arm_order": arms, "arms": arms_out,
                             "coding_success": "NOT MEASURED", "patch_correctness": "NOT MEASURED"})
        result = {"kind": "opt_in_matched_frontier_ab", "suite_id": suite["suite_id"],
                  "model": args.model, "settings": settings, "seed": args.seed,
                  "tasks": rows, "skipped": skipped,
                  "coding_success": "NOT MEASURED", "patch_correctness": "NOT MEASURED",
                  "provider_identity_is_self_reported": True,
                  "adapter_invocation_is_user_configured": True}
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"tasks_run": len(rows), "tasks_skipped": len(skipped), "output": str(args.output)}))
    except (ValueError, OSError, subprocess.SubprocessError, json.JSONDecodeError) as exc:
        parser.error(str(exc))


if __name__ == "__main__":
    main()
