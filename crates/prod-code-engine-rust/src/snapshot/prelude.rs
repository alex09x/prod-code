/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The names the standard library puts in scope by itself. A workspace analyzed without the
//! sysroot (`[rust] sysroot = false`, or no `rust-src` to discover) resolves none of them; a
//! diagnostic about one is the setting, not the code's error (#1447).

use super::RustEngineSnapshot;

/// The items of the Rust 2021 prelude, `std::prelude::rust_2021`.
const PRELUDE: &[&str] = &[
    "AsMut",
    "AsRef",
    "Box",
    "Clone",
    "Copy",
    "Default",
    "DoubleEndedIterator",
    "Drop",
    "Eq",
    "Err",
    "ExactSizeIterator",
    "Extend",
    "Fn",
    "FnMut",
    "FnOnce",
    "From",
    "FromIterator",
    "Into",
    "IntoIterator",
    "Iterator",
    "None",
    "Ok",
    "Option",
    "Ord",
    "PartialEq",
    "PartialOrd",
    "Result",
    "Send",
    "Sized",
    "Some",
    "String",
    "Sync",
    "ToOwned",
    "ToString",
    "TryFrom",
    "TryInto",
    "Unpin",
    "Vec",
    "drop",
];

/// The macros `std` exports to every crate.
const MACROS: &[&str] = &[
    "assert",
    "assert_eq",
    "assert_ne",
    "cfg",
    "column",
    "compile_error",
    "concat",
    "dbg",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "env",
    "eprint",
    "eprintln",
    "file",
    "format",
    "format_args",
    "include",
    "include_bytes",
    "include_str",
    "line",
    "matches",
    "module_path",
    "option_env",
    "panic",
    "print",
    "println",
    "stringify",
    "todo",
    "unimplemented",
    "unreachable",
    "vec",
    "write",
    "writeln",
];

/// Whether `name` is a prelude item or a standard macro.
pub(super) fn is_standard_name(name: &str) -> bool {
    PRELUDE.contains(&name) || MACROS.contains(&name)
}

/// The name a native diagnostic is about, when it is one only the standard library defines: the
/// unresolved value or type (E0425, E0412, E0433) is the range itself; an unresolved macro call's
/// range starts with the macro's name.
pub(super) fn standard_name_in(code: &str, range_text: &str) -> bool {
    match code {
        "E0425" | "E0412" | "E0433" => is_standard_name(range_text.trim()),
        "unresolved-macro-call" => range_text
            .split('!')
            .next()
            .is_some_and(|name| MACROS.contains(&name.trim())),
        _ => false,
    }
}

impl RustEngineSnapshot {
    /// Whether the analysis has no standard library: no `core` crate in its graph.
    pub(super) fn without_standard_library(&self) -> bool {
        self.analysis()
            .and_then(|analysis| Ok(analysis.fetch_crates()?))
            .is_ok_and(|crates| {
                !crates
                    .iter()
                    .any(|info| info.name.as_deref() == Some("core"))
            })
    }
}
