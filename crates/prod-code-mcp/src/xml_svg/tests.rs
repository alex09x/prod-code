/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::parser::validate_xml;

#[test]
fn valid_svg_passes_with_zero_errors() {
    let svg = r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <circle cx="50" cy="50" r="40" fill="red" />
</svg>"#;
    let rep = validate_xml("test.svg", svg, true);
    assert_eq!(rep.errors, 0, "{:?}", rep.items);
}

#[test]
fn non_svg_root_in_svg_file_is_error() {
    let bad_svg = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body></body></html>"#;
    let rep = validate_xml("bad.svg", bad_svg, true);
    assert!(rep.errors > 0);
    assert!(rep.items.iter().any(|d| {
        d.message
            .contains("SVG document root element must be `<svg>`")
    }));
}

#[test]
fn mismatched_tags_report_error_with_line_and_col() {
    let bad_xml = r#"<root>
  <child>
</root>"#;
    let rep = validate_xml("bad.xml", bad_xml, false);
    assert!(rep.errors > 0);
    assert!(rep.items.iter().any(|d| {
        d.message
            .contains("mismatched closing tag `</root>`; expected `</child>`")
    }));
}

#[test]
fn unclosed_quote_reports_error() {
    let bad_xml = r#"<root attr="unclosed>content</root>"#;
    let rep = validate_xml("bad.xml", bad_xml, false);
    assert!(rep.errors > 0);
    assert!(
        rep.items
            .iter()
            .any(|d| d.message.contains("unclosed quote"))
    );
}

#[test]
fn duplicate_attributes_report_error() {
    let bad_xml = r#"<root attr="1" attr="2" />"#;
    let rep = validate_xml("bad.xml", bad_xml, false);
    assert!(rep.errors > 0);
    assert!(
        rep.items
            .iter()
            .any(|d| d.message.contains("duplicate attribute `attr`"))
    );
}
