#!/usr/bin/env python3
"""Stage untracked entries into a capture directory for isolated benchmarking.

Reads a NUL-delimited list of relative paths from a file (produced by
``git ls-files --others --exclude-standard -z``), classifies each entry
via ``lstat``, and either copies it into a staging directory or fails.

Supported entry types:
  - regular file: copied byte-for-byte with executable-mode preservation
  - symlink: link target recorded (link is not followed)

Rejected entry types:
  - FIFO, socket, block device, character device, any other unsupported mode

Regular files are opened with O_NOFOLLOW (where available) and verified
via fstat to prevent TOCTOU symlink substitution between classification
and reading.

Interface::

    python3 scripts/capture-untracked.py \\
        --root <repo-root> \\
        --paths <untracked-list-file> \\
        --destination <capture-dir> \\
        --manifest <manifest-output>

The manifest records one JSON line per entry::

    {"path": "rel/path", "type": "file", "mode": "100755", "hash": "sha256hex"}
    {"path": "rel/path", "type": "symlink", "mode": "120000", "target_hash": "sha256hex"}
"""

import argparse
import hashlib
import json
import os
import stat
import sys

# ── File-copy helpers ────────────────────────────────────────────────────────

# O_NOFOLLOW may not exist on all platforms; fall back gracefully.
_O_NOFOLLOW = getattr(os, "O_NOFOLLOW", 0)


def _copy_regular_file(src_abs: str, dst_abs: str) -> str:
    """Copy a regular file securely and return its SHA-256 hex digest.

    Opens with O_NOFOLLOW (when available), verifies the opened fd points
    to a regular file via fstat, then streams the content to the destination.
    Preserves the source file's executable bit.
    """
    flags = os.O_RDONLY
    if _O_NOFOLLOW:
        flags |= _O_NOFOLLOW

    try:
        fd = os.open(src_abs, flags)
    except OSError as exc:
        raise OSError(f"cannot open (possible symlink): {exc}") from exc

    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):
            raise ValueError(
                f"expected regular file, got mode {st.st_mode:o}"
            )

        hasher = hashlib.sha256()
        # Open destination for writing (binary, truncate).
        with os.fdopen(fd, "rb") as src_fh, open(dst_abs, "wb") as dst_fh:
            while True:
                chunk = src_fh.read(1 << 20)  # 1 MiB
                if not chunk:
                    break
                hasher.update(chunk)
                dst_fh.write(chunk)

        # Preserve executable bit: 0o755 or 0o644.
        src_mode = st.st_mode & 0o777
        os.chmod(dst_abs, src_mode)

        return hasher.hexdigest()
    except BaseException:
        # fd may already be closed by fdopen; ignore errors on double-close.
        try:
            os.close(fd)
        except OSError:
            pass
        raise

def _stage_entry(
    root: str,
    rel_path: str,
    capture_dir: str,
    manifest_lines: list[str],
) -> None:
    """Classify and stage a single untracked entry."""
    src_abs = os.path.join(root, rel_path)

    try:
        st = os.lstat(src_abs)
    except OSError as exc:
        raise OSError(f"cannot lstat untracked entry: {exc}") from exc

    mode = st.st_mode
    dst_abs = os.path.join(capture_dir, rel_path)

    os.makedirs(os.path.dirname(dst_abs), exist_ok=True)

    if stat.S_ISREG(mode):
        # Regular file: copy with TOCTOU-safe open/fstat flow.
        digest = _copy_regular_file(src_abs, dst_abs)
        manifest_lines.append(
            json.dumps(
                {
                    "path": rel_path,
                    "type": "file",
                    "mode": "100755" if (mode & 0o111) else "100644",
                    "hash": digest,
                },
                sort_keys=True,
            )
        )
    elif stat.S_ISLNK(mode):
        # Symlink: record target, recreate without following.
        target = os.readlink(src_abs)
        os.symlink(target, dst_abs)
        target_hash = hashlib.sha256(
            os.fsencode(target)
        ).hexdigest()
        manifest_lines.append(
            json.dumps(
                {
                    "path": rel_path,
                    "type": "symlink",
                    "mode": "120000",
                    "target_hash": target_hash,
                },
                sort_keys=True,
            )
        )
    elif stat.S_ISFIFO(mode):
        raise ValueError(f"unsupported entry type FIFO: {rel_path}")
    elif stat.S_ISSOCK(mode):
        raise ValueError(f"unsupported entry type socket: {rel_path}")
    elif stat.S_ISBLK(mode):
        raise ValueError(f"unsupported entry type block device: {rel_path}")
    elif stat.S_ISCHR(mode):
        raise ValueError(f"unsupported entry type character device: {rel_path}")
    else:
        raise ValueError(
            f"unsupported entry type (mode {mode:o}): {rel_path}"
        )


# ── Main ─────────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Stage untracked entries into a capture directory."
    )
    parser.add_argument(
        "--root",
        required=True,
        help="Repository root (absolute path)",
    )
    parser.add_argument(
        "--paths",
        required=True,
        help="NUL-delimited file of relative untracked paths",
    )
    parser.add_argument(
        "--destination",
        required=True,
        help="Capture directory (created if needed)",
    )
    parser.add_argument(
        "--manifest",
        required=True,
        help="Manifest output file (JSON lines)",
    )
    args = parser.parse_args()

    root = os.path.abspath(args.root)
    capture_dir = os.path.abspath(args.destination)
    manifest_lines: list[str] = []

    # Read NUL-delimited paths.
    with open(args.paths, "rb") as fh:
        raw = fh.read()

    if not raw:
        # No untracked entries — write an empty manifest.
        with open(args.manifest, "w", encoding="utf-8") as mf:
            pass
        print("0 entries captured (no untracked files)")
        return

    # Split on NUL; decode with surrogateescape for non-UTF-8 paths.
    paths = raw.split(b"\0")
    for entry in paths:
        if not entry:
            continue
        rel_path = os.fsdecode(entry)
        _stage_entry(root, rel_path, capture_dir, manifest_lines)

    # Write manifest atomically.
    tmp_manifest = args.manifest + ".tmp"
    with open(tmp_manifest, "w", encoding="utf-8") as mf:
        for line in manifest_lines:
            mf.write(line + "\n")
    os.rename(tmp_manifest, args.manifest)

    print(f"{len(manifest_lines)} entries captured")


if __name__ == "__main__":
    main()
