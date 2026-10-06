/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use super::syntax::{char_literal, find_bytes, line_end, split_args, token_before};
use super::types::Spelled;

pub fn spelled(ty: &str) -> Spelled {
    let ty = ty.trim();
    if ty.starts_with(['&', '*']) || ty.starts_with("fn(") || ty == "!" {
        return Spelled::Inert;
    }
    if let Some(inner) = ty.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
        return split_args(inner)
            .iter()
            .map(|t| spelled(t))
            .max_by_key(|s| match s {
                Spelled::Inert => 0,
                Spelled::Primitive => 1,
                Spelled::MayDrop => 2,
            })
            .unwrap_or(Spelled::Inert);
    }
    if let Some(inner) = ty.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        return spelled(inner.rsplit_once(';').map_or(inner, |(element, _)| element));
    }
    if matches!(
        ty,
        "bool"
            | "char"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
    ) {
        Spelled::Primitive
    } else {
        Spelled::MayDrop
    }
}

/// Where the parameter `name` is written in `raw`, its declaration (`mut n: u32`): the last
/// place before the colon where it stands as a whole name.
pub fn name_in(raw: &str, name: &str) -> Option<usize> {
    let head = &raw[..raw.find(':')?];
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    head.match_indices(name)
        .map(|(at, _)| at)
        .filter(|at| {
            !head[..*at].chars().next_back().is_some_and(ident)
                && !head[at + name.len()..].chars().next().is_some_and(ident)
        })
        .last()
}

/// Whether rust-analyzer's hover on the parameter `name` vouches that its type has no drop glue
/// (#441): `Ok` for `no Drop` on a type it resolved, otherwise why the type may have a
/// destructor for all the hover shows. A type it cannot resolve is `{unknown}`, and has
/// `no Drop` as well, which proves nothing.
pub fn no_drop_glue(hover: &str, name: &str) -> Result<(), String> {
    let mut lines = hover.lines().map(str::trim);
    let declared = lines
        .by_ref()
        .skip_while(|l| *l != "```rust")
        .nth(1)
        .ok_or_else(|| "the analyzer's hover does not show its type".to_string())?;
    let ty = declared
        .strip_prefix("mut ")
        .unwrap_or(declared)
        .strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(':'))
        .map(str::trim)
        .ok_or_else(|| format!("the analyzer's hover is about `{declared}`, not it"))?;
    if ty.contains("{unknown}") {
        return Err(format!("the analyzer does not resolve its type (`{ty}`)"));
    }
    match lines.find(|l| {
        matches!(
            *l,
            "no Drop" | "needs Drop" | "impl Drop" | "type param may need Drop"
        )
    }) {
        Some("no Drop") => Ok(()),
        Some(glue) => Err(format!("the analyzer reports `{glue}` for it")),
        None => Err("the analyzer's hover does not say whether its type has drop glue".into()),
    }
}

/// Asks the analyzer about the parameter `name` of `file`, written at byte `at` of `text`; see
/// [`no_drop_glue`]. A failed query or no hover vouches for nothing.
pub async fn drop_glue(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
    name: &str,
) -> Result<(), String> {
    let (line, col) = crate::signature::position_at(text, at).map_err(|e| e.to_string())?;
    let uri = url::Url::from_file_path(file)
        .map_err(|()| format!("{} has no file URI", file.display()))?
        .to_string();
    let res = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        }),
    )
    .await
    .map_err(|e| format!("the hover query failed: {e:#}"))?;
    let hover = res
        .pointer("/contents/value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "the analyzer gave no hover for it".to_string())?;
    no_drop_glue(hover, name)
}

/// The parameters that may run a destructor (`owned`), in the order a Rust function drops them
/// before bundling and after (#441). A function drops its parameters last to first, and the
/// struct, where the first bundled parameter was, drops its fields in the order `fields` declares
/// them. With `forward` it is the order a future of an `async` function drops them in when it is
/// dropped before it is polled: first to last, as the fields of the future.
pub fn drop_order(
    count: usize,
    bundled: &[usize],
    fields: &[usize],
    owned: &[bool],
    forward: bool,
) -> (Vec<usize>, Vec<usize>) {
    let first = bundled.first().copied().unwrap_or(0);
    let mut was: Vec<usize> = (0..count).collect();
    let mut units: Vec<Vec<usize>> = (0..count)
        .filter(|i| *i == first || !bundled.contains(i))
        .map(|i| if i == first { fields.to_vec() } else { vec![i] })
        .collect();
    if !forward {
        was.reverse();
        units.reverse();
    }
    let owned_only =
        |order: Vec<usize>| -> Vec<usize> { order.into_iter().filter(|i| owned[*i]).collect() };
    (owned_only(was), owned_only(units.concat()))
}

/// The refusal for a bundle that would drop the parameters in another order; see
/// [`drop_order`]. The names are listed already quoted; `unsure` says why each parameter
/// spelled as a primitive was taken for one that may have a destructor.
pub fn dropped_differently(
    callee: &str,
    name: &str,
    bundled: &str,
    was: &str,
    now: &str,
    forward: bool,
    unsure: &[String],
) -> anyhow::Error {
    let unsure: String = unsure.iter().map(|u| format!("; {u}")).collect();
    let (who, then, advice) = if forward {
        (
            format!("`{callee}` is `async`: a future dropped before it is polled drops"),
            ", and one that has run drops them the other way round, which the same fields \
             cannot keep as well",
            "Bundle at most one parameter whose type may have a destructor",
        )
    } else {
        (
            format!("`{callee}` drops"),
            "",
            "Bundle parameters that are next to each other, or apart only by references and \
             primitives",
        )
    };
    anyhow::anyhow!(
        "{who} the parameters that may have a destructor in the order {was}; with {bundled} in \
         `{name}` it would drop them in the order {now}{then}{unsure} (#441). {advice}; nothing \
         was rewritten"
    )
}

/// `code` (Rust source) with every comment and the inside of every string and character literal
/// made spaces, so that what is left is code at the same offsets. A string keeps its quotes.
/// Block comments nest, as in Rust, and `'a` is a lifetime rather than a character.
pub fn rust_code(code: &str) -> Vec<u8> {
    let mut out = code.as_bytes().to_vec();
    let bytes = code.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let mut blank = |from: usize, to: usize| out[from..to].fill(b' ');
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        if rest.starts_with(b"//") {
            let end = line_end(bytes, i);
            blank(i, end);
            i = end;
        } else if rest.starts_with(b"/*") {
            let (mut j, mut depth) = (i + 2, 1);
            while j < bytes.len() && depth > 0 {
                if bytes[j..].starts_with(b"/*") {
                    depth += 1;
                    j += 2;
                } else if bytes[j..].starts_with(b"*/") {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            let j = j.min(bytes.len());
            blank(i, j);
            i = j;
        } else if bytes[i] == b'r'
            && (i == 0
                || !word(bytes[i - 1])
                || (matches!(bytes[i - 1], b'b' | b'c') && (i == 1 || !word(bytes[i - 2]))))
            && rest[1..].iter().find(|b| **b != b'#') == Some(&b'"')
        {
            // A raw string: no escapes, and it ends at a quote with as many `#` as it began.
            let hashes = rest[1..].iter().take_while(|b| **b == b'#').count();
            let open = i + 1 + hashes;
            let close: Vec<u8> = std::iter::once(b'"')
                .chain(std::iter::repeat_n(b'#', hashes))
                .collect();
            let end = find_bytes(&bytes[open + 1..], &close).map_or(bytes.len(), |n| open + 1 + n);
            blank(i + 1, open);
            blank(open + 1, end);
            if end < bytes.len() {
                blank(end + 1, end + close.len());
            }
            i = (end + close.len()).min(bytes.len());
        } else if bytes[i] == b'"' || (bytes[i] == b'\'' && char_literal(code, i)) {
            let quote = bytes[i];
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != quote {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let j = j.min(bytes.len());
            blank(i + 1, j);
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Whether the Rust function whose name starts at `name` is declared `async`: its qualifiers are
/// read back from `fn` over line breaks and comments (`pub(crate)\nasync /* … */ fn`), and an
/// `async` in a comment or a string is not one. `None` where no `fn` comes before the name.
pub fn declared_async(text: &str, name: usize) -> Option<bool> {
    let code = rust_code(&text[..name]);
    let mut end = code.len();
    if token_before(&code, &mut end)? != b"fn" {
        return None;
    }
    while let Some(token) = token_before(&code, &mut end) {
        match token {
            b"async" => return Some(true),
            b"const" | b"unsafe" | b"safe" | b"extern" | b"default" => {}
            t if t.first() == Some(&b'"') => {}
            _ => break,
        }
    }
    Some(false)
}

/// The manifest in `dir`, parsed; `None` where there is none.
pub fn manifest_in(dir: &Path) -> Option<Result<toml::Table, String>> {
    let path = dir.join("Cargo.toml");
    if !path.is_file() {
        return None;
    }
    Some(
        std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))
            .and_then(|text| {
                text.parse::<toml::Table>()
                    .map_err(|e| format!("{} is not valid TOML: {}", path.display(), e.message()))
            }),
    )
}

/// The edition of the crate `file` belongs to, as Cargo reads it: the `package.edition` of the
/// nearest manifest, 2015 where it has none, and with `edition.workspace = true` the
/// `workspace.package.edition` of the workspace that `package.workspace` names or, without it,
/// of the nearest manifest from the crate's directory up that has a `[workspace]`.
pub fn rust_edition(file: &Path) -> Result<u32, String> {
    let (dir, manifest) = file
        .ancestors()
        .skip(1)
        .find_map(|dir| Some((dir, manifest_in(dir)?)))
        .ok_or_else(|| format!("no Cargo.toml in a directory above {}", file.display()))?;
    let path = dir.join("Cargo.toml");
    let manifest = manifest?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{} has no [package]", path.display()))?;
    let (edition, from) = match package.get("edition") {
        None => return Ok(2015),
        Some(toml::Value::Table(inherit)) => {
            if inherit.len() != 1
                || inherit.get("workspace").and_then(toml::Value::as_bool) != Some(true)
            {
                return Err(format!(
                    "`package.edition` in {} is neither a string nor `{{ workspace = true }}`",
                    path.display()
                ));
            }
            let root = match package.get("workspace") {
                Some(toml::Value::String(to)) => dir.join(to),
                Some(_) => {
                    return Err(format!(
                        "`package.workspace` in {} is not a path",
                        path.display()
                    ));
                }
                None => {
                    let mut found = None;
                    for up in dir.ancestors() {
                        match manifest_in(up) {
                            Some(Ok(m))
                                if m.get("workspace").is_some_and(toml::Value::is_table) =>
                            {
                                found = Some(up.to_path_buf());
                                break;
                            }
                            Some(Err(e)) => return Err(e),
                            _ => {}
                        }
                    }
                    found.ok_or_else(|| {
                        format!(
                            "{} inherits `edition` from its workspace, and no Cargo.toml from its \
                             directory up has a [workspace]",
                            path.display()
                        )
                    })?
                }
            };
            let root_path = root.join("Cargo.toml");
            let workspace = manifest_in(&root)
                .unwrap_or_else(|| Err(format!("there is no {}", root_path.display())))?;
            let edition = workspace
                .get("workspace")
                .and_then(|w| w.get("package"))
                .and_then(|p| p.get("edition"))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "{} inherits `edition` from {}, which has no `workspace.package.edition`",
                        path.display(),
                        root_path.display()
                    )
                })?;
            (edition, root_path)
        }
        Some(edition) => (edition.clone(), path),
    };
    match edition.as_str() {
        Some("2015") => Ok(2015),
        Some("2018") => Ok(2018),
        Some("2021") => Ok(2021),
        Some("2024") => Ok(2024),
        _ => Err(format!(
            "the edition {edition} in {} is not one this check knows",
            from.display()
        )),
    }
}
