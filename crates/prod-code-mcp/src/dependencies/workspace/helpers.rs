/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub fn extract_maven_artifact_id(pom_content: &str) -> Option<String> {
    let search_content = if let Some(parent_end) = pom_content.find("</parent>") {
        &pom_content[parent_end + "</parent>".len()..]
    } else {
        pom_content
    };
    if let Some(start) = search_content.find("<artifactId>") {
        let after_start = &search_content[start + "<artifactId>".len()..];
        if let Some(end) = after_start.find("</artifactId>") {
            return Some(after_start[..end].trim().to_string());
        }
    }
    None
}

pub fn extract_gradle_project_block(root_content: &str, project_name: &str) -> Option<String> {
    let alt_name = project_name.replace(':', "-");
    let last_name = project_name.split(':').next_back().unwrap_or(project_name);
    let patterns = [
        format!("project(':{project_name}')"),
        format!("project(\":{project_name}\")"),
        format!("project('{project_name}')"),
        format!("project(\"{project_name}\")"),
        format!("project(':{alt_name}')"),
        format!("project(\":{alt_name}\")"),
        format!("project(':{last_name}')"),
        format!("project(\":{last_name}\")"),
    ];

    let mut start_idx = None;
    for pat in &patterns {
        if let Some(pos) = root_content.find(pat) {
            start_idx = Some(pos + pat.len());
            break;
        }
    }

    let start_search = start_idx?;
    let brace_offset = root_content[start_search..].find('{')?;
    let brace_start = start_search + brace_offset;

    let mut depth = 0;
    let mut end_idx = None;
    for (i, c) in root_content[brace_start..].char_indices() {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                end_idx = Some(brace_start + i + 1);
                break;
            }
        }
    }

    end_idx.map(|end| root_content[brace_start..end].to_string())
}

/// Helper to convert kebab-case or snake_case identifiers to camelCase (for Gradle Type-Safe Project Accessors)
pub fn kebab_to_camel(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;
    for c in s.chars() {
        if c == '-' || c == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }
    result
}
