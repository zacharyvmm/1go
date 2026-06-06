set shell := ["bash", "-c"]

default:
    @just --list

build: build-rust build-node build-python
build-rust:
    cargo build --release
build-node:
    cd crates/bindings/scah-node && bun run build
build-python:
    cd crates/bindings/scah-python && cargo run --bin stub_gen && uvx maturin build --release

dev: dev-rust dev-node dev-python
dev-rust:
    cargo build
dev-node:
    cd crates/bindings/scah-node && bun run build:debug
dev-python:
    cd crates/bindings/scah-python && cargo run --bin stub_gen && uvx maturin build

test: test-rust test-node test-python
test-rust:
    cargo test --all-targets --all-features
test-node:
    cd crates/bindings/scah-node && bun test
test-python:
    source ./crates/bindings/scah-python/.venv/bin/activate && uv run pytest ./crates/bindings/scah-python/tests/

format:
    cargo fmt --all
    cd crates/bindings/scah-node && bun run format

lint:
    cargo clippy --all-targets --all-features -- -D warnings
    cd crates/bindings/scah-node && bun run lint

bench: bench-rust bench-node bench-python
bench-simple-all: bench-rust-simple-all bench-node-simple-all bench-python-simple-all
bench-first: bench-rust-first bench-node-first bench-python-first
bench-whatwg: bench-rust-whatwg bench-node-whatwg bench-python-whatwg
bench-nested: bench-rust-nested bench-node-nested bench-python-nested
bench-rust: bench-rust-simple-all bench-rust-whatwg bench-rust-nested
bench-rust-simple-all:
    cargo bench -p scah-benches --bench speed_bench_simple_all
bench-rust-first:
    cargo bench -p scah-benches --bench speed_bench_simple_first
bench-rust-whatwg:
    cargo bench -p scah-benches --bench speed_bench_spec_all_links
bench-rust-nested:
    cargo bench -p scah-benches --bench speed_bench_nested_queries
bench-node: bench-node-simple-all bench-node-whatwg bench-node-nested
bench-node-simple-all:
    cd crates/bindings/scah-node && bun run bench:image:simple
bench-node-first:
    cd crates/bindings/scah-node && bun run bench:image:first
bench-node-whatwg:
    cd crates/bindings/scah-node && bun run bench:image:whatwg
bench-node-nested:
    cd crates/bindings/scah-node && bun run bench:image:nested
bench-python: bench-python-simple-all bench-python-whatwg bench-python-nested
bench-python-simple-all:
    cd crates/bindings/scah-python && source .venv/bin/activate && uv run --all-extras pytest benches/test_synthetic.py --benchmark-columns=min,mean,max --benchmark-sort=mean --benchmark-warmup-iterations 5 --benchmark-json benches/synthetic.json && python3 ./benches/utils/figure.py ./benches/synthetic.json -o ./benches/images/synthetic.png && rm ./benches/synthetic.json
bench-python-first:
    cd crates/bindings/scah-python && source .venv/bin/activate && uv run --all-extras pytest benches/test_synthetic_first.py --benchmark-columns=min,mean,max --benchmark-sort=mean --benchmark-warmup-iterations 5 --benchmark-json benches/synthetic_first.json && python3 ./benches/utils/figure.py ./benches/synthetic_first.json -o ./benches/images/synthetic_first.png && rm ./benches/synthetic_first.json
bench-python-whatwg:
    cd crates/bindings/scah-python && source .venv/bin/activate && uv run --all-extras pytest benches/test_spec.py --benchmark-columns=min,mean,max --benchmark-sort=mean --benchmark-warmup-iterations 5 --benchmark-json benches/whatwg.json && python3 ./benches/utils/figure.py ./benches/whatwg.json -o ./benches/images/whatwg.png && rm ./benches/whatwg.json
bench-python-nested:
    cd crates/bindings/scah-python && source .venv/bin/activate && uv run --all-extras pytest benches/test_structural.py --benchmark-columns=min,mean,max --benchmark-sort=mean --benchmark-warmup-iterations 5 --benchmark-json benches/nested.json && python3 ./benches/utils/figure.py ./benches/nested.json -o ./benches/images/nested.png && rm ./benches/nested.json
generate-graph-data:
    cargo criterion -p scah-benches --message-format=json >> criterion.json
generate-graphs:
    source ./crates/bindings/scah-python/.venv/bin/activate && python3 ./crates/bindings/scah-python/benches/utils/criterion_figure.py ./criterion.json
download-html-spec-bench:
    mkdir -p benches/bench_data
    curl -L "https://html.spec.whatwg.org/" -o benches/bench_data/html.spec.whatwg.org.html

bump new_version:
    just bump-rust "{{new_version}}"
    just bump-node "{{new_version}}"
    just bump-python "{{new_version}}"
    cargo check
bump-rust new_version:
    sed -i.bak 's/^version = "[^"]*"/version = "{{new_version}}"/' crates/scah-reader/Cargo.toml
    sed -i.bak 's/^version = "[^"]*"/version = "{{new_version}}"/' crates/scah-query-ir/Cargo.toml
    sed -i.bak 's/^version = "[^"]*"/version = "{{new_version}}"/' crates/scah-macros/Cargo.toml
    sed -i.bak 's/^version = "[^"]*"/version = "{{new_version}}"/' crates/scah/Cargo.toml
    sed -E -i.bak 's/^(scah-reader = \{ version = )"[^"]*"/\1"{{new_version}}"/' crates/scah-query-ir/Cargo.toml
    sed -E -i.bak 's/^(scah-query-ir = \{ version = )"[^"]*"/\1"{{new_version}}"/' crates/scah-macros/Cargo.toml
    sed -E -i.bak 's/^(scah-reader = \{ version = )"[^"]*"/\1"{{new_version}}"/' crates/scah/Cargo.toml
    sed -E -i.bak 's/^(scah-query-ir = \{ version = )"[^"]*"/\1"{{new_version}}"/' crates/scah/Cargo.toml
    sed -E -i.bak 's/^(scah-macros = \{ version = )"[^"]*"/\1"{{new_version}}"/' crates/scah/Cargo.toml
    sed -i.bak 's/^version = "[^"]*"/version = "{{new_version}}"/' benches/Cargo.toml
    sed -i.bak 's/^scah = "[^"]*"/scah = "{{new_version}}"/' README.md
    rm -f crates/scah-reader/Cargo.toml.bak crates/scah-query-ir/Cargo.toml.bak crates/scah-macros/Cargo.toml.bak crates/scah/Cargo.toml.bak benches/Cargo.toml.bak README.md.bak
bump-node new_version:
    sed -i.bak 's/^version = "[^"]*"/version = "{{new_version}}"/' crates/bindings/scah-node/Cargo.toml
    sed -i.bak 's/^  "version": "[^"]*",/  "version": "{{new_version}}",/' crates/bindings/scah-node/package.json
    sed -E -i.bak '/^  "optionalDependencies": \{/,/^  \}/ s/^    ("@zacharymm\/scah-[^"]+": )"[^"]+"(,?)$/    \1"{{new_version}}"\2/' crates/bindings/scah-node/package.json
    rm -f crates/bindings/scah-node/Cargo.toml.bak crates/bindings/scah-node/package.json.bak
bump-python new_version:
    sed -i.bak 's/^version = "[^"]*"/version = "{{new_version}}"/' crates/bindings/scah-python/Cargo.toml
    sed -i.bak 's/^version = "[^"]*"/version = "{{new_version}}"/' crates/bindings/scah-python/pyproject.toml
    rm -f crates/bindings/scah-python/Cargo.toml.bak crates/bindings/scah-python/pyproject.toml.bak
trigger-release new_version:
    git tag -a v{{new_version}} -m "Version {{new_version}} release"
    git push origin v{{new_version}}

code-cov:
    # cargo llvm-cov --html
    cargo llvm-cov
