/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;
use regex::Regex;
use std::sync::LazyLock;

/// Prove the complete literal in its language, rather than accepting a numeric
/// prefix or borrowing another language's separators/suffixes. In C++, `1_000`
/// and `1f` can invoke user-defined literal operators and are not scalar proof.
pub fn is_literal(text: &str, lang: Language) -> bool {
    // Unary plus performs a Number conversion, which throws for JavaScript BigInt.
    if matches!(lang, Language::JavaScript | Language::TypeScript)
        && text.starts_with('+')
        && text.ends_with('n')
    {
        return false;
    }
    static JS: LazyLock<Regex> = LazyLock::new(|| pattern("_", "n?", "", false, false));
    static PY: LazyLock<Regex> = LazyLock::new(|| pattern("_", "", "[jJ]?", true, false));
    static C: LazyLock<Regex> = LazyLock::new(|| c_pattern(""));
    static CPP: LazyLock<Regex> = LazyLock::new(|| c_pattern("'"));
    static GO: LazyLock<Regex> = LazyLock::new(|| pattern("_", "i?", "i?", true, false));
    static SWIFT: LazyLock<Regex> = LazyLock::new(swift_pattern);
    static JAVA: LazyLock<Regex> = LazyLock::new(|| pattern("_", "[lL]?", "[fFdD]?", false, false));
    static RUST: LazyLock<Regex> = LazyLock::new(|| {
        // Rust allows underscores before a type suffix: `0x1F_u8`, `1_f32`.
        pattern(
            "_",
            "(?:_*[iu](?:8|16|32|64|128|size))?",
            "(?:_*(?:f32|f64))?",
            false,
            false,
        )
    });
    let regex = match lang {
        Language::JavaScript | Language::TypeScript => &*JS,
        Language::Python => &*PY,
        Language::C => &*C,
        Language::Cpp => &*CPP,
        Language::Go => &*GO,
        Language::Swift => &*SWIFT,
        Language::Java => &*JAVA,
        Language::Rust => &*RUST,
    };
    regex.is_match(text)
}

fn c_pattern(separator: &str) -> Regex {
    pattern(
        separator,
        "(?:[uU](?:ll|LL|[lL])?|(?:ll|LL|[lL])[uU]?)?",
        "[fFlL]?",
        false,
        true,
    )
}

fn swift_pattern() -> Regex {
    // Swift requires 0x, a leading hex digit, and a binary exponent. Unlike C,
    // it permits repeated/trailing underscores but has no float suffixes.
    let ordinary = pattern("_", "", "", false, false);
    let h = "[0-9a-fA-F][0-9a-fA-F_]*";
    let d = "[0-9][0-9_]*";
    Regex::new(&format!(
        "(?:{}|^[+-]?0x{h}(?:\\.{h})?[pP][+-]?{d}$)",
        ordinary.as_str()
    ))
    .expect("Swift numeric literal pattern")
}

fn pattern(
    separator: &str,
    integer: &str,
    float: &str,
    prefix_separator: bool,
    c_float: bool,
) -> Regex {
    let digits = |class| {
        if separator.is_empty() {
            format!("[{class}]+")
        } else {
            format!("[{class}](?:{separator}?[{class}])*")
        }
    };
    let d = digits("0-9");
    let h = digits("0-9a-fA-F");
    let b = digits("01");
    let o = digits("0-7");
    let prefix = if prefix_separator { "_?" } else { "" };
    let exponent = format!("[eE][+-]?{d}");
    let decimal = if c_float {
        // Unlike Java, an integer followed by `f` is a C++ user-defined literal.
        format!("(?:(?:{d}\\.(?:{d})?|\\.{d})(?:{exponent})?|{d}{exponent})")
    } else {
        format!("(?:{d}(?:\\.(?:{d})?)?|\\.{d})(?:{exponent})?")
    };
    // C/C++ hexadecimal floats require a binary exponent. Retain the language's
    // digit separators and built-in suffixes, never user-defined literal suffixes.
    let hexadecimal = if c_float {
        format!("|0[xX](?:{h}(?:\\.(?:{h})?)?|\\.{h})[pP][+-]?{d}{float}")
    } else {
        String::new()
    };
    Regex::new(&format!(
        "^[+-]?(?:(?:0[xX]{prefix}{h}|0[bB]{prefix}{b}|0[oO]{prefix}{o}|{d}){integer}|{decimal}{float}{hexadecimal})$"
    )).expect("complete numeric literal pattern")
}
