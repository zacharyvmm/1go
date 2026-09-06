# Text-performance follow-up, 6 September 2026

This follow-up passes all eight unchanged performance limits in the native x86 comparison. It reduces entity lookup and capture bookkeeping costs without changing the parser architecture, text semantics, benchmark fixtures, or gate budgets.

Branch: `perf/text-performance-experiment`. The comparison baseline is [PR #68 at c4fff2a](https://github.com/zacharyvmm/scah/commit/c4fff2a82549401a0fce7686d6f99f9aff5fb9cb). The fixed main baseline remains [7847d18](https://github.com/zacharyvmm/scah/commit/7847d18684d647916506d29328fa47414f2a978a). The [measurement record](text-experiment-2026-09-06.json) contains individual rounds and measured source hashes. The [first experiment](text-experiment-2026-09-05.md) records the earlier optimizations separately.

The [subsequent trials](text-experiment-2026-09-06-trials.md) were not retained; the implementation measured here remains the published baseline.

## Changes

- Derive a small entity-search index at compile time and search only names with the same initial byte.
- After a complete name fails to match, limit shorter-prefix searches to the maximum length of a legacy spelling without a semicolon. Longer prefixes cannot succeed. Keep the existing longest-prefix behavior.
- On text-capture paths, bypass attribute parsing for tags that end immediately after their name. The ordinary parser retains its previous path.
- Count raw and normalized save hits in one pass. Count closing captures while finalizing their ranges, replacing two additional passes over the saved elements.

An exhaustive reference test checks extended and truncated forms of all 2,231 generated entity names against a simple longest-prefix scan. The search index requires no per-parse allocation.

## Gate comparisons

Negative percentages mean less elapsed time. Each percentage is the median of three paired round ratios, using Criterion slope point estimates.

| Workload | Versus previous PR #68 | Versus main | Allowed versus main | Result |
| --- | ---: | ---: | ---: | --- |
| No content | +1.17% | 0.00% | +5% | Pass |
| Inner HTML only | +2.17% | +2.00% | +5% | Pass |
| Raw text only | -6.41% | +4.24% | +20% | Pass |
| Normalized text, no matches | +2.37% | +7.04% | +20% | Pass |
| Normalized text, sparse matches | +0.42% | +6.20% | +20% | Pass |
| Normalized prose | -1.03% | +56.25% | +130% | Pass |
| Ordinary parser, no match | -0.84% | +0.69% | +5% | Pass |
| Ordinary parser, match | -1.64% | -3.30% | +5% | Pass |

Main has simpler legacy text extraction and no equivalent raw-text API. Its raw-only benchmark uses legacy normalized extraction as a proxy. Passing a legacy compatibility budget does not establish equal performance or equal behavior.

## Broader text workloads

Both revisions use the corrected hidden-heavy benchmark that selects `div`. These comparisons do not count the earlier fixture correction as a performance improvement.

Timing columns are medians of round mean estimates in microseconds. Percentages are medians of paired round ratios. Every broader case uses mean estimates consistently because Criterion's automatic sampling can omit slope estimates for slower cases. Percentages need not equal ratios of the displayed timing medians.

| Workload, 1,000 repetitions | Previous PR #68, us | Candidate, us | Time change |
| --- | ---: | ---: | ---: |
| Both text modes | 458.68 | 445.07 | -2.02% |
| Whitespace-heavy text | 490.91 | 462.07 | -5.84% |
| Sparse entities | 272.90 | 251.03 | -7.87% |
| Plain text without entities | 206.80 | 198.24 | -4.55% |
| Dense entities | 366.63 | 334.90 | -9.73% |
| Unknown entity names | 2693.55 | 788.07 | -70.21% |
| Hidden content | 502.36 | 286.80 | -42.91% |
| Preformatted text | 231.27 | 219.46 | -5.11% |

These workloads exercise different HTML shapes and attribute requirements. The strong entity and hidden-content gains do not generalize to dense prose with attributes. The gate prose case improved only 1.03% versus the previous PR #68 and remains 56.25% above legacy main in this comparison.

## Method and rejected checkpoint

Timing ran on the Acer's Intel Core i5-10300H under Linux x86-64 with Rust 1.96.0, AC power, the existing powersave governor, and affinity pinned to logical CPU 2. Criterion 0.8.2 used three rounds, a one-second warm-up, and a two-second measurement. Text-gate cases used 50 samples, ordinary cases 20, and broader cases 100. Variant order reversed on the middle round. Builds finished before timing, and comparisons used separately built, saved executables with the repository's bench profile.

The first complete candidate applied the tag shortcut to every parsing mode. Its inner-HTML result was +5.97% versus main, outside the +5% limit. That candidate was rejected. The final shortcut applies only when text capture is enabled, and all comparisons were rerun. The measurement record retains the rejected checkpoint's inner-HTML estimates and source hashes.

Exploratory SIMD whitespace scanning and compiler-inlining changes did not produce a dependable prose improvement and are not included.

## Validation

The final implementation passed these checks on ARM64 macOS with Rust 1.95.0:

- `cargo fmt --all -- --check`
- `cargo test --release -p scah`
- `cargo clippy --workspace --all-targets --exclude scah-benches --exclude scah-cursor-benches -- -D warnings`
- `cargo bench -p scah-benches --bench memory_bench_text_extraction`
- `cargo bench -p scah-benches --bench speed_bench_text_extraction -- --test`

The allocation checks passed. Unmatched normalized queries retain zero extra allocation over equivalent no-content queries at 1,000 and 10,000 paragraphs.

These are native Acer timings, not a confirmed GitHub-hosted CI result for the new commit. The earlier table-separator and moved-element review findings remain separate work.

To repeat the repository gates on x86-64 from this branch:

```sh
taskset -c 2 bash scripts/check-text-performance.sh 7847d18684d647916506d29328fa47414f2a978a
taskset -c 2 bash scripts/check-sibling-performance.sh 7847d18684d647916506d29328fa47414f2a978a
```

For broader comparisons, build the text-extraction speed benchmark separately at c4fff2a and at the candidate. Run the eight named workloads at size 1,000 with the Criterion settings above, alternating revision order between rounds. Use paired mean estimates consistently for those workloads.
