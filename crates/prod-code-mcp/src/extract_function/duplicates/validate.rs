/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::extract_function::binding::read_after_but_not_returned;
use crate::extract_function::rewrite::{
    apply_edits, errors_in, indent_at, rename_placeholder, with_arguments,
};
use crate::extract_function::tokens::Token;
use crate::extract_function::types::{Duplicate, MAX_DUPLICATES, Occurrence, PLACEHOLDER, Rewrite};

type Target = (PathBuf, usize, usize, String, String);

#[allow(clippy::too_many_arguments)]
pub async fn process_and_validate_duplicates(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    name: &str,
    extracted: &str,
    selection: &str,
    sel_tokens: &[(Token, usize, usize)],
    start: usize,
    end: usize,
    rewrite: Option<&Rewrite>,
    call: Option<String>,
    call_indent: &str,
    copies: &[(PathBuf, String, Occurrence)],
    varying: &[usize],
    own_literals: &[String],
    parameters: &mut Vec<(String, String)>,
    untyped: Option<&str>,
    parameterized: bool,
    base_edits: &[(usize, usize, String)],
    other_files: bool,
    function: Option<(usize, usize)>,
) -> Result<(
    Vec<Duplicate>,
    Vec<(PathBuf, String)>,
    Vec<String>,
    Option<String>,
)> {
    let home = if other_files {
        Some(crate::move_item::module_of(file)?.1)
    } else {
        None
    };
    let is_method = call
        .as_deref()
        .is_some_and(|c| c.contains("self.") || c.contains("Self::"));

    let call_for = |literals: &[String]| -> Option<String> {
        let c = call.as_deref()?;
        if parameters.is_empty() {
            Some(c.to_string())
        } else {
            with_arguments(c, literals)
        }
    };

    let mut found: Vec<(Duplicate, Option<Target>)> = Vec::new();
    for (n, (path, path_text, c)) in copies.iter().enumerate() {
        let shown = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        let line = crate::signature::position_at(path_text, c.from)?.0;
        let literals: Vec<String> = varying
            .iter()
            .map(|k| {
                c.differs
                    .iter()
                    .find(|(dk, _)| dk == k)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_else(|| selection[sel_tokens[*k].1..sel_tokens[*k].2].to_string())
            })
            .collect();
        let same_file = path == file;
        let place = if same_file {
            rewrite
                .and_then(|r| r.mapped(start, end, c.from, c.to))
                .map(|at| (at, at + (c.to - c.from)))
        } else {
            Some((c.from, c.to))
        };
        let qualify = |t: String| match &home {
            Some(home) if !same_file => {
                let (_, module) = crate::move_item::module_of(path)
                    .unwrap_or_else(|_| (PathBuf::new(), home.clone()));
                t.replace(
                    &format!("{PLACEHOLDER}("),
                    &format!("{}::{PLACEHOLDER}(", home.spelled_from(&module.krate)),
                )
            }
            _ => t,
        };
        let plain_call = call.clone().map(qualify);
        let param_call = call_for(&literals).map(qualify);
        let reason = if n >= MAX_DUPLICATES {
            Some(format!("not tried: only the first {MAX_DUPLICATES} are"))
        } else if call.is_none() {
            Some(
                "the extraction changed code around the selection too, so its call does not stand on its own"
                    .to_string(),
            )
        } else if !c.differs.is_empty() && !parameterized {
            Some(match untyped {
                Some(lit) => {
                    format!("it differs in literals, and the type of `{lit}` is not known")
                }
                None => "it differs in literals the new function could not take".to_string(),
            })
        } else if !same_file && is_method {
            Some("the new function is a method; another file cannot call it through `self`".into())
        } else if place.is_none() {
            Some("the extraction itself changed this code".to_string())
        } else {
            call.as_deref()
                .and_then(|c_str| read_after_but_not_returned(path_text, selection, c_str, c.to))
                .map(|name| {
                    format!(
                        "the code after it reads `{name}`, which the new function does not \
                         return; with the call there, `{name}` would be whatever it is before \
                         it, or nothing"
                    )
                })
        };
        let target = match (reason.is_none(), place, plain_call, param_call) {
            (true, Some((from, to)), Some(plain), Some(with)) => {
                Some((path.clone(), from, to, plain, with))
            }
            _ => None,
        };
        found.push((
            Duplicate {
                file: shown,
                line,
                replaced: false,
                reason,
                passes: if c.differs.is_empty() {
                    Vec::new()
                } else {
                    literals
                },
            },
            target,
        ));
    }

    let build = |base: &[(usize, usize, String)],
                 accepted: &[(PathBuf, usize, usize, String)]|
     -> Vec<(PathBuf, String)> {
        let mut main_edits = base.to_vec();
        let mut others: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
        for (path, from, to, call_text) in accepted {
            let here = if path == file {
                indent_at(extracted, *from).to_string()
            } else {
                indent_at(&std::fs::read_to_string(path).unwrap_or_default(), *from).to_string()
            };
            let placed = call_text
                .replace(&format!("\n{call_indent}"), &format!("\n{here}"))
                .trim()
                .to_string();
            if path == file {
                main_edits.push((*from, *to, placed));
            } else {
                others
                    .entry(path.clone())
                    .or_default()
                    .push((*from, *to, placed));
            }
        }
        if !others.is_empty()
            && let Some((fs, _)) = function
            && let Some(def) = extracted[fs..].find(&format!("fn {PLACEHOLDER}"))
            && !extracted[fs..fs + def].contains("pub")
        {
            main_edits.push((fs + def, fs + def, "pub(crate) ".to_string()));
        }
        let mut files = vec![(
            file.to_path_buf(),
            rename_placeholder(&apply_edits(extracted, &main_edits), name),
        )];
        for (path, edits) in others {
            let original = std::fs::read_to_string(&path).unwrap_or_default();
            files.push((
                path,
                rename_placeholder(&apply_edits(&original, &edits), name),
            ));
        }
        files
    };

    let mut accepted: Vec<(PathBuf, usize, usize, String)> = Vec::new();
    let mut result = build(&[], &accepted);
    let mut diagnostics = errors_in(remote, root, &result).await?;
    let base_clean = diagnostics.is_empty();
    let mut kept: Vec<usize> = Vec::new();
    for (n, (duplicate, target)) in found.iter_mut().enumerate() {
        let Some((path, from, to, plain, _)) = target.clone() else {
            continue;
        };
        if !duplicate.passes.is_empty() {
            continue;
        }
        if !base_clean {
            duplicate.reason = Some("not tried: the extraction itself does not type-check".into());
            continue;
        }
        let mut trial = accepted.clone();
        trial.push((path, from, to, plain));
        let candidate = build(&[], &trial);
        let found_errors = errors_in(remote, root, &candidate).await?;
        if found_errors.is_empty() {
            accepted = trial;
            result = candidate;
            duplicate.replaced = true;
            kept.push(n);
        } else {
            duplicate.reason = Some(format!(
                "with the call there the result does not type-check: {}",
                found_errors[0]
            ));
        }
    }
    let near: Vec<usize> = found
        .iter()
        .enumerate()
        .filter(|(_, (d, t))| !d.passes.is_empty() && t.is_some())
        .map(|(n, _)| n)
        .collect();
    let mut with_parameters = false;
    if base_clean && parameterized && !parameters.is_empty() && !near.is_empty() {
        let mut accepted_p: Vec<(PathBuf, usize, usize, String)> = kept
            .iter()
            .filter_map(|n| found[*n].1.clone())
            .map(|(p, f, t, _, with)| (p, f, t, with))
            .collect();
        let mut result_p = build(base_edits, &accepted_p);
        if errors_in(remote, root, &result_p).await?.is_empty() {
            for n in near {
                let Some((path, from, to, _, with)) = found[n].1.clone() else {
                    continue;
                };
                let mut trial = accepted_p.clone();
                trial.push((path, from, to, with));
                let candidate = build(base_edits, &trial);
                let found_errors = errors_in(remote, root, &candidate).await?;
                if found_errors.is_empty() {
                    accepted_p = trial;
                    result_p = candidate;
                    found[n].0.replaced = true;
                    with_parameters = true;
                } else {
                    found[n].0.reason = Some(format!(
                        "with the call there the result does not type-check: {}",
                        found_errors[0]
                    ));
                }
            }
        }
        if with_parameters {
            result = result_p;
        }
    }
    let final_call = if with_parameters {
        call_for(own_literals)
    } else {
        call
    };
    if !with_parameters {
        parameters.clear();
    }
    if found.iter().any(|(d, _)| d.replaced) {
        diagnostics = Vec::new();
    }

    Ok((
        found.into_iter().map(|(d, _)| d).collect(),
        result,
        diagnostics,
        final_call,
    ))
}
