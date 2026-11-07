/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// The refusal a failed `textDocument/rename` answers with. A bare `method not found` told an
/// agent nothing about what to do (#1965); for Swift, where the server renames only what it has a
/// build and an index of, the text names the two things that can be done. It does not claim to
/// know why the server answered so.
pub(super) fn rename_refusal(is_swift: bool, error: &str) -> String {
    if is_swift && error.contains("timed out") {
        return format!(
            "rename refused: {error} (sourcekit-lsp did not complete textDocument/rename within budget; ensure the target builds cleanly or wait for background indexing to finish)"
        );
    }
    if is_swift && error.contains("method not found") {
        return format!(
            "rename refused: {error} (sourcekit-lsp does not offer a rename for this file; it renames what it has a build and an index of, so build the target on the node (`prod-code check`) and ask again, check the server's version (`prod-code status`), or list the uses with code_references and change them yourself)"
        );
    }
    format!("rename refused: {error}")
}

#[cfg(test)]
mod tests {
    use super::rename_refusal;

    #[test]
    fn a_swift_method_not_found_names_what_can_be_done() {
        let text = rename_refusal(
            true,
            "textDocument/rename failed: method not found: textDocument/rename",
        );
        assert!(text.starts_with("rename refused: textDocument/rename failed: method not found"));
        assert!(text.contains("prod-code check"), "{text}");
        assert!(text.contains("code_references"), "{text}");
    }

    #[test]
    fn a_swift_timeout_keeps_its_own_advice() {
        let text = rename_refusal(true, "timeout waiting for textDocument/rename timed out");
        assert!(text.contains("wait for background indexing"), "{text}");
        assert!(!text.contains("does not offer a rename"), "{text}");
    }

    #[test]
    fn other_languages_and_other_errors_are_passed_on_as_they_were() {
        assert_eq!(
            rename_refusal(false, "method not found: textDocument/rename"),
            "rename refused: method not found: textDocument/rename"
        );
        assert_eq!(
            rename_refusal(true, "invalid new name"),
            "rename refused: invalid new name"
        );
    }
}
