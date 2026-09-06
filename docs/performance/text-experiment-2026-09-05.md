# Text-performance experiment, 5 September 2026

The [follow-up experiment](text-experiment-2026-09-06.md) records subsequent optimizations and updated comparisons.

The experiment passes all six text-extraction limits and both ordinary-parser limits in three-round comparisons on the Acer's x86-64 CPU. No architectural refactor or gate-budget change was needed.

Branch: `perf/text-performance-experiment`.

The starting point is [PR #62 at ec0ac04](https://github.com/zacharyvmm/scah/commit/ec0ac044b24733ec5627f62126e9e8379133168b). The main baseline is [7847d18](https://github.com/zacharyvmm/scah/commit/7847d18684d647916506d29328fa47414f2a978a). Raw round estimates, compiler settings, and hashes of the measured source files are in [the measurement record](text-experiment-2026-09-05.json).

## Changes

- Copy prose containing single internal spaces in contiguous runs, and skip repeated HTML whitespace as a group. Preserve Unicode spaces and the existing handling of preformatted text.
- Resolve the complete common spellings `&amp;`, `&lt;`, `&gt;`, `&quot;`, and `&nbsp;` before searching the full entity table. Other names and semicolon-less references retain the general decoder.
- Store the normalized-text requirement for `hidden` as a flag instead of repeatedly inserting and searching a generic attribute key.
- Remove a redundant flush on opening tags and skip flush calls when neither raw nor normalized capture is active.

The hidden-heavy benchmark now selects `div`, which its fixture actually contains. The previous `p` query never captured text. This correction affects the broader benchmark suite; the six timed gate workloads are unchanged.

## Measurements

Intel Core i5-10300H, Linux x86-64, AC power, existing powersave governor, affinity pinned to logical CPU 2. Compiler: Rust 1.96.0. Both revisions use the repository's release/bench settings: optimization level 3, fat LTO, and one codegen unit. Builds use separate target directories and finish before timing begins.

Criterion 0.8.2 runs three rounds, with a one-second warm-up and two-second measurement per case. Text workloads use 50 samples; ordinary workloads use 20. Variant order reverses on the middle round. The text comparison includes main, the original PR, and the candidate; the ordinary comparison uses main and the candidate.

Each percentage is the median of the three paired slope ratios, following the repository gate calculation. Negative means faster. This differs from dividing the separate median timings stored in the JSON record.

| Text workload | Versus original PR | Versus main | Allowed versus main | Result |
| --- | ---: | ---: | ---: | --- |
| No content | -1.81% | -9.01% | +5% | Pass |
| Inner HTML only | +5.71% | +0.41% | +5% | Pass |
| Normalized text, no matches | -14.37% | +6.75% | +20% | Pass |
| Normalized text, sparse matches | -10.17% | +9.92% | +20% | Pass |
| Normalized prose | -22.62% | +60.53% | +130% | Pass |
| Raw text only | -7.38% | +9.88% | +20% | Pass |

The separate ordinary-parser gate allows +5% versus main. no match measured +2.22%; match measured -1.22%. Both pass.

Inner-HTML-only was 5.71% slower than the PR baseline in this comparison, while remaining 0.41% above main. This is a tradeoff to watch in CI, rather than an improvement across every workload. Short exploratory runs varied, so the reported results use the full three-round configuration.

Main's legacy text extractor does less work and has no raw-text API. The existing wider legacy budgets therefore compare different behavior. They were not relaxed for this experiment.

Callgrind profiling helped identify entity lookup, whitespace processing, attribute parsing, and repeated parser work outside active captures. Its instruction counts informed the changes; the table above comes from native elapsed-time benchmarks, not profiler timings.

## Validation

The final implementation passed:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --exclude scah-benches --exclude scah-cursor-benches -- -D warnings`
- `cargo test --release -p scah`
- `cargo bench -p scah-benches --bench memory_bench_text_extraction`
- `cargo bench -p scah-benches --bench speed_bench_text_extraction -- --test`
- Three-round native x86 comparisons using both gate benchmarks and their unchanged thresholds.

The correctness suite includes a new check of all 2,231 generated entity names and a test mixing prose runs, HTML whitespace, inline elements, entities, and multibyte Unicode. The allocation check reports zero extra bytes for unmatched text queries compared with equivalent no-content queries at both 1,000 and 10,000 paragraphs.

Correctness, lint, and allocation checks ran on the local ARM64 Mac with Rust 1.95.0. Timing ran on the Acer with Rust 1.96.0. GitHub-hosted CI has not been rerun for this experimental branch, so these measurements do not establish a CI pass on its hardware.

To repeat the repository gates on x86-64:

```sh
taskset -c 2 bash scripts/check-text-performance.sh 7847d18684d647916506d29328fa47414f2a978a
taskset -c 2 bash scripts/check-sibling-performance.sh 7847d18684d647916506d29328fa47414f2a978a
```

This experiment retains the current parser and shared text storage. The earlier review findings concerning table separators and moved elements still need separate fixes.
