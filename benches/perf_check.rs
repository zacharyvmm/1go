use scah::{Query, Save, parse};
use std::time::Instant;

fn main() {
    // Medium-sized HTML page
    let mut html = String::from("<html><head><title>T</title></head><body>\n");
    for i in 0..1000 {
        html.push_str(&format!(
            "<div class=\"item\" id=\"d{i}\"><a href=\"/p{i}\" class=\"link\">Item {i}</a><p>Desc</p></div>\n"
        ));
    }
    html.push_str("</body></html>");
    let html: &str = &html;

    let q = &[Query::all("div.item > a.link", Save::all()).unwrap().build()];

    // Warmup
    for _ in 0..50 { let _ = parse(html, q); }

    let start = Instant::now();
    let n = 300;
    for _ in 0..n {
        let _ = parse(html, q);
    }
    let elapsed = start.elapsed();
    println!("{n} iterations: {elapsed:?}");
    println!("per parse: {:?}", elapsed / n);
}
