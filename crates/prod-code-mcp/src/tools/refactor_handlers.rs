/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Handlers for AST-guided refactoring tools.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use url::Url;

use super::{
    compile_gate, execute_lsp_query, refuse_incomplete, representative_source_file,
    resolve_file_path, rewritten_files,
};
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_change_signature(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let specs = args
        .get("params")
        .and_then(|v| v.as_array())
        .context("Missing 'params' argument: the parameter list the function should end up with")?;
    let mut params = Vec::with_capacity(specs.len());
    for spec in specs {
        let spec = spec
            .as_str()
            .context("every entry of `params` is a string: `name`, or `name: Type = expression`")?;
        params.push(crate::signature::parse_param(spec)?);
    }
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    // The compile gate runs `cargo check` on a preview and writes it on the compiler's word
    // alone, past the Go adapter's own refusals of a gopls edit that is not the signature change
    // asked for. Go gets no such gate: `verify` is refused before anything is planned.
    if file_path.extension().is_some_and(|e| e == "go")
        && let Some(asked) = args.get("verify").filter(|v| !v.is_null())
    {
        anyhow::bail!(
            "`verify: {asked}` is not supported for Go: it runs `cargo check`, which does not \
             build Go, and nothing was written. A Go signature change is already type-checked with every \
             package that uses it and refused unless gopls's edit is exactly the requested \
             parameter list; omit `verify`"
        );
    }
    anyhow::ensure!(
        !verify || ext == "rs",
        "verify: compile for change_signature is supported only for Rust; the gateway compile gate runs cargo check and nothing was written"
    );
    let modifiers = crate::signature::Modifiers {
        returns: args
            .get("returns")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        visibility: args
            .get("visibility")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        asyncness: args.get("async").and_then(|v| v.as_bool()),
    };
    let mut change = crate::signature::change_with(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &params,
        &modifiers,
        apply && !verify,
        force,
    )
    .await?;
    // With `verify` the planner ran as a dry run and the compile gate writes; a reorder that
    // compiles can still run differently through a reference it did not rewrite (#446).
    if apply {
        change.ensure_writable(force)?;
    }
    let gate = if verify {
        let files = change.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                change.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        change.applied = true;
    }
    let clean = change.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = change.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_move(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: the target module's file")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let target = resolve_file_path(workspace_root, to);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    anyhow::ensure!(
        !verify || ext == "rs",
        "`verify: compile` runs `cargo check` and is for Rust files; the analyzer's check of \
         the result is reported without it"
    );
    let mut moved = crate::move_item::move_item(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &target,
        apply && !verify,
        force,
    )
    .await?;
    refuse_incomplete(apply, &moved.unmatched)?;
    let gate = if verify {
        let files = moved.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                moved.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        moved.applied = true;
    }
    let clean = moved.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = moved.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_introduce_parameter_object(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument: what the new struct is called")?;
    let specs = args
        .get("params")
        .and_then(|v| v.as_array())
        .context("Missing 'params' argument: the parameters to bundle, by name")?;
    let mut params = Vec::with_capacity(specs.len());
    for spec in specs {
        params.push(
            spec.as_str()
                .context("every entry of `params` is a parameter name")?
                .to_string(),
        );
    }
    let file_path = resolve_file_path(workspace_root, path_str);
    let binding = args
        .get("binding")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| crate::parameter_object::default_binding(&file_path, name));
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    // The compile gate is `cargo check`; it has nothing to say about another language, and a
    // pass from it would be read as a verdict on files it never compiled.
    anyhow::ensure!(
        !verify
            || crate::parameter_object::Language::of(&file_path)
                == Some(crate::parameter_object::Language::Rust),
        "`verify: compile` runs `cargo check`, which judges Rust only; {} is checked by its \
         language server's diagnostics alone",
        path_str
    );
    let mut done = crate::parameter_object::introduce(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &params,
        name,
        &binding,
        apply && !verify,
        force,
    )
    .await?;
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.unmatched.is_empty()
        && done.diagnostics.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_extract_delegate(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let symbol = args
        .get("symbol")
        .and_then(|v| v.as_str())
        .or_else(|| args.get("class").and_then(|v| v.as_str()))
        .or_else(|| args.get("type").and_then(|v| v.as_str()));
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);

    if symbol.is_none() && line.is_none() {
        anyhow::bail!(
            "Specify either `symbol` (or `class`/`type`) or `line` and `character` for the class/struct"
        );
    }

    let list = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let text = |key: &str| -> Result<String> {
        args.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let fields = list("fields");
    anyhow::ensure!(!fields.is_empty(), "Missing or empty 'fields' argument");
    let methods = list("methods");
    let helper_name = text("name")?;
    let field_name = text("field")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");

    let file_path = resolve_file_path(workspace_root, path_str);
    anyhow::ensure!(
        !verify
            || crate::parameter_object::Language::of(&file_path)
                == Some(crate::parameter_object::Language::Rust),
        "`verify: compile` runs `cargo check`, which judges Rust only; {} is checked by its \
         language server's diagnostics alone",
        path_str
    );

    let mut done = crate::extract_delegate::extract_delegate_polyglot(
        remote,
        workspace_root,
        &file_path,
        symbol,
        line,
        character,
        &fields,
        &methods,
        &helper_name,
        &field_name,
        apply && !verify,
        force,
        None,
    )
    .await?;

    let gate = if verify {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };

    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.unmatched.is_empty()
        && done.diagnostics.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut out = done.render();
    if let Some(gate) = &gate {
        out.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(out)
    } else {
        McpToolCallResult::error(out)
    })
}

pub(crate) async fn handle_extract_parameter(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument: what the new parameter is called")?;
    let ty = args.get("type").and_then(|v| v.as_str());
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    // The compile gate runs `cargo check`, which says nothing about a TypeScript, Python or Go
    // file; letting it pass one would claim a verdict nobody gave.
    anyhow::ensure!(
        !verify
            || crate::extract_parameter::Syntax::of(&file_path)
                == Some(crate::extract_parameter::Syntax::Rust),
        "`verify: compile` runs `cargo check` and is for Rust files; the analyzer's check of \
         the result is reported without it"
    );
    let mut done = crate::extract_parameter::extract(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        ty,
        replace_all,
        apply && !verify,
        force,
    )
    .await?;
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_extract_field(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let file_path = resolve_file_path(workspace_root, path_str);
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let end_line = args
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let end_character = args
        .get("end_character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let range = args.get("range").and_then(|v| v.as_str());
    let expr_arg = args.get("expression").and_then(|v| v.as_str());

    let (start_pos, end_pos) = if let (Some(l), Some(c), Some(el), Some(ec)) =
        (line, character, end_line, end_character)
    {
        ((l, c), (el, ec))
    } else if let Some(r) = range {
        let (s, e) = if let Some(pair) = r.split_once("..") {
            pair
        } else {
            r.split_once('-')
                .context("Invalid 'range' format, expected LINE:COL-LINE:COL")?
        };
        let parse_pos = |p: &str| -> Result<(u32, u32)> {
            let (l, c) = p.split_once(':').context("Expected LINE:COL")?;
            Ok((l.trim().parse()?, c.trim().parse()?))
        };
        (parse_pos(s)?, parse_pos(e)?)
    } else if let Some(expr) = expr_arg {
        let content = std::fs::read_to_string(&file_path)
            .with_context(|| format!("cannot read {}", file_path.display()))?;
        let pos = content
            .find(expr)
            .with_context(|| format!("expression `{expr}` not found in {}", file_path.display()))?;
        let (sl, sc) = crate::signature::position_at(&content, pos)?;
        let (el, ec) = crate::signature::position_at(&content, pos + expr.len())?;
        ((sl, sc), (el, ec))
    } else {
        anyhow::bail!(
            "Missing selection: provide 'line', 'character', 'end_line', 'end_character', or 'range', or 'expression'"
        );
    };

    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument: what the new field is called")?;
    let ty = args.get("type").and_then(|v| v.as_str());
    let init = args.get("init").and_then(|v| v.as_str());
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";
    anyhow::ensure!(
        !verify || is_rust,
        "verify: compile is only supported for Rust code_extract_field; no files were written"
    );

    let mut done = if is_rust {
        crate::extract_field::extract(
            remote,
            workspace_root,
            &file_path,
            start_pos,
            end_pos,
            name,
            ty,
            init,
            replace_all,
            apply && !verify,
            force,
        )
        .await?
    } else {
        crate::extract_field::extract_polyglot(
            remote,
            workspace_root,
            &file_path,
            start_pos,
            end_pos,
            name,
            ty,
            init,
            replace_all,
            apply && !verify,
            force,
        )
        .await?
    };
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && done.blocked.is_empty() {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_generify(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .or_else(|| args.get("file"))
        .and_then(|v| v.as_str());
    let symbol = args
        .get("symbol")
        .or_else(|| args.get("function"))
        .and_then(|v| v.as_str());
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .or_else(|| args.get("col"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let param = args
        .get("param")
        .or_else(|| args.get("parameter"))
        .and_then(|v| v.as_str())
        .context("Missing 'param' argument: the parameter to make generic")?;
    let bound = args
        .get("bound")
        .or_else(|| args.get("trait"))
        .or_else(|| args.get("constraint"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let type_param = args
        .get("type_param")
        .or_else(|| args.get("as"))
        .and_then(|v| v.as_str())
        .unwrap_or("T");
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut declaration_line = line;

    let file_path = if let Some(p) = path_str {
        resolve_file_path(workspace_root, p)
    } else if let Some(sym) = symbol {
        let wanted = sym
            .rsplit_once("::")
            .map(|(_, name)| name)
            .or_else(|| sym.rsplit_once('.').map(|(_, name)| name))
            .unwrap_or(sym);
        let supported_source = |path: &Path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some(
                    "rs" | "ts"
                        | "tsx"
                        | "js"
                        | "jsx"
                        | "py"
                        | "cpp"
                        | "cc"
                        | "cxx"
                        | "h"
                        | "hpp"
                        | "c"
                        | "swift"
                        | "go"
                        | "java"
                )
            )
        };
        let candidates: std::collections::BTreeSet<(PathBuf, u32, u32)> =
            crate::tools::workspace_symbol_search(remote, workspace_root, sym, None, 100)
                .await?
                .into_iter()
                .filter(|hit| {
                    let name = hit
                        .name
                        .rsplit_once("::")
                        .map(|(_, name)| name)
                        .or_else(|| hit.name.rsplit_once('.').map(|(_, name)| name))
                        .unwrap_or(&hit.name);
                    name.eq_ignore_ascii_case(wanted) && supported_source(&hit.path)
                })
                .map(|hit| {
                    (
                        std::fs::canonicalize(&hit.path).unwrap_or(hit.path),
                        hit.line,
                        hit.col,
                    )
                })
                .collect();
        match candidates.len() {
            0 => anyhow::bail!("no supported declaration for symbol {sym} was found"),
            1 => {
                let (path, found_line, _) = candidates.into_iter().next().unwrap();
                declaration_line = Some(found_line);
                path
            }
            _ => anyhow::bail!(
                "multiple declarations for symbol {sym} were found; pass a path to select one"
            ),
        }
    } else {
        anyhow::bail!("Missing 'path' or 'symbol' argument");
    };

    let done = crate::generify::generify_polyglot(
        remote,
        workspace_root,
        &file_path,
        symbol,
        declaration_line,
        character,
        param,
        bound,
        type_param,
        apply,
        force,
    )
    .await?;

    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else if force {
        McpToolCallResult::text(format!("{text}\n[forced: applied with diagnostics]"))
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_invert_boolean(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let symbol = args
        .get("symbol")
        .or_else(|| args.get("function"))
        .and_then(|v| v.as_str());
    let new_name = args
        .get("new_name")
        .and_then(|v| v.as_str())
        .context("Missing 'new_name' argument: the name of the inverted predicate")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";
    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust invert_boolean")?;
        let c = character.context("Missing 'character' argument for Rust invert_boolean")?;
        crate::invert_boolean::invert(
            remote,
            workspace_root,
            &file_path,
            l,
            c,
            new_name,
            apply && !verify,
            force,
        )
        .await?
    } else {
        crate::invert_boolean::invert_polyglot(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            symbol,
            new_name,
            apply && !verify,
            force,
        )
        .await?
    };
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && (done.unmatched.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_make_static(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";

    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust make_static")?;
        let c = character.unwrap_or(1);
        crate::make_static::make_static(
            remote,
            workspace_root,
            &file_path,
            l,
            c,
            apply && !verify,
            force,
        )
        .await?
    } else {
        let symbol = args.get("symbol").and_then(|v| v.as_str());
        let method_arg = args
            .get("method")
            .or_else(|| args.get("method_name"))
            .and_then(|v| v.as_str());
        let class_arg = args
            .get("class_name")
            .or_else(|| args.get("struct_name"))
            .and_then(|v| v.as_str());

        let (resolved_class, resolved_method) = if let Some(m) = method_arg {
            (class_arg.map(str::to_string), m.to_string())
        } else if let Some(s) = symbol {
            if let Some((cls, mth)) = s.split_once("::").or_else(|| s.split_once('.')) {
                (Some(cls.to_string()), mth.to_string())
            } else {
                (class_arg.map(str::to_string), s.to_string())
            }
        } else if let Some(l) = line {
            let text = std::fs::read_to_string(&file_path)?;
            let (mth, cls) = crate::make_static::find_method_at_line(&text, l)
                .context("Could not find method at given line")?;
            (class_arg.map(str::to_string).or(cls), mth)
        } else {
            anyhow::bail!("Missing 'method', 'symbol', or line position");
        };

        crate::make_static::make_static_polyglot(
            remote,
            workspace_root,
            &file_path,
            resolved_class.as_deref(),
            &resolved_method,
            apply && !verify,
            force,
        )
        .await?
    };

    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && done.blocked.is_empty() {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_convert_to_method(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";

    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust convert_to_method")?;
        let c = character.unwrap_or(1);
        crate::to_method::convert_to_method(
            remote,
            workspace_root,
            &file_path,
            l,
            c,
            apply && !verify,
            force,
        )
        .await?
    } else {
        let symbol = args.get("symbol").and_then(|v| v.as_str());
        let method_arg = args
            .get("method")
            .or_else(|| args.get("method_name"))
            .and_then(|v| v.as_str());
        let class_arg = args
            .get("class_name")
            .or_else(|| args.get("struct_name"))
            .and_then(|v| v.as_str());

        let (resolved_class, resolved_method) = if let Some(m) = method_arg {
            (class_arg.map(str::to_string), m.to_string())
        } else if let Some(s) = symbol {
            if let Some((cls, mth)) = s.split_once("::").or_else(|| s.split_once('.')) {
                (Some(cls.to_string()), mth.to_string())
            } else {
                (class_arg.map(str::to_string), s.to_string())
            }
        } else if let Some(l) = line {
            let text = std::fs::read_to_string(&file_path)?;
            let (mth, cls) = crate::make_static::find_method_at_line(&text, l)
                .context("Could not find method at given line")?;
            (class_arg.map(str::to_string).or(cls), mth)
        } else {
            anyhow::bail!("Missing 'method', 'symbol', or line position");
        };

        crate::to_method::convert_to_method_polyglot(
            remote,
            workspace_root,
            &file_path,
            resolved_class.as_deref(),
            &resolved_method,
            apply && !verify,
            force,
        )
        .await?
    };

    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && (done.unmatched.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_inline_parameter(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let function = args
        .get("function")
        .or_else(|| args.get("symbol"))
        .and_then(|v| v.as_str());
    let param = args
        .get("parameter")
        .or_else(|| args.get("param"))
        .and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";
    anyhow::ensure!(
        !verify || is_rust,
        "verify: compile is only supported for Rust inline_parameter; no files were written"
    );

    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust inline_parameter")?;
        let c = character.unwrap_or(1);
        crate::inline_parameter::inline_parameter(
            remote,
            workspace_root,
            &file_path,
            l,
            c,
            apply && !verify,
            force,
        )
        .await?
    } else {
        crate::inline_parameter::inline_parameter_polyglot(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            function,
            param,
            apply && !verify,
            force,
        )
        .await?
    };

    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && (done.unmatched.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.unmatched.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_extract_trait(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let methods: Vec<String> = args
        .get("methods")
        .and_then(|v| v.as_array())
        .context("Missing 'methods' argument")?
        .iter()
        .filter_map(|m| m.as_str().map(str::to_string))
        .collect();
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let migrate_callers = args
        .get("migrate_callers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::extract_trait::extract_trait_ext(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        &methods,
        name,
        migrate_callers,
        apply,
        force,
    )
    .await?;
    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_replace_constructor_with_factory(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let type_name = args
        .get("type_name")
        .and_then(|v| v.as_str())
        .context("Missing 'type_name' argument")?;
    let factory_name = args.get("factory_name").and_then(|v| v.as_str());
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::replace_constructor::replace_constructor_with_factory(
        remote,
        workspace_root,
        &file_path,
        type_name,
        factory_name,
        apply,
        force,
        verify,
    )
    .await?;
    let text = done.render(2048);
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_replace_constructor_with_builder(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let type_name = args
        .get("type_name")
        .and_then(|v| v.as_str())
        .context("Missing 'type_name' argument")?;
    let builder_name = args.get("builder_name").and_then(|v| v.as_str());
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::replace_constructor::replace_constructor_with_builder(
        remote,
        workspace_root,
        &file_path,
        type_name,
        builder_name,
        apply,
        force,
        verify,
    )
    .await?;
    let text = done.render(2048);
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_pull_up(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let class_name = args
        .get("class_name")
        .or_else(|| args.get("symbol"))
        .and_then(|v| v.as_str())
        .context("Missing 'class_name' (or `symbol`) argument")?;
    let members: Vec<String> = args
        .get("members")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let target_class = args.get("target_class").and_then(|v| v.as_str());
    let clean_siblings = args
        .get("clean_siblings")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);

    let done = crate::pull_push::pull_up_impl(
        remote,
        workspace_root,
        &file_path,
        class_name,
        target_class,
        &members,
        clean_siblings,
        apply,
        force,
        verify,
    )
    .await?;

    let text = done.render(2048);
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_push_down(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let class_name = args
        .get("class_name")
        .or_else(|| args.get("symbol"))
        .and_then(|v| v.as_str())
        .context("Missing 'class_name' (or `symbol`) argument")?;
    let members: Vec<String> = args
        .get("members")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let target_classes: Option<Vec<String>> = args
        .get("target_classes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        });
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);

    let done = crate::pull_push::push_down_impl(
        remote,
        workspace_root,
        &file_path,
        class_name,
        target_classes.as_deref(),
        &members,
        apply,
        force,
        verify,
    )
    .await?;

    let text = done.render(2048);
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_replace_inheritance_with_delegation(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let sub_type = args
        .get("sub_type")
        .or_else(|| args.get("symbol"))
        .or_else(|| args.get("class_name"))
        .and_then(|v| v.as_str())
        .context("Missing 'sub_type' (or `symbol`) argument")?;
    let base_type = args.get("base_type").and_then(|v| v.as_str());
    let field_name = args.get("field_name").and_then(|v| v.as_str());
    let methods: Option<Vec<String>> = args.get("methods").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect()
    });
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);

    let done = crate::replace_inheritance::replace_inheritance_impl(
        remote,
        workspace_root,
        &file_path,
        sub_type,
        base_type,
        field_name,
        methods.as_deref(),
        apply,
        force,
        verify,
    )
    .await?;

    let text = done.render(2048);
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_replace_conditional_with_polymorphism(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let base_name = args
        .get("base_name")
        .and_then(|v| v.as_str())
        .context("Missing 'base_name' argument")?;
    let method_name = args
        .get("method_name")
        .and_then(|v| v.as_str())
        .context("Missing 'method_name' argument")?;
    let line = args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let col = args.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let params: Vec<String> = args
        .get("params")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let return_type = args.get("return_type").and_then(|v| v.as_str());
    let target_var = args.get("target_var").and_then(|v| v.as_str());
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);

    let done = crate::replace_conditional::replace_conditional_impl(
        remote,
        workspace_root,
        &file_path,
        line,
        col,
        base_name,
        method_name,
        &params,
        return_type,
        target_var,
        apply,
        force,
        verify,
    )
    .await?;

    let text = done.render(2048);
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_extract_interface(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let symbol = args
        .get("symbol")
        .or_else(|| args.get("type_name"))
        .and_then(|v| v.as_str())
        .context("Missing 'symbol' argument")?;
    let interface_name = args
        .get("interface_name")
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .context("Missing 'interface_name' argument")?;
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let file_path = resolve_file_path(workspace_root, path_str);
    let methods: Vec<String> = args
        .get("methods")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let line = args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let col = args.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if file_path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
        anyhow::ensure!(
            line > 0 && col > 0,
            "Rust extract_interface requires one-based line and character positions"
        );
    }
    let migrate_callers = args
        .get("migrate_callers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    let done = crate::extract_interface::extract_interface_impl(
        remote,
        workspace_root,
        &file_path,
        symbol,
        interface_name,
        &methods,
        line,
        col,
        migrate_callers,
        apply,
        force,
        verify,
    )
    .await?;

    let text = done.render(2048);
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_move_method(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file = resolve_file_path(workspace_root, path_str);
    let (line, character) = (num("line")?, num("character")?);
    let done = match (
        args.get("to_param").and_then(|v| v.as_str()),
        args.get("to_type").and_then(|v| v.as_str()),
    ) {
        (Some(to_param), None) => {
            crate::move_method::move_method(
                remote,
                workspace_root,
                &file,
                line,
                character,
                to_param,
                apply,
                force,
            )
            .await?
        }
        (None, Some(to_type)) => {
            crate::move_method::move_associated_function(
                remote,
                workspace_root,
                &file,
                line,
                character,
                to_type,
                apply,
                force,
            )
            .await?
        }
        _ => anyhow::bail!(
            "give `to_param` (a method: the parameter whose type it moves to) or `to_type` (an \
             associated function: the type it moves to), one of them"
        ),
    };
    let text = done.render(8000);
    let clean = done.diagnostics.is_empty() && done.blocked.is_empty() && done.unmatched.is_empty();
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_move_module(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument: the module's file")?;
    let to_str = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: where the module's file goes")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let mut done = crate::move_module::move_module(
        remote,
        workspace_root,
        &resolve_file_path(workspace_root, path_str),
        &resolve_file_path(workspace_root, to_str),
    )
    .await?;
    let gate = if verify {
        Some(
            compile_gate(
                remote,
                workspace_root,
                &done.rewritten,
                done.diagnostics.is_empty(),
                false,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    let compiles = gate.as_ref().is_none_or(|g| g.passed);
    // The gate only judges: the move also deletes the files it left, which only `write` does.
    if apply && (compiles || force) {
        done.write(force)?;
    }
    let clean = done.diagnostics.is_empty() && compiles;
    let mut text = done.render(8000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
        if apply && !compiles && !force {
            text.push_str("\nnothing was written: the compiler rejects it. Pass `force: true` to write it anyway.\n");
        }
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_extract_function(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let duplicates = args
        .get("duplicates")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let flag = |key: &str| args.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
    let apply = flag("apply");
    let force = flag("force");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::extract_function::extract_function(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        duplicates,
        flag("parameterize"),
        flag("other_files"),
    )
    .await?;
    // rust-analyzer does not check borrows: a duplicate whose call moves a value the code after
    // it still uses type-checks and does not compile. So the compiler sees any such result.
    let is_rust = file_path.extension().and_then(|e| e.to_str()) == Some("rs");
    let verify = (args.get("verify").and_then(|v| v.as_str()) == Some("compile")
        || done.replaced() > 0)
        && is_rust;
    let gate = if verify {
        Some(
            compile_gate(
                remote,
                workspace_root,
                &done.rewritten,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        if apply {
            done.write(force)?;
        }
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render();
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_introduce_variable(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::introduce_variable::introduce_variable(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        apply,
        force,
    )
    .await?;
    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_wrap_return(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .or_else(|| args.get("file"))
        .and_then(|v| v.as_str());
    let symbol = args
        .get("symbol")
        .or_else(|| args.get("function"))
        .and_then(|v| v.as_str());
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .or_else(|| args.get("col"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let wrapper = crate::wrap_return::Wrapper::parse(
        args.get("wrapper")
            .and_then(|v| v.as_str())
            .context("Missing 'wrapper' argument: `option`, `result`, `promise`, `pointer`, or custom envelope")?,
    )?;
    let constructor = args.get("constructor").and_then(|v| v.as_str());
    let error = args.get("error").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");

    let file_path = if let Some(p) = path_str {
        resolve_file_path(workspace_root, p)
    } else if let Some(sym) = symbol {
        let mut found = None;
        for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
            let p = entry.path();
            if p.is_file()
                && let Ok(content) = std::fs::read_to_string(p)
                && content.contains(sym)
            {
                found = Some(p.to_path_buf());
                break;
            }
        }
        found.with_context(|| format!("could not find file declaring symbol `{sym}`"))?
    } else {
        anyhow::bail!("Missing 'path' or 'symbol' argument");
    };

    let mut done = crate::wrap_return::wrap_polyglot_ext(
        remote,
        workspace_root,
        &file_path,
        symbol,
        line,
        character,
        wrapper,
        constructor,
        error,
        apply && !verify,
        force,
    )
    .await?;
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && (done.blocked.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_encapsulate_field(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let by_value = args.get("by_value").and_then(|v| v.as_bool());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";

    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust field encapsulation")?;
        let c = character.unwrap_or(1);
        crate::encapsulate_field::encapsulate(
            remote,
            workspace_root,
            &file_path,
            l,
            c,
            by_value,
            apply && !verify,
            force,
        )
        .await?
    } else {
        let symbol = args.get("symbol").and_then(|v| v.as_str());
        let field_arg = args
            .get("field")
            .or_else(|| args.get("field_name"))
            .and_then(|v| v.as_str());
        let class_arg = args
            .get("class_name")
            .or_else(|| args.get("struct_name"))
            .and_then(|v| v.as_str());

        let (resolved_class, resolved_field) = if let Some(f) = field_arg {
            (class_arg, f.to_string())
        } else if let Some(s) = symbol {
            if let Some((cls, fld)) = s.split_once("::").or_else(|| s.split_once('.')) {
                (Some(cls), fld.to_string())
            } else {
                (class_arg, s.to_string())
            }
        } else if let Some(l) = line {
            let text = std::fs::read_to_string(&file_path)?;
            let fld = crate::encapsulate_field::field_at_line_col(&text, l, character.unwrap_or(1))
                .context("Could not find field at given line/character")?;
            (class_arg, fld)
        } else {
            anyhow::bail!("Missing 'field', 'symbol', or line/character position");
        };

        crate::encapsulate_field::encapsulate_polyglot(
            remote,
            workspace_root,
            &file_path,
            resolved_class,
            &resolved_field,
            by_value,
            apply && !verify,
            force,
        )
        .await?
    };
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && (done.blocked.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_schema_rename(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let field = args
        .get("field")
        .and_then(|v| v.as_str())
        .context("Missing 'field' argument")?;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let workspace_edit = args
        .get("workspace_edit")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let scope = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let repos = match args.get("repos") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(list)) => list
            .iter()
            .map(|v| {
                let path = v
                    .as_str()
                    .with_context(|| format!("repos takes paths, got {v}"))?;
                let path = resolve_file_path(workspace_root, path);
                std::fs::canonicalize(&path)
                    .with_context(|| format!("repository {} cannot be read", path.display()))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => anyhow::bail!("repos takes a list of paths, got {other}"),
    };
    if !repos.is_empty() {
        anyhow::ensure!(
            scope.is_none() && !verify,
            "`path` and `verify` narrow or check one repository; drop them to rename across `repos`"
        );
        let mut roots = vec![workspace_root.to_path_buf()];
        roots.extend(repos);
        let done = crate::schema::rename_across(remote, &roots, field, to, apply, force).await?;
        if workspace_edit {
            let json = serde_json::to_string_pretty(&done.workspace_edit())?;
            return Ok(if done.clean() {
                McpToolCallResult::text(json)
            } else {
                McpToolCallResult::error(json)
            });
        }
        let text = done.render(6000);
        return Ok(if done.clean() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let mut done = crate::schema::rename(
        remote,
        workspace_root,
        field,
        to,
        apply && !verify,
        force,
        scope.as_deref(),
    )
    .await?;
    let gate = if verify {
        let files = done
            .rewritten
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect::<Vec<_>>();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    if workspace_edit {
        let json = serde_json::to_string_pretty(&done.workspace_edit())?;
        return Ok(if clean {
            McpToolCallResult::text(json)
        } else {
            McpToolCallResult::error(json)
        });
    }
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_codemod(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let rule = args
        .get("rule")
        .and_then(|v| v.as_str())
        .context("Missing 'rule' argument")?;
    if !rule.contains("==>>") {
        return Ok(McpToolCallResult::error(
            "a rule is `pattern ==>> replacement`, for example `$a.unwrap() ==>> $a.expect(\"invariant\")`"
                .to_string(),
        ));
    }
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    // `path` restricts the rewrite to one file and doubles as the resolve context.
    let scope = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| crate::codemod::resolve_workspace_scope(workspace_root, p))
        .transpose()?;

    // First: Run polyglot structural AST codemod engine across target scope.
    let outcome = crate::codemod::run_codemod(workspace_root, rule, scope.as_deref(), apply)?;
    if outcome.files_matched > 0 {
        let mut text = format!("`{rule}`\n");
        text.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            outcome.changed_lines, outcome.files_matched
        ));
        const MAX_DIFF: usize = 6000;
        if outcome.diff.len() > MAX_DIFF {
            let cut: String = outcome.diff.chars().take(MAX_DIFF).collect();
            text.push_str(&cut);
            text.push_str("\n… diff truncated\n");
        } else {
            text.push_str(&outcome.diff);
        }
        if apply {
            let written_paths: Vec<String> = outcome
                .rewritten_files
                .iter()
                .map(|(p, _)| {
                    p.strip_prefix(workspace_root)
                        .map(|r| r.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| p.to_string_lossy().into_owned())
                })
                .collect();
            text.push_str(&format!(
                "\n[applied to {} file(s): {}]\n",
                written_paths.len(),
                written_paths.join(", ")
            ));
        } else {
            text.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        return Ok(McpToolCallResult::text(text.trim_end().to_string()));
    }

    // Fallback: If polyglot AST matched 0 files, check if there's a Rust file context to query rust-analyzer SSR
    let is_rust_target = match &scope {
        Some(p) => p.extension().is_some_and(|ext| ext == "rs"),
        None => true,
    };

    if is_rust_target {
        let context = match scope.clone() {
            Some(p) => Some(p),
            None => representative_source_file(workspace_root),
        };
        if let Some(context) = context
            && let Ok(uri) = Url::from_file_path(&context)
            && let Ok(edit) = execute_lsp_query(
                remote,
                workspace_root,
                &context,
                "prodCode/structuralReplace",
                serde_json::json!({
                    "rule": rule,
                    "scope": scope.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    "textDocument": { "uri": uri.to_string() },
                    "position": { "line": 0, "character": 0 },
                }),
            )
            .await
        {
            let rewritten = rewritten_files(&edit);
            if !rewritten.is_empty() {
                let mut text = format!("`{rule}`\n");
                let mut changed_lines = 0usize;
                let mut body = String::new();
                for (path, new_text) in &rewritten {
                    let rel = std::path::Path::new(path)
                        .strip_prefix(workspace_root)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| path.clone());
                    let old_text = crate::refactor::text_before_apply(Path::new(path));
                    let diff = similar::TextDiff::from_lines(&old_text, new_text);
                    let file_changed = diff
                        .iter_all_changes()
                        .filter(|c| c.tag() != similar::ChangeTag::Equal)
                        .count();
                    changed_lines += file_changed;
                    body.push_str(
                        &diff
                            .unified_diff()
                            .context_radius(2)
                            .header(&format!("a/{rel}"), &format!("b/{rel}"))
                            .to_string(),
                    );
                }
                text.push_str(&format!(
                    "{} changed line(s) in {} file(s)\n\n",
                    changed_lines,
                    rewritten.len()
                ));
                const MAX_DIFF: usize = 6000;
                if body.len() > MAX_DIFF {
                    let cut: String = body.chars().take(MAX_DIFF).collect();
                    text.push_str(&cut);
                    text.push_str("\n… diff truncated\n");
                } else {
                    text.push_str(&body);
                }
                if apply {
                    let written = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
                    text.push_str(&format!(
                        "\n[applied to {} file(s): {}]\n",
                        written.len(),
                        written.join(", ")
                    ));
                } else {
                    text.push_str(
                        "\nnothing was written; pass `apply: true` to make these edits\n",
                    );
                }
                return Ok(McpToolCallResult::text(text.trim_end().to_string()));
            }
        }
    }

    Ok(McpToolCallResult::text(format!(
        "`{rule}` matches nothing{}",
        match &scope {
            Some(p) => format!(" in {}", p.display()),
            None => " in this workspace".to_string(),
        }
    )))
}
