//! Capacity profiling harness — run under valgrind massif to measure
//! allocation peaks for different HTML workloads.
//!
//! Usage:
//!   cargo build --release --example capacity_profile
//!   valgrind --tool=massif --massif-out-file=massif.out \
//!     ./target/release/examples/capacity_profile <mode>
//!   ms_print massif.out

use scah::{Query, Save, parse};

macro_rules! run_mode {
    ($sizes:expr, $label:expr, $html_fn:expr, $sel:expr, $save:expr) => {
        for &size in $sizes.iter() {
            let html = $html_fn(size);
            let query = $sel($save).unwrap().build();
            let queries = [query];
            let store = parse(&html, &queries);
            report($label, size, &store);
        }
    };
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    let sizes = [1_000, 10_000, 100_000];

    match mode.as_str() {
        "text-heavy" => run_mode!(
            sizes,
            "text-heavy",
            text_heavy_html,
            |s| Query::all("p", s),
            Save::only_text_content()
        ),
        "tag-heavy" => run_mode!(
            sizes,
            "tag-heavy",
            tag_heavy_html,
            |s| Query::all("div.card", s),
            Save::all()
        ),
        "attribute-heavy" => run_mode!(
            sizes,
            "attribute-heavy",
            attribute_heavy_html,
            |s| Query::all("a[href]", s),
            Save::all()
        ),
        "wildcard" => run_mode!(
            sizes,
            "wildcard",
            tag_heavy_html,
            |s| Query::all("*", s),
            Save::none()
        ),
        "first-match" => run_mode!(
            sizes,
            "first-match",
            tag_heavy_html,
            |s| Query::first("div.card", s),
            Save::all()
        ),
        "save-none" => run_mode!(
            sizes,
            "save-none",
            tag_heavy_html,
            |s| Query::all("div.card", s),
            Save::none()
        ),
        _ => {
            eprintln!("modes: text-heavy tag-heavy attribute-heavy wildcard first-match save-none")
        }
    }
}

fn report(label: &str, size: usize, store: &scah::Store) {
    let total_elements: usize = store.elements.iter().count();
    let total_attrs: usize = store.attributes.iter().count();
    let elem_cap = store.elements.capacity();
    let attr_cap = store.attributes.capacity();
    let er = if total_elements > 0 {
        size as f64 / total_elements as f64
    } else {
        f64::NAN
    };
    let ar = if total_attrs > 0 {
        size as f64 / total_attrs as f64
    } else {
        f64::NAN
    };
    eprintln!(
        "{label:>16} | html={size:>8} | elems={total_elements:>6} ecap={elem_cap:>6} \
         r={er:>5.1} | attrs={total_attrs:>6} acap={attr_cap:>6} r={ar:>5.1}",
    );
    std::hint::black_box(&store.elements);
    std::hint::black_box(&store.attributes);
}

fn text_heavy_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 120);
    html.push_str("<html><body>");
    for i in 0..count {
        html.push_str(&format!(
            "<p>Paragraph {i} with lots of text content to fill up the text arena. \
             Lorem ipsum dolor sit amet consectetur adipiscing elit.</p>"
        ));
    }
    html.push_str("</body></html>");
    html
}

fn tag_heavy_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 80);
    html.push_str("<html><body><main>");
    for i in 0..count {
        html.push_str(&format!(
            "<div class='card' id='card{i}'><span class='title'>Card {i}</span>\
             <span class='body'>Content</span></div>"
        ));
    }
    html.push_str("</main></body></html>");
    html
}

fn attribute_heavy_html(count: usize) -> String {
    let mut html = String::with_capacity(count * 200);
    html.push_str("<html><body><nav>");
    for i in 0..count {
        html.push_str(&format!(
            "<a href='/page/{i}' data-id='{i}' aria-label='Link {i}' \
             class='nav-link' target='_blank' rel='noopener' \
             title='Go to page {i}'>Link {i}</a>"
        ));
    }
    html.push_str("</nav></body></html>");
    html
}
