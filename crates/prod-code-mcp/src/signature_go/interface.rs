/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::parse::{ident_at, identifier_uses};
use crate::signature_go::shadowing::package_name;
use crate::signature_go::syntax::{canonical, go_identifier, is_ident};
use crate::signature_go::text::{closing, is_ident_byte, skip_opaque, skip_space};
use crate::signature_go::types::Receiver;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// A receiver method satisfying an interface cannot be extended without changing that interface
/// too. The reference response does not distinguish the interface-typed selector from a concrete
/// one, so a current interface declaration of the same method makes the whole plan uncertain.
pub(crate) fn declares_interface_method(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    for at in identifier_uses(text, 0, text.len(), "interface") {
        let open = skip_space(text, at + "interface".len());
        if bytes.get(open) != Some(&b'{') {
            continue;
        }
        let Some(close) = closing(text, open) else {
            continue;
        };
        let mut cursor = open + 1;
        while cursor < close {
            cursor = skip_space(text, cursor);
            if cursor >= close {
                break;
            }
            if let Some(end) = skip_opaque(bytes, cursor) {
                cursor = end;
            } else if matches!(bytes[cursor], b'(' | b'[' | b'{') {
                cursor = closing(text, cursor).map_or(close, |end| end + 1);
            } else if let Some(word) = ident_at(text, cursor) {
                let after = skip_space(text, cursor + word.len());
                if word == name && bytes.get(after) == Some(&b'(') {
                    return true;
                }
                cursor += word.len();
            } else {
                cursor += 1;
            }
        }
    }
    false
}

/// An interface obligation need not be reported by gopls as a reference to a concrete method.
/// Scan the checkout before extending a receiver method so an omitted interface declaration
/// cannot let an interface dispatch compile only after a destructive write.
pub(crate) fn interface_method_file(root: &Path, name: &str) -> Result<Option<PathBuf>> {
    fn visit(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
        for entry in std::fs::read_dir(dir).with_context(|| {
            format!(
                "cannot read {} while checking interface obligations",
                dir.display()
            )
        })? {
            let entry = entry.with_context(|| {
                format!(
                    "cannot inspect {} while checking interface obligations",
                    dir.display()
                )
            })?;
            let path = entry.path();
            let kind = entry.file_type().with_context(|| {
                format!(
                    "cannot inspect {} while checking interface obligations",
                    path.display()
                )
            })?;
            if kind.is_dir() {
                if entry.file_name() != ".git"
                    && let Some(found) = visit(&path, name)?
                {
                    return Ok(Some(found));
                }
            } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "go")
            {
                let text = std::fs::read_to_string(&path).with_context(|| {
                    format!(
                        "cannot read {} while checking interface obligations",
                        path.display()
                    )
                })?;
                if declares_interface_method(&text, name) {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }
    visit(root, name)
}

/// An unexported method name has package identity. Safe deletion therefore scans only regular Go
/// files in the declaring package for local interface obligations; an interface with the same
/// spelling in a different package is unrelated. Signature changes retain the broader recursive
/// scan above because narrowing that established planner is outside this helper's contract.
pub(crate) fn package_interface_method_file(file: &Path, name: &str) -> Result<Option<PathBuf>> {
    let directory = file
        .parent()
        .with_context(|| format!("{} has no containing package directory", file.display()))?;
    let declaration_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let package = package_name(&declaration_text)
        .with_context(|| format!("cannot identify the Go package in {}", file.display()))?;
    for entry in std::fs::read_dir(directory).with_context(|| {
        format!(
            "cannot inspect {} while checking package interface obligations",
            directory.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "cannot inspect an entry in {} while checking package interface obligations",
                directory.display()
            )
        })?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "go") {
            continue;
        }
        let kind = entry.file_type().with_context(|| {
            format!(
                "cannot inspect {} while checking package interface obligations",
                path.display()
            )
        })?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "linked Go source {} cannot be inspected for package interface obligations",
            path.display()
        );
        if !kind.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "cannot read {} while checking package interface obligations",
                path.display()
            )
        })?;
        if package_name(&text) == Some(package) && declares_interface_method(&text, name) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// The use of a receiver method must be a selector call. An interface member declaration and a
/// method value have no dot before the name; a selector whose receiver spells the receiver type
/// is a method expression. The latter is rejected even when a local happens to use that name:
/// no spelling-only check can prove that it is a value rather than a type.
pub(crate) fn receiver_selector_call(
    text: &str,
    name_at: usize,
    receiver_type: &str,
) -> Result<()> {
    let mut dot = name_at;
    while dot > 0 && matches!(text.as_bytes()[dot - 1], b' ' | b'\t') {
        dot -= 1;
    }
    anyhow::ensure!(
        dot > 0 && text.as_bytes()[dot - 1] == b'.',
        "the method name is not preceded by a selector"
    );
    let before = text[..dot - 1].trim_end();
    let receiver_type = receiver_type.trim_start_matches('*');
    let bare_type = before
        .strip_suffix(')')
        .and_then(|before| before.strip_suffix(receiver_type))
        .and_then(|before| before.strip_suffix("(*"))
        .is_some()
        || before == receiver_type
        || before.strip_suffix(receiver_type).is_some_and(|prefix| {
            prefix.is_empty()
                || !is_ident_byte(prefix.as_bytes().last().copied().unwrap_or_default())
        });
    anyhow::ensure!(
        !bare_type,
        "the selector receiver can be the receiver type `{receiver_type}`, a method expression"
    );
    Ok(())
}

/// A receiver is safe for insertion only when it binds one ordinary value name to one named type
/// or pointer-to-named-type. Parameterized receivers and receiver aliases need type information
/// beyond the source proof this adapter has, so they remain refused.
pub(crate) fn ordinary_receiver(receiver: &str) -> Result<Receiver> {
    let receiver = canonical(receiver);
    let binding_end = receiver
        .bytes()
        .position(|byte| !is_ident_byte(byte))
        .unwrap_or(receiver.len());
    let binding = &receiver[..binding_end];
    anyhow::ensure!(
        go_identifier(binding),
        "the receiver does not bind one ordinary name"
    );
    let ty = receiver[binding_end..].trim();
    let named = ty.strip_prefix('*').unwrap_or(ty);
    anyhow::ensure!(
        is_ident(named) && !named.is_empty(),
        "the receiver type `{}` is not an ordinary named value or pointer type",
        ty
    );
    Ok(Receiver {
        binding: binding.to_string(),
        ty: ty.to_string(),
    })
}
