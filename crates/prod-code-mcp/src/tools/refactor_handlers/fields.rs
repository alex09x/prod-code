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

use anyhow::{Context, Result};

use crate::protocol::McpToolCallResult;
use crate::tools::{compile_gate, refuse_incomplete, resolve_file_path};

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
