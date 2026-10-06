/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Determines whether a symbol is a root entry point based on name, language conventions,
/// file path, and export status.
pub fn is_root_entry_point(
    language: &str,
    rel_path: &str,
    name: &str,
    kind: &str,
    exported: bool,
    include_exported: bool,
) -> (bool, Option<String>) {
    let bare = name.split('(').next().unwrap_or(name);

    // 1. Universal entry point function names
    if matches!(bare, "main" | "init") {
        return (true, Some(format!("entry-point function `{bare}`")));
    }

    // 2. Trait and interface implementations (dynamic dispatch candidates)
    if kind == "trait-method" || (kind == "method" && language != "rust") {
        return (
            true,
            Some("trait or interface implementation (dynamic dispatch)".to_string()),
        );
    }

    // 3. Known lifecycle / standard methods
    if matches!(
        bare,
        "new" | "default" | "drop" | "fmt" | "eq" | "hash" | "clone"
    ) || bare.starts_with("__")
    {
        return (
            true,
            Some(format!("standard lifecycle/protocol hook `{bare}`")),
        );
    }

    // 4. Test entry points
    if bare.starts_with("test") || bare.starts_with("Test") {
        return (true, Some(format!("test entry point `{bare}`")));
    }

    // 5. Entry script conventions (for module/script entry points in dynamic languages)
    let lower_path = rel_path.to_ascii_lowercase();
    let is_entry_script = match language {
        "python" => lower_path.ends_with("/__main__.py") || lower_path == "__main__.py",
        "typescript" | "javascript" => {
            (lower_path == "index.ts"
                || lower_path == "index.js"
                || lower_path == "main.ts"
                || lower_path == "main.js"
                || lower_path == "server.ts"
                || lower_path == "server.js")
                && exported
        }
        _ => false,
    };

    if is_entry_script {
        return (true, Some(format!("entry script item in `{rel_path}`")));
    }

    // 6. Library mode: when `include_exported == false`, all exported items are library entry points
    if exported && !include_exported {
        return (
            true,
            Some("exported public API (library root entry point)".to_string()),
        );
    }

    (false, None)
}
