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
fn coverage_b2_parameter_format_matrix() {
    for lang in [Language::TypeScript, Language::Python] {
        for (ty, value, expected) in [("T", "v", "x: T = v"), ("T", "", "x: T"), ("", "v", "x = v"), ("", "", "x")] {
            assert_eq!(format_polyglot_param("x", ty, value, lang), expected);
        }
    }
    for (lang, ty, value, expected) in [
        (Language::JavaScript, "T", "v", "x = v"), (Language::JavaScript, "T", "", "x"),
        (Language::Cpp, "T", "v", "T x"), (Language::C, "T", "v", "T x"),
        (Language::Go, "T", "v", "x T"), (Language::Java, "", "", "Object x"),
        (Language::Java, "T", "v", "T x"), (Language::Swift, "T", "v", "x: T = v"),
        (Language::Swift, "T", "", "x: T"), (Language::Rust, "T", "v", "x: T = v"),
        (Language::Rust, "T", "", "x: T"),
    ] {
        assert_eq!(format_polyglot_param("x", ty, value, lang), expected);
    }
}
