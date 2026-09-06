# Proposed experiment: specialize text capture modes

Status: proposed, not implemented. This requires the user's approval because it changes the parser's internal dispatch and specialization structure.

## Problem

The parser currently specializes capture as enabled or disabled. When capture is enabled, raw-only, normalized-only, and mixed requests share the same compiled parsing path. Normalized-only requests consequently retain raw-capture state checks and bookkeeping around opening tags, text fragments, and closing tags.

At PR #68 revision f033b86, the dense-prose Callgrind profile attributed about 30% of instructions directly to the capture parser and another 8% to fragment flushing. Those figures identify possible work to remove; they do not predict the speedup from specialization. The profile also includes necessary parsing and normalization work.

## Proposed change

Extend the existing compile-time specialization to represent raw and normalized capture independently. Dispatch from the query requirements into four paths:

- Neither representation: retain the existing no-text behavior.
- Raw only: compile out normalized-text classification, suppression, separators, and decoding.
- Normalized only: compile out raw-source tracking, raw capture counts, and raw tape writes.
- Both: retain the complete behavior for mixed requests and callers saving both representations.

Keep one shared parser implementation with compile-time flags rather than manually maintaining four copies. Apply those flags through fragment flushing, source-position tracking, capture counting, and close finalization. Runtime checks still handle active selections, nesting, suppression, and query retirement where needed.

The public API, output strings, shared text tapes, per-element ranges, and supported malformed-HTML behavior remain unchanged. This proposal does not include a new tokenizer, lazy extraction, a new storage architecture, or a fused entity decoder.

## Tradeoffs

More compiled variants can increase executable size and compilation time. Larger code can also harm instruction-cache behavior, so a gain in normalized prose could accompany a loss elsewhere. The change reaches parser dispatch, close handling, and early-exit paths; it needs correctness checks across sibling queries, query retirement, raw-text elements, and mixed save modes.

## How to evaluate it

Use f033b86 as the baseline. The subsequent sentinel and scratch-borrowing trials were not retained because their small prose gains did not produce a dependable pass across all existing limits. Measure dense prose in repeated, alternating native x86 runs, then run all eight existing performance limits without changing their budgets. Check representative raw-only, mixed, hidden, entity-heavy, and preformatted workloads. Record executable code size alongside timing results.

Run the release correctness suite with and without instrumentation, lint checks, benchmark smoke checks, and allocation checks. Keep the specialization only if the repeatable performance benefit justifies its code-size and maintenance costs. Keep unsuccessful experiments out of PR #68.
