# scah (scan HTML)
> CSS selectors meet streaming XML/HTML parsing. Filter StAX events and build targeted DOMs without loading the entire document.

[![Crates.io](https://img.shields.io/crates/v/scah)](https://crates.io/crates/scah)
[![npm](https://img.shields.io/npm/v/%40zacharymm%2Fscah)](https://www.npmjs.com/package/@zacharymm/scah)
[![PyPI](https://img.shields.io/pypi/v/scah)](https://pypi.org/project/scah/)

## What is scah?

**scah** is a high-performance parsing library that bridges the gap between SAX/StAX streaming efficiency and DOM convenience. Instead of loading an entire document into memory or manually tracking parser state, you declare what you want with **CSS selectors**; the library handles the streaming complexity and builds a targeted DOM containing only your selections.

- **Streaming core**: Built on StAX; constant memory regardless of document size
- **Familiar API**: CSS selectors (including combinators like `>`, ` `, `+` (coming soon), `~` (coming soon))
- **Multi-language**: Rust API, a stable C ABI (`scah-ffi`), and shallow Python / TypeScript wrappers over that ABI
- **Composable queries**: Chain selections and nest them with closures for **structured querying**; not only more efficient than flat filtering, but a fundamentally better pattern for extracting hierarchical data relationships

## Architecture

```text
Rust API
  Native borrowed QueryBuilder and zero-copy Store

C ABI implementation
  Owned selectors, stores, result-list owners, and borrowed element IDs

Python / Node / future bindings
  Shallow wrappers over exported C ABI
```

The C ABI (`crates/scah-ffi`, header `include/scah.h`) owns selector tapes, HTML, stores, and element lists. Result elements are borrowed store-local IDs exposed through a list owner (no per-result C heap allocation). Every returned handle has a matching null-safe free function. Building a query does not consume the builder; appending does not consume the child. Section IDs are append tokens, not permanent random-access mutation handles. Returned string views are borrowed from the owning handle and remain valid only while that handle is alive. Rust panics never cross the ABI. Regenerate the committed header with `cargo run -p scah-ffi --features header-gen --bin generate_header`.

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
    let text = a.text_content(&store).unwrap_or_default();
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
        first("h1", Save::only_text_content()),
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

| Constructor | `inner_html` | `text_content` | Use case |
|-------------|:---:|:---:|----------|
| `Save::all()` | Yes | Yes | Full extraction |
| `Save::only_inner_html()` | Yes | No | Raw markup only |
| `Save::only_text_content()` | No | Yes | Lightweight text scraping |
| `Save::none()` | No | No | Structure-only (attributes still saved) |

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
| Adjacent sibling | `h1 + p` | Coming soon |
| General sibling | `h1 ~ p` | Coming soon |

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

const query = Query.all('main > section', { innerHtml: true, textContent: true })
  .then((p) => [
    p.all('> a[href]', { innerHtml: true, textContent: true }),
    p.all('div a', { innerHtml: true, textContent: true }),
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
