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

**Dependencies**: SCaH and Criterion only for the core regression benchmarks.
The package also has an optional Linux-only Gungraun dependency for
instruction-count benchmarks (`linux-instruction-benches` feature).

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
| First-match placement | `Query::first` with early/middle/late/no-match positions. Reports **latency** (not throughput), since early exit may process only part of the input. |
| Nested product catalog | Hierarchical queries against product-like HTML |
| Multi-query pressure | Parsing one document with 1/4/16/32 independent queries |
| Instruction counts | Deterministic CPU metrics via Gungraun (Linux only). Fixture construction and validation occur outside the measured region. Requires Valgrind and `gungraun-runner` to execute. |

Every benchmark validates its expected results before timing begins.
A performance improvement that silently drops results will cause the
benchmark setup to fail rather than appear faster.

## CI

Criterion validation runs in `--test` mode on every PR, exercising benchmark
setup and correctness checks. The instruction-count benchmark target is
compiled with its `linux-instruction-benches` feature enabled but is not
executed in standard CI (execution requires Valgrind and `gungraun-runner`).
