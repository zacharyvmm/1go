#!/usr/bin/env python3
"""Unit tests for benchmark result parsing and comparison helpers."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COMPARE = ROOT / "scripts" / "compare_bench_results.py"


class CompareBenchResultsTests(unittest.TestCase):
    def _write(self, path: Path, results: list[dict]) -> None:
        path.write_text(json.dumps({"results": results}), encoding="utf-8")

    def test_detects_regression_over_threshold(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp) / "base.json"
            cand = Path(tmp) / "cand.json"
            self._write(base, [{"name": "lookup_only_100", "median_ns": 100.0}])
            self._write(cand, [{"name": "lookup_only_100", "median_ns": 110.0}])
            proc = subprocess.run(
                [
                    sys.executable,
                    str(COMPARE),
                    str(base),
                    str(cand),
                    "--threshold",
                    "5",
                    "--required",
                    "lookup_only_100",
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 1)
            payload = json.loads(proc.stdout)
            self.assertFalse(payload["ok"])
            self.assertAlmostEqual(payload["comparisons"][0]["delta_pct"], 10.0)

    def test_passes_within_threshold(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp) / "base.json"
            cand = Path(tmp) / "cand.json"
            out = Path(tmp) / "out.json"
            self._write(base, [{"name": "lookup_only_100", "median_ns": 100.0}])
            self._write(cand, [{"name": "lookup_only_100", "median_ns": 104.0}])
            proc = subprocess.run(
                [
                    sys.executable,
                    str(COMPARE),
                    str(base),
                    str(cand),
                    "--threshold",
                    "5",
                    "--output",
                    str(out),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            payload = json.loads(out.read_text())
            self.assertTrue(payload["ok"])

    def test_missing_required_case_fails(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp) / "base.json"
            cand = Path(tmp) / "cand.json"
            self._write(base, [{"name": "lookup_only_100", "median_ns": 100.0}])
            self._write(cand, [{"name": "other", "median_ns": 100.0}])
            proc = subprocess.run(
                [
                    sys.executable,
                    str(COMPARE),
                    str(base),
                    str(cand),
                    "--required",
                    "lookup_only_100",
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 1)
            self.assertIn("missing candidate case", proc.stderr)

if __name__ == "__main__":
    unittest.main()
