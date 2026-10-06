/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::scan::find_caller_migrations;
use super::types::CallerMigration;
use crate::move_polyglot::{insert_or_merge_py_import, insert_or_merge_ts_import, is_symbol_used};
use crate::parameter_object::Language;
use crate::signature_polyglot::collect_workspace_sources;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Migrates caller type annotations in a single file text.
pub fn migrate_caller_annotations(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
) -> (String, Vec<CallerMigration>) {
    let mut candidates =
        find_caller_migrations(text, type_name, interface_name, extracted_methods, lang);
    if candidates.is_empty() {
        return (text.to_string(), Vec::new());
    }

    // Sort descending by start offset to apply edits without shifting offsets
    candidates.sort_by_key(|c| std::cmp::Reverse(c.start));

    let mut out = text.to_string();
    let mut migrations = Vec::new();

    for c in candidates {
        out.replace_range(c.start..c.end, &c.replacement);
        migrations.push(c.migration);
    }

    migrations.reverse();
    (out, migrations)
}

/// Migrates caller type annotations across the declaring file and external workspace files.
pub fn migrate_callers_in_workspace(
    root: &Path,
    declaring_file: &Path,
    declaring_text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
) -> Result<Vec<(PathBuf, String)>> {
    let mut results = Vec::new();

    // 1. Declaring file
    let (migrated_declaring, _) = migrate_caller_annotations(
        declaring_text,
        type_name,
        interface_name,
        extracted_methods,
        lang,
    );
    results.push((declaring_file.to_path_buf(), migrated_declaring));

    // 2. Other workspace files
    let sources = collect_workspace_sources(root, lang);
    for other in sources {
        if other == *declaring_file {
            continue;
        }
        let Ok(other_text) = std::fs::read_to_string(&other) else {
            continue;
        };
        if !is_symbol_used(&other_text, type_name) {
            continue;
        }

        let (mut new_other_text, migrations) = migrate_caller_annotations(
            &other_text,
            type_name,
            interface_name,
            extracted_methods,
            lang,
        );

        if !migrations.is_empty() {
            // Add appropriate imports
            match lang {
                Language::TypeScript | Language::JavaScript => {
                    let rel =
                        crate::move_polyglot::relative_import_specifier(&other, declaring_file);
                    let (with_import, _) =
                        insert_or_merge_ts_import(&new_other_text, interface_name, &rel);
                    new_other_text = with_import;
                }
                Language::Python => {
                    let mod_spec =
                        crate::move_polyglot::python_module_specifier(&other, declaring_file, root);
                    let (with_import, _) =
                        insert_or_merge_py_import(&new_other_text, interface_name, &mod_spec);
                    new_other_text = with_import;
                }
                Language::Cpp | Language::C => {
                    let header_name = declaring_file
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if !header_name.is_empty() && !new_other_text.contains(&header_name) {
                        new_other_text.insert_str(0, &format!("#include \"{header_name}\"\n"));
                    }
                }
                Language::Rust => {
                    // Handled via module spelled_from
                }
                _ => {}
            }

            results.push((other, new_other_text));
        }
    }

    Ok(results)
}
