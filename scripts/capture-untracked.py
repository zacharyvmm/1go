#!/usr/bin/env python3
"""Stage untracked entries into a capture directory for isolated benchmarking.

Reads a NUL-delimited list of relative paths from a file (produced by
``git ls-files --others --exclude-standard -z``), classifies each entry
via ``lstat``, and either copies it into a staging directory or fails.

Two subcommands:

    capture — stage entries and write a manifest
    inspect — scan live entries without copying, for verification

Both modes produce identical manifest formats for byte-for-byte comparison.
"""

import argparse
import hashlib
import json
import os
import stat
import sys
from typing import List

# ── Constants ────────────────────────────────────────────────────────────────

_O_NOFOLLOW = getattr(os, "O_NOFOLLOW", 0)


# ── File helpers ─────────────────────────────────────────────────────────────


def _hash_regular_file(src_abs: str) -> str:
    """Hash a regular file and return its SHA-256 hex digest."""
    flags = os.O_RDONLY
    if _O_NOFOLLOW:
        flags |= _O_NOFOLLOW

    try:
        fd = os.open(src_abs, flags)
    except OSError as exc:
        raise OSError(f"cannot open (possible symlink): {exc}") from exc

    try:
        st_before = os.fstat(fd)
        if not stat.S_ISREG(st_before.st_mode):
            raise ValueError(f"expected regular file, got mode {st_before.st_mode:o}")

        hasher = hashlib.sha256()
        with os.fdopen(fd, "rb") as src_fh:
            while True:
                chunk = src_fh.read(1 << 20)
                if not chunk:
                    break
                hasher.update(chunk)

        # Re-stat via path (fd closed by fdopen).
        st_after = os.stat(src_abs)
        if st_before.st_dev != st_after.st_dev:
            raise OSError("file device changed during read")
        if st_before.st_ino != st_after.st_ino:
            raise OSError("file inode changed during read")
        if st_before.st_size != st_after.st_size:
            raise OSError("file size changed during read")

        return hasher.hexdigest()
    except BaseException:
        try:
            os.close(fd)
        except OSError:
            pass
        raise


def _copy_regular_file(src_abs: str, dst_abs: str) -> str:
    """Copy a regular file securely and return its SHA-256 hex digest.

    Normalizes the destination mode to 0o755 (executable) or 0o644.
    """
    flags = os.O_RDONLY
    if _O_NOFOLLOW:
        flags |= _O_NOFOLLOW

    try:
        fd = os.open(src_abs, flags)
    except OSError as exc:
        raise OSError(f"cannot open (possible symlink): {exc}") from exc

    try:
        st_before = os.fstat(fd)
        if not stat.S_ISREG(st_before.st_mode):
            raise ValueError(f"expected regular file, got mode {st_before.st_mode:o}")

        hasher = hashlib.sha256()
        with os.fdopen(fd, "rb") as src_fh, open(dst_abs, "wb") as dst_fh:
            while True:
                chunk = src_fh.read(1 << 20)
                if not chunk:
                    break
                hasher.update(chunk)
                dst_fh.write(chunk)

        # Re-stat via path (fd closed by fdopen).
        st_after = os.stat(src_abs)
        if st_before.st_dev != st_after.st_dev:
            raise OSError("file device changed during read")
        if st_before.st_ino != st_after.st_ino:
            raise OSError("file inode changed during read")
        if st_before.st_size != st_after.st_size:
            raise OSError("file size changed during read")

        # Normalize mode: preserve only executable bit.
        src_mode = st_before.st_mode & 0o777
        normalized = 0o755 if (src_mode & 0o111) else 0o644
        os.chmod(dst_abs, normalized)

        return hasher.hexdigest()
    except BaseException:
        try:
            os.close(fd)
        except OSError:
            pass
        raise


def _hash_symlink_target(src_abs: str) -> str:
    """Read a symlink's target and return its SHA-256 hex digest."""
    target = os.readlink(src_abs)
    return hashlib.sha256(os.fsencode(target)).hexdigest()


# ── Entry classification ────────────────────────────────────────────────────


def _classify_entry(root: str, rel_path: str, capture_dir: str | None) -> dict:
    """Classify a single untracked entry and return its manifest record.

    When *capture_dir* is not None, also stage the entry.
    """
    src_abs = os.path.join(root, rel_path)

    try:
        st = os.lstat(src_abs)
    except OSError as exc:
        raise OSError(f"cannot lstat untracked entry: {exc}") from exc

    mode = st.st_mode

    if stat.S_ISREG(mode):
        if capture_dir is not None:
            dst_abs = os.path.join(capture_dir, rel_path)
            os.makedirs(os.path.dirname(dst_abs), exist_ok=True)
            digest = _copy_regular_file(src_abs, dst_abs)
        else:
            digest = _hash_regular_file(src_abs)

        return {
            "path": rel_path,
            "type": "file",
            "mode": "100755" if (mode & 0o111) else "100644",
            "hash": digest,
        }

    if stat.S_ISLNK(mode):
        target_hash = _hash_symlink_target(src_abs)
        if capture_dir is not None:
            dst_abs = os.path.join(capture_dir, rel_path)
            os.makedirs(os.path.dirname(dst_abs), exist_ok=True)
            target = os.readlink(src_abs)
            os.symlink(target, dst_abs)

        return {
            "path": rel_path,
            "type": "symlink",
            "mode": "120000",
            "target_hash": target_hash,
        }

    if stat.S_ISFIFO(mode):
        raise ValueError(f"unsupported entry type FIFO: {rel_path}")
    if stat.S_ISSOCK(mode):
        raise ValueError(f"unsupported entry type socket: {rel_path}")
    if stat.S_ISBLK(mode):
        raise ValueError(f"unsupported entry type block device: {rel_path}")
    if stat.S_ISCHR(mode):
        raise ValueError(f"unsupported entry type character device: {rel_path}")

    raise ValueError(f"unsupported entry type (mode {mode:o}): {rel_path}")


# ── Manifest I/O ─────────────────────────────────────────────────────────────


def _read_paths(paths_file: str) -> List[str]:
    """Read NUL-delimited paths file."""
    with open(paths_file, "rb") as fh:
        raw = fh.read()

    if not raw:
        return []

    result: List[str] = []
    for entry in raw.split(b"\0"):
        if not entry:
            continue
        result.append(os.fsdecode(entry))
    return result


def _write_manifest(path: str, records: List[dict]) -> None:
    """Write sorted JSON-lines manifest atomically."""
    records.sort(key=lambda r: r["path"])
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        for rec in records:
            json.dump(rec, fh, sort_keys=True)
            fh.write("\n")
    os.replace(tmp, path)


# ── CLI ──────────────────────────────────────────────────────────────────────


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Stage or inspect untracked entries."
    )
    sub = parser.add_subparsers(dest="command", required=True)

    cap = sub.add_parser("capture", help="Stage entries into a capture directory")
    cap.add_argument("--root", required=True, help="Repository root")
    cap.add_argument("--paths", required=True, help="NUL-delimited paths file")
    cap.add_argument("--destination", required=True, help="Capture directory")
    cap.add_argument("--manifest", required=True, help="Manifest output (JSONL)")

    insp = sub.add_parser("inspect", help="Inspect live entries without copying")
    insp.add_argument("--root", required=True, help="Repository root")
    insp.add_argument("--paths", required=True, help="NUL-delimited paths file")
    insp.add_argument("--manifest", required=True, help="Manifest output (JSONL)")

    return parser


def main() -> None:
    parser = _build_parser()
    args = parser.parse_args()

    root = os.path.abspath(args.root)
    paths = _read_paths(args.paths)
    records: List[dict] = []

    capture_dir = None
    if args.command == "capture":
        capture_dir = os.path.abspath(args.destination)

    for rel_path in paths:
        records.append(_classify_entry(root, rel_path, capture_dir))

    _write_manifest(args.manifest, records)

    action = "captured" if args.command == "capture" else "inspected"
    print(f"{len(records)} entries {action}")


if __name__ == "__main__":
    main()
