/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io;

/// Returns true if `path_str` is equal to `root_str` or is a path under `root_str`
/// (separated by a component boundary `/` or `\`).
pub fn is_under_root(path_str: &str, root_str: &str) -> bool {
    let clean_root = root_str.trim_end_matches(['/', '\\']);
    if path_str == clean_root {
        return true;
    }
    if path_str.starts_with(clean_root) {
        let remainder = &path_str[clean_root.len()..];
        return remainder.starts_with('/') || remainder.starts_with('\\');
    }
    false
}

/// Relocates a standalone path or `file://` URI if and only if it matches `from_str`
/// or has `from_str` as a path-component prefix.
///
/// External paths and sibling directories (e.g. `/work/repository-deps` when `from` is
/// `/work/repo`) are left completely untouched.
pub fn relocate_path_or_uri(s: &str, from_str: &str, to_str: &str) -> String {
    let from_clean = from_str.trim_end_matches(['/', '\\']);
    let to_clean = to_str.trim_end_matches(['/', '\\']);

    // 1. Direct path check
    if s == from_clean {
        return to_clean.to_string();
    }
    if s == format!("{from_clean}/") {
        return format!("{to_clean}/");
    }
    if s == format!("{from_clean}\\") {
        return format!("{to_clean}\\");
    }
    if s.starts_with(from_clean) {
        let remainder = &s[from_clean.len()..];
        if remainder.starts_with('/') || remainder.starts_with('\\') {
            return format!("{}{}", to_clean, remainder);
        }
    }

    // 2. URI check: file:// or file:///
    if let Some(uri_rest) = s.strip_prefix("file://") {
        if uri_rest == from_clean {
            return format!("file://{to_clean}");
        }
        if uri_rest == format!("{from_clean}/") {
            return format!("file://{to_clean}/");
        }
        if uri_rest == format!("{from_clean}\\") {
            return format!("file://{to_clean}\\");
        }
        if uri_rest.starts_with(from_clean) {
            let remainder = &uri_rest[from_clean.len()..];
            if remainder.starts_with('/') || remainder.starts_with('\\') {
                return format!("file://{to_clean}{remainder}");
            }
        }
    }

    s.to_string()
}

/// Relocates an argument token which may be a path, a URI, a quoted string, a key=value pair,
/// or a compiler flag with an attached path (e.g. `-I/path`, `-isystem/path`).
pub fn relocate_arg_token(token: &str, from_str: &str, to_str: &str) -> String {
    if token.is_empty() {
        return String::new();
    }

    // Handle full surrounding quotes: "..." or '...'
    if (token.starts_with('"') && token.ends_with('"') && token.len() >= 2)
        || (token.starts_with('\'') && token.ends_with('\'') && token.len() >= 2)
    {
        let quote = &token[0..1];
        let inner = &token[1..token.len() - 1];
        let relocated = relocate_arg_token(inner, from_str, to_str);
        return format!("{quote}{relocated}{quote}");
    }

    // Handle key=value tokens, e.g. -DFOO="/path" or VAR=/path
    if let Some((k, v)) = token.split_once('=') {
        let relocated_v = relocate_arg_token(v, from_str, to_str);
        if relocated_v != v {
            return format!("{k}={relocated_v}");
        }
    }

    // Handle compiler flags with attached path (quoted or unquoted)
    const ATTACHED_FLAG_PREFIXES: &[&str] = &[
        "-I",
        "-isystem",
        "-iquote",
        "-idirafter",
        "-iframework",
        "-iprefix",
        "-iwithprefix",
        "-iwithprefixbefore",
        "-isysroot",
        "--sysroot=",
        "-L",
        "-B",
        "-o",
        "-Wl,-rpath,",
        "-Wl,-rpath=",
        "-Wl,-R,",
        "-Wl,-L,",
    ];

    for &flag in ATTACHED_FLAG_PREFIXES {
        if let Some(rest) = token.strip_prefix(flag) {
            let relocated_rest = relocate_arg_token(rest, from_str, to_str);
            if relocated_rest != rest {
                return format!("{flag}{relocated_rest}");
            }
        }
    }

    // Handle prefix-mapping flags: -fdebug-prefix-map=old=new
    const PREFIX_MAP_FLAGS: &[&str] = &[
        "-fdebug-prefix-map=",
        "-ffile-prefix-map=",
        "-fmacro-prefix-map=",
    ];

    for &flag in PREFIX_MAP_FLAGS {
        if let Some(rest) = token.strip_prefix(flag) {
            if let Some((old_part, new_part)) = rest.split_once('=') {
                let relocated_old = relocate_arg_token(old_part, from_str, to_str);
                let relocated_new = relocate_arg_token(new_part, from_str, to_str);
                return format!("{flag}{relocated_old}={relocated_new}");
            }
        }
    }

    // Base path or URI relocation
    relocate_path_or_uri(token, from_str, to_str)
}

/// Relocates paths in a shell-style compilation command string, preserving exact whitespace,
/// quotes, and delimiters while rewriting only tokens that match `from_str` with path-component boundaries.
pub fn relocate_command_string(cmd: &str, from_str: &str, to_str: &str) -> String {
    let mut result = String::with_capacity(cmd.len());
    let chars: Vec<char> = cmd.chars().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        // Consume whitespace
        if chars[i].is_whitespace() {
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // Consume a token (argument)
        let start = i;
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut escape_next = false;

        while i < n {
            let c = chars[i];
            if escape_next {
                escape_next = false;
                i += 1;
                continue;
            }

            if c == '\\' && !in_single_quote {
                escape_next = true;
                i += 1;
                continue;
            }

            if c == '\'' && !in_double_quote {
                in_single_quote = !in_single_quote;
                i += 1;
                continue;
            }

            if c == '"' && !in_single_quote {
                in_double_quote = !in_double_quote;
                i += 1;
                continue;
            }

            if !in_single_quote && !in_double_quote && c.is_whitespace() {
                break;
            }

            i += 1;
        }

        let token: String = chars[start..i].iter().collect();
        let relocated_token = relocate_arg_token(&token, from_str, to_str);
        result.push_str(&relocated_token);
    }

    result
}

/// Relocates compilation database content (`compile_commands.json`).
///
/// Parses JSON and updates `directory`, `file`, `output`, `arguments`, and `command`
/// fields preserving component boundaries. Sibling paths sharing prefixes are left untouched.
/// Falls back to line-by-line command relocation if JSON parsing fails.
pub fn relocate_compile_commands_content(
    content: &str,
    from_str: &str,
    to_str: &str,
) -> io::Result<String> {
    if let Ok(mut json_val) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(arr) = json_val.as_array_mut() {
            for entry in arr {
                if let Some(obj) = entry.as_object_mut() {
                    if let Some(dir) = obj.get("directory").and_then(|v| v.as_str()) {
                        let relocated_dir = relocate_arg_token(dir, from_str, to_str);
                        obj.insert(
                            "directory".to_string(),
                            serde_json::Value::String(relocated_dir),
                        );
                    }
                    if let Some(file) = obj.get("file").and_then(|v| v.as_str()) {
                        let relocated_file = relocate_arg_token(file, from_str, to_str);
                        obj.insert(
                            "file".to_string(),
                            serde_json::Value::String(relocated_file),
                        );
                    }
                    if let Some(output) = obj.get("output").and_then(|v| v.as_str()) {
                        let relocated_output = relocate_arg_token(output, from_str, to_str);
                        obj.insert(
                            "output".to_string(),
                            serde_json::Value::String(relocated_output),
                        );
                    }
                    if let Some(args) = obj.get_mut("arguments").and_then(|v| v.as_array_mut()) {
                        for arg in args {
                            if let Some(s) = arg.as_str() {
                                let relocated_arg = relocate_arg_token(s, from_str, to_str);
                                *arg = serde_json::Value::String(relocated_arg);
                            }
                        }
                    }
                    if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
                        let relocated_cmd = relocate_command_string(cmd, from_str, to_str);
                        obj.insert(
                            "command".to_string(),
                            serde_json::Value::String(relocated_cmd),
                        );
                    }
                }
            }
            return serde_json::to_string_pretty(&json_val).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to serialize compile_commands.json: {e}"),
                )
            });
        }
    }

    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        out.push_str(&relocate_command_string(line, from_str, to_str));
        out.push('\n');
    }
    Ok(out)
}
