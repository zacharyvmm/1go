# scah (scan HTML)
> CSS selectors meet streaming XML/HTML parsing. Filter StAX events and build targeted DOMs without loading the entire document.

[![Crates.io](https://img.shields.io/crates/v/scah)](https://crates.io/crates/scah)
[![npm](https://img.shields.io/npm/v/%40zacharymm%2Fscah)](https://www.npmjs.com/package/@zacharymm/scah)
[![PyPI](https://img.shields.io/pypi/v/scah)](https://pypi.org/project/scah/)

## What is scah?

**scah** is a high-performance parsing library that bridges the gap between SAX/StAX streaming efficiency and DOM convenience. Instead of loading an entire document into memory or manually tracking parser state, you declare what you want with **CSS selectors**; the library handles the streaming complexity and builds a targeted DOM containing only your selections.

- **Streaming core**: Built on StAX; constant memory regardless of document size
- **Familiar API**: CSS selectors (including combinators like `>`, ` `, `+` (coming soon), `~` (coming soon))
- **Multi-language**: Rust core with Python and TypeScript/JavaScript bindings
- **Composable queries**: Chain selections and nest them with closures for **structured querying**; not only more efficient than flat filtering, but a fundamentally better pattern for extracting hierarchical data relationships

## Quick Start

### Rust

```toml
# Cargo.toml
[dependencies]
scah = "0.0.21"
```

#### Basic usage
```rust
use scah::{Query, Save, parse};

let html = r#"<ul><li><a href="/one">One</a></li><li><a href="/two">Two</a></li></ul>"#;

let queries = &[
    Query::all("a[href]", Save::all())
        .expect("valid selector")
        .build()
];
let store = parse(html, queries).expect("parse succeeds");

for a in store.get("a[href]").unwrap() {
    let href = a.attribute(&store, "href").unwrap();
    let text = a.text(&store).unwrap_or_default();
    println!("{text}: {href}");
}
// Output:
//   One: /one
//   Two: /two
```

#### Structured querying with `.then()`

Instead of flat filtering, nest queries with closures. Child queries only run within the context of their parent match:

```rust
use scah::{Query, Save, parse};

let query = Query::all("main > section", Save::all())
    .expect("valid selector")
    .then(|section| {
        Ok([
            section.all("> a[href]", Save::all())?,
            section.all("div a", Save::all())?,
        ])
    })
    .expect("valid child selectors")
    .build();

let queries = [query];
let store = parse(html, &queries).expect("parse succeeds");

// Access nested results through parent elements
for section in store.get("main > section").unwrap() {
    println!("Section: {}", section.inner_html.unwrap_or(""));

    if let Some(links) = section.get(&store, "> a[href]") {
        for link in links {
            println!("\tDirect link: {}", link.attribute(&store, "href").unwrap());
        }
    }
}
```

If selectors come from user input, `Query::all(...)` and `Query::first(...)` return `Result`, so malformed selectors surface as `SelectorParseError`. For fixed selectors in examples or tests, use `.expect(...)` explicitly if you want panic-on-invalid-selector behavior.

#### Compile-time queries with `query!`

For selectors that are known at compile time, prefer the `query!` macro. It validates the selector tree during compilation and emits a `StaticQuery` backed by inline arrays instead of heap-allocated query storage.

```rust
use scah::{Save, parse, query};

let html = r#"
    <article>
        <h1>Title</h1>
        <a href="/one">One</a>
        <a href="/two">Two</a>
    </article>
"#;

let query = query! {
    all("article", Save::none()) => {
        first("h1", Save::only_text()),
        all("a[href]", Save::all()),
    }
};
let queries = [query]; 
let store = parse(html, &queries).expect("parse succeeds");
let articles = store.get("article").unwrap();
assert_eq!(articles.len(), 1);
for article in articles {
    assert_eq!(article.get("a[href]").unwrap().count(), 2);
}
```

Use the runtime builder when you have dynamic sources. Use `query!` when the selector tree is authored in Rust code and should fail at compile time if it becomes invalid.

#### `Save` options

Control what data is captured per selector:

| Constructor | `inner_html` | `raw_text` | `text` | Use case |
|-------------|:---:|:---:|:---:|----------|
| `Save::all()` | Yes | Yes | Yes | Full extraction (captures both text modes; more work than the old two-field `Save::all()`) |
| `Save::only_inner_html()` | Yes | No | No | Raw markup only |
| `Save::only_raw_text()` | No | Yes | No | Source-preserving text |
| `Save::only_text()` | No | No | Yes | Normalized text scraping |
| `Save::none()` | No | No | No | Structure-only (attributes still saved) |

#### Supported CSS selector syntax

| Syntax | Example | Status |
|--------|---------|--------|
| Tag name | `a`, `div` | Working |
| ID | `#my-id` | Working |
| Class | `.my-class` | Working |
| Descendant | `main section a` | Working |
| Child | `main > section` | Working |
| Attribute presence | `a[href]` | Working |
| Attribute exact | `a[href="url"]` | Working |
| Attribute prefix | `a[href^="https"]` | Working |
| Attribute suffix | `a[href$=".com"]` | Working |
| Attribute substring | `a[href*="example"]` | Working |
| Adjacent sibling | `h1 + p` | Working |
| General sibling | `h1 ~ p` | Working |

> Full API documentation: [docs.rs/scah](https://docs.rs/scah)

#### Benchmarks
![Criterion Nested](https://raw.githubusercontent.com/zacharyvmm/scah/main/benches/images/nested_all_selection_10000.png)

![Criterion Simple](https://raw.githubusercontent.com/zacharyvmm/scah/main/benches/images/simple_all_selection_10000.png)

![Criterion WhatWg HTML Spec](https://raw.githubusercontent.com/zacharyvmm/scah/main/benches/images/whatwg_html_spec_all_links.png)

The repository includes two Rust benchmark tracks:

- Cross-library comparisons for simple `all` and `first` selectors.
- Runtime-builder vs `query!` macro comparisons to measure query-construction overhead separately from execution.

### Python
```bash
pip install -U scah
```
```python
from scah import Query, Save, parse 

query = Query.all("main > section", Save.all())
    .then(lambda section: [
        section.all("> a[href]", Save.all()),
        section.all("div a", Save.all()),
    ])
    .build()

store = parse(html, [query])
```

#### Benchmarks
##### Real Html BenchMark ([html.spec.whatwg.org](https://html.spec.whatwg.org/)) (select all `a` tags):
![WhatWg Html Spec BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-python/benches/images/whatwg.png)

##### Nested Html BenchMark (select all `Products`):
![Nested Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-python/benches/images/nested.png)

##### Structural Html BenchMark (select all `a` tags):
![Structural Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-python/benches/images/synthetic.png)

##### First Element Html BenchMark (select first `a`):
![First Element Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-python/benches/images/synthetic_first.png)

### Typescript / Javascript
```bash
npm install scah@npm:@zacharymm/scah
```

```ts
import { Query, parse } from 'scah';

const query = Query.all('main > section', { innerHtml: true, rawText: true, text: true })
  .then((p) => [
    p.all('> a[href]', { innerHtml: true, rawText: true, text: true }),
    p.all('div a', { innerHtml: true, rawText: true, text: true }),
  ])
  .build();

const store = parse(html, [query]);
```

#### Benchmarks
##### Real Html BenchMark ([html.spec.whatwg.org](https://html.spec.whatwg.org/)) (select all `a` tags):
![Real Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-node/benchmark/images/whatwg.png)

##### Nested Html BenchMark (select all `a` tags):
![Nested Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-node/benchmark/images/nested.png)

##### Synthetic Html BenchMark (select all `a` tags):
![Synthetic Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-node/benchmark/images/synthetic.png)

##### First Element Html BenchMark (select first `a`):
![First Element Html BenchMark](https://raw.githubusercontent.com/zacharyvmm/scah/main/crates/bindings/scah-node/benchmark/images/synthetic_first.png)

## Text extraction

Scah exposes two text modes (plus inner HTML):

| Field | Meaning |
| ----- | ------- |
| `inner_html` / `innerHtml` | Raw markup between the element's tags |
| `raw_text` / `rawText` | Source-preserving descendant text. Whitespace and entity spellings are retained; markup itself is omitted. Includes content inside `script` / `style` / `template` / `hidden` subtrees. |
| `text` | Normalized, human-readable descendant text. Whitespace and structural boundaries are normalized, non-content elements are omitted, and HTML character references are decoded. This is **not** browser `innerText` and does not process CSS. |

Normalized `text` specifically:

- Omits `script`, `style`, `template`, and elements with a `hidden` attribute (no text and no structural separators from those subtrees).
- Inserts line breaks for blocks / `<br>` / `<hr>`, and tabs between table cells.
- Preserves preformatted whitespace in `pre` and `textarea` after HTML's initial-newline rule (only a newline immediately after the start tag is removed).
- Decodes HTML character references (named, numeric, legacy semicolon-less forms in data state, C1 remapping, and U+FFFD replacement for invalid scalars).

Because Scah does not evaluate CSS, "block" is a fixed structural HTML set:
`address`, `article`, `aside`, `blockquote`, `body`, `caption`, `colgroup`, `dd`, `details`,
`dialog`, `div`, `dl`, `dt`, `fieldset`, `figcaption`, `figure`, `footer`,
`form`, `h1` through `h6`, `header`, `hgroup`, `legend`, `li`, `main`, `menu`,
`nav`, `ol`, `p`, `pre`, `search`, `section`, `summary`, `table`, `tbody`,
`tfoot`, `thead`, and `ul`. Table rows and cells use newline and tab boundaries,
respectively. Other elements use inline concatenation semantics.

`None` / `null` means the query did not request that representation. An empty string means it was requested but the element produced no text.

### Node `Save` options

Node uses plain option objects (no runtime `Save` helpers):

```ts
{
  innerHtml?: boolean
  rawText?: boolean
  text?: boolean
  attributes?: boolean
  textContent?: boolean // legacy alias for text
}
```

Rust and Python retain `Save::only_raw_text()` / `Save.only_raw_text()` style constructors.

### Migration from `text_content`

| Before | After |
| ------ | ----- |
| `Save::only_text_content()` | `Save::only_text()` |
| `Save { text_content: true, ... }` | `Save { text: true, ... }` |
| `element.text_content(&store)` | `element.text(&store)` |
| Python `element.text_content` | Python `element.text` |
| Node `element.textContent` | Node `element.text` |
| No raw equivalent | `raw_text` / `rawText` |

Old `text_content` output was already a normalized join, so it maps conceptually to the new `text`, not `raw_text`.

`Save::all()` now captures both `raw_text` and normalized `text` (plus `inner_html`), which can perform more work than the old two-field `Save::all()`. Prefer `Save::only_text()` or `Save::only_raw_text()` when only one representation is needed.

`Store::with_capacity` reserves capacity for both text representations by default. Query-aware `parse` construction grows requested text buffers and range sidecars only after a query matches.
