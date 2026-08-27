# Scalar element-tape experiment

This benchmark tests the prerequisite for another SIMD implementation: whether
a compact semantic element tape can pay for itself before its indexing pass is
vectorised.

The generated document is attribute-dense and contains four open tags per row.
Each tape record contains four `u32` source offsets plus flags (20 bytes with
the current Rust layout). The experiment compares:

- `streaming_eager`: one byte pass, attributes parsed while advancing to `>`;
- `streaming_lazy`: one byte pass, with attributes parsed only after a name match;
- `span_eager`: find `>` first, then revisit every open tag's attributes;
- `span_lazy`: find `>` first, then revisit attributes after a name match;
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

| Selector | Streaming eager | Streaming lazy | Span eager | Span lazy | Tape eager fresh | Tape eager reused | Tape lazy reused | Production |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `a` | 1.093 ms | 1.184 ms | 2.146 ms | 1.291 ms | 2.214 ms | 2.206 ms | 1.347 ms | 2.620 ms |
| `a.promoted[href]` | 1.186 ms | 1.180 ms | 2.255 ms | 1.592 ms | 2.325 ms | 2.308 ms | 1.641 ms | 2.362 ms |
| `[data-index]` | 1.152 ms | 1.142 ms | 2.235 ms | 2.243 ms | 2.289 ms | 2.283 ms | 2.285 ms | 2.717 ms |

The table reports Criterion point estimates. The corresponding 95% confidence
intervals are available in Criterion's generated report. Differences below one
percent are treated as ties here unless repeated runs keep their intervals
separate.

Relative to the single-pass streaming control with the same eager or lazy policy:

| Selector | Tape eager fresh vs. streaming eager | Tape eager reused vs. streaming eager | Tape lazy reused vs. streaming lazy |
|---|---:|---:|---:|
| `a` | +102.6% | +101.8% | +13.8% |
| `a.promoted[href]` | +96.0% | +94.6% | +39.1% |
| `[data-index]` | +98.7% | +98.2% | +100.1% |

The earlier `linear_*` controls were actually span-based two-scan
implementations. Renaming them and adding the streaming controls changes the
result materially: the scalar tape loses to a true single-pass frontend in all
three workloads. Retained allocation does not close the gap.

The phase-isolated measurements change how that result should be interpreted:

| 10,000-row phase | Time | Effective input throughput |
|---|---:|---:|
| Scalar element indexing | 1.261 ms | 1.64 GiB/s |
| Consume tape for `a` | 0.047 ms | 44.34 GiB/s |
| Consume tape for `a.promoted[href]` | 0.348 ms | 5.94 GiB/s |
| Consume tape for `[data-index]` | 0.969 ms | 2.13 GiB/s |

For selective queries, the scalar indexing pass is still the dominant tape
cost. That observation motivates a vectorised indexer, but it no longer makes
the scalar tape competitive with the streaming baseline.

## Auto-SIMD follow-up

Two portable approaches were added without architecture-specific intrinsics:

1. Repeated `memchr` searches, which automatically select NEON on AArch64.
2. A branch-free dense classifier that LLVM can auto-vectorise across 64 input
   bytes per loop, followed by a scalar state machine over the classification
   bytes.

The repeated-search version reproduced the short-span failure mode: indexing
rose from 1.261 ms to 1.867 ms. Each individual search was SIMD accelerated,
but HTML supplied another quote or tag boundary too quickly to amortise search
setup.

The dense classifier itself reached 25.9 GiB/s. Inspection of the release
assembly confirmed 128-bit NEON loads, `cmeq` byte comparisons, vector `orr`,
and 64-byte vector stores. Complete indexing reached 1.78 GiB/s because the
subsequent semantic state machine, not classification, became dominant.

At 10,000 rows, measured in the same run as the table above:

| Selector | Scalar lazy tape | Dense auto-SIMD lazy tape | Streaming lazy |
|---|---:|---:|---:|
| `a` | 1.347 ms | 1.193 ms | 1.184 ms |
| `a.promoted[href]` | 1.641 ms | 1.497 ms | 1.180 ms |
| `[data-index]` | 2.285 ms | 2.144 ms | 1.142 ms |

This automatic-SIMD test beats the scalar tape, but not the true streaming
control. The one-byte-per-input-byte scratch buffer is deliberately diagnostic.
A packed-mask implementation could avoid its second byte-by-byte pass, but it
would still need to recover the remaining tape-write and consume costs.

## Conclusion

The experiment strongly supports deferred attribute tokenisation. It also shows
that a scalar element tape cannot beat an equivalent single-pass streaming
frontend because writing and rereading the records adds overhead.

It does not rule out a SIMD element indexer. On the 10,000-row tag-only case,
consuming the completed tape takes only 47 microseconds. Any production design
still has to beat the full streaming path, not the old span-based control. The
new `production_query_scaling` group also measures 1, 4, 16, and 64 active
queries against attribute-dense and attribute-sparse documents so parser-side
query traversal costs are visible before integration decisions are made.

Run with:

```sh
cargo bench -p scah-benches --bench speed_bench_element_tape -- --noplot
```

## Scope

The experimental frontend handles normal tags, quoted values, comments, and
declarations needed by the generated workload. It is not a replacement HTML
parser and intentionally does not implement raw-text elements or Scah's error
recovery rules. Those costs remain represented only by `production_parse`.
