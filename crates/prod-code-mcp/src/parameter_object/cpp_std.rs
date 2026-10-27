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

/// The C++ standard `file` builds with, as a year (2017, 2020), from what its build declares: its
/// own entry in a compilation database at the root or in `build/`, else `compile_flags.txt`, else a
/// `CMakeLists.txt` from the file's directory up to the root. A file the database has no entry
/// for, such as a header, is compiled by any of its translation units, so it gets the oldest
/// standard among them (#1513). `None` when nothing says, and then the compiler's default
/// applies, which is C++17 for current GCC and clang.
pub(crate) fn cpp_standard(root: &Path, file: &Path) -> Option<u32> {
    for name in ["compile_commands.json", "build/compile_commands.json"] {
        if let Some(year) = std::fs::read_to_string(root.join(name))
            .ok()
            .and_then(|t| std_in_database(&t, file))
        {
            return Some(year);
        }
    }
    if let Some(year) = std::fs::read_to_string(root.join("compile_flags.txt"))
        .ok()
        .and_then(|t| std_in_flags(&t))
    {
        return Some(year);
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

/// The standard of `file`'s entry in a `compile_commands.json`, by the last `-std` its command
/// gives, which is the one the compiler obeys; an entry with none is the compiler's default,
/// C++17. Without an entry for `file`, the oldest standard of all entries.
pub(crate) fn std_in_database(text: &str, file: &Path) -> Option<u32> {
    const DEFAULT: u32 = 2017;
    let entries: Vec<serde_json::Value> = serde_json::from_str(text).ok()?;
    let same = |a: &Path, b: &Path| {
        a == b
            || std::fs::canonicalize(a)
                .ok()
                .zip(std::fs::canonicalize(b).ok())
                .is_some_and(|(a, b)| a == b)
    };
    let mut oldest: Option<u32> = None;
    for entry in &entries {
        let flags = match entry.get("arguments").and_then(|a| a.as_array()) {
            Some(args) => args
                .iter()
                .filter_map(|a| a.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            None => entry
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string(),
        };
        let year = std_in_flags(&flags).unwrap_or(DEFAULT);
        let source = entry.get("file").and_then(|f| f.as_str()).map(Path::new);
        let directory = entry
            .get("directory")
            .and_then(|d| d.as_str())
            .map(Path::new);
        if let Some(source) = source {
            let source = match directory {
                Some(dir) if source.is_relative() => dir.join(source),
                _ => source.to_path_buf(),
            };
            if same(&source, file) {
                return Some(year);
            }
        }
        oldest = Some(oldest.map_or(year, |o| o.min(year)));
    }
    oldest
}

/// The year of the last `-std=c++NN` (or `gnu++NN`, or MSVC's `/std:c++NN`) in compiler flags:
/// the compiler obeys the last one given (#1513).
pub(crate) fn std_in_flags(text: &str) -> Option<u32> {
    ["std=c++", "std=gnu++", "std:c++"]
        .iter()
        .flat_map(|marker| {
            text.match_indices(marker)
                .filter_map(|(at, m)| std_year(&text[at + m.len()..]).map(|year| (at, year)))
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, year)| year)
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
