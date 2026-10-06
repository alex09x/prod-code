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

use anyhow::{Context, Result, bail};

use super::outline::range_lines;
use super::types::{BuilderPlan, CANARY_METHOD, Verification};

/// Puts `addition` into `text` right after 1-based line `after`, preceded by a newline.
/// Returns the new text and the 1-based line the addition begins on.
pub(crate) fn insert_after(text: &str, after: u32, addition: &str, newline: &str) -> (String, u32) {
    let mut out = String::with_capacity(text.len() + addition.len() + 2 * newline.len());
    let mut line = 1u32;
    for segment in text.split_inclusive('\n') {
        out.push_str(segment);
        if line == after {
            if !segment.ends_with('\n') {
                out.push_str(newline);
            }
            out.push_str(newline);
            out.push_str(addition);
            out.push_str(newline);
        }
        line += 1;
    }
    if line <= after {
        out.push_str(newline);
        out.push_str(addition);
        out.push_str(newline);
    }
    (out, after + 2)
}

enum Probe {
    Free,
    Taken(String, String),
    Blind(String),
}

pub(crate) async fn verify(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    file: &str,
    original: &str,
    plan: &BuilderPlan,
) -> Result<Verification> {
    match probe_names(remote, root, path, file, original, plan).await {
        Ok(Probe::Free) => {}
        Ok(Probe::Taken(name, place)) => bail!(
            "`{name}` already names {place} where the builder would be inserted (a glob import, the prelude or an extern crate); the builder would shadow it. Pass another `builder_name`"
        ),
        Ok(Probe::Blind(reason)) => return Ok(Verification::Unverified { reason }),
        Err(err) => {
            return Ok(Verification::Unverified {
                reason: format!(
                    "the analyzer gave no usable answer about which names are free: {err:#}"
                ),
            });
        }
    }
    let (ind, nl) = (&plan.indent, plan.newline);
    let canary = [
        format!("{ind}#[allow(dead_code)]"),
        format!("{ind}fn __prod_code_scope_canary() {{"),
        format!("{ind}    let _ = ().{CANARY_METHOD}();"),
        format!("{ind}}}"),
    ];
    let (probe, first) = insert_after(&plan.file_text, plan.code_lines.1, &canary.join(nl), nl);
    let canary_lines = first..=first + canary.len() as u32 - 1;
    let shift = probe.matches('\n').count() as u32 - plan.file_text.matches('\n').count() as u32;
    let reports =
        match crate::diagnostics::validate_texts(remote, root, &[(path.to_path_buf(), probe)], &[])
            .await
        {
            Ok(reports) => reports,
            Err(err) => {
                return Ok(Verification::Unverified {
                    reason: format!("the analyzer could not check the builder: {err:#}"),
                });
            }
        };
    let errors: Vec<_> = reports
        .iter()
        .flat_map(|r| r.items.iter())
        .filter(|d| d.severity == "error")
        .collect();
    let canary_seen = errors
        .iter()
        .any(|d| canary_lines.contains(&d.line) && d.message.contains(CANARY_METHOD));
    if !canary_seen {
        return Ok(Verification::Unverified {
            reason: format!(
                "the analyzer did not report a deliberate error placed next to the builder in {file}, so its silence about the builder proves nothing (an inactive `cfg`, a file outside the crate, or an engine still loading)"
            ),
        });
    }
    let diagnostics: Vec<String> = errors
        .iter()
        .filter(|d| !canary_lines.contains(&d.line))
        .map(|d| {
            let line = if d.line > *canary_lines.end() {
                d.line - shift
            } else {
                d.line
            };
            format!(
                "{}{} ({file}:{line}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.col
            )
        })
        .collect();
    Ok(if diagnostics.is_empty() {
        Verification::Clean
    } else {
        Verification::Rejected { diagnostics }
    })
}

/// Asks the analyzer what the struct's name and the builder's names resolve to at the insertion
/// point, in the file as it is: the struct's name must resolve to its declaration (or the
/// analyzer is not looking), the builder's must resolve to nothing.
async fn probe_names(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    file: &str,
    original: &str,
    plan: &BuilderPlan,
) -> Result<Probe> {
    let names = [
        ("__ProdCodeProbeTarget", plan.type_name.as_str()),
        ("__ProdCodeProbeBuilder", plan.builder_name.as_str()),
        ("__ProdCodeProbeError", plan.error_name.as_str()),
    ];
    let prefixes: Vec<String> = names
        .iter()
        .map(|(alias, _)| format!("{}#[allow(dead_code)] type {alias} = ", plan.indent))
        .collect();
    let snippet = names
        .iter()
        .zip(&prefixes)
        .map(|((_, name), prefix)| format!("{prefix}{name};"))
        .collect::<Vec<_>>()
        .join(plan.newline);
    let (text, first) = insert_after(original, plan.declaration_lines.1, &snippet, plan.newline);
    let mut session =
        crate::session::LspSession::open_for_validation(remote, root, Some(path)).await?;
    let uri = session.open_text(path, &text).await?;
    let mut answers = Vec::new();
    for (k, prefix) in prefixes.iter().enumerate() {
        let answer = session
            .request(
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": {
                        "line": first - 1 + k as u32,
                        "character": prefix.encode_utf16().count(),
                    },
                }),
            )
            .await;
        match answer {
            Ok(answer) => answers.push(answer),
            Err(err) => {
                session.close().await;
                return Err(err);
            }
        }
    }
    session.close().await;
    // A location that cannot be read is not the absence of one: the name could be taken.
    let resolved = names
        .iter()
        .zip(&answers)
        .map(|((_, name), answer)| {
            locations(answer)
                .with_context(|| format!("the answer for `{name}` at the insertion point"))
        })
        .collect::<Result<Vec<_>>>()?;
    let declared = resolved[0].iter().any(|(target, line)| {
        (target == path || target.ends_with(file))
            && (plan.declaration_lines.0..=plan.declaration_lines.1).contains(line)
    });
    if !declared {
        return Ok(Probe::Blind(format!(
            "the analyzer did not resolve `{}` to its declaration at the insertion point, so it cannot tell which names are free there (an engine still loading, or a scope it does not analyse)",
            plan.type_name
        )));
    }
    for ((_, name), targets) in names.iter().zip(&resolved).skip(1) {
        if let Some((target, line)) = targets.first() {
            let shown = target
                .strip_prefix(root)
                .unwrap_or(target)
                .to_string_lossy()
                .into_owned();
            return Ok(Probe::Taken(name.to_string(), format!("{shown}:{line}")));
        }
    }
    Ok(Probe::Free)
}

/// The files and 1-based lines a definition answer points at: none for `null` or `[]`, else a
/// `Location`, `Location[]` or `LocationLink[]`. Anything else, or an entry without a file URI
/// and a valid range, is an error rather than one location fewer.
pub(crate) fn locations(answer: &serde_json::Value) -> Result<Vec<(std::path::PathBuf, u32)>> {
    let items = match answer {
        serde_json::Value::Null => return Ok(Vec::new()),
        serde_json::Value::Array(items) => items.as_slice(),
        other => std::slice::from_ref(other),
    };
    items
        .iter()
        .map(|item| {
            let (uri, range) = match item.get("targetUri") {
                Some(uri) => (
                    Some(uri),
                    item.get("targetSelectionRange")
                        .or_else(|| item.get("targetRange")),
                ),
                None => (item.get("uri"), item.get("range")),
            };
            let path = uri
                .and_then(|u| u.as_str())
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.to_file_path().ok());
            match (path, range.and_then(range_lines)) {
                (Some(path), Some((line, _))) => Ok((path, line)),
                _ => bail!("malformed definition answer: {item}"),
            }
        })
        .collect()
}
