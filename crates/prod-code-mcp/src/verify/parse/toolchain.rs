/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What rustup says when it cannot prepare the toolchain a project asks for.

use crate::verify::types::Diagnostic;

/// Where the repair of a build node's toolchain is written down.
const TOOLCHAIN_DOC: &str =
    "https://github.com/alex09x/prod-code/blob/main/docs/build-node-toolchains.md";

/// A note for rustup's `detected conflict` while it installs a component a `rust-toolchain.toml`
/// asks for (#2029). The command never starts, and the line reads as if two components of the
/// project could not be installed together. They can: the file rustup stops at is one the
/// toolchain on the node already holds without any installed component owning it, left by an
/// install that was interrupted or ran beside another one. Every later install of that component
/// stops at it until the file is removed on the node.
pub fn toolchain_notes(stderr: &str) -> Vec<Diagnostic> {
    stderr
        .lines()
        .find_map(component_conflict)
        .map(|(component, file)| Diagnostic {
            level: "note".to_string(),
            code: None,
            message: format!(
                "the command did not run: rustup could not install `{component}` on the build \
                 node, because the toolchain there already holds `{file}`. That is state left on \
                 the node by an install that was interrupted or ran beside another one, not a \
                 conflict between the components this project asks for; removing the files no \
                 installed component owns lets the install finish. Steps: {TOOLCHAIN_DOC}"
            ),
            file: None,
            line: None,
            column: None,
        })
        .into_iter()
        .collect()
}

/// The component and the file of rustup's
/// `error: failed to install component: '<component>', detected conflict: '<file>'`.
fn component_conflict(line: &str) -> Option<(&str, &str)> {
    let rest = line
        .trim()
        .strip_prefix("error: failed to install component: '")?;
    let (component, rest) = rest.split_once("', detected conflict: '")?;
    Some((component, rest.strip_suffix('\'')?))
}

#[cfg(test)]
mod tests {
    use crate::verify::detect::detect_tools;
    use crate::verify::parse::parse_verification_output;
    use crate::verify::types::VerifyKind;

    const CONFLICT: &str = "info: downloading component llvm-tools\n\
        info: rolling back changes\n\
        error: failed to install component: 'llvm-tools-preview-x86_64-unknown-linux-gnu', \
        detected conflict: 'lib/rustlib/x86_64-unknown-linux-gnu/lib/libLLVM-22-rust-1.98.1-stable.so'\n";

    #[test]
    fn a_component_rustup_cannot_install_is_explained_as_node_state() {
        let checkout = tempfile::tempdir().unwrap();
        let tools = detect_tools(checkout.path());
        for kind in [VerifyKind::Test, VerifyKind::Check, VerifyKind::Lint] {
            let (diagnostics, ..) = parse_verification_output("rust", kind, "", CONFLICT, &tools);
            let notes: Vec<_> = diagnostics.iter().filter(|d| d.level == "note").collect();
            assert_eq!(notes.len(), 1, "{kind:?}: {diagnostics:?}");
            let note = notes[0].render();
            assert!(
                note.contains("`llvm-tools-preview-x86_64-unknown-linux-gnu`")
                    && note.contains(
                        "`lib/rustlib/x86_64-unknown-linux-gnu/lib/libLLVM-22-rust-1.98.1-stable.so`"
                    )
                    && note.contains("not a conflict between the components")
                    && note.contains("docs/build-node-toolchains.md"),
                "{note}"
            );
        }
    }

    #[test]
    fn other_failures_get_no_toolchain_note() {
        let checkout = tempfile::tempdir().unwrap();
        let tools = detect_tools(checkout.path());
        for stderr in [
            "error: could not compile `demo` (lib) due to 1 previous error\n",
            "error: failed to install component: 'rust-src', no space left on device\n",
            "error: toolchain '1.98.1-x86_64-unknown-linux-gnu' is not installed\n",
        ] {
            let (diagnostics, ..) =
                parse_verification_output("rust", VerifyKind::Test, "", stderr, &tools);
            assert!(
                diagnostics.iter().all(|d| d.level != "note"),
                "{diagnostics:?}"
            );
        }
        // Another language's output is not rustup's.
        let (diagnostics, ..) =
            parse_verification_output("go", VerifyKind::Test, "", CONFLICT, &tools);
        assert!(diagnostics.iter().all(|d| d.level != "note"));
    }
}
