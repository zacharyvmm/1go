use scah::{Query, Save, parse};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};

struct TrackingAllocator;

static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

fn record_allocation(bytes: usize) {
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_BYTES.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(current) => peak = current,
        }
    }
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_allocation(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_allocation(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                record_allocation(new_size - layout.size());
            } else {
                LIVE_BYTES.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn prose_html(paragraphs: usize) -> String {
    let mut html = String::from("<article>");
    for i in 0..paragraphs {
        html.push_str(&format!(
            "<p data-unused='{i}'>paragraph {i} with &amp; normalized text</p>"
        ));
    }
    html.push_str("</article>");
    html
}

fn sparse_matches_html(paragraphs: usize) -> String {
    let mut html = String::from("<article>");
    for i in 0..paragraphs {
        if i % 100 == 0 {
            html.push_str(&format!("<p class='hit'>selected {i}</p>"));
        } else {
            html.push_str(&format!("<p>unmatched paragraph {i}</p>"));
        }
    }
    html.push_str("</article>");
    html
}

fn measure_peak<F, T>(operation: F) -> usize
where
    F: FnOnce() -> T,
{
    let baseline = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(baseline, Ordering::Relaxed);
    let value = operation();
    black_box(&value);
    let peak = PEAK_BYTES.load(Ordering::Relaxed).saturating_sub(baseline);
    drop(value);
    peak
}

fn no_match_peak(size: usize) -> usize {
    let html = prose_html(size);
    let queries = [Query::all(
        "article > span.missing",
        Save::only_text().without_attributes(),
    )
    .unwrap()
    .build()];
    measure_peak(|| parse(black_box(&html), black_box(&queries)).unwrap())
}

fn no_content_no_match_peak(size: usize) -> usize {
    let html = prose_html(size);
    let queries = [Query::all("article > span.missing", Save::name_only())
        .unwrap()
        .build()];
    measure_peak(|| parse(black_box(&html), black_box(&queries)).unwrap())
}

fn sparse_text_peak(size: usize) -> usize {
    let html = sparse_matches_html(size);
    let queries = [Query::all("p.hit", Save::only_text().without_attributes())
        .unwrap()
        .build()];
    measure_peak(|| parse(black_box(&html), black_box(&queries)).unwrap())
}

fn both_modes_peak(size: usize) -> usize {
    let html = prose_html(size);
    let queries = [Query::all("p", Save::all().without_attributes())
        .unwrap()
        .build()];
    measure_peak(|| parse(black_box(&html), black_box(&queries)).unwrap())
}

fn main() {
    for size in [1_000, 10_000] {
        let no_content = no_content_no_match_peak(size);
        let no_match = no_match_peak(size);
        println!("no_content_no_matches/{size}: {no_content} bytes");
        println!(
            "text_only_no_matches/{size}: {no_match} bytes ({:+} text bytes)",
            no_match as isize - no_content as isize
        );
        let allowed_overhead = no_content / 20 + 4_096;
        assert!(
            no_match <= no_content + allowed_overhead,
            "unmatched text query added {} bytes; limit is {allowed_overhead}",
            no_match.saturating_sub(no_content)
        );
        println!(
            "text_only_sparse_matches/{size}: {} bytes",
            sparse_text_peak(size)
        );
        println!("both_modes/{size}: {} bytes", both_modes_peak(size));
    }
}
