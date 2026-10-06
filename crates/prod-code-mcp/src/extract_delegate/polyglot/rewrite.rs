/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::go::rewrite_go_literals;
use crate::extract_delegate::common::is_ident;
use crate::parameter_object::Language;

pub fn rewrite_external_file(
    code: &str,
    lang: Language,
    owner: &str,
    field: &str,
    fields: &[String],
    helper: &str,
) -> (String, usize) {
    let code_transformed;
    let code = if lang == Language::Go {
        code_transformed = rewrite_go_literals(code, owner, fields, field, helper);
        &code_transformed
    } else {
        code
    };
    let mut out = String::new();
    let mut accesses = 0;

    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with('#')
        {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if trimmed.starts_with("import ")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("package ")
        {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let mut current_line = line.to_string();
        for f in fields {
            let needle = format!(".{f}");
            if current_line.contains(&needle) {
                let mut new_line = String::new();
                let mut rest = current_line.as_str();
                while let Some(pos) = rest.find(&needle) {
                    let before = &rest[..pos];
                    let after = &rest[pos + needle.len()..];
                    let before_trimmed = before.trim_end();
                    let is_inside_helper = before_trimmed.ends_with(helper)
                        || (before_trimmed.ends_with("this")
                            && current_line.contains(&format!("class {helper}")))
                        || (before_trimmed.ends_with("self")
                            && current_line.contains(&format!("class {helper}")));
                    let is_method_call = after.trim_start().starts_with('(');
                    let is_ident_continuation = after.chars().next().is_some_and(is_ident);

                    if is_inside_helper || is_method_call || is_ident_continuation {
                        new_line.push_str(&rest[..pos + needle.len()]);
                        rest = after;
                    } else {
                        new_line.push_str(before);
                        new_line.push_str(&format!(".{field}.{f}"));
                        accesses += 1;
                        rest = after;
                    }
                }
                new_line.push_str(rest);
                current_line = new_line;
            }

            if lang == Language::Cpp || lang == Language::C {
                let arrow_needle = format!("->{f}");
                if current_line.contains(&arrow_needle) {
                    let mut new_line = String::new();
                    let mut rest = current_line.as_str();
                    while let Some(pos) = rest.find(&arrow_needle) {
                        let before = &rest[..pos];
                        let after = &rest[pos + arrow_needle.len()..];
                        let is_method_call = after.trim_start().starts_with('(');
                        let is_ident_continuation = after.chars().next().is_some_and(is_ident);

                        if is_method_call || is_ident_continuation {
                            new_line.push_str(&rest[..pos + arrow_needle.len()]);
                            rest = after;
                        } else {
                            new_line.push_str(before);
                            new_line.push_str(&format!("->{field}.{f}"));
                            accesses += 1;
                            rest = after;
                        }
                    }
                    new_line.push_str(rest);
                    current_line = new_line;
                }
            }
        }

        out.push_str(&current_line);
        out.push('\n');
    }

    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, accesses)
}

pub fn owner_region<'a>(text: &'a str, lang: Language, owner: &str) -> Option<&'a str> {
    if lang == Language::Go {
        return Some(text);
    }
    let markers = match lang {
        Language::TypeScript | Language::JavaScript | Language::Python => {
            vec![format!("class {owner}")]
        }
        Language::Cpp | Language::C => {
            vec![format!("class {owner}"), format!("struct {owner}")]
        }
        Language::Swift => vec![
            format!("class {owner}"),
            format!("struct {owner}"),
            format!("actor {owner}"),
        ],
        Language::Go => return Some(text),
        Language::Rust | Language::Java => return None,
    };
    let start = markers
        .iter()
        .filter_map(|marker| text.rfind(marker))
        .max()?;
    if lang == Language::Python {
        return Some(&text[start..]);
    }
    let open = start + text[start..].find('{')?;
    let close = crate::parameter_object::matching_bracket(text, open)?;
    Some(&text[open + 1..close])
}
