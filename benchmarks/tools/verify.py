"""Measure VEXA's verifier on seeded true/false repository claims.

This is verifier correctness, not a frontier-model hallucination-rate experiment.
All fixture source is authored here; nothing is executed from the fixture.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = str(args.binary.resolve(strict=True))
    with tempfile.TemporaryDirectory(prefix="vexa-claims-") as temporary:
        root = Path(temporary)
        repo = root / "repo"
        repo.mkdir()
        (repo / ".git").mkdir()
        source = b"pub fn real(value: i32) -> i32 { value + 1 }\n"
        (repo / "lib.rs").write_bytes(source)
        (repo / "broken.rs").write_text("fn malformed( {\n", encoding="utf-8")
        (repo / "binary.dat").write_bytes(b"\xff\x00")
        env = os.environ.copy()
        env.update(VEXA_CACHE_ROOT=str(root / "cache"), VEXA_CONFIG_ROOT=str(root / "config"))
        cases = []

        def add(category, expected, **claim):
            cases.append({"category": category, "expected": expected, "claim": claim})

        add("file", "verified", kind="file_exists", path="lib.rs")
        add("file", "contradicted", kind="file_exists", path="invented.rs")
        add("boundary", "unknown", kind="file_exists", path="../outside.rs")
        add("boundary", "unknown", kind="file_exists", path=str(root / "outside.rs"))
        add("hash", "verified", kind="file_hash", path="lib.rs", expected_sha256=hashlib.sha256(source).hexdigest())
        add("hash", "contradicted", kind="file_hash", path="lib.rs", expected_sha256="0" * 64)
        add("hash", "unknown", kind="file_hash", path="lib.rs", expected_sha256="not-a-hash")
        add("symbol", "verified", kind="symbol_exists", path="lib.rs", symbol="real")
        add("symbol", "contradicted", kind="symbol_exists", path="lib.rs", symbol="invented")
        add("signature", "verified", kind="signature_exists", path="lib.rs", symbol="real", signature="pub fn real(value: i32) -> i32")
        add("signature", "contradicted", kind="signature_exists", path="lib.rs", symbol="real", signature="pub fn real(value: String) -> bool")
        add("malformed", "unknown", kind="symbol_exists", path="broken.rs", symbol="invented")
        add("unsupported", "unknown", kind="symbol_exists", path="binary.dat", symbol="invented")
        command = [binary, "verify", "--cwd", str(repo), "claims"]
        run = subprocess.run(command, input=json.dumps([c["claim"] for c in cases]),
                             env=env, capture_output=True, encoding="utf-8", timeout=120)
        response = json.loads(run.stdout)
        for case, actual in zip(cases, response["claims"], strict=True):
            case.update(actual=actual["state"], reason=actual["reason"], passed=actual["state"] == case["expected"])
        if run.returncode == 0:
            raise RuntimeError("A contradicted/unknown batch incorrectly returned success")
        logs = []
        for text, exit_code, expected in [
                ("test real ... ok\n", 0, "passed"),
                ("test invented ... FAILED\n", 1, "failed"),
                ("test hidden ... FAILED\n", 0, "failed"),
                ("Finished build\n", None, "unknown")]:
            command = [binary, "verify", "log"]
            if exit_code is not None:
                command.extend(["--exit-code", str(exit_code)])
            observed = subprocess.run(command, input=text, env=env, capture_output=True,
                                      encoding="utf-8", timeout=30, check=True)
            summary = json.loads(observed.stdout)
            logs.append({"log": text, "exit_code": exit_code, "expected": expected,
                         "actual": summary["status"], "passed": summary["status"] == expected})
    negative = [c for c in cases if c["expected"] != "verified"]
    report = {"kind": "seeded_verifier_correctness", "frontier_hallucination_rate": "NOT MEASURED",
              "cases": cases, "log_cases": logs,
              "claim_accuracy": sum(c["passed"] for c in cases) / len(cases),
              "false_acceptance_rate": sum(c["actual"] == "verified" for c in negative) / len(negative),
              "all_passed": all(c["passed"] for c in cases + logs)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: v for k, v in report.items() if not isinstance(v, list)}, indent=2))
    raise SystemExit(0 if report["all_passed"] else 1)


if __name__ == "__main__":
    main()
