/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A build that failed because the node lacks a system library (#2057). The cluster may place a
//! workspace on any node, and a GUI crate that linked on one node fails on another without the
//! X11 or Wayland development packages; the linker names the libraries, and the report names
//! the node and how to hold the workspace to one that has them.

/// Most library names listed in one hint.
const MAX_LISTED: usize = 12;

/// The system libraries a linker or `pkg-config` probe could not find, in the order first seen.
pub fn missing_system_libraries(output: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in output.lines() {
        let name = linker_library(line).or_else(|| pkg_config_library(line));
        if let Some(name) = name
            && !found.iter().any(|seen| seen == name)
        {
            found.push(name.to_string());
        }
    }
    found
}

/// `rust-lld: error: unable to find library -lxcb`, `/usr/bin/ld: cannot find -lxcb: ...`,
/// `ld: library not found for -lxcb`.
fn linker_library(line: &str) -> Option<&str> {
    [
        "unable to find library -l",
        "cannot find -l",
        "library not found for -l",
    ]
    .iter()
    .find_map(|marker| line.find(marker).map(|at| &line[at + marker.len()..]))
    .map(library_name)
    .filter(|name| !name.is_empty())
}

/// `The system library `xkbcommon` required by crate `...` was not found.`
fn pkg_config_library(line: &str) -> Option<&str> {
    let rest = line.split("The system library `").nth(1)?;
    let (name, tail) = rest.split_once('`')?;
    (tail.contains("was not found") && !name.is_empty()).then_some(name)
}

fn library_name(rest: &str) -> &str {
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '+')))
        .unwrap_or(rest.len());
    rest[..end].trim_end_matches('.')
}

/// What to tell the caller when the command failed for want of system libraries on `ran_on`.
pub fn missing_system_libraries_hint(output: &str, ran_on: &str) -> Option<String> {
    let missing = missing_system_libraries(output);
    if missing.is_empty() {
        return None;
    }
    let mut listed = missing[..missing.len().min(MAX_LISTED)].join(", ");
    if missing.len() > MAX_LISTED {
        listed.push_str(&format!(" and {} more", missing.len() - MAX_LISTED));
    }
    Some(format!(
        "The node {ran_on} lacks system libraries this build links or probes: {listed}. Install \
         their development packages on that node, or hold this workspace to a node that has \
         them with `[cluster] pin = \"<node>\"` in prod-code.toml (or `--pin-node <node>` / \
         PROD_CODE_PIN_NODE); `prod-code cluster` lists the nodes."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linker_and_pkg_config_failures_name_each_library_once() {
        let output = "\
error: linking with `cc` failed: exit status: 1
  = note: rust-lld: error: unable to find library -lxcb
          rust-lld: error: unable to find library -lxkbcommon
          rust-lld: error: unable to find library -lxkbcommon-x11
          rust-lld: error: unable to find library -lxcb
/usr/bin/ld: cannot find -lfontconfig: No such file or directory
ld: library not found for -lfreetype
The system library `wayland-client` required by crate `wayland-sys` was not found.
";
        assert_eq!(
            missing_system_libraries(output),
            [
                "xcb",
                "xkbcommon",
                "xkbcommon-x11",
                "fontconfig",
                "freetype",
                "wayland-client"
            ]
        );
        let hint = missing_system_libraries_hint(output, "192.0.2.7:9400").expect("a hint");
        assert!(hint.contains("192.0.2.7:9400"), "{hint}");
        assert!(hint.contains("xcb, xkbcommon, xkbcommon-x11"), "{hint}");
        assert!(hint.contains("[cluster] pin"), "{hint}");
    }

    #[test]
    fn other_failures_get_no_hint() {
        let output = "error[E0425]: cannot find value `x` in this scope\ntest result: FAILED.";
        assert!(missing_system_libraries(output).is_empty());
        assert_eq!(missing_system_libraries_hint(output, "node"), None);
    }
}
