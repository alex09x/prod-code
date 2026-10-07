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

use super::extract_specifier;

fn c_cpp_stem(file: &str) -> &str {
    file.strip_suffix(".h")
        .or_else(|| file.strip_suffix(".hpp"))
        .or_else(|| file.strip_suffix(".hxx"))
        .or_else(|| file.strip_suffix(".cpp"))
        .or_else(|| file.strip_suffix(".cc"))
        .or_else(|| file.strip_suffix(".cxx"))
        .or_else(|| file.strip_suffix(".c"))
        .unwrap_or(file)
}

pub(crate) fn c_cpp_proves_import(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
) -> bool {
    let decl_stem = decl_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let decl_name = decl_file.file_name().and_then(|s| s.to_str()).unwrap_or("");

    if caller_path.file_stem().and_then(|s| s.to_str()) == Some(decl_stem) {
        return true;
    }

    if content.lines().any(|l| {
        let trimmed = l.trim();
        if !trimmed.starts_with("#include") {
            return false;
        }
        let spec = extract_specifier(trimmed);
        let spec_file = spec.rsplit('/').next().unwrap_or(spec);
        let spec_stem = c_cpp_stem(spec_file);
        spec_file == decl_name || spec_stem == decl_stem
    }) {
        return true;
    }

    if let Ok(decl_content) = std::fs::read_to_string(decl_file) {
        let caller_name = caller_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let caller_stem = caller_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        let decl_headers: Vec<&str> = decl_content
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("#include"))
            .map(extract_specifier)
            .filter(|s| !s.is_empty())
            .collect();

        for h in &decl_headers {
            let h_file = h.rsplit('/').next().unwrap_or(h);
            let h_stem = c_cpp_stem(h_file);
            if (caller_name == h_file || caller_stem == h_stem) && content.contains(fn_name) {
                return true;
            }
        }

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("#include") {
                let spec = extract_specifier(trimmed);
                let spec_file = spec.rsplit('/').next().unwrap_or(spec);
                let spec_stem = c_cpp_stem(spec_file);
                for h in &decl_headers {
                    let h_file = h.rsplit('/').next().unwrap_or(h);
                    let h_stem = c_cpp_stem(h_file);
                    if spec == *h || spec_file == h_file || spec_stem == h_stem {
                        return true;
                    }
                }
            }
        }
    }

    false
}
