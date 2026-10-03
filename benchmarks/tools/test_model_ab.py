"""Deterministic adapter contract tests; these are not model benchmark results."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import model_ab


class ModelABContractTests(unittest.TestCase):
    def run_adapter(self, payload):
        code = "import json,sys; json.load(sys.stdin); print(json.dumps(" + repr(payload) + "))"
        return model_ab.call_adapter([sys.executable, "-c", code],
                                     {"model": "model-x", "settings": {"temperature": 0}, "context": "fixture"})

    def test_missing_optional_metrics_are_null(self):
        result = self.run_adapter({"model": "model-x", "settings": {"temperature": 0},
                                   "answer": "answer", "claims": [], "correction_packet": None})
        self.assertIsNone(result["usage"])
        self.assertIsNone(result["ttft_ms"])
        self.assertGreaterEqual(result["measured_latency_ms"], 0)

    def test_model_and_settings_must_match(self):
        with self.assertRaisesRegex(ValueError, "model or settings"):
            self.run_adapter({"model": "other", "settings": {"temperature": 0},
                              "answer": "answer", "claims": []})
        with self.assertRaisesRegex(ValueError, "model or settings"):
            self.run_adapter({"model": "model-x", "settings": {}, "answer": "answer", "claims": []})

    def test_sensitive_paths_are_skipped_and_secret_content_is_refused(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root / ".env").write_text("secret", encoding="utf-8")
            (root / "source.py").write_text("safe", encoding="utf-8")
            self.assertEqual(model_ab.safe_fixture_files(root), [("source.py", b"safe")])
            (root / "source.txt").write_text("api_key=not-a-real-key", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "secret-like"):
                model_ab.safe_fixture_files(root)

    def test_manifest_paths_must_be_relative_and_scoped(self):
        for value in ("../outside.json", "C:/outside.json", "tasks/../outside.json", "tasks\\outside.json",
                      "tasks/alternate:stream.json"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                model_ab.checked_relative_path(value, "tasks/")

    def test_oversized_source_is_skipped_before_read(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root / "small.py").write_bytes(b"pass\n")
            large = root / "large.log"
            with large.open("wb") as stream:
                stream.truncate(model_ab.MAX_INDEXED_FILE_BYTES + 1)
            self.assertEqual(model_ab.safe_fixture_files(root), [("small.py", b"pass\n")])

    def test_unknown_claims_stay_distinct_from_contradictions(self):
        claims = [{"kind": "file_exists", "path": "missing.py", "category": "files"},
                  {"kind": "file_hash", "path": "source.py", "expected_sha256": "bad", "category": "files"}]
        output = {"claims": [{"state": "unknown"}, {"state": "contradicted"}]}
        completed = subprocess.CompletedProcess([], 0, json.dumps(output), "")
        with patch.object(model_ab.subprocess, "run", return_value=completed):
            result = model_ab.verify_claims(Path("unused"), Path("unused"), claims, {})
        self.assertEqual(result["counts"], {"verified": 0, "contradicted": 1, "unknown": 1})
        self.assertEqual(result["unsupported_claim_rate"], 1.0)
        self.assertEqual(result["counts_by_category"]["files"]["unknown"], 1)
        self.assertNotIn("hallucination_rate", result)


if __name__ == "__main__":
    unittest.main()
