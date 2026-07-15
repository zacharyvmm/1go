use std::fs::File;
use std::io::BufReader;
use std::io::prelude::*;

use scah::{Query, Save, parse};

#[test]
#[ignore = "Real files"]
fn test_all_anchor_tags_for_whatwg_html_spec() -> std::io::Result<()> {
    // 26th of december 2025 16:50
    let file = File::open("/home/zmm/Downloads/html.spec.whatwg.index.html")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader.read_to_string(&mut contents)?;

    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(&contents, queries).expect("parse succeeds");

    assert_eq!(store.get("a").unwrap().count(), 64580);

    //println!("{:#?}", map);
    Ok(())
}

#[test]
#[ignore = "Real files"]
fn test_all_anchor_tags_for_albert_einstein_wikipedia() -> std::io::Result<()> {
    // 26th of december 2025 16:50
    let file = File::open("/home/zmm/Downloads/Albert_Einstein.html")?;
    let mut buf_reader = BufReader::new(file);
    let mut contents = String::new();
    buf_reader.read_to_string(&mut contents)?;

    let queries = &[Query::all("a", Save::all()).unwrap().build()];
    let store = parse(&contents, queries).expect("parse succeeds");

    assert_eq!(store.get("a").unwrap().count(), 3848);
    //println!("{:#?}", map);

    Ok(())
}
