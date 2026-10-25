/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;

#[test]
fn entities_and_name_characters() {
    for (entity, valid) in [
        ("amp", true),
        ("lt", true),
        ("gt", true),
        ("quot", true),
        ("apos", true),
        ("#10", true),
        ("#xAf", true),
        ("#XFF", true),
        ("#x", false),
        ("#z", false),
        ("#", false),
        ("#4294967296", false),
        ("nbsp", false),
        ("", false),
    ] {
        assert_eq!(is_known_xml_entity(entity), valid, "{entity}");
    }
    for (byte, start, rest) in [
        (b'a', true, true),
        (b'Z', true, true),
        (b'_', true, true),
        (b':', true, true),
        (b'0', false, true),
        (b'.', false, true),
        (b'-', false, true),
        (b' ', false, false),
        (0xff, false, false),
    ] {
        assert_eq!(is_xml_name_start_char(byte), start);
        assert_eq!(is_xml_name_char(byte), rest);
    }
}

#[test]
fn cursor_boundaries_and_recovery() {
    let mut p = XmlParser::new("a.xml", " \r\nns:a-0.x>tail", false);
    assert!(!p.consume(b"<"));
    p.skip_whitespace();
    assert_eq!((p.idx, p.line, p.col), (3, 2, 1));
    assert_eq!(p.read_xml_name(), "ns:a-0.x");
    assert!(p.consume(b">"));
    p.skip_until(b'?');
    assert_eq!(p.peek(), None);
    assert_eq!(p.advance(), None);
    assert_eq!(p.read_xml_name(), "");
    assert_eq!(p.idx, 16);
    let mut invalid = XmlParser::new("a.xml", "9a", false);
    assert_eq!(invalid.read_xml_name(), "");
    assert_eq!(invalid.idx, 0);
}
