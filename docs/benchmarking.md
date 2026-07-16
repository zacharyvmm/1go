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

1. Resolves the baseline revision (default: `origin/main`).
2. Creates a temporary detached Git worktree for the baseline revision.
3. Compiles the benchmark binary for both baseline and current tree in **separate**
   `CARGO_TARGET_DIR` directories.
4. Runs the baseline benchmarks and saves Criterion baseline data.
5. Copies only Criterion's measurement data to the current target directory.
6. Runs the current benchmarks against the saved baseline.
7. Copies the final Criterion report (with percentage-change estimates) to
   `target/bench-compare/latest/`.
8. Writes metadata (`metadata.txt`) recording both revisions, toolchain info,
   and whether the working tree was dirty.
9. Cleans up the temporary worktree and directories.

**Key properties**:

- Your current branch is never switched. No `git checkout` or `git switch`.
- Uncommitted changes are included in the current measurements.
- A dirty working tree is reported but does not prevent the comparison.
- Build artifacts for baseline and current are fully isolated (separate
  `CARGO_TARGET_DIR` paths).
- Only Criterion measurement data is shared between the two runs.

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
BASE_REF=<commit> just bench-compare
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

## Benchmark scenarios

The regression suite covers:

| Scenario | Description |
|----------|-------------|
| Query construction | Building `Query` objects independently from parsing |
| Synthetic link parsing | Parsing link-heavy documents at 100/1K/10K scales |
| First-match placement | `Query::first` with early/middle/late/no-match positions. Validates exactly one result with exact content, attributes, class, and expected position. Reports **latency** (not throughput). |
| Nested product catalog | Hierarchical queries against product-like HTML |
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
