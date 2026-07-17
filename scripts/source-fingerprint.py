#!/usr/bin/env python3
"""Deterministic source-tree fingerprint.

Walks a snapshot root, produces an unambiguous binary manifest, and prints
the SHA-256 of that manifest to stdout.

The manifest format is NUL-delimited records:

    entry-type NUL mode NUL content-hash NUL relative-path NUL

Where:
  - entry-type is "file" or "symlink"
  - mode is one of "100644", "100755", "120000"
  - content-hash is the hex SHA-256 (64 chars) for files,
    or the SHA-256 of the symlink target string for symlinks
  - relative-path uses OS-native separators, never containing NUL

Uses only the Python standard library.
"""

import argparse
import hashlib
import os
import stat
import sys
import tempfile


def _mode_repr(st_mode: int) -> str:
    """Return a Git-style mode string for the given stat mode."""
    if stat.S_ISLNK(st_mode):
        return "120000"
    if stat.S_ISREG(st_mode):
        return "100755" if st_mode & stat.S_IXUSR else "100644"
    if stat.S_ISDIR(st_mode):
        # Directories skipped — they carry no content in Git snapshots.
        raise RuntimeError("internal: directories should be filtered before mode_repr")
    raise RuntimeError(
        f"unsupported file type (mode 0o{st_mode:o}); "
        f"only regular files and symlinks are supported"
    )


def _is_git_metadata(rel_path: str) -> bool:
    """True when *rel_path* is the root .git administration entry.

    In a standard checkout .git is a directory. In a linked worktree it is a
    file containing the path of the real gitdir. Both must be excluded so the
    fingerprint depends only on source content.
    """
    return rel_path == ".git" or rel_path.startswith(f".git{os.sep}")


def fingerprint(root: str, manifest_path: str) -> str:
    """Walk *root*, write the binary manifest, and return its SHA-256 hex.

    Raises RuntimeError on any unreadable or unsupported entry.
    """
    entries: list[tuple[str, str, str, str]] = []
    # entry-type, mode, content-hash, relative-path

    for dirpath, dirnames, filenames in os.walk(root):
        # Exclude .git from both traversal and file listing.
        if ".git" in dirnames:
            dirnames.remove(".git")

        for name in filenames:
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, root)

            # Belt-and-suspenders — os.walk won't traverse into .git after
            # dirnames removal, but .git-as-file would still appear in filenames.
            if _is_git_metadata(rel):
                continue

            try:
                lst = os.lstat(full)
            except OSError as exc:
                raise RuntimeError(
                    f"failed to stat {rel!r}: {exc}"
                ) from exc

            if stat.S_ISLNK(lst.st_mode):
                try:
                    target = os.readlink(full)
                except OSError as exc:
                    raise RuntimeError(
                        f"failed to read symlink {rel!r}: {exc}"
                    ) from exc
                payload = target.encode("utf-8", errors="surrogateescape")
                content_hash = hashlib.sha256(payload).hexdigest()
                mode = _mode_repr(lst.st_mode)
                entries.append(("symlink", mode, content_hash, rel))

            elif stat.S_ISREG(lst.st_mode):
                try:
                    with open(full, "rb") as fh:
                        content_hash = hashlib.sha256(
                            fh.read()
                        ).hexdigest()
                except OSError as exc:
                    raise RuntimeError(
                        f"failed to read {rel!r}: {exc}"
                    ) from exc
                mode = _mode_repr(lst.st_mode)
                entries.append(("file", mode, content_hash, rel))

            else:
                raise RuntimeError(
                    f"unsupported entry type for {rel!r}: "
                    f"mode 0o{lst.st_mode:o}; "
                    f"only regular files and symlinks are supported"
                )

    # Sort by raw relative path for determinism.
    entries.sort(key=lambda e: e[3])

    # Build and hash the manifest in a single pass.
    hasher = hashlib.sha256()
    manifest_bytes: list[bytes] = []

    for entry_type, mode, content_hash, rel_path in entries:
        record = (
            entry_type.encode("utf-8") + b"\0"
            + mode.encode("utf-8") + b"\0"
            + content_hash.encode("utf-8") + b"\0"
            + rel_path.encode("utf-8", errors="surrogateescape") + b"\0"
        )
        manifest_bytes.append(record)

    # Write manifest atomically via a temporary file in the same directory.
    manifest_dir = os.path.dirname(os.path.abspath(manifest_path))
    fd, tmp_path = tempfile.mkstemp(
        dir=manifest_dir, prefix=".tmp-manifest-", suffix=".bin"
    )
    try:
        with os.fdopen(fd, "wb") as fh:
            for chunk in manifest_bytes:
                hasher.update(chunk)
                fh.write(chunk)
        os.rename(tmp_path, manifest_path)
    except BaseException:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass
        raise

    return hasher.hexdigest()


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
    args = parser.parse_args()

    if not os.path.isdir(args.root):
        print(
            f"error: root is not a directory: {args.root}",
            file=sys.stderr,
        )
        sys.exit(1)

    try:
        fp = fingerprint(args.root, args.manifest)
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        sys.exit(1)

    print(fp)


if __name__ == "__main__":
    main()
