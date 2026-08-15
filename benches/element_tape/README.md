# Scalar element-tape experiment

This benchmark tests the prerequisite for another SIMD implementation: whether
a compact semantic element tape can pay for itself before its indexing pass is
vectorised.

The generated document is attribute-dense and contains four open tags per row.
Each tape record contains four `u32` source offsets plus flags (20 bytes with
the current Rust layout). The experiment compares:

- `linear_eager`: one pass, attributes parsed for every open tag;
- `linear_lazy`: one pass, tag-name rejection before attribute parsing;
- `tape_eager_fresh`: two passes and a newly allocated tape;
- `tape_eager_reused`: two passes with retained tape capacity;
- `tape_lazy_reused`: two passes with retained capacity and deferred attributes;
- `production_parse`: the complete Scah parser using `Save::none()`.

The frontend implementations validate their match count against
`production_parse` before Criterion measures them. `production_parse` is a
calibration point rather than a direct comparison: it also performs HTML stack
handling, query-cursor execution, recovery, and result storage.

## Results

Measured on an Apple M5 with macOS 26.6, using the workspace bench profile
(optimised, fat LTO), 30 Criterion samples, and a 10,000-row document:

| Selector | Linear eager | Linear lazy | Tape eager fresh | Tape eager reused | Tape lazy reused | Production |
|---|---:|---:|---:|---:|---:|---:|
| `a` | 2.111 ms | 1.224 ms | 2.178 ms | 2.131 ms | 1.248 ms | 2.518 ms |
| `a.promoted[href]` | 2.190 ms | 1.526 ms | 2.270 ms | 2.246 ms | 1.569 ms | 2.348 ms |
| `[data-index]` | 2.170 ms | 2.162 ms | 2.220 ms | 2.201 ms | 2.181 ms | 2.633 ms |

Relative to `linear_eager`:

| Selector | Tape eager fresh | Tape eager reused | Tape lazy reused | Linear lazy |
|---|---:|---:|---:|---:|
| `a` | +3.2% | +0.9% | -40.9% | -42.0% |
| `a.promoted[href]` | +3.6% | +2.5% | -28.4% | -30.3% |
| `[data-index]` | +2.3% | +1.5% | +0.5% | -0.3% |

The lazy tape remained 2.0%, 2.9%, and 0.9% slower than the equivalent lazy
linear frontend for the three selectors. Reusing allocation saved at most 2.2%
in the large cases and did not make eager tape parsing beat linear parsing.

The phase-isolated measurements change how that result should be interpreted:

| 10,000-row phase | Time | Effective input throughput |
|---|---:|---:|
| Scalar element indexing | 1.192 ms | 1.73 GiB/s |
| Consume tape for `a` | 0.047 ms | 44.26 GiB/s |
| Consume tape for `a.promoted[href]` | 0.344 ms | 6.00 GiB/s |
| Consume tape for `[data-index]` | 0.973 ms | 2.12 GiB/s |

For selective queries, the scalar indexing pass—not tape consumption—is the
dominant cost. The small difference between scalar linear and scalar tape paths
therefore does not predict the result of a genuinely vectorised indexer.

## Auto-SIMD follow-up

Two portable approaches were added without architecture-specific intrinsics:

1. Repeated `memchr` searches, which automatically select NEON on AArch64.
2. A branch-free dense classifier that LLVM can auto-vectorise across 64 input
   bytes per loop, followed by a scalar state machine over the classification
   bytes.

The repeated-search version reproduced the short-span failure mode: indexing
rose from 1.192 ms to 1.869 ms. Each individual search was SIMD accelerated,
but HTML supplied another quote or tag boundary too quickly to amortise search
setup.

The dense classifier itself reached 25.6 GiB/s. Inspection of the release
assembly confirmed 128-bit NEON loads, `cmeq` byte comparisons, vector `orr`,
and 64-byte vector stores. Complete indexing reached 1.82 GiB/s because the
subsequent semantic state machine, not classification, became dominant.

At 10,000 rows:

| Selector | Scalar lazy tape | Dense auto-SIMD lazy tape | Change | Scalar linear lazy |
|---|---:|---:|---:|---:|
| `a` | 1.248 ms | 1.173 ms | -6.1% | 1.224 ms |
| `a.promoted[href]` | 1.569 ms | 1.477 ms | -5.9% | 1.526 ms |
| `[data-index]` | 2.181 ms | 2.134 ms | -2.1% | 2.162 ms |

This is a useful automatic-SIMD test and it does beat both scalar controls, but
the one-byte-per-input-byte scratch buffer is deliberately diagnostic. A
simdjson-style implementation would retain packed structural masks per block so
the semantic state machine can skip entire non-structural regions without a
second byte-by-byte pass.

## Conclusion

The experiment strongly supports deferred attribute tokenisation. It also shows
that a scalar element tape cannot beat an equivalent scalar linear frontend,
because writing and rereading the records adds overhead.

It does **not** rule out a SIMD element indexer. On the 10,000-row tag-only case,
96% of lazy-tape time is the scalar indexing pass; consuming the completed tape
takes only 47 microseconds. A several-gigabyte-per-second indexer could therefore
change the end-to-end result substantially. The opportunity shrinks for broad
attribute selectors because their second pass still approaches one millisecond.

## Production integration follow-up

The scalar architecture was then integrated into Scah behind an incremental
tag-indexer interface. Opening tags use a two-phase handoff: discover the name,
ask the active query cursors whether attributes can matter, and then either
tokenize the attributes or scan straight to the tag end. This is important:
an earlier integration that first found the end and then tokenized attributes
regressed the universal-attribute case by roughly 26% because it scanned every
candidate tag twice.

On the same 10,000-row document, the final scalar production parser measured:

| Selector | Before refactor | Two-phase scalar parser | Change |
|---|---:|---:|---:|
| `a` | 2.518 ms | 2.219 ms | -11.9% |
| `a.promoted[href]` | 2.348 ms | 2.055 ms | -12.5% |
| `[data-index]` | 2.633 ms | 2.590 ms | -1.6% |

The next useful experiment is a packed-mask NEON indexing backend behind that
interface. It should preserve incremental delivery and provide cached end hints
for spans already discovered while classifying a block. Real HTML corpora remain
necessary before selecting that backend for production.

Run with:

```sh
cargo bench -p scah-benches --bench speed_bench_element_tape -- --noplot
```

## Scope

The experimental frontend handles normal tags, quoted values, comments, and
declarations needed by the generated workload. It is not a replacement HTML
parser and intentionally does not implement raw-text elements or Scah's error
recovery rules. Those costs remain represented only by `production_parse`.
