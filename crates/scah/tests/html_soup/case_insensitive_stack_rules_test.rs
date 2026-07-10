//! Regression tests for case-insensitive implied-close and scope-barrier rules.
//!
//! PR #24 review finding: the implied-close and scope-rule logic in
//! `OpenElementStack` was case-sensitive, so mixed-case HTML tags could match
//! selectors correctly but fail HTML stack rules (e.g. `<LI>` not implicitly
//! closing a previous `<LI>`).

use super::helpers::{parse_with_saves, texts};
use scah::Save;

/// A mixed-case `<LI>` must implicitly close the previous `<LI>`, producing
/// two distinct list items with separate text content.
#[test]
fn mixed_case_li_implicitly_closes_previous_li() {
    let html = "<UL><LI>A<LI>B</UL>";
    let store = parse_with_saves(html, &[("ul li", Save::only_text_content())]);
    assert_eq!(texts(&store, "ul li"), vec![Some("A"), Some("B")]);
}

/// A mixed-case block tag (`<DIV>`) must implicitly close an open `<P>`.
#[test]
fn mixed_case_block_tag_closes_open_p() {
    let html = "<P>intro<DIV>block</DIV>";
    let store = parse_with_saves(
        html,
        &[
            ("p", Save::only_text_content()),
            ("div", Save::only_text_content()),
        ],
    );
    assert_eq!(texts(&store, "p"), vec![Some("intro")]);
    assert_eq!(texts(&store, "div"), vec![Some("block")]);
}

/// Mixed-case table cells (`<TD>`) must implicitly close the previous cell.
#[test]
fn mixed_case_table_cells_implicitly_close_previous_cell() {
    let html = "<TABLE><TR><TD>A<TD>B</TR></TABLE>";
    let store = parse_with_saves(html, &[("td", Save::only_text_content())]);
    assert_eq!(texts(&store, "td"), vec![Some("A"), Some("B")]);
}

/// Mixed-case `<OPTION>` must implicitly close the previous `<OPTION>`.
#[test]
fn mixed_case_options_implicitly_close_previous_option() {
    let html = "<SELECT><OPTION>A<OPTION>B</SELECT>";
    let store = parse_with_saves(html, &[("option", Save::only_text_content())]);
    assert_eq!(texts(&store, "option"), vec![Some("A"), Some("B")]);
}

/// Mixed-case `<BUTTON>` must implicitly close the previous `<BUTTON>`.
#[test]
fn mixed_case_button_implicitly_closes_previous_button() {
    let html = "<DIV><BUTTON>A<BUTTON>B</DIV>";
    let store = parse_with_saves(html, &[("button", Save::only_text_content())]);
    assert_eq!(texts(&store, "button"), vec![Some("A"), Some("B")]);
}
