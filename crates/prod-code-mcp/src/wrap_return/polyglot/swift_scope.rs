/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

pub(super) enum Membership {
    Same,
    Different,
    Unknown,
}

/// Reuse the standard SwiftPM target layout facility, anchored to the nearest package.
/// A directory is not a module. Custom source layouts require semantic ownership;
/// never execute a manifest merely to obtain fallback caller coverage.
fn target(root: &Path, file: &Path) -> Option<(PathBuf, PathBuf, String)> {
    let package = file
        .parent()?
        .ancestors()
        .take_while(|p| p.starts_with(root))
        .find(|p| p.join("Package.swift").is_file())?;
    let manifest = std::fs::read_to_string(package.join("Package.swift")).ok()?;
    let custom = regex::Regex::new(r"\b(path|sources|exclude)\s*:").ok()?;
    if custom.is_match(&manifest) {
        return None;
    }
    let relative = file.strip_prefix(package).ok()?;
    let name = crate::diagnostics::swift_target_name(relative)?;
    let mut parts = relative.components();
    let bucket = parts.next()?.as_os_str();
    if bucket != "Sources" && bucket != "Tests" {
        return None;
    }
    let directory = package.join(bucket).join(&name);
    if !file.starts_with(&directory) {
        return None;
    }
    Some((package.to_path_buf(), directory, name))
}

pub(super) fn membership(root: &Path, caller: &Path, selected: &Path, content: &str) -> Membership {
    let (Some((package, directory, name)), Some((other_package, other_directory, _))) =
        (target(root, selected), target(root, caller))
    else {
        return Membership::Unknown;
    };
    if package == other_package && directory == other_directory {
        return Membership::Same;
    }
    let mask = crate::caller_migration::mask::lexical_code_mask(
        content,
        crate::parameter_object::Language::Swift,
    );
    let code = String::from_utf8(
        content
            .bytes()
            .zip(mask)
            .map(|(b, yes)| if yes { b } else { b' ' })
            .collect(),
    )
    .expect("mask preserves UTF-8 code");
    // Keep every unrecognized token so unsupported import spelling remains
    // unknown, rather than silently proving that a dependent target is unrelated.
    let token = regex::Regex::new(r"[\p{L}_][\p{L}\p{M}\p{N}_]*|[^\s]").expect("import tokens");
    let ident = regex::Regex::new(r"^[\p{L}_][\p{L}\p{M}\p{N}_]*$").expect("swift identifier");
    let words: Vec<_> = token.find_iter(&code).map(|m| m.as_str()).collect();
    if words.iter().enumerate().any(|(i, word)| {
        if *word != "import" {
            return false;
        }
        let next = i
            + 1
            + usize::from(words.get(i + 1).is_some_and(|w| {
                matches!(
                    *w,
                    "typealias" | "struct" | "class" | "enum" | "protocol" | "let" | "var" | "func"
                )
            }));
        match words.get(next) {
            Some(module) if *module == name => true,
            Some(module) if ident.is_match(module) => false,
            _ => true,
        }
    }) {
        Membership::Unknown
    } else {
        Membership::Different
    }
}

pub(super) fn admit(
    files: (&Path, &Path, &Path),
    lang: crate::parameter_object::Language,
    source: (&str, &str),
    semantic: bool,
    unmatched: &mut Vec<String>,
) -> bool {
    let (root, caller, selected) = files;
    let (content, name) = source;
    if lang != crate::parameter_object::Language::Swift || semantic || !content.contains(name) {
        return true;
    }
    match membership(root, caller, selected, content) {
        Membership::Same => true,
        Membership::Different => false,
        Membership::Unknown => {
            unmatched.push(format!(
                "{}: Swift target/module ownership is unknown",
                crate::wrap_return::utils::display(root, caller)
            ));
            false
        }
    }
}

pub(super) fn same_target(caller: &Path, selected: &Path, content: &str) -> bool {
    let Some(root) = selected.ancestors().last() else {
        return false;
    };
    matches!(
        membership(root, caller, selected, content),
        Membership::Same
    )
}
