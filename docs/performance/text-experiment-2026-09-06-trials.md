# Further text optimization trials, 6 September 2026

Neither trial was retained. Both improved dense prose by about 3% in their full comparisons, but neither produced a dependable pass across all eight unchanged limits. The parser source remains at [PR #68 revision f033b86](https://github.com/zacharyvmm/scah/commit/f033b8626c1be2071c763b598a92148ad394bbb3), whose GitHub CI completed successfully.

The [measurement record](text-experiment-2026-09-06-trials.json) includes every round, candidate source hashes, and patches relative to f033b86 so the rejected experiments can be reproduced. The [previous report](text-experiment-2026-09-06.md) describes the retained implementation.

## What was tested

The first trial represented absent source offsets with `usize::MAX` instead of `Option<usize>`, reducing offset storage and fragment-flush branches. It also made the collapsed-text writer borrow the pending separator directly, avoiding temporary removal and restoration of the decode buffer and repeated mode checks.

The second trial kept only the direct borrowing change. It preserved the existing parser layout and source-offset representation. Its focused three-round screen improved prose by 1.69% and ordinary no-match parsing by 2.23% versus f033b86, which justified a complete comparison. The complete comparison subsequently missed the ordinary no-match limit.

## Full gate results

Percentages compare each candidate against the same-run f033b86 baseline or fixed main revision 7847d18. Negative values mean less elapsed time. Each result is the median of three paired round ratios using Criterion slope estimates.

| Workload | Combined trial vs PR #68 | Combined vs main | Scratch-only vs PR #68 | Scratch-only vs main | Limit vs main |
| --- | ---: | ---: | ---: | ---: | ---: |
| No content | -3.29% | -1.15% | -4.79% | -1.34% | +5% |
| Inner HTML only | +1.88% | +5.41% | -10.12% | +0.51% | +5% |
| Raw text only | -5.43% | -1.00% | -3.66% | -1.80% | +20% |
| Normalized text, no matches | +8.14% | +12.88% | -0.24% | +11.47% | +20% |
| Normalized text, sparse matches | -3.58% | +7.50% | -1.44% | +6.44% | +20% |
| Normalized prose | -3.40% | +55.74% | -3.07% | +67.01% | +130% |
| Ordinary parser, no match | +11.43% | +12.22% | +6.11% | +6.71% | +5% |
| Ordinary parser, match | +8.49% | -2.96% | +0.21% | -5.58% | +5% |

The combined trial passed six limits. Inner HTML missed its 5% limit at +5.41%, and ordinary no-match parsing missed its 5% limit at +12.22%. The scratch-only trial passed seven limits, missing ordinary no-match parsing at +6.71%.

Main uses a simpler legacy extractor. It has no equivalent raw-text API, so raw-only uses legacy normalized text as a proxy. These comparisons do not establish equivalent behavior. Compare candidates with their contemporaneous baselines; main-relative percentages from separate sessions should not be treated as a continuous performance trend.

## Confirmation and variability

The two combined-trial failures received a separate five-round confirmation with two-second warm-ups and five-second measurements. Those repeats passed, at -2.22% versus main for inner HTML and +1.82% for ordinary no-match parsing. The latter was still 2.61% slower than f033b86. These results do not erase the original failed run.

The direction of some results changed between focused screens, complete runs, and confirmation. Changes in cases outside normalized-text writing cannot be credited directly to the borrowing change. Given the small prose gain, the inconsistent limits did not justify retaining either trial. No further repeats were used to seek a passing result for the scratch-only candidate.

## Broader workloads

These are median paired changes against f033b86 at 1,000 repetitions. All broader cases use mean point estimates consistently because Criterion automatic sampling can omit slopes. The hidden-content fixture is identical in both revisions.

| Workload | Combined trial | Scratch-only trial |
| --- | ---: | ---: |
| Both text modes | -2.30% | -1.81% |
| Whitespace-heavy text | -1.73% | -2.32% |
| Sparse entities | +11.59% | -0.70% |
| Plain text without entities | -4.37% | +1.40% |
| Dense entities | +1.31% | -11.78% |
| Unknown entity names | -2.65% | -3.71% |
| Hidden content | -5.29% | -7.01% |
| Preformatted text | -6.73% | -2.30% |

## Method and checks

Timings ran on the Acer's Intel Core i5-10300H under Linux x86-64 with Rust 1.96.0, AC power, the existing powersave governor, and affinity pinned to logical CPU 2. Criterion 0.8.2 used the repository bench profile, three rounds, a one-second warm-up, and a two-second measurement. Text cases used 50 samples, ordinary cases 20, and broader cases 100. Variant order reversed in the middle round. Builds finished before timing; saved executables preserved every revision. The scratch-only focused screen used three-second measurements.

Callgrind counted 466,310,711 instructions for f033b86 and 462,910,711 for the combined trial over 100 parses of 1,000 prose paragraphs, a 0.73% reduction. Fragment-flush instructions fell from 35,398,000 to 30,298,000. These are instruction counts, not elapsed-time shares or a predicted speedup. Neither trial materially changed the complete benchmark executable size.

Both candidates passed release correctness tests, instrumented release tests, formatting, workspace lint, allocation checks, and text-benchmark smoke tests on ARM64 macOS. Unmatched normalized queries still allocated zero extra bytes over equivalent no-content queries at 1,000 and 10,000 paragraphs. The exact commands are in the measurement record. Passing correctness checks did not resolve the performance misses.

After the trials, all parser and benchmark source edits were reverted to f033b86. This follow-up adds only the experiment record and the proposed next experiment. It does not change gate budgets, timed fixtures, public APIs, or text semantics.

## Next experiment

The [capture-mode specialization proposal](text-mode-specialization-proposal.md) would compile separate raw-only, normalized-only, mixed, and no-text paths from a shared implementation. It may remove irrelevant bookkeeping but can increase code size and affect instruction-cache behavior. It remains unimplemented pending the user's requested architectural approval.
