/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A basic outline of a shell script (`.sh`, `.bash`, `.zsh`), read here so that it needs no
//! `bash-language-server` on any node (#1851): the functions it defines, with their lines.

use super::options::OutlineOptions;

/// The functions of shell `text`: `name() {`, `function name {` and `function name() {`, at
/// any nesting. Comments, here-document bodies and lines inside quotes are not looked into.
pub(crate) fn shell_outline(text: &str, path: &str, options: &OutlineOptions) -> String {
    let mut out = format!("Outline for {path}:\n");
    if options
        .kinds
        .as_ref()
        .is_some_and(|kinds| !kinds.iter().any(|k| k.eq_ignore_ascii_case("function")))
    {
        out.push_str("  (no functions)");
        return out;
    }
    let mut listed = 0usize;
    let mut heredoc_end: Option<String> = None;
    for (i, line) in text.lines().enumerate() {
        if let Some(end) = &heredoc_end {
            if line.trim() == end {
                heredoc_end = None;
            }
            continue;
        }
        if let Some(marker) = heredoc_marker(line) {
            heredoc_end = Some(marker);
        }
        let code = line.trim_start();
        if code.starts_with('#') {
            continue;
        }
        if let Some(name) = function_name(code) {
            listed += 1;
            out.push_str(&format!("  [Function] {name} (line {})\n", i + 1));
        }
    }
    if listed == 0 {
        out.push_str("  (no functions)");
    }
    out.trim_end().to_string()
}

/// The terminator of a here-document started on `line` (`<<EOF`, `<<-'EOF'`), if one is.
fn heredoc_marker(line: &str) -> Option<String> {
    let rest = &line[line.find("<<")? + 2..];
    if rest.starts_with('<') {
        return None; // a here-string
    }
    let word = rest
        .trim_start_matches('-')
        .trim_start()
        .trim_start_matches(['\'', '"'])
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()?;
    // `$((1<<2))` is arithmetic, not a here-document.
    (!word.is_empty() && !word.starts_with(|c: char| c.is_ascii_digit())).then(|| word.to_string())
}

/// The name a function definition on `code` (comment-free, left-trimmed) gives.
fn function_name(code: &str) -> Option<&str> {
    let ident = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':');
    let (name, rest) = if let Some(after) = code.strip_prefix("function ") {
        let after = after.trim_start();
        let end = after.find(|c: char| !ident(c)).unwrap_or(after.len());
        (&after[..end], after[end..].trim_start())
    } else {
        let end = code.find(|c: char| !ident(c))?;
        let rest = code[end..].trim_start();
        let rest = rest.strip_prefix('(')?.trim_start().strip_prefix(')')?;
        (&code[..end], rest.trim_start())
    };
    let rest = rest.strip_prefix("()").map(str::trim_start).unwrap_or(rest);
    (!name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && rest.starts_with(['{', '(']))
    .then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn functions_are_listed_in_each_form_and_commands_are_not() {
        let text = "#!/bin/sh\n# helper() {\nset -e\nhelper() {\n  echo hi\n}\nfunction build {\n  :\n}\nfunction deploy-now() {\n  :\n}\ncat <<EOF\nnot_a() {\nEOF\nif [ -f x ]; then run() { :; }; fi\necho \"x\" | tr a-z A-Z\nkeep (){ :; }\n";
        let options = OutlineOptions::all(usize::MAX, false, "include_locals");
        assert_eq!(
            shell_outline(text, "run.sh", &options),
            "Outline for run.sh:\n  [Function] helper (line 4)\n  [Function] build (line 7)\n  \
             [Function] deploy-now (line 10)\n  [Function] keep (line 18)"
        );
        assert!(shell_outline("echo hi\n", "x.sh", &options).ends_with("(no functions)"));
    }
}
