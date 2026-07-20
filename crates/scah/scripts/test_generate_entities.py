#!/usr/bin/env python3
"""Tests for generate_entities.py escaping and determinism helpers."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

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
        self.assertEqual(lit("\t"), '"\\t"')
        self.assertEqual(lit("\u0001"), '"\\u{1}"')

    def test_format_and_non_bmp(self) -> None:
        lit = self.gen.rust_string_literal
        self.assertEqual(lit("\u2061"), '"\\u{2061}"')
        self.assertEqual(lit("𝔄"), '"𝔄"')
        self.assertEqual(lit("\u2242\u0338"), '"\u2242\u0338"')

    def test_no_utf16_surrogates(self) -> None:
        # Non-BMP must not become JSON-style surrogate pairs.
        encoded = self.gen.rust_string_literal("𝔄")
        self.assertNotIn("\\uD", encoded.upper())
        self.assertNotIn("\\u{D", encoded.upper())


if __name__ == "__main__":
    unittest.main()
