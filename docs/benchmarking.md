# Benchmarking SCaH

SCaH has two benchmarking systems that serve different purposes.

## Comparison benchmarks

**Purpose**: Compare SCaH against other HTML/CSS-selection libraries (tl, scraper,
lexbor, lxml, lol_html). These generate public-facing performance results.

**Location**: `benches/` (the `scah-benches` package)

**Dependencies**: Includes competitor libraries, so compilation is slower and
includes unrelated build noise.

**Commands**:

```bash
just bench              # Run all comparison benchmarks (Rust + Node + Python)
just bench-rust         # Rust comparison benchmarks only
just bench-simple-all   # Simple "all" selector comparison
just bench-first        # Simple "first" selector comparison
just bench-whatwg       # WHATWG HTML spec benchmark
just bench-nested       # Nested product catalog comparison
```

**When to use**: Less frequently. When you need to demonstrate SCaH's performance
relative to alternatives, or when preparing public benchmark results.

## Regression benchmarks

**Purpose**: Compare current SCaH code against `origin/main` (or another revision)
to detect performance regressions during development.

**Location**: `benches/regression/` (the `scah-regression-benches` package)

**Dependencies**: The Criterion benchmark target depends on SCaH and Criterion. The regression
benchmark package also declares an optional, Linux-targeted Gungraun
dependency for instruction-count benchmarking.

**Commands**:

```bash
just bench-regression        # Run the full regression suite
just bench-regression-quick  # Run with reduced sample sizes (quick profile)
just bench-compare           # Compare current tree against origin/main
just bench-compare-quick     # Quick comparison against origin/main
just bench-compare HEAD~1    # Compare against a specific revision
```

### How `bench-compare` works

`just bench-compare` (backed by `scripts/bench-compare.sh`):

1. Resolves the baseline and current revisions.
2. Captures the current source state atomically: a binary-safe diff of tracked
   changes plus all untracked entries staged into a capture directory. The
   capture is verified coherent (tracked and untracked state unchanged during
   staging, with up to three retry attempts).
3. Creates a temporary detached Git worktree for the baseline revision.
4. Creates a second temporary detached worktree for the current revision,
   applies tracked dirty changes, and copies untracked files from the capture
   directory — producing an **isolated source snapshot** that never reads
   bytes from the live repository.
5. Validates that no symlink in either snapshot escapes the snapshot root
   (rejects absolute targets, `../` escapes, and chained escapes).
6. Fingerprints both snapshots before any Cargo invocation. The fingerprint
   depends only on relative source paths, file types, modes, file contents,
   and symlink targets — it does not depend on clone or temporary-directory
   location. `.git` administration metadata is excluded.
7. Hashes both `Cargo.lock` files before any Cargo invocation.
8. Compiles the benchmark binary for both baseline and current snapshot in
   **separate** `CARGO_TARGET_DIR` directories.
9. Fingerprints both snapshots after compilation to detect persistent mutation.
10. Runs the baseline benchmarks and saves Criterion baseline data.
11. Copies only named Criterion saved-baseline measurement directories to the
    current target directory (excluding `report/`, `new/`, `change/`).
12. Runs the current benchmarks against the saved baseline — from the
    snapshot, not the live repository. Requires fresh measurement and
    comparison output.
13. Fingerprints both snapshots after measurement to detect persistent mutation.
14. Re-fingerprints both snapshots and re-hashes both lockfiles **after**
    all Cargo commands complete.
15. Verifies that all before/after values are identical. If any snapshot
    or lockfile changed during compilation or measurement, publication is
    aborted with a diagnostic identifying the phase and affected resource.
16. Stages the Criterion report, metadata, source manifest, and untracked
    capture manifest. Validates that all required artifacts are present.
17. Atomically publishes the report to `target/bench-compare/latest/`.
18. Cleans up the temporary worktrees, capture directories, and the
    repository-common lock.

- Both baseline and current measurements run in detached temporary worktrees.
- Dirty tracked changes are applied to the current snapshot; non-ignored
  untracked files are staged during capture and copied from the staging
  directory — the live repository is never read during snapshot reconstruction.
- Unsupported filesystem entries (FIFOs, sockets, devices) are rejected before
  any Cargo invocation, preventing hangs.
- Edits made to the live repository **after** snapshot capture are not part of
  the measurement. The snapshot is verified to be coherent (tracked and
  untracked state captured atomically with retry).
- Your current branch is never switched. No `git checkout` or `git switch`.
- A dirty working tree is reported but does not prevent the comparison.
- Build artifacts for baseline and current are fully isolated (separate
  `CARGO_TARGET_DIR` paths).
- Only named Criterion saved-baseline measurement directories are transferred
  to the current target — baseline reports, `new/`, and `change/` are excluded.
- Fresh current measurement and comparison output is required; a stale report
  cannot satisfy validation.
- Source fingerprints are checked before and after each Cargo phase (compile and
  measurement) to detect persistent mutation. Endpoint equality does not prove
  the absence of transient mutation within one Cargo process.
- Metadata records explicit before/after fingerprints and lockfile hashes
  (`source_snapshot_endpoint_fingerprints_match`, `lockfile_endpoint_hashes_match`).
- Symlinks that escape the snapshot root (absolute targets, `../` escapes,
  chained escapes) are rejected before any Cargo invocation. Safe internal
  symlinks and broken internal symlinks are permitted.
- Harness changes under `benches/regression` remain rejected unless
  explicitly overridden with `ALLOW_BENCH_HARNESS_DIFF=1`.
- Linked worktrees of the same repository share one comparison lock derived
  from the common Git directory.
- `target/bench-compare/latest` contains local dirty-state metadata
  including `current-source-manifest.bin`, `current-source-manifest.sha256`,
  `tracked-diff.patch`, `untracked-files.txt`, and
  `untracked-capture-manifest.jsonl`.
- The previous successful report remains available after an integrity
  verification failure. No new report is published on failure.
- The source fingerprint uses a NUL-delimited binary manifest format that
  is unambiguous for filenames containing tabs, newlines, or other
  special characters. The fingerprint is the SHA-256 of this binary manifest.
### Fingerprint details

The source fingerprint is produced by `scripts/source-fingerprint.py` (stdlib only):

- Each entry records: `entry-type`, `mode`, `content-hash`, `relative-path`,
  NUL-delimited.
- Entry types: `file` or `symlink`.
- Modes: `100644` (non-executable regular file), `100755` (executable
  regular file), `120000` (symlink).
- For regular files, the content hash is SHA-256 of file contents.
- For symlinks, the content hash is SHA-256 of the symlink target string
  (the link is never followed). This applies to symlinks to regular files,
  symlinks to directories, and broken symlinks — all are fingerprinted by
  their target string, not by the target's contents.
- `.git` (whether a directory, regular file, or symlink) is excluded.
- The walker uses explicit `os.scandir()` recursion: every entry is
  inspected with `os.lstat()`, and directory symlinks are detected before
  they could be mistaken for real directories. Failures to open a directory,
  stat an entry, read a file, or read a symlink target all abort the
  comparison — no entries are ever silently skipped.
- Unsupported entry types (FIFOs, sockets, devices) cause a fatal error.
- The manifest is sorted by byte-oriented relative path for determinism.
- Identical source trees at different paths produce identical fingerprints.

### First-merge limitation

The baseline revision must already contain the `scah-regression-benches` package.
During the initial infrastructure PR, `origin/main` does not yet have it.

If you see:

```
error: The baseline revision does not contain the SCaH regression benchmark package.
```

This is expected. Merge the infrastructure first, then use it as the baseline
for subsequent performance changes. Alternatively, compare against a revision
that already contains the harness:

```bash
just bench-compare <commit>
```

### Harness-integrity check

bench-compare refuses to run when `benches/regression` differs from the selected
baseline. Criterion benchmark IDs are meaningful only when both revisions use
the same workload.

For benchmark-infrastructure development only, this check can be bypassed with
`ALLOW_BENCH_HARNESS_DIFF=1`. Results produced with that override may not be
valid performance comparisons.

## Best practices

### Machine stability

- Run benchmarks on an otherwise idle machine.
- Use AC power on laptops and disable power-saving mode.
- Avoid benchmarking while the machine is thermally saturated.
- Close or pause CPU-heavy applications (browsers, IDEs, compilers).

### Interpreting results

- Criterion reports point estimates, confidence intervals, and throughput changes.
- Treat differences below ~2% cautiously unless consistently reproduced.
- Repeat suspicious small regressions before acting on them.
- The first version of the comparison script **reports results but does not
  hard-fail on slowdowns**. Interpretation is up to the developer.

### Benchmark profiles

Set `SCAH_BENCH_PROFILE` to control measurement depth:

| Profile | Sample size | Warm-up | Measurement | Use case |
|---------|------------|---------|-------------|----------|
| `full` (default) | 100 | 3s | 5s | Thorough comparison |
| `quick` | 30 | 1s | 2s | Rapid feedback during development |

```bash
SCAH_BENCH_PROFILE=quick just bench-regression
just bench-compare-quick   # shorthand
```

## Benchmark scenarios

The regression suite covers:

| Scenario | Description |
|----------|-------------|
| Query construction | Building `Query` objects independently from parsing |
| Synthetic link parsing | Parsing link-heavy documents at 100/1K/10K scales |
| First-match placement | `Query::first` with early/middle/late/no-match positions. Validates exactly one result with exact content, attributes, class, and expected position. Reports **latency** (not throughput). |
| Nested product catalog | Hierarchical queries against product-like HTML. `nested_all` scenarios report full-document byte throughput; `nested_first` reports **latency** only because `Query::first` exits after the first product and nested child queries complete. |
| Multi-query pressure | Parsing one document with 1/4/16/32 independent queries |
| Instruction counts | Deterministic CPU metrics via Gungraun (Linux only). Requires Valgrind and `gungraun-runner` to execute. On non-Linux targets, compiles to a small explanatory fallback. |
Every benchmark validates its expected results before timing begins.
A performance improvement that silently drops results will cause the
benchmark setup to fail rather than appear faster.

### Save-mode validation

Save-mode benchmarks (`Save::none`, `Save::only_inner_html`, `Save::only_text_content`,
`Save::all`) validate exact representative inner HTML, text content, attributes,
and first/last element association before timing begins. Empty strings, truncated
content, wrong element association, and incorrect entity encoding all cause
validation failures before measurement starts. SCaH preserves source-level entity
encoding in both inner HTML and text content.

### Gungraun setup and teardown

Fixture creation, query construction, correctness validation, and destruction of
setup inputs occur outside the measured instruction-count region. Each benchmark
returns its setup input so the teardown function deallocates the large fixture
allocations after measurement concludes. The parsed `Store` is intentionally
dropped inside the measured operation to match Criterion's per-iteration parse
behavior.

### First-match correctness

Every first-match setup validates all of the following before timing begins:

- Exactly one result is returned (no spurious extra matches).
- The result appears at the expected position in the document.
- The text content matches exactly (no substring or partial matching).
- The inner HTML matches exactly.
- The `href` attribute matches exactly.
- The target class is exactly `target`.

A regression that returns multiple elements, the wrong element, corrupted
content, or empty fields causes validation failure before measurement starts.

### Portability

The instruction-count benchmark executes only on Linux with Valgrind and
gungraun-runner. On non-Linux targets, the benchmark binary compiles to a
small explanatory fallback. Gungraun itself is included only for Linux
targets.

## CI

Criterion validation runs in `--test` mode on every PR, exercising benchmark
setup and correctness checks. The instruction-count benchmark target is
compiled with its `linux-instruction-benches` feature enabled but is not
executed in standard CI (execution requires Valgrind and `gungraun-runner`).
A macOS portability check ensures the benchmark package compiles with all
features on non-Linux platforms.
