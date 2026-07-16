//! Benchmark: forward-parity vs delimiter-first for `next_until_unescaped`.
//!
//! Compares two algorithms for finding the next unescaped delimiter in a byte
//! stream, as used by HTML attribute tokenization and CSS selector parsing.
//!
//! Forward-parity candidate retained for comparison with the production
//! delimiter-first scanner.
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use scah::Reader;
use std::hint::black_box;

// ── Benchmark case representation ─────────────────────────────────────

struct ScanCase {
    name: String,
    data: Vec<u8>,
    delimiter: u8,
    /// Byte position where the scanner is expected to stop.
    /// Equal to `data.len()` when no unescaped delimiter is present.
    expected_position: usize,
}

// ── Algorithm implementations ──────────────────────────────────────────

/// Forward-parity: scans byte-by-byte, tracking escape-run parity.
/// Retained as a benchmark-only comparison candidate.
#[inline(never)]
fn forward_parity(bytes: &[u8], delimiter: u8, escape: u8) -> usize {
    let mut esc_run = false;
    let len = bytes.len();
    let mut pos = 0;
    while pos < len {
        let byte = bytes[pos];
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

/// Production delimiter-first scanner via `Reader::next_until_unescaped()`.
/// This measures the actual shipping implementation.
#[inline(never)]
fn production_scanner(data: &[u8], delimiter: u8, escape: u8) -> usize {
    let mut reader = Reader::from_bytes(data);
    reader.next_until_unescaped(delimiter, escape);
    reader.get_position()
}

/// Bytes actually examined by the scanner for a given case.
/// For delimiter-hit cases this includes the delimiter byte itself;
/// for unterminated cases this is the full input length.
#[inline]
fn bytes_scanned(case: &ScanCase) -> usize {
    if case.expected_position < case.data.len() {
        // The scanner reads through and including the delimiter.
        case.expected_position + 1
    } else {
        // No unescaped delimiter: the entire input is examined.
        case.data.len()
    }
}

// ── Correctness validation ─────────────────────────────────────────────

fn validate_cases(cases: &[ScanCase]) {
    // Pre-validate throughput helper with explicit cases.
    let stop_at_five = ScanCase {
        name: "throughput_stop".into(),
        data: b"hello\"xx".to_vec(),
        delimiter: b'"',
        expected_position: 5,
    };
    assert_eq!(bytes_scanned(&stop_at_five), 6);

    let eof = ScanCase {
        name: "throughput_eof".into(),
        data: b"hello".to_vec(),
        delimiter: b'"',
        expected_position: 5,
    };
    assert_eq!(bytes_scanned(&eof), 5);

    for case in cases {
        let candidate = forward_parity(&case.data, case.delimiter, b'\\');
        let production = production_scanner(&case.data, case.delimiter, b'\\');

        assert_eq!(
            candidate, case.expected_position,
            "forward_parity mismatch for {}: got {}, expected {}",
            case.name, candidate, case.expected_position,
        );
        assert_eq!(
            production, case.expected_position,
            "production_scanner mismatch for {}: got {}, expected {}",
            case.name, production, case.expected_position,
        );
        assert_eq!(
            candidate, production,
            "scanner mismatch for {}: fp={}, prod={}",
            case.name, candidate, production,
        );
    }
}

// ── Input generators ───────────────────────────────────────────────────

fn ordinary_short() -> Vec<ScanCase> {
    vec![
        ScanCase {
            name: "hello_8".into(),
            data: b"hello\"xx".to_vec(),
            delimiter: b'"',
            expected_position: 5,
        },
        ScanCase {
            name: "hello-world_16".into(),
            data: b"hello-world\"xxyy".to_vec(),
            delimiter: b'"',
            expected_position: 11,
        },
        ScanCase {
            name: "url_24".into(),
            data: b"https://example.com\"xx".to_vec(),
            delimiter: b'"',
            expected_position: 19,
        },
        ScanCase {
            name: "button-primary_24".into(),
            data: b"button-primary active\"x".to_vec(),
            delimiter: b'"',
            expected_position: 21,
        },
    ]
}

fn ordinary_medium() -> Vec<ScanCase> {
    let sizes = [128, 256, 512];
    sizes
        .iter()
        .map(|&n| {
            let mut v = vec![b'a'; n];
            let expected = v.len();
            v.push(b'"');
            v.push(b'x');
            ScanCase {
                name: format!("ordinary_{n}"),
                data: v,
                delimiter: b'"',
                expected_position: expected,
            }
        })
        .collect()
}

fn ordinary_long() -> Vec<ScanCase> {
    let sizes = [1024, 4096, 16384];
    sizes
        .iter()
        .map(|&n| {
            let mut v = vec![b'a'; n];
            let expected = v.len();
            v.push(b'"');
            v.push(b'x');
            ScanCase {
                name: format!("ordinary_{n}"),
                data: v,
                delimiter: b'"',
                expected_position: expected,
            }
        })
        .collect()
}

fn one_escaped_quote() -> Vec<ScanCase> {
    vec![
        ScanCase {
            name: "escaped_double".into(),
            // Each \" is escaped (odd run=1); the final " is unescaped.
            data: br#"hello \"world\" end"xx"#.to_vec(),
            delimiter: b'"',
            expected_position: 19,
        },
        ScanCase {
            name: "escaped_single".into(),
            // Each \' is escaped; the final ' is unescaped.
            data: br#"hello \'world\' end'xx"#.to_vec(),
            delimiter: b'\'',
            expected_position: 19,
        },
    ]
}

fn repeated_escaped() -> Vec<ScanCase> {
    // Value containing many escaped delimiters followed by one unescaped.
    let mut v = Vec::new();
    for _ in 0..50 {
        v.extend_from_slice(b"data\\\"");
    }
    let expected = v.len();
    v.push(b'"');
    v.push(b'x');
    vec![ScanCase {
        name: "repeated_escaped_50".into(),
        data: v,
        delimiter: b'"',
        expected_position: expected,
    }]
}

fn even_escape_runs() -> Vec<ScanCase> {
    vec![
        ScanCase {
            name: "two_escapes".into(),
            // \\"def  →  \\ is even, so " closes at pos 2.
            data: br#"\\"def"#.to_vec(),
            delimiter: b'"',
            expected_position: 2,
        },
        ScanCase {
            name: "four_escapes".into(),
            // \\\\"def  →  \\\\ is even, closes at pos 4.
            data: br#"\\\\"def"#.to_vec(),
            delimiter: b'"',
            expected_position: 4,
        },
    ]
}

fn odd_escape_runs() -> Vec<ScanCase> {
    vec![
        ScanCase {
            name: "one_escape".into(),
            // \"def"ghi  →  \" is odd (1), skips; unescaped " at pos 5.
            data: br#"\"def"ghi"#.to_vec(),
            delimiter: b'"',
            expected_position: 5,
        },
        ScanCase {
            name: "three_escapes".into(),
            // \\\"def"ghi  →  \\\ is odd (3), skips; unescaped " at pos 7.
            data: br#"\\\"def"ghi"#.to_vec(),
            delimiter: b'"',
            expected_position: 7,
        },
    ]
}

fn unterminated() -> Vec<ScanCase> {
    let sizes = [1024, 4096, 16384];
    sizes
        .iter()
        .map(|&n| {
            let mut v = vec![b'a'; n];
            // Escaped quote at end, no unescaped delimiter → runs to EOF.
            v.extend_from_slice(b"\\\"");
            let len = v.len();
            ScanCase {
                name: format!("unterminated_{n}"),
                data: v,
                delimiter: b'"',
                expected_position: len,
            }
        })
        .collect()
}

fn realistic_attribute_values() -> Vec<ScanCase> {
    // Value-only: the scanner starts inside the attribute value, after the
    // opening quote has been consumed. The sentinel byte after the closing
    // quote distinguishes stop-at-delimiter from run-to-EOF.
    vec![
        ScanCase {
            name: "href_url".into(),
            data: br#"https://example.com/search?q=test"x"#.to_vec(),
            delimiter: b'"',
            expected_position: 33,
        },
        ScanCase {
            name: "class_list".into(),
            data: br#"button button-primary active"x"#.to_vec(),
            delimiter: b'"',
            expected_position: 28,
        },
        ScanCase {
            name: "title_escaped".into(),
            data: br#"hello \"world\""x"#.to_vec(),
            delimiter: b'"',
            expected_position: 15,
        },
        ScanCase {
            name: "data_json".into(),
            data: br#"{\"key\":\"value\"}"x"#.to_vec(),
            delimiter: b'"',
            expected_position: 19,
        },
    ]
}

// ── Benchmarks ─────────────────────────────────────────────────────────

fn bench_comparison(c: &mut Criterion) {
    // Collect all cases and validate before benchmarking.
    let mut all_cases: Vec<ScanCase> = Vec::new();
    all_cases.extend(ordinary_short());
    all_cases.extend(ordinary_medium());
    all_cases.extend(ordinary_long());
    all_cases.extend(one_escaped_quote());
    all_cases.extend(repeated_escaped());
    all_cases.extend(even_escape_runs());
    all_cases.extend(odd_escape_runs());
    all_cases.extend(unterminated());
    all_cases.extend(realistic_attribute_values());

    validate_cases(&all_cases);

    let mut group = c.benchmark_group("escape_scanner");

    // Helper: benchmark one algorithm across all cases.
    fn bench_algo(
        group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
        algo_name: &str,
        cases: &[ScanCase],
        run: fn(&[u8], u8, u8) -> usize,
    ) {
        for case in cases {
            group.throughput(Throughput::Bytes(bytes_scanned(case) as u64));
            group.bench_with_input(BenchmarkId::new(algo_name, &case.name), case, |b, c| {
                b.iter(|| {
                    let pos = run(
                        black_box(c.data.as_slice()),
                        black_box(c.delimiter),
                        black_box(b'\\'),
                    );
                    black_box(pos)
                })
            });
        }
    }

    bench_algo(&mut group, "forward_parity", &all_cases, forward_parity);
    bench_algo(
        &mut group,
        "production_delimiter_first",
        &all_cases,
        production_scanner,
    );

    group.finish();
}

criterion_group!(benches, bench_comparison);
criterion_main!(benches);
