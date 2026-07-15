//! Benchmark: forward-parity vs delimiter-first for `next_until_unescaped`.
//!
//! Compares two algorithms for finding the next unescaped delimiter in a byte
//! stream, as used by HTML attribute tokenization and CSS selector parsing.
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box as bb;

// ── Algorithm implementations ───────────────────────────────────────

/// Forward-parity: scans byte-by-byte, tracking escape-run parity.
/// This matches the current `Reader::next_until_unescaped` implementation.
#[inline(never)]
fn forward_parity(bytes: &[u8], delimiter: u8, escape: u8) -> usize {
    let mut esc_run = false;
    let len = bytes.len();
    let mut pos = 0;
    while pos < len {
        let byte = unsafe { *bytes.get_unchecked(pos) };
        if byte == delimiter && !esc_run {
            return pos;
        }
        if byte == escape {
            esc_run = !esc_run;
        } else {
            esc_run = false;
        }
        pos += 1;
    }
    len
}

/// Delimiter-first: find the next delimiter byte, then scan backward
/// through the immediately preceding escape run to decide whether it
/// is escaped. Only the escape-run scan touches the escape byte.
#[inline(never)]
fn delimiter_first(bytes: &[u8], delimiter: u8, escape: u8) -> usize {
    let len = bytes.len();
    let mut pos = 0;
    while pos < len {
        // Find next delimiter
        let slice = &bytes[pos..];
        let delim_pos = match slice.iter().position(|&b| b == delimiter) {
            None => return len,
            Some(offset) => pos + offset,
        };

        // Scan backward from just before the delimiter for an escape run
        let mut escape_count = 0u32;
        let mut scan = delim_pos;
        while scan > 0 && bytes[scan - 1] == escape {
            escape_count += 1;
            scan -= 1;
        }

        if escape_count % 2 == 0 {
            return delim_pos;
        }
        pos = delim_pos + 1;
    }
    len
}

// ── Correctness smoke tests ─────────────────────────────────────────

#[test]
fn algorithms_agree_on_correctness_cases() {
    let cases: &[(&str, u8)] = &[
        (r#"abc"def"#, b'"'),
        (r#"abc\"def"ghi"#, b'"'),
        (r#"abc\\"def"#, b'"'),
        (r#"abc\\\"def"ghi"#, b'"'),
        (r#"abc\"def"#, b'"'),
        (r#"\a""#, b'"'),
        ("data:more", b':'),
        ("hello", b'"'),
        (r#"\\"def"#, b'"'),
        (r#"\"def"ghi"#, b'"'),
        (r#"hello \"world\" end"#, b'"'),
        (r#"hello \'world\' end'"#, b'\''),
    ];

    for (input, delim) in cases {
        let bytes = input.as_bytes();
        let fp = forward_parity(bytes, *delim, b'\\');
        let df = delimiter_first(bytes, *delim, b'\\');
        assert_eq!(
            fp, df,
            "mismatch on {input:?} (delim={delim}): fp={fp}, df={df}"
        );
    }
}

// ── Input generators ─────────────────────────────────────────────────

fn ordinary_short() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("hello_8", b"hello\"xx".to_vec()),
        ("hello-world_16", b"hello-world\"xxyy".to_vec()),
        ("url_24", b"https://example.com\"xx".to_vec()),
        ("button-primary_24", b"button-primary active\"x".to_vec()),
    ]
}

fn ordinary_medium() -> Vec<(String, Vec<u8>)> {
    let sizes = [128, 256, 512];
    sizes
        .iter()
        .map(|&n| {
            let mut v = vec![b'a'; n];
            v.push(b'"');
            v.push(b'x');
            (format!("ordinary_{n}"), v)
        })
        .collect()
}

fn ordinary_long() -> Vec<(String, Vec<u8>)> {
    let sizes = [1024, 4096, 16384];
    sizes
        .iter()
        .map(|&n| {
            let mut v = vec![b'a'; n];
            v.push(b'"');
            v.push(b'x');
            (format!("ordinary_{n}"), v)
        })
        .collect()
}

fn one_escaped_quote() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("escaped_double", br#"hello \"world\" end"xx"#.to_vec()),
        ("escaped_single", br#"hello \'world\' end'xx"#.to_vec()),
    ]
}

fn repeated_escaped() -> Vec<(String, Vec<u8>)> {
    // value containing many escaped delimiters
    let mut v = Vec::new();
    for _ in 0..50 {
        v.extend_from_slice(b"data\\\"");
    }
    v.push(b'"');
    v.push(b'x');
    vec![("repeated_escaped_50".into(), v)]
}

fn even_escape_runs() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (r"two_escapes", br#"\\"def"#.to_vec()),
        (r"four_escapes", br#"\\\\"def"#.to_vec()),
    ]
}

fn odd_escape_runs() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (r"one_escape", br#"\"def"ghi"#.to_vec()),
        (r"three_escapes", br#"\\\"def"ghi"#.to_vec()),
    ]
}

fn unterminated() -> Vec<(String, Vec<u8>)> {
    let sizes = [1024, 4096, 16384];
    sizes
        .iter()
        .map(|&n| {
            let mut v = vec![b'a'; n];
            v.extend_from_slice(b"\\\""); // escaped quote at end, no unescaped
            (format!("unterminated_{n}"), v)
        })
        .collect()
}

fn mixed_html_attrs() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "href",
            br#"href="https://example.com/search?q=test"x"#.to_vec(),
        ),
        (
            "class",
            br#"class="button button-primary active"x"#.to_vec(),
        ),
        ("title", br#"title="hello \"world\""x"#.to_vec()),
        ("data_json", br#"data-json="{\"key\":\"value\"}"x"#.to_vec()),
    ]
}

// ── Benchmarks ──────────────────────────────────────────────────────

fn bench_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("escape_scanner");

    // Ordinary short values
    for (name, data) in ordinary_short() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    // Ordinary medium values
    for (name, data) in ordinary_medium() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    // Ordinary long values
    for (name, data) in ordinary_long() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    // One escaped quote
    for (name, data) in one_escaped_quote() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    // Repeated escaped
    for (name, data) in repeated_escaped() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    // Even escape runs
    for (name, data) in even_escape_runs() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    // Odd escape runs
    for (name, data) in odd_escape_runs() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    // Unterminated
    for (name, data) in unterminated() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    // Mixed HTML attributes
    for (name, data) in mixed_html_attrs() {
        group.throughput(Throughput::Bytes(data.len() as u64));
        group.bench_with_input(BenchmarkId::new("forward_parity", &name), &data, |b, d| {
            b.iter(|| forward_parity(bb(d), b'"', b'\\'))
        });
        group.bench_with_input(BenchmarkId::new("delimiter_first", &name), &data, |b, d| {
            b.iter(|| delimiter_first(bb(d), b'"', b'\\'))
        });
    }

    group.finish();
}

criterion_group!(benches, bench_comparison);
criterion_main!(benches);
