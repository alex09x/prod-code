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

use crate::parameter_object::Language;

/// Proves whether `caller_path` genuinely imports or shares scope with `decl_file` for `fn_name`.
/// Used when semantic references are empty to prevent cross-file text fallback from rewriting
/// unrelated local aliases or same-named symbols in other files (#982).
pub(crate) fn proves_cross_file_import(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
    lang: Language,
) -> bool {
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let decl_name = decl_file.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if decl_stem.is_empty() {
        return false;
    }

    match lang {
        Language::TypeScript | Language::JavaScript => {
            ts_js_proves_import(content, decl_stem, fn_name)
        }
        Language::Python => python_proves_import(content, decl_stem, fn_name),
        Language::Go => {
            // In Go, files in the same directory share package scope.
            caller_path.parent() == decl_file.parent()
        }
        Language::Cpp | Language::C => {
            caller_path.file_stem() == decl_file.file_stem()
                || content.lines().any(|l| {
                    let trimmed = l.trim();
                    trimmed.starts_with("#include")
                        && (trimmed.contains(decl_name) || trimmed.contains(decl_stem))
                })
        }
        Language::Swift => caller_path.parent() == decl_file.parent(),
        _ => false,
    }
}

fn extract_specifier(s: &str) -> &str {
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '"' || c == '\'' || c == '`' {
            let quote = c;
            let start = i + 1;
            for (j, end_c) in chars {
                if end_c == quote {
                    return &s[start..j];
                }
            }
            return &s[start..];
        }
    }
    ""
}

fn specifier_matches_stem(specifier: &str, decl_stem: &str) -> bool {
    if specifier.is_empty() {
        return false;
    }
    let trimmed = specifier.trim();
    let mod_file = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let mod_stem = mod_file
        .strip_suffix(".ts")
        .or_else(|| mod_file.strip_suffix(".tsx"))
        .or_else(|| mod_file.strip_suffix(".js"))
        .or_else(|| mod_file.strip_suffix(".jsx"))
        .or_else(|| mod_file.strip_suffix(".mjs"))
        .or_else(|| mod_file.strip_suffix(".cjs"))
        .unwrap_or(mod_file);
    mod_stem == decl_stem
}

fn clause_imports_name(clause: &str, fn_name: &str) -> bool {
    let trimmed = clause.trim();
    if trimmed.contains('*') {
        return true;
    }
    if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if open < close {
            let inner = &trimmed[open + 1..close];
            for item in inner.split(',') {
                let parts: Vec<&str> = item.split_whitespace().collect();
                match parts.as_slice() {
                    [name] if *name == fn_name => return true,
                    [_orig, "as", local] if *local == fn_name => return true,
                    [orig, "as", _local] if *orig == fn_name => {
                        // Aliased away to a different name, so calls to fn_name
                        // do not refer to this imported symbol.
                        return false;
                    }
                    _ => {}
                }
            }
            return false;
        }
    }
    // Default import or bare name: import fn_name from "..."
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    words.contains(&fn_name)
}

fn ts_js_proves_import(content: &str, decl_stem: &str, fn_name: &str) -> bool {
    for part in content.split("import") {
        if let Some((clause, rest)) = part.split_once("from") {
            let specifier = extract_specifier(rest);
            if specifier_matches_stem(specifier, decl_stem) && clause_imports_name(clause, fn_name)
            {
                return true;
            }
        }
    }
    let mut search_idx = 0;
    while let Some(req_pos) = content[search_idx..].find("require(") {
        let abs_req_pos = search_idx + req_pos;
        let rest = &content[abs_req_pos + "require(".len()..];
        let specifier = extract_specifier(rest);
        if specifier_matches_stem(specifier, decl_stem) {
            let line_before = content[..abs_req_pos]
                .lines()
                .next_back()
                .unwrap_or("")
                .trim();
            if let Some((lhs, _)) = line_before.split_once('=') {
                let lhs = lhs.trim();
                if let (Some(open), Some(close)) = (lhs.find('{'), lhs.rfind('}')) {
                    if open < close {
                        let inner = &lhs[open + 1..close];
                        for item in inner.split(',') {
                            let item = item.trim();
                            if let Some((orig, local)) = item.split_once(':') {
                                let orig = orig.trim();
                                let local = local.trim();
                                if orig == fn_name && local != fn_name {
                                    return false;
                                }
                                if local == fn_name {
                                    return true;
                                }
                            } else if item == fn_name {
                                return true;
                            }
                        }
                    }
                } else {
                    let lhs_clean = lhs
                        .trim_start_matches("const")
                        .trim_start_matches("let")
                        .trim_start_matches("var")
                        .trim_start_matches("import")
                        .trim();
                    if !lhs_clean.is_empty()
                        && lhs_clean
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
                    {
                        return true;
                    }
                }
            }
        }
        search_idx = abs_req_pos + "require(".len();
    }
    false
}

fn python_proves_import(content: &str, decl_stem: &str, fn_name: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("from ") {
            if let Some((mod_part, clause)) = rest.split_once(" import ") {
                let mod_name = mod_part
                    .trim()
                    .rsplit('.')
                    .next()
                    .unwrap_or(mod_part.trim());
                let mod_stem = mod_name.trim_start_matches('.');
                if mod_stem == decl_stem {
                    if clause.trim() == "*" {
                        return true;
                    }
                    for item in clause.split(',') {
                        let parts: Vec<&str> = item.split_whitespace().collect();
                        match parts.as_slice() {
                            [name] if *name == fn_name => return true,
                            [_orig, "as", local] if *local == fn_name => return true,
                            [orig, "as", _local] if *orig == fn_name => {
                                return false;
                            }
                            _ => {}
                        }
                    }
                }
            }
        } else if let Some(rest) = trimmed.strip_prefix("import ") {
            for entry in rest.split(',') {
                let (mod_part, _alias) = entry.split_once(" as ").unwrap_or((entry, ""));
                let mod_name = mod_part
                    .trim()
                    .rsplit('.')
                    .next()
                    .unwrap_or(mod_part.trim());
                if mod_name == decl_stem {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ts_js_proves_import_positive() {
        let content = r#"import { calculate } from "./math";"#;
        assert!(proves_cross_file_import(
            content,
            Path::new("src/client.ts"),
            Path::new("src/math.ts"),
            "calculate",
            Language::TypeScript,
        ));

        let content_namespace = r#"import * as math from "./math";"#;
        assert!(proves_cross_file_import(
            content_namespace,
            Path::new("src/client.ts"),
            Path::new("src/math.ts"),
            "calculate",
            Language::TypeScript,
        ));

        let content_require_obj = r#"const math = require("./math");"#;
        assert!(proves_cross_file_import(
            content_require_obj,
            Path::new("src/client.ts"),
            Path::new("src/math.ts"),
            "calculate",
            Language::TypeScript,
        ));

        let content_require_destruct = r#"const { calculate } = require("./math");"#;
        assert!(proves_cross_file_import(
            content_require_destruct,
            Path::new("src/client.ts"),
            Path::new("src/math.ts"),
            "calculate",
            Language::TypeScript,
        ));

        let content_multiline = "import {\n  calculate,\n  other,\n} from \"./math\";";
        assert!(proves_cross_file_import(
            content_multiline,
            Path::new("src/client.ts"),
            Path::new("src/math.ts"),
            "calculate",
            Language::TypeScript,
        ));
    }

    #[test]
    fn test_ts_js_proves_import_rejects_unrelated_alias() {
        let content = r#"import { unrelated as retry } from "./other";"#;
        assert!(!proves_cross_file_import(
            content,
            Path::new("repro/caller.ts"),
            Path::new("repro/selected.ts"),
            "retry",
            Language::TypeScript,
        ));

        let content_different_mod = r#"import { retry } from "./other";"#;
        assert!(!proves_cross_file_import(
            content_different_mod,
            Path::new("repro/caller.ts"),
            Path::new("repro/selected.ts"),
            "retry",
            Language::TypeScript,
        ));
    }

    #[test]
    fn test_python_proves_import() {
        let content = "from db import find_user\n";
        assert!(proves_cross_file_import(
            content,
            Path::new("service.py"),
            Path::new("db.py"),
            "find_user",
            Language::Python,
        ));

        let content_import_as = "import db as database\n";
        assert!(proves_cross_file_import(
            content_import_as,
            Path::new("service.py"),
            Path::new("db.py"),
            "find_user",
            Language::Python,
        ));

        let content_other = "from other import find_user\n";
        assert!(!proves_cross_file_import(
            content_other,
            Path::new("service.py"),
            Path::new("db.py"),
            "find_user",
            Language::Python,
        ));
    }
}
