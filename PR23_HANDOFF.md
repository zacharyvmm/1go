# Handoff: follow-up work from closed PR #23

## Current state

- PR #23 (`global-maxima`) was reviewed and closed. Do not reopen or merge it.
- PR #24 (`core-optimizations`) is the replacement extraction. It contains only the validated pre-SIMD commits:
  - allocation and text-capture reductions;
  - `memchr`-based forward scanning;
  - implied-close buffer reuse.
- PR #24 passed:
  - `cargo test --workspace`
  - `cargo fmt --all -- --check`
  - `cargo clippy -p scah --all-targets -- -D warnings`

The remaining work from #23 should be implemented in small, independent PRs based on `main` after #24 lands. Do **not** cherry-pick the later #23 commits: they mix useful fixes with broken parser changes and abandoned SIMD/tape experiments.

## Review addendum: current agent implementation

The current implementation of this handoff plan is **not ready to merge** until it fixes these two correctness blockers:

1. Do not strip a trailing `/` from every unquoted token before `>`. In HTML,
   `/` is valid inside an unquoted attribute value. For example,
   `<div data=/foo/>after</div>` must preserve `data="/foo/"`; only a solidus
   recognized in the parser's self-closing-tag state may be discarded.
2. Raw-text closing tags must allow HTML whitespace before `>`. For example,
   `<style>x</style ><a href="ok">ok</a>` must exit raw-text mode and match
   the `<a>`. An exact string check for only `</style>` swallows the remainder
   of the document in this case.

The implementation review ran the new migration tests in both debug and
release, plus formatting and Clippy. All passed, but they did not cover either
case above. Add these tests before considering the work complete.

## Priority 1: selector correctness

Create one focused PR for the query-engine fixes.

Desired behavior:

- A flat descendant selector returns a physical element once, even if it has multiple matching ancestors. For example, `div a` should return one nested `<a>`, not one result per ancestor path.
- A nested `.then()` query remains scoped by parent; the same physical child may appear once under each distinct selected parent.
- `first()` returns the first match for each parent scope and then completes correctly.
- Attribute selectors work for the dedicated `id` and `class` fields: `[id]`, `[id="x"]`, `[class]`, and `[class~="foo"]`.
- Attribute match operators require `=`. Examples such as `[class~]`, `[id^]`, `[href$]`, and `[href*]` must return a selector parse error.

Likely areas:

- `crates/scah/src/engine/executor.rs`
- `crates/scah-query-ir/src/query/selector/eq.rs`
- `crates/scah-query-ir/src/query/selector/builder.rs`
- selector and integration tests

Implementation notes:

- The #23 save-time dedup idea is useful, but preserve the distinction between a flat query and separate `.then()` parent scopes.
- Attribute names should be handled case-insensitively in HTML. If `id`/`class` are routed through specialized fields, use case-insensitive routing too.

## Priority 2: parser edge-case correctness

Create a separate parser-correctness PR.

Desired behavior:

- Treat form feed (`U+000C`) as HTML whitespace in tags and attributes.
- Do not terminate an HTML comment at a `>` inside `<!-- ... -->`; handle malformed/abrupt comments without swallowing the remainder of the document or panicking.
- Empty, whitespace-only, comment-only, and nested-empty elements must not panic when text content is requested.
- Treat `script`, `style`, `textarea`, and `title` content as raw text for element discovery. Matching must be ASCII-case-insensitive, including `<SCRIPT>`.
- Recognize valid raw-text end tags with optional HTML whitespace before `>`
  (for example, `</style >`), rather than only an exact `</style>` literal.

Likely areas:

- `crates/scah/src/html/element/builder.rs`
- `crates/scah/src/html/parser.rs`
- `crates/scah/src/store/text_content.rs`
- parser and HTML-soup tests

Do not copy #23's raw-text helper unchanged: it checks only lowercase first letters and regresses uppercase `<SCRIPT>`.

## Priority 3: void and trailing-solidus handling

Implement this as its own PR because it is easy to regress parser cursor positions.

Required semantics:

- HTML void elements (`br`, `img`, `input`, etc.) are self-closing with or without a trailing `/`.
- A trailing `/` is not exposed as an attribute.
- A `/` that is part of an unquoted attribute value is preserved. For example,
  `<div data=/foo/>after</div>` has `data="/foo/"`; its final slash is not a
  self-closing marker.
- A non-void element such as `<div />after</div>` is **not** self-closing in HTML mode and must save `after` as its inner HTML/text content.
- The tokenizer must consume the complete opening tag, including the trailing solidus and `>`, before content capture begins.

Required regression tests:

```html
<div />after</div>
<hr />
<input disabled />
<div data=/foo/>after</div>
<style>x</style ><a href="ok">ok</a>
```

Run these through both debug and release parsing paths. In #23, debug saved `"/>after"` for the first case, while its release-only fast path saved no content at all.

If this parser claims HTML case-insensitive tag handling, also add coverage for
uppercase void elements such as `<BR>`; the current void-element matcher is
lowercase-only.

## Optional: benchmarks

The class and ID benchmark cases from #23 can be ported later if benchmark maintenance is desired. Keep this separate from runtime behavior changes. Do not bring along unrelated competitor upgrades, git dependency changes, generated binding files, or lockfile churn merely to add benchmarks.

## Explicit exclusions

Do not port these parts of #23:

- `crates/scah-reader/src/simd.rs` and the SIMD scanner architecture;
- the tape, lazy-parser, and parallel-parser experiments;
- the release-only `simple_tag_parser` fast path;
- broad parser rewrites bundled with SIMD work;
- Node/Python generated artifacts and unrelated lockfile updates.

The public `parse()` path did not enable the proposed SIMD reader, so that code added complexity and warnings without a demonstrated default-path benefit. The full #23 branch also failed formatting and Clippy with warnings denied.

## Suggested delivery sequence

1. Land PR #24.
2. Land the selector-correctness PR with focused tests.
3. Land raw-text/comments/empty-content correctness.
4. Land void/trailing-solidus handling with debug-and-release parity tests.
5. Optionally add benchmark coverage after behavior is stable.

For every follow-up PR, run:

```bash
cargo test --workspace
cargo fmt --all -- --check
cargo clippy -p scah --all-targets -- -D warnings
```
