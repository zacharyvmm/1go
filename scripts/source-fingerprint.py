#!/usr/bin/env python3
"""Deterministic source-tree fingerprint.

Walks a snapshot root using explicit os.scandir() recursion, produces an
unambiguous binary manifest, and prints the SHA-256 of that manifest
to stdout.

The manifest format is NUL-delimited records:

    entry-type NUL mode NUL content-hash NUL relative-path NUL

Where:
  - entry-type is "file" or "symlink"
  - mode is one of "100644", "100755", "120000"
  - content-hash is the hex SHA-256 (64 chars) for files,
    or the SHA-256 of the symlink target string for symlinks
  - relative-path uses OS-native separators, never containing NUL

Directory symlinks are recorded as symlink entries (never followed).
Broken symlinks are valid entries represented by their target string.
Real directories are recursively traversed.

Test hooks (environment-variable-controlled, no production impact):
  SCAH_TEST_FAIL_SCANDIR=<relpath>  → raise OSError for that directory
  SCAH_TEST_FAIL_LSTAT=<relpath>    → raise OSError when lstating entry

Uses only the Python standard library.
"""

import argparse
import hashlib
import os
import stat
import sys
import tempfile
from dataclasses import dataclass


@dataclass(frozen=True)
class ManifestEntry:
    """A single source-tree entry in the fingerprint manifest."""

    entry_type: str      # "file" or "symlink"
    mode: str            # "100644", "100755", "120000"
    content_hash: str    # hex SHA-256
    relative_path: str   # relative path with OS-native separators


# ── Test hooks ────────────────────────────────────────────────────────────────


def _test_fail_scandir(relative_directory: str) -> None:
    """Raise OSError if the env var targets this directory (test hook)."""
    target = os.environ.get("SCAH_TEST_FAIL_SCANDIR", "")
    if target and target == relative_directory:
        raise OSError(
            f"mocked scandir failure for {relative_directory!r}"
        )


def _test_fail_lstat(relative_path: str) -> None:
    """Raise OSError if the env var targets this path (test hook)."""
    target = os.environ.get("SCAH_TEST_FAIL_LSTAT", "")
    if target and target == relative_path:
        raise OSError(
            f"mocked lstat failure for {relative_path!r}"
        )


# ── Mode helpers ──────────────────────────────────────────────────────────────


def _mode_repr(st_mode: int) -> str:
    """Return a Git-style mode string for the given stat mode."""
    if stat.S_ISLNK(st_mode):
        return "120000"
    if stat.S_ISREG(st_mode):
        return "100755" if st_mode & stat.S_IXUSR else "100644"
    if stat.S_ISDIR(st_mode):
        raise RuntimeError(
            "internal: directories should be filtered before mode_repr"
        )
    raise RuntimeError(
        f"unsupported file type (mode 0o{st_mode:o}); "
        "only regular files and symlinks are supported"
    )


def _is_git_metadata(rel_path: str) -> bool:
    """True when *rel_path* is the root .git administration entry.

    In a standard checkout .git is a directory. In a linked worktree it is a
    file containing the path of the real gitdir. Both must be excluded so the
    fingerprint depends only on source content.
    """
    return rel_path == ".git" or rel_path.startswith(f".git{os.sep}")


# ── Hashing ───────────────────────────────────────────────────────────────────


def _hash_regular_file(path: str) -> str:
    """Return SHA-256 hex digest of a regular file's contents (streamed)."""
    hasher = hashlib.sha256()
    try:
        with open(path, "rb") as fh:
            for chunk in iter(lambda: fh.read(1024 * 1024), b""):
                hasher.update(chunk)
    except OSError as exc:
        raise RuntimeError(
            f"failed to read {path!r}: {exc}"
        ) from exc
    return hasher.hexdigest()


def _hash_symlink_target(path: str) -> str:
    """Return SHA-256 hex digest of a symlink's target string."""
    try:
        target = os.readlink(path)
    except OSError as exc:
        raise RuntimeError(
            f"failed to read symlink {path!r}: {exc}"
        ) from exc
    payload = target.encode("utf-8", errors="surrogateescape")
    return hashlib.sha256(payload).hexdigest()


# ── Entry collection ──────────────────────────────────────────────────────────


def _collect_entry(
    root: str,
    relative_path: str,
    entries: list[ManifestEntry],
) -> None:
    """Classify and record a single directory entry.

    Inspects the entry with os.lstat() (no symlink following).  Symlinks
    (including symlinks to directories and broken symlinks) are recorded
    as symlink entries and never recursed into.  Real directories are
    recursed into.  Regular files are hashed and recorded.
    """
    full_path = os.path.join(root, relative_path)

    # Test hook: mock lstat failure.
    _test_fail_lstat(relative_path)

    try:
        st = os.lstat(full_path)
    except OSError as exc:
        raise RuntimeError(
            f"failed to stat {relative_path!r}: {exc}"
        ) from exc

    # Check symlink BEFORE directory — a symlink to a directory is still
    # a symlink and should be recorded as such, not recursed.
    if stat.S_ISLNK(st.st_mode):
        content_hash = _hash_symlink_target(full_path)
        mode = _mode_repr(st.st_mode)
        entries.append(
            ManifestEntry(
                entry_type="symlink",
                mode=mode,
                content_hash=content_hash,
                relative_path=relative_path,
            )
        )

    elif stat.S_ISDIR(st.st_mode):
        _collect_directory(root, relative_path, entries)

    elif stat.S_ISREG(st.st_mode):
        content_hash = _hash_regular_file(full_path)
        mode = _mode_repr(st.st_mode)
        entries.append(
            ManifestEntry(
                entry_type="file",
                mode=mode,
                content_hash=content_hash,
                relative_path=relative_path,
            )
        )

    else:
        raise RuntimeError(
            f"unsupported entry type for {relative_path!r}: "
            f"mode 0o{st.st_mode:o}; "
            "only regular files, directories, and symlinks are supported"
        )


def _collect_directory(
    root: str,
    relative_directory: str,
    entries: list[ManifestEntry],
) -> None:
    """Recursively collect entries from a directory using os.scandir().

    Entries are sorted by byte-oriented name for deterministic ordering.
    Directory-open failures propagate immediately — subtrees are never
    silently skipped.
    """
    directory_path = (
        root
        if not relative_directory
        else os.path.join(root, relative_directory)
    )

    # Test hook: mock scandir failure.
    _test_fail_scandir(relative_directory)

    try:
        with os.scandir(directory_path) as iterator:
            directory_entries = list(iterator)
    except OSError as exc:
        display_path = relative_directory or "."
        raise RuntimeError(
            f"failed to traverse directory {display_path!r}: {exc}"
        ) from exc

    directory_entries.sort(key=lambda entry: os.fsencode(entry.name))

    for entry in directory_entries:
        relative_path = (
            entry.name
            if not relative_directory
            else os.path.join(relative_directory, entry.name)
        )

        if _is_git_metadata(relative_path):
            continue

        _collect_entry(root, relative_path, entries)


def collect_entries(root: str) -> list[ManifestEntry]:
    """Walk *root* and collect all source entries."""
    entries: list[ManifestEntry] = []
    _collect_directory(root, "", entries)
    return entries


# ── Manifest serialization ───────────────────────────────────────────────────


def write_manifest(
    entries: list[ManifestEntry],
    manifest_path: str,
) -> str:
    """Write the binary manifest atomically and return its SHA-256 hex digest.

    Entries are sorted by byte-oriented relative path for determinism.
    The manifest is written to a temporary file first, then atomically
    renamed into place with os.replace().
    """
    entries.sort(key=lambda entry: os.fsencode(entry.relative_path))

    hasher = hashlib.sha256()
    manifest_dir = os.path.dirname(os.path.abspath(manifest_path))
    fd, tmp_path = tempfile.mkstemp(
        dir=manifest_dir, prefix=".tmp-manifest-", suffix=".bin"
    )

    try:
        with os.fdopen(fd, "wb") as fh:
            for entry in entries:
                record = (
                    entry.entry_type.encode("utf-8") + b"\0"
                    + entry.mode.encode("utf-8") + b"\0"
                    + entry.content_hash.encode("utf-8") + b"\0"
                    + entry.relative_path.encode(
                        "utf-8", errors="surrogateescape"
                    )
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


# ── Public API ───────────────────────────────────────────────────────────────


def fingerprint(root: str, manifest_path: str) -> str:
    """Walk *root*, write the binary manifest, and return its SHA-256 hex.

    Raises RuntimeError on any unreadable, inaccessible, or unsupported
    entry.  No entry is ever silently omitted.
    """
    entries = collect_entries(root)
    return write_manifest(entries, manifest_path)




def validate_symlink_containment(root, entries):
    """Validate every symlink stays inside root. Raises RuntimeError on escape."""
    root_abs = os.path.abspath(root)
    max_hops = 40

    for entry in entries:
        if entry.entry_type != "symlink":
            continue

        resolved = _resolve_symlink_chain(
            root_abs, entry.relative_path, max_hops
        )
        if resolved is None:
            target = _read_link_target(os.path.join(root_abs, entry.relative_path))
            raise RuntimeError(
                f"escaping symlink: {entry.relative_path} -> {target}"
            )

def _resolve_symlink_chain(root_abs, rel_path, max_hops):
    """Resolve a symlink chain. Return normalized absolute path if contained, None if escaping.

    Raises RuntimeError on symlink loops or excessive chain length.
    """
    seen_inodes = set()
    current_path = os.path.join(root_abs, rel_path)
    hops = 0

    while hops < max_hops:
        hops += 1

        if not os.path.islink(current_path):
            try:
                if os.path.commonpath([os.path.abspath(current_path), root_abs]) != root_abs:
                    return None
            except ValueError:
                return None
            return os.path.abspath(current_path)

        try:
            st = os.lstat(current_path)
            inode_key = (st.st_dev, st.st_ino)
            if inode_key in seen_inodes:
                raise RuntimeError(
                    f"symlink loop detected at: {rel_path}"
                )
            seen_inodes.add(inode_key)
        except OSError:
            pass

        target = os.readlink(current_path)
        if os.path.isabs(target):
            return None

        current_dir = os.path.dirname(current_path)
        current_path = os.path.normpath(os.path.join(current_dir, target))

        try:
            if os.path.commonpath([os.path.abspath(current_path), root_abs]) != root_abs:
                return None
        except ValueError:
            return None

    raise RuntimeError(
        f"symlink chain too long at: {rel_path}"
    )


def _read_link_target(path):
    """Read symlink target, handling errors."""
    try:
        return os.readlink(path)
    except OSError:
        return "<unreadable>"
# ── CLI ──────────────────────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Deterministic source-tree fingerprint"
    )
    parser.add_argument(
        "--root",
        required=True,
        help="Snapshot root directory",
    )
    parser.add_argument(
        "--manifest",
        required=True,
        help="Path for the binary manifest output",
    )
    parser.add_argument(
        "--reject-escaping-symlinks",
        action="store_true",
        help="Reject symlinks that escape the root directory.",
    )
    args = parser.parse_args()

    if not os.path.isdir(args.root):
        print(
            f"error: root is not a directory: {args.root}",
            file=sys.stderr,
        )
        sys.exit(1)

    try:
        entries = collect_entries(args.root)
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)

    if args.reject_escaping_symlinks:
        try:
            validate_symlink_containment(args.root, entries)
        except RuntimeError as exc:
            print(f"error: {exc}", file=sys.stderr)
            sys.exit(1)

    try:
        fp = write_manifest(entries, args.manifest)
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)

    print(fp)


if __name__ == "__main__":
    main()
