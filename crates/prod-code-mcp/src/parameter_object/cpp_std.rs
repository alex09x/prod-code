/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

/// The C++ standard the project builds with, as a year (2017, 2020), from what its build
/// declares: a compilation database or `compile_flags.txt` at the root or in `build/`, else a
/// `CMakeLists.txt` from the file's directory up to the root. `None` when none of them says, and
/// then the compiler's default applies, which is C++17 for current GCC and clang.
pub(crate) fn cpp_standard(root: &Path, file: &Path) -> Option<u32> {
    for name in [
        "compile_commands.json",
        "build/compile_commands.json",
        "compile_flags.txt",
    ] {
        if let Some(year) = std::fs::read_to_string(root.join(name))
            .ok()
            .and_then(|t| std_in_flags(&t))
        {
            return Some(year);
        }
    }
    let mut dir = file.parent();
    while let Some(d) = dir {
        if let Some(year) = std::fs::read_to_string(d.join("CMakeLists.txt"))
            .ok()
            .and_then(|t| std_in_cmake(&t))
        {
            return Some(year);
        }
        if d == root || !d.starts_with(root) {
            break;
        }
        dir = d.parent();
    }
    None
}

/// The year of the first `-std=c++NN` (or `gnu++NN`, or MSVC's `/std:c++NN`) in compiler flags.
pub(crate) fn std_in_flags(text: &str) -> Option<u32> {
    ["std=c++", "std=gnu++", "std:c++"]
        .iter()
        .find_map(|marker| {
            text.match_indices(marker)
                .find_map(|(at, m)| std_year(&text[at + m.len()..]))
        })
}

/// The year a `CMakeLists.txt` asks for: `set(CMAKE_CXX_STANDARD 20)`, `cxx_std_20`, or a flag.
pub(crate) fn std_in_cmake(text: &str) -> Option<u32> {
    text.match_indices("CMAKE_CXX_STANDARD")
        .find_map(|(at, m)| std_year(text[at + m.len()..].trim_start()))
        .or_else(|| {
            text.match_indices("cxx_std_")
                .find_map(|(at, m)| std_year(&text[at + m.len()..]))
        })
        .or_else(|| std_in_flags(text))
}

/// A standard's version as a year: `17` and `1z` are 2017, `20` and `2a` are 2020, `98` is 1998.
pub(crate) fn std_year(version: &str) -> Option<u32> {
    let token: String = version
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    Some(match token.as_str() {
        "98" => 1998,
        "03" => 2003,
        "0x" => 2011,
        "1y" => 2014,
        "1z" => 2017,
        "2a" => 2020,
        "2b" => 2023,
        "2c" => 2026,
        t if t.len() == 2 && t.bytes().all(|b| b.is_ascii_digit()) => {
            2000 + t.parse::<u32>().ok()?
        }
        _ => return None,
    })
}
