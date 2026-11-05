/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A basic outline of an assembly file (`.s`, `.S`, `.asm`), read here since no language server
//! serves assembly (#1935): the symbols it declares global, its labels, and its macros.

use super::options::OutlineOptions;

/// The labels, `.globl` / `.global` symbols and `.macro` definitions of assembly `text`, in
/// order, each with its line. A label is an identifier followed by `:` at the start of a line;
/// local labels (`.L…`, numeric `1:`) are not part of the outline.
pub(crate) fn assembly_outline(text: &str, path: &str, options: &OutlineOptions) -> String {
    let mut out = format!("Outline for {path}:\n");
    let mut listed = 0usize;
    let wants = |kind: &str| {
        options
            .kinds
            .as_ref()
            .is_none_or(|kinds| kinds.iter().any(|k| k.eq_ignore_ascii_case(kind)))
    };
    for (i, line) in text.lines().enumerate() {
        let code = line
            .split(['#', ';'])
            .next()
            .unwrap_or("")
            .split("//")
            .next()
            .unwrap_or("")
            .trim();
        let mut push = |kind: &str, name: &str| {
            if !name.is_empty() && wants(kind) {
                listed += 1;
                out.push_str(&format!("  [{kind}] {name} (line {})\n", i + 1));
            }
        };
        if let Some(rest) = code
            .strip_prefix(".globl")
            .or_else(|| code.strip_prefix(".global"))
        {
            for name in rest.split(',') {
                push("Global", name.trim());
            }
        } else if let Some(rest) = code
            .strip_prefix(".macro")
            .or_else(|| code.strip_prefix(".MACRO"))
        {
            push("Macro", rest.split_whitespace().next().unwrap_or(""));
        } else if let Some((label, _)) = code.split_once(':') {
            let name = label.trim();
            let local = name.starts_with(".L") || name.chars().all(|c| c.is_ascii_digit());
            let identifier = !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '@'));
            if identifier && !local {
                push("Label", name);
            }
        }
    }
    if listed == 0 {
        out.push_str("  (no labels, globals or macros)");
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_globals_and_macros_are_listed_and_local_labels_are_not() {
        let text = ".text\n.globl _start, helper\n_start:\n    mov x0, #0 // exit\n1:\n    b 1b\n.Lskip:\nhelper: # entry\n.macro SAVE reg\n  push \\reg\n.endm\n";
        let options = OutlineOptions::all(usize::MAX, false, "include_locals");
        let outline = assembly_outline(text, "boot.S", &options);
        assert_eq!(
            outline,
            "Outline for boot.S:\n  [Global] _start (line 2)\n  [Global] helper (line 2)\n  \
             [Label] _start (line 3)\n  [Label] helper (line 8)\n  [Macro] SAVE (line 9)"
        );
        assert!(
            assembly_outline("nop\n", "x.s", &options).ends_with("(no labels, globals or macros)")
        );
    }
}
