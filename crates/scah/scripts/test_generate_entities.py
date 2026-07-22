#!/usr/bin/env python3
"""Tests for generate_entities.py escaping, determinism, and fixture updates."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import re
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


def find_repo_root(start: Path) -> Path:
    for parent in [start, *start.parents]:
        if (parent / "Cargo.toml").exists() and (parent / ".github").exists():
            return parent
    raise RuntimeError("repository root not found")


def extract_single_sha256(text: str, source: str) -> str:
    hashes = sorted(set(re.findall(r"\b[a-f0-9]{64}\b", text)))

    if len(hashes) != 1:
        raise AssertionError(
            f"{source} must contain exactly one unique SHA-256; got {hashes}"
        )

    return hashes[0]


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
            self.assertIn("NAMED_ENTITY_COUNT: usize = 2", written_table)
            # "amp;" then "lt;" as UTF-8 in the name blob; "&" then "<" in values.
            self.assertIn(
                "0x61, 0x6D, 0x70, 0x3B, 0x6C, 0x74, 0x3B", written_table
            )
            self.assertIn("0x26, 0x3C", written_table)
            self.assertIn("ENTITY_NAME_ENDS", written_table)
            self.assertNotIn("Retrieved:", written_table)

            # Default generation from the updated fixture must match.
            regenerated = self.gen.generate_from_bytes(written_fixture)
            self.assertEqual(regenerated, written_table)

    def test_fetch_alias_fails_with_message(self) -> None:
        code = self.gen.main(["--fetch"])
        self.assertEqual(code, 2)


class ProvenanceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.gen = load_generator()
        cls.repo_root = find_repo_root(SCRIPT.resolve())

    def test_recorded_fixture_hashes_match(self) -> None:
        fixture = self.gen.DEFAULT_INPUT.read_bytes()
        fixture_hash = hashlib.sha256(fixture).hexdigest()
        source = self.gen.validate_source(fixture)
        entry_count = len(source)

        generated_table = self.gen.DESTINATION.read_text(encoding="utf-8")
        root_notice = (
            self.repo_root / "THIRD_PARTY_LICENSES" / "WHATWG-HTML.txt"
        ).read_text(encoding="utf-8")
        package_notice = (
            self.repo_root
            / "crates"
            / "scah"
            / "THIRD_PARTY_LICENSES"
            / "WHATWG-HTML.txt"
        ).read_text(encoding="utf-8")
        readme = (
            self.repo_root / "crates" / "scah" / "scripts" / "README.md"
        ).read_text(encoding="utf-8")

        self.assertEqual(root_notice, package_notice)
        self.assertEqual(
            extract_single_sha256(generated_table, "generated table"),
            fixture_hash,
        )
        self.assertEqual(
            extract_single_sha256(root_notice, "root notice"),
            fixture_hash,
        )
        self.assertEqual(
            extract_single_sha256(package_notice, "package notice"),
            fixture_hash,
        )
        self.assertEqual(
            extract_single_sha256(readme, "generator README"),
            fixture_hash,
        )
        self.assertIn(f"Source SHA-256: {fixture_hash}", generated_table)
        self.assertIn(f"// Entry count: {entry_count}", generated_table)
        self.assertIn(f"Entry count: {entry_count}", readme)
        self.assertIn("https://creativecommons.org/licenses/by/4.0/", root_notice)
        self.assertIn(
            "Creative Commons Attribution 4.0 International Public License",
            root_notice,
        )
        self.assertIn("BSD 3-Clause License", root_notice)
        self.assertIn("https://html.spec.whatwg.org/entities.json", root_notice)


if __name__ == "__main__":
    unittest.main()
