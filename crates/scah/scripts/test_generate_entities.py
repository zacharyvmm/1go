#!/usr/bin/env python3
"""Tests for generate_entities.py escaping, determinism, and fixture updates."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).with_name("generate_entities.py")


def load_generator():
    spec = importlib.util.spec_from_file_location("generate_entities", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class RustStringLiteralTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.gen = load_generator()

    def test_ascii_and_escapes(self) -> None:
        lit = self.gen.rust_string_literal
        self.assertEqual(lit("plain"), '"plain"')
        self.assertEqual(lit('quote: "'), '"quote: \\""')
        self.assertEqual(lit("slash: \\"), '"slash: \\\\"')
        self.assertEqual(lit("\n"), '"\\n"')
        self.assertEqual(lit("\r"), '"\\r"')
        self.assertEqual(lit("\t"), '"\\t"')
        self.assertEqual(lit("\0"), '"\\0"')
        self.assertEqual(lit("\u0001"), '"\\u{1}"')
        self.assertEqual(lit("\u007f"), '"\\u{7F}"')

    def test_format_and_non_bmp(self) -> None:
        lit = self.gen.rust_string_literal
        self.assertEqual(lit("\u00a0"), '"\\u{A0}"')
        self.assertEqual(lit("\u2061"), '"\\u{2061}"')
        self.assertEqual(lit("\u2028"), '"\\u{2028}"')
        self.assertEqual(lit("\u2029"), '"\\u{2029}"')
        self.assertEqual(lit("𝔄"), '"𝔄"')
        self.assertEqual(lit("\u2242\u0338"), '"\u2242\u0338"')

    def test_no_utf16_surrogates(self) -> None:
        # Non-BMP must not become JSON-style surrogate pairs.
        encoded = self.gen.rust_string_literal("𝔄")
        self.assertNotIn("\\uD", encoded.upper())
        self.assertNotIn("\\u{D", encoded.upper())


class GenerationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.gen = load_generator()

    def test_generation_is_deterministic(self) -> None:
        raw = self.gen.DEFAULT_INPUT.read_bytes()
        source, source_hash = self.gen.load_source(raw)

        first = self.gen.generate_table(source, source_hash)
        second = self.gen.generate_table(source, source_hash)

        self.assertEqual(first, second)
        self.assertNotIn("Retrieved:", first)
        self.assertNotIn("2026-", first)
        self.assertIn("Third-party license: THIRD_PARTY_LICENSES/WHATWG-HTML.txt", first)
        self.assertIn(f"Source SHA-256: {source_hash}", first)

    def test_update_fixture_uses_same_downloaded_bytes(self) -> None:
        payload = {
            "&amp;": {"codepoints": [38], "characters": "&"},
            "&lt;": {"codepoints": [60], "characters": "<"},
        }
        raw = json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
        source_hash = hashlib.sha256(raw).hexdigest()

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            fixture_path = tmp_path / "entities.json"
            table_path = tmp_path / "entities_table.rs"
            fixture_path.write_bytes(b'{"stale":{"characters":"x"}}')

            with mock.patch.object(self.gen, "fetch_upstream", return_value=raw):
                self.gen.update_fixture(fixture_path=fixture_path, table_path=table_path)

            written_fixture = fixture_path.read_bytes()
            written_table = table_path.read_text(encoding="utf-8")

            self.assertEqual(written_fixture, raw)
            self.assertIn(f"Source SHA-256: {source_hash}", written_table)
            self.assertIn('("amp;", "&")', written_table)
            self.assertIn('("lt;", "<")', written_table)
            self.assertNotIn("Retrieved:", written_table)

            # Default generation from the updated fixture must match.
            regenerated = self.gen.generate_from_bytes(written_fixture)
            self.assertEqual(regenerated, written_table)

    def test_fetch_alias_fails_with_message(self) -> None:
        code = self.gen.main(["--fetch"])
        self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()
