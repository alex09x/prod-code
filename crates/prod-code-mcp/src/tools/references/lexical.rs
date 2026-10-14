/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Component, Path};

use crate::parameter_object::Language;
use crate::tools::symbols::declared_at;
use crate::wrap_return::polyglot::shadow::{body_has_local_decl, is_locally_shadowed};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImportMatch {
    Proven,
    Uncertain,
    None,
}

impl ImportMatch {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Proven, _) | (_, Self::Proven) => Self::Proven,
            (Self::Uncertain, _) | (_, Self::Uncertain) => Self::Uncertain,
            _ => Self::None,
        }
    }
}

fn normalize_segments(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => {
                if let Some(s) = s.to_str() {
                    out.push(s.to_string());
                }
            }
            _ => {}
        }
    }
    out
}

const CODE_EXTS: &[&str] = &[
    "tsx", "ts", "jsx", "js", "mjs", "cjs", "astro", "svelte", "vue", "pyi", "py", "rs", "hpp",
    "h", "c", "cpp", "cc", "cxx",
];

fn split_known_ext(s: &str) -> (&str, Option<&str>) {
    for &ext in CODE_EXTS {
        if let Some(stem) = s.strip_suffix(ext)
            && let Some(stripped) = stem.strip_suffix('.')
            && !stripped.is_empty()
            && !stripped.ends_with(['.', '/', '\\'])
        {
            return (stripped, Some(ext));
        }
    }
    (s, None)
}

fn ext_compatible(spec_ext: &str, decl_ext: &str) -> bool {
    spec_ext.eq_ignore_ascii_case(decl_ext)
        || matches!(
            (spec_ext, decl_ext),
            ("js", "ts" | "tsx" | "jsx")
                | ("jsx", "tsx")
                | ("mjs", "mts")
                | ("cjs", "cts")
                | ("h" | "hpp", "c" | "cpp" | "cc" | "cxx")
        )
}

fn has_competing_file_for_stem(stem_path: &Path, ignore_ext: Option<&str>) -> bool {
    ["ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "pyi", "rs"]
        .iter()
        .any(|&ext| {
            !ignore_ext.is_some_and(|ig| ig.eq_ignore_ascii_case(ext))
                && stem_path.with_extension(ext).is_file()
        })
}

fn specifier_matches_decl(
    spec: &str,
    caller_path: &Path,
    decl_path: &Path,
    decl_segs: &[&str],
    is_index: bool,
) -> ImportMatch {
    let raw = spec
        .trim()
        .trim_end_matches(';')
        .trim_end_matches(')')
        .trim();
    let unquoted = if let Some((_, rest)) = raw.split_once(['\'', '"', '<']) {
        rest.split(['\'', '"', '>']).next().unwrap_or(raw)
    } else {
        raw
    };
    let unquoted_trimmed = unquoted.trim();
    let is_dir_spec = unquoted_trimmed.ends_with('/')
        || unquoted_trimmed.ends_with('\\')
        || matches!(unquoted_trimmed, "." | "..");
    let trimmed_no_slash = unquoted_trimmed.trim_end_matches(['/', '\\']);
    let (path_str, explicit_ext) = if is_dir_spec {
        (trimmed_no_slash, None)
    } else {
        split_known_ext(trimmed_no_slash)
    };
    let decl_ext = decl_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if let Some(spec_ext) = explicit_ext
        && !ext_compatible(spec_ext, decl_ext)
    {
        return ImportMatch::None;
    }
    let caller_dir = caller_path.parent().unwrap_or_else(|| Path::new(""));

    if unquoted_trimmed.starts_with('.') {
        let resolved_segs = if !unquoted_trimmed.contains('/') && !unquoted_trimmed.contains('\\') {
            let dots = unquoted_trimmed.chars().take_while(|&c| c == '.').count();
            let mut segs = normalize_segments(caller_dir);
            for _ in 0..dots.saturating_sub(1) {
                segs.pop();
            }
            let (tail, _) = split_known_ext(&unquoted_trimmed[dots..]);
            for part in tail.split('.').filter(|s| !s.is_empty()) {
                segs.push(part.to_string());
            }
            segs
        } else {
            let mut joined = caller_dir.to_path_buf();
            for part in path_str.split(['/', '\\']) {
                match part {
                    "" | "." => {}
                    ".." => {
                        joined.pop();
                    }
                    c => joined.push(c),
                }
            }
            normalize_segments(&joined)
        };
        let decl_stem = decl_path.with_extension("");
        let decl_norm = normalize_segments(&decl_stem);
        if !is_dir_spec && resolved_segs == decl_norm {
            if let Some(spec_ext) = explicit_ext {
                if !spec_ext.eq_ignore_ascii_case(decl_ext)
                    && decl_path.with_extension(spec_ext).is_file()
                {
                    return ImportMatch::None;
                }
                return ImportMatch::Proven;
            }
            if has_competing_file_for_stem(&decl_stem, Some(decl_ext)) {
                return ImportMatch::Uncertain;
            }
            if matches!(decl_ext, "py" | "pyi") && decl_stem.join("__init__.py").is_file() {
                return ImportMatch::None;
            }
            return ImportMatch::Proven;
        }
        if is_index
            && explicit_ext.is_none()
            && let Some(parent) = decl_path.parent()
            && resolved_segs == normalize_segments(parent)
        {
            if !is_dir_spec && has_competing_file_for_stem(parent, None) {
                return if matches!(decl_ext, "py" | "pyi" | "rs") {
                    ImportMatch::Uncertain
                } else {
                    ImportMatch::None
                };
            }
            return ImportMatch::Proven;
        }
        return ImportMatch::None;
    }

    if !unquoted_trimmed.is_empty()
        && !is_dir_spec
        && caller_dir.join(unquoted_trimmed) == decl_path
    {
        return ImportMatch::Proven;
    }

    let sep: &[char] = if path_str.contains(['/', '\\']) {
        &['/', '\\']
    } else {
        &['.', ':']
    };
    let spec_segs: Vec<&str> = path_str
        .split(sep)
        .filter(|s| {
            !s.is_empty() && !matches!(*s, "." | ".." | "@" | "~" | "crate" | "super" | "self")
        })
        .collect();
    if spec_segs.is_empty()
        || (is_index
            && spec_segs.len() == 1
            && matches!(spec_segs[0], "index" | "mod" | "__init__"))
    {
        return ImportMatch::None;
    }
    if (!is_dir_spec && decl_segs.ends_with(&spec_segs))
        || (is_index
            && explicit_ext.is_none()
            && decl_segs.len() >= 2
            && decl_segs[..decl_segs.len() - 1].ends_with(&spec_segs))
    {
        ImportMatch::Uncertain
    } else {
        ImportMatch::None
    }
}

fn clause_binds_symbol(binding_part: &str, name: &str) -> bool {
    let mut trimmed = binding_part
        .trim()
        .trim_start_matches("import")
        .trim_start_matches("export")
        .trim();
    if let Some(rest) = trimmed.strip_prefix("type")
        && rest.starts_with(char::is_whitespace)
    {
        trimmed = rest.trim();
    }
    if trimmed.contains('*') {
        return true;
    }
    let item_matches = |item: &str| -> bool {
        let parts: Vec<&str> = item.split_whitespace().collect();
        let parts = if parts.first() == Some(&"type") {
            &parts[1..]
        } else {
            &parts[..]
        };
        matches!(parts, [orig] | [orig, "as", _] if *orig == name)
    };
    if let (Some(open), Some(close)) = (trimmed.find('{'), trimmed.rfind('}'))
        && open < close
    {
        return trimmed[open + 1..close].split(',').any(item_matches)
            || trimmed[..open].trim().trim_end_matches(',').trim() == name;
    }
    trimmed.split(',').any(item_matches)
}

pub(crate) fn imports_declaration(
    text: &str,
    caller_path: &Path,
    decl_path: &Path,
    name: &str,
) -> ImportMatch {
    let file_stem = decl_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if file_stem.is_empty() {
        return ImportMatch::None;
    }
    let is_index = matches!(file_stem, "index" | "mod" | "__init__");
    let without_ext = decl_path.with_extension("");
    let decl_segs: Vec<&str> = without_ext
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let check_spec =
        |spec: &str| specifier_matches_decl(spec, caller_path, decl_path, &decl_segs, is_index);
    let lines: Vec<&str> = text.lines().collect();
    let mut best = ImportMatch::None;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("#include ")
            || trimmed.starts_with("#include<")
            || trimmed.starts_with("#include\"")
        {
            best = best.merge(check_spec(trimmed.trim_start_matches("#include").trim()));
            i += 1;
            continue;
        }
        if trimmed.starts_with("use ") || trimmed.starts_with("pub use ") {
            let mut stmt = String::from(trimmed);
            while !stmt.contains(';') && i + 1 < lines.len() {
                i += 1;
                stmt.push(' ');
                stmt.push_str(lines[i].trim());
            }
            let target_stem = if is_index && decl_segs.len() >= 2 {
                decl_segs[decl_segs.len() - 2]
            } else {
                file_stem
            };
            if (crate::tools::symbols::names_word(&stmt, name) || stmt.contains('*'))
                && crate::tools::symbols::names_word(&stmt, target_stem)
            {
                best = best.merge(ImportMatch::Uncertain);
            }
            i += 1;
            continue;
        }
        if trimmed.starts_with("import ")
            || trimmed.starts_with("import{")
            || trimmed.starts_with("export ")
            || trimmed.starts_with("export{")
            || trimmed.starts_with("from ")
        {
            let mut stmt = String::from(trimmed);
            let mut j = i;
            if (trimmed.contains('{') && !trimmed.contains('}'))
                || (trimmed.starts_with("from ") && trimmed.ends_with('('))
            {
                while j + 1 < lines.len() && j - i < 25 {
                    j += 1;
                    let next = lines[j].trim();
                    stmt.push(' ');
                    stmt.push_str(next);
                    if next.contains(';')
                        || next.contains("from ")
                        || next.starts_with('}')
                        || next.ends_with(')')
                    {
                        break;
                    }
                }
            }
            let matched = if stmt.starts_with("from ") {
                stmt.trim_start_matches("from ")
                    .split_once(" import ")
                    .filter(|(_, b)| clause_binds_symbol(b.trim().trim_matches(['(', ')']), name))
                    .map_or(ImportMatch::None, |(s, _)| check_spec(s))
            } else if let Some((binding_part, spec_part)) = stmt
                .rsplit_once(" from ")
                .or_else(|| stmt.rsplit_once("}from "))
                .or_else(|| stmt.rsplit_once("}from'"))
                .or_else(|| stmt.rsplit_once("}from\""))
            {
                let binding_full = if binding_part.contains('{') && !binding_part.ends_with('}') {
                    format!("{binding_part}}}")
                } else {
                    binding_part.to_string()
                };
                if clause_binds_symbol(&binding_full, name) {
                    check_spec(spec_part)
                } else {
                    ImportMatch::None
                }
            } else {
                check_spec(stmt.trim_start_matches("import").trim())
            };
            best = best.merge(matched);
            i = j + 1;
            continue;
        }
        for call in ["require(", "import("] {
            if let Some((_, after)) = trimmed.split_once(call) {
                best = best.merge(check_spec(after));
            }
        }
        i += 1;
    }
    best
}

pub(crate) fn is_occurrence_shadowed(
    masked_code: &str,
    text_lines: &[&str],
    line_idx: usize,
    byte_offset: usize,
    name: &str,
) -> bool {
    if text_lines
        .get(line_idx)
        .is_some_and(|line| declared_at(line, name).is_some())
    {
        return true;
    }
    if masked_code[..byte_offset].trim_end().ends_with('.') {
        return true;
    }
    is_locally_shadowed(masked_code, byte_offset, name, Language::TypeScript)
        || body_has_local_decl(&masked_code[..byte_offset], name, Language::TypeScript)
}
