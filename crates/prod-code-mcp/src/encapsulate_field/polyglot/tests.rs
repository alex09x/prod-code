/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::Language;
use super::{has_ambiguous_property_use, rewrite_external_line};

#[test]
fn property_uses_are_ambiguous_unless_their_receiver_is_this_or_self() {
    let cases: &[(&str, bool)] = &[
        ("self.count = 1\nthis.count\nthis->count\n", false),
        ("other.count", true),
        ("p->count", true),
        ("self.count; other.count", true),
        ("x = self.count\ny = self.count", false),
        ("nothing here", false),
        ("", false),
    ];
    for (text, expected) in cases {
        assert_eq!(
            has_ambiguous_property_use(text, "count"),
            *expected,
            "{text:?}"
        );
    }
}

#[test]
fn external_line_rewriting_follows_the_language() {
    let go = rewrite_external_line(Language::Go, "c.count = 4", "count");
    assert_eq!(go, ("c.SetCount(4)".to_string(), 0, 1));
    let go_read = rewrite_external_line(Language::Go, "return c.Count", "count");
    assert_eq!(go_read, ("return c.Count()".to_string(), 1, 0));
    let py = rewrite_external_line(Language::Python, "print(b.count)", "count");
    assert_eq!(py, ("print(b.get_count())".to_string(), 1, 0));
    let py_write = rewrite_external_line(Language::Python, "b.count = 3", "count");
    assert_eq!(py_write, ("b.set_count(3)".to_string(), 0, 1));
}
