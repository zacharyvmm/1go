#!/usr/bin/env python3
"""Deterministic Criterion baseline measurement manifest.

Consumes the exact inventory produced by ``copy-criterion-baseline.py`` and
walks only those saved-baseline directories. Emits a NUL-delimited binary
manifest matching the source-fingerprint record layout:

    entry-type NUL mode NUL content-hash NUL relative-path NUL

Only regular files are recorded. Regular directories are traversed. Symlinks
and special filesystem entries under an inventoried baseline directory are
rejected.

Interface::

    python3 scripts/criterion-baseline-manifest.py \\
        --criterion-root <current-target>/criterion \\
        --inventory <inventory.jsonl> \\
        --manifest <manifest.bin> \\
        [--baseline <baseline-name>]

Prints the SHA-256 of the complete manifest to stdout.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import sys
import tempfile
from typing import Iterable, List, Optional, Set, Tuple


class BaselineManifestError(RuntimeError):
    """Raised when baseline inventory or traversal is invalid."""


# ── Path / inventory helpers ─────────────────────────────────────────────────


def _normalize_relative(path: str, field: str) -> str:
    """Normalize a relative inventory path and reject escapes."""
    if not isinstance(path, str) or not path:
        raise BaselineManifestError(f"inventory {field} must be a non-empty string")

    if os.path.isabs(path):
        raise BaselineManifestError(f"inventory {field} must be relative: {path!r}")

    normalized = os.path.normpath(path)
    if normalized in ("", "."):
        raise BaselineManifestError(f"inventory {field} is empty after normalization")

    if normalized.startswith("..") or f"{os.sep}.." in f"{os.sep}{normalized}{os.sep}":
        raise BaselineManifestError(f"inventory {field} escapes with '..': {path!r}")

    if normalized.startswith(f".{os.sep}") or normalized == ".":
        raise BaselineManifestError(f"inventory {field} is not a usable relative path: {path!r}")

    return normalized


def _load_inventory(
    inventory_path: str,
    baseline_name: Optional[str],
) -> List[dict]:
    """Load and validate the Criterion baseline inventory JSONL."""
    records: List[dict] = []
    seen_baseline_paths: Set[str] = set()
    inferred_baseline: Optional[str] = baseline_name

    try:
        with open(inventory_path, "r", encoding="utf-8") as fh:
            lines = fh.readlines()
    except OSError as exc:
        raise BaselineManifestError(f"cannot read inventory: {exc}") from exc

    for line_no, raw in enumerate(lines, start=1):
        line = raw.strip()
        if not line:
            continue

        try:
            entry = json.loads(line)
        except json.JSONDecodeError as exc:
            raise BaselineManifestError(
                f"inventory line {line_no}: malformed JSON: {exc}"
            ) from exc

        if not isinstance(entry, dict):
            raise BaselineManifestError(
                f"inventory line {line_no}: expected a JSON object"
            )

        if "benchmark_path" not in entry or "baseline_path" not in entry:
            raise BaselineManifestError(
                f"inventory line {line_no}: requires benchmark_path and baseline_path"
            )

        benchmark_path = _normalize_relative(entry["benchmark_path"], "benchmark_path")
        baseline_path = _normalize_relative(entry["baseline_path"], "baseline_path")

        if os.path.dirname(baseline_path) != benchmark_path:
            raise BaselineManifestError(
                f"inventory line {line_no}: dirname(baseline_path) "
                f"({os.path.dirname(baseline_path)!r}) != "
                f"benchmark_path ({benchmark_path!r})"
            )

        entry_baseline = os.path.basename(baseline_path)
        if inferred_baseline is None:
            inferred_baseline = entry_baseline
        elif entry_baseline != inferred_baseline:
            raise BaselineManifestError(
                f"inventory line {line_no}: baseline name mismatch: "
                f"expected {inferred_baseline!r}, got {entry_baseline!r}"
            )

        if baseline_name is not None and entry_baseline != baseline_name:
            raise BaselineManifestError(
                f"inventory line {line_no}: basename(baseline_path) "
                f"({entry_baseline!r}) != --baseline ({baseline_name!r})"
            )

        if baseline_path in seen_baseline_paths:
            raise BaselineManifestError(
                f"inventory line {line_no}: duplicate baseline_path {baseline_path!r}"
            )
        seen_baseline_paths.add(baseline_path)

        records.append(
            {
                "benchmark_path": benchmark_path,
                "baseline_path": baseline_path,
            }
        )

    if not records:
        raise BaselineManifestError("baseline inventory is empty")

    return records


def _ensure_under_root(root: str, candidate: str, label: str) -> str:
    """Return the absolute path of *candidate* if it stays under *root*."""
    abs_root = os.path.abspath(root)
    abs_candidate = os.path.abspath(candidate)
    try:
        if os.path.commonpath([abs_root, abs_candidate]) != abs_root:
            raise BaselineManifestError(f"{label} escapes criterion root: {candidate}")
    except ValueError as exc:
        raise BaselineManifestError(
            f"{label} escapes criterion root: {candidate}"
        ) from exc
    return abs_candidate


# ── Hashing / mode ───────────────────────────────────────────────────────────


def _normalized_mode(st_mode: int) -> str:
    return "100755" if (st_mode & 0o111) else "100644"


def _hash_regular_file(path: str) -> str:
    hasher = hashlib.sha256()
    try:
        with open(path, "rb") as fh:
            while True:
                chunk = fh.read(1 << 20)
                if not chunk:
                    break
                hasher.update(chunk)
    except OSError as exc:
        raise BaselineManifestError(f"failed to read {path!r}: {exc}") from exc
    return hasher.hexdigest()


# ── Traversal ────────────────────────────────────────────────────────────────


def _collect_under_baseline(
    criterion_root: str,
    baseline_rel: str,
    entries: List[Tuple[str, str, str, str]],
) -> None:
    """Recursively collect regular files under one inventoried baseline path."""
    baseline_abs = _ensure_under_root(
        criterion_root,
        os.path.join(criterion_root, baseline_rel),
        "baseline_path",
    )

    try:
        st = os.lstat(baseline_abs)
    except OSError as exc:
        raise BaselineManifestError(
            f"cannot lstat baseline directory {baseline_rel!r}: {exc}"
        ) from exc

    if stat.S_ISLNK(st.st_mode):
        raise BaselineManifestError(
            f"baseline path is a symlink (rejected): {baseline_rel}"
        )
    if not stat.S_ISDIR(st.st_mode):
        raise BaselineManifestError(
            f"baseline path is not a directory: {baseline_rel}"
        )

    _walk_directory(criterion_root, baseline_rel, entries)


def _walk_directory(
    criterion_root: str,
    relative_directory: str,
    entries: List[Tuple[str, str, str, str]],
) -> None:
    directory_path = os.path.join(criterion_root, relative_directory)

    try:
        with os.scandir(directory_path) as iterator:
            children = list(iterator)
    except OSError as exc:
        raise BaselineManifestError(
            f"failed to traverse directory {relative_directory!r}: {exc}"
        ) from exc

    children.sort(key=lambda entry: os.fsencode(entry.name))

    for child in children:
        relative_path = os.path.join(relative_directory, child.name)
        _ensure_under_root(
            criterion_root,
            os.path.join(criterion_root, relative_path),
            "baseline entry",
        )

        try:
            st = os.lstat(os.path.join(criterion_root, relative_path))
        except OSError as exc:
            raise BaselineManifestError(
                f"cannot lstat baseline entry {relative_path!r}: {exc}"
            ) from exc

        mode = st.st_mode
        if stat.S_ISLNK(mode):
            raise BaselineManifestError(
                f"unsupported symlink in baseline data: {relative_path}"
            )
        if stat.S_ISDIR(mode):
            _walk_directory(criterion_root, relative_path, entries)
            continue
        if stat.S_ISFIFO(mode):
            raise BaselineManifestError(
                f"unsupported FIFO in baseline data: {relative_path}"
            )
        if stat.S_ISSOCK(mode):
            raise BaselineManifestError(
                f"unsupported socket in baseline data: {relative_path}"
            )
        if stat.S_ISBLK(mode):
            raise BaselineManifestError(
                f"unsupported block device in baseline data: {relative_path}"
            )
        if stat.S_ISCHR(mode):
            raise BaselineManifestError(
                f"unsupported character device in baseline data: {relative_path}"
            )
        if not stat.S_ISREG(mode):
            raise BaselineManifestError(
                f"unsupported entry type in baseline data "
                f"(mode {mode:o}): {relative_path}"
            )

        content_hash = _hash_regular_file(os.path.join(criterion_root, relative_path))
        entries.append(
            (
                "file",
                _normalized_mode(mode),
                content_hash,
                relative_path,
            )
        )


def write_manifest(
    entries: Iterable[Tuple[str, str, str, str]],
    manifest_path: str,
) -> str:
    """Write the binary manifest atomically and return its SHA-256 digest."""
    ordered = sorted(entries, key=lambda item: os.fsencode(item[3]))

    hasher = hashlib.sha256()
    manifest_dir = os.path.dirname(os.path.abspath(manifest_path)) or "."
    os.makedirs(manifest_dir, exist_ok=True)
    fd, tmp_path = tempfile.mkstemp(
        dir=manifest_dir,
        prefix=".tmp-criterion-manifest-",
        suffix=".bin",
    )

    try:
        with os.fdopen(fd, "wb") as fh:
            for entry_type, mode, content_hash, relative_path in ordered:
                record = (
                    entry_type.encode("utf-8")
                    + b"\0"
                    + mode.encode("utf-8")
                    + b"\0"
                    + content_hash.encode("utf-8")
                    + b"\0"
                    + relative_path.encode("utf-8", errors="surrogateescape")
                    + b"\0"
                )
                hasher.update(record)
                fh.write(record)
        os.replace(tmp_path, manifest_path)
    except BaseException:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass
        raise

    return hasher.hexdigest()


def generate_manifest(
    criterion_root: str,
    inventory_path: str,
    manifest_path: str,
    baseline_name: Optional[str] = None,
) -> str:
    """Generate the baseline manifest and return its SHA-256 digest."""
    criterion_root = os.path.abspath(criterion_root)
    if not os.path.isdir(criterion_root):
        raise BaselineManifestError(
            f"criterion root does not exist: {criterion_root}"
        )

    inventory = _load_inventory(inventory_path, baseline_name)
    entries: List[Tuple[str, str, str, str]] = []

    for record in inventory:
        _collect_under_baseline(criterion_root, record["baseline_path"], entries)

    return write_manifest(entries, manifest_path)


# ── CLI ──────────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate a deterministic Criterion baseline measurement manifest."
    )
    parser.add_argument(
        "--criterion-root",
        required=True,
        help="Criterion root containing copied baseline directories",
    )
    parser.add_argument(
        "--inventory",
        required=True,
        help="Baseline inventory JSONL from copy-criterion-baseline.py",
    )
    parser.add_argument(
        "--manifest",
        required=True,
        help="Output binary manifest path",
    )
    parser.add_argument(
        "--baseline",
        default=None,
        help="Expected saved-baseline name (optional; inferred from inventory)",
    )
    args = parser.parse_args()

    try:
        digest = generate_manifest(
            args.criterion_root,
            args.inventory,
            args.manifest,
            baseline_name=args.baseline,
        )
    except BaselineManifestError as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)

    print(digest)


if __name__ == "__main__":
    main()
