//! Report peak resident cursor slots and active obligations as integer counts.
//!
//! This is inspection / PR-documentation tooling. Structural bounds remain
//! enforced by `scah` unit tests under `bench-internals`.

use scah_cursor_benches::{DEPTHS, cursor_cases, measure_case};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut failed = false;

    println!("case,depth,peak_resident_cursor_slots,peak_active_obligations");

    let cases = cursor_cases();
    let mut descendant_slots_at_8 = None;
    let mut sequential_first_slots_at_8 = None;

    for case in cases {
        for &depth in DEPTHS {
            let stats = measure_case(case, depth);
            println!(
                "{},{},{},{}",
                case.name, depth, stats.peak_resident_cursor_slots, stats.peak_active_obligations
            );

            if let Some(max) = (case.max_resident)(depth)
                && stats.peak_resident_cursor_slots > max
            {
                eprintln!(
                    "error: {} depth={depth}: peak_resident_cursor_slots {} exceeds budget {max}",
                    case.name, stats.peak_resident_cursor_slots
                );
                failed = true;
            }

            if case.name == "descendant_div_p" {
                if depth == 8 {
                    descendant_slots_at_8 = Some(stats.peak_resident_cursor_slots);
                } else if depth == 512 {
                    let at_8 = descendant_slots_at_8.expect("depth 8 measured before 512");
                    if stats.peak_resident_cursor_slots != at_8 {
                        eprintln!(
                            "error: descendant_div_p peak resident slots grew with depth: 8={at_8}, 512={}",
                            stats.peak_resident_cursor_slots
                        );
                        failed = true;
                    }
                }
            }

            if case.name == "then_article_first_div_gt_p_sequential" {
                if depth == 8 {
                    sequential_first_slots_at_8 = Some(stats.peak_resident_cursor_slots);
                } else if depth == 512 {
                    let at_8 = sequential_first_slots_at_8.expect("depth 8 measured before 512");
                    if stats.peak_resident_cursor_slots != at_8 {
                        eprintln!(
                            "error: then_article_first_div_gt_p_sequential peak resident slots grew with depth: 8={at_8}, 512={}",
                            stats.peak_resident_cursor_slots
                        );
                        failed = true;
                    }
                }
            }
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
