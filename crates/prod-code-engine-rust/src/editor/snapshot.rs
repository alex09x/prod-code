/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use ra_ap_ide::{
    AssistResolveStrategy, CompletionItemImport, FilePosition, FileRange, HighlightRelatedConfig,
    InlayHintPosition, InlayKind, ReferenceCategory, SingleResolve, SourceChange, TextRange,
    TextSize,
};
use serde_json::{Value, json};

use super::lines::{Lines, document, uri_of};
use super::mapping::{
    COMPLETION_LIMIT, completion_config, completion_item, inlay_config, text_edits,
};
use crate::RustEngineSnapshot;

impl RustEngineSnapshot {
    /// Completions at an LSP position, with `data` on the items that need an import.
    pub fn completion(&self, params: &Value) -> Result<Value> {
        let (path, file_id, text) = document(self, params)?;
        let lines = Lines::new(&text);
        let position = FilePosition {
            file_id,
            offset: lines.offset_of(&params["position"]),
        };
        let trigger = params
            .pointer("/context/triggerCharacter")
            .and_then(Value::as_str)
            .and_then(|s| s.chars().next());
        let items = self
            .analysis
            .completions(&completion_config(), position, trigger)?
            .unwrap_or_default();
        let incomplete = items.len() >= COMPLETION_LIMIT;
        let at = json!({ "uri": uri_of(&path), "position": params["position"] });
        let items: Vec<Value> = items
            .into_iter()
            .map(|item| completion_item(&lines, item, &at))
            .collect();
        Ok(json!({ "isIncomplete": incomplete, "items": items }))
    }

    /// The item of `completionItem/resolve` with the edits that add its imports.
    pub fn resolve_completion(&self, item: &Value) -> Result<Value> {
        let mut item = item.clone();
        let Some(data) = item.get("data").cloned() else {
            return Ok(item);
        };
        let request = json!({ "textDocument": { "uri": data["uri"] } });
        let (_, file_id, text) = document(self, &request)?;
        let lines = Lines::new(&text);
        let position = FilePosition {
            file_id,
            offset: lines.offset_of(&data["position"]),
        };
        let imports: Vec<CompletionItemImport> = data["imports"]
            .as_array()
            .map(|imports| {
                imports
                    .iter()
                    .filter_map(|import| {
                        Some(CompletionItemImport {
                            path: import.get("path")?.as_str()?.to_string(),
                            as_underscore: import
                                .get("as_underscore")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let edits =
            self.analysis
                .resolve_completion_edits(&completion_config(), position, imports)?;
        let mut additional: Vec<Value> = item
            .get("additionalTextEdits")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for edit in &edits {
            additional.extend(text_edits(&lines, edit));
        }
        if !additional.is_empty() {
            item["additionalTextEdits"] = json!(additional);
        }
        Ok(item)
    }

    /// The signature of the call around an LSP position, with its parameters as label offsets.
    pub fn signature_help(&self, params: &Value) -> Result<Value> {
        let (_, file_id, text) = document(self, params)?;
        let lines = Lines::new(&text);
        let position = FilePosition {
            file_id,
            offset: lines.offset_of(&params["position"]),
        };
        let Some(help) = self.analysis.signature_help(position)? else {
            return Ok(Value::Null);
        };
        let label = help.signature.clone();
        let utf16 = |byte: TextSize| -> usize {
            label
                .get(..usize::from(byte))
                .map_or(0, |before| before.encode_utf16().count())
        };
        let parameters: Vec<Value> = help
            .parameter_ranges()
            .iter()
            .map(|range| json!({ "label": [utf16(range.start()), utf16(range.end())] }))
            .collect();
        let mut signature = json!({ "label": label, "parameters": parameters });
        if let Some(docs) = &help.doc {
            signature["documentation"] = json!({ "kind": "markdown", "value": docs.as_str() });
        }
        if let Some(active) = help.active_parameter {
            signature["activeParameter"] = json!(active);
        }
        Ok(json!({
            "signatures": [signature],
            "activeSignature": 0,
            "activeParameter": help.active_parameter,
        }))
    }

    /// Type, parameter and chaining hints inside an LSP range.
    pub fn inlay_hints(&self, params: &Value) -> Result<Value> {
        let (_, file_id, text) = document(self, params)?;
        let lines = Lines::new(&text);
        let range = params
            .get("range")
            .map(|range| lines.range_of(range))
            .unwrap_or_else(|| TextRange::up_to(TextSize::of(text.as_str())));
        let hints = self
            .analysis
            .inlay_hints(&inlay_config(), file_id, Some(range))?;
        let hints: Vec<Value> = hints
            .into_iter()
            .map(|hint| {
                let at = match hint.position {
                    InlayHintPosition::Before => hint.range.start(),
                    InlayHintPosition::After => hint.range.end(),
                };
                let label: String = hint.label.parts.iter().map(|p| p.text.as_str()).collect();
                let mut out = json!({
                    "position": lines.position(at),
                    "label": label,
                    "paddingLeft": hint.pad_left,
                    "paddingRight": hint.pad_right,
                });
                match hint.kind {
                    InlayKind::Type | InlayKind::Chaining => out["kind"] = json!(1),
                    InlayKind::Parameter | InlayKind::GenericParameter => out["kind"] = json!(2),
                    _ => {}
                }
                out
            })
            .collect();
        Ok(json!(hints))
    }

    /// Every use of the symbol at an LSP position in its file, reads and writes told apart,
    /// or the exit points of the function, loop or closure at a keyword.
    pub fn document_highlight(&self, params: &Value) -> Result<Value> {
        let (_, file_id, text) = document(self, params)?;
        let lines = Lines::new(&text);
        let position = FilePosition {
            file_id,
            offset: lines.offset_of(&params["position"]),
        };
        let config = HighlightRelatedConfig {
            references: true,
            exit_points: true,
            break_points: true,
            closure_captures: true,
            yield_points: true,
            branch_exit_points: true,
        };
        let ranges = self
            .analysis
            .highlight_related(config, position)?
            .unwrap_or_default();
        let highlights: Vec<Value> = ranges
            .into_iter()
            .map(|highlight| {
                let kind = if highlight.category.contains(ReferenceCategory::WRITE) {
                    3
                } else if highlight.category.contains(ReferenceCategory::READ) {
                    2
                } else {
                    1
                };
                json!({ "range": lines.range(highlight.range), "kind": kind })
            })
            .collect();
        Ok(json!(highlights))
    }

    /// The code actions (refactorings and quick fixes) for an LSP range. Each carries in
    /// `data` what `codeAction/resolve` needs to compute its edit.
    pub fn code_actions(&self, params: &Value) -> Result<Value> {
        let (path, file_id, text) = document(self, params)?;
        let lines = Lines::new(&text);
        let frange = FileRange {
            file_id,
            range: lines.range_of(&params["range"]),
        };
        let (assist_config, diagnostics_config) = Self::assist_configs();
        let assists = self.analysis.assists_with_fixes(
            &assist_config,
            &diagnostics_config,
            AssistResolveStrategy::None,
            frange,
        )?;
        let actions: Vec<Value> = assists
            .into_iter()
            .map(|assist| {
                let kind = match assist.id.1 {
                    ra_ap_ide::AssistKind::QuickFix => "quickfix",
                    ra_ap_ide::AssistKind::Generate => "refactor",
                    ra_ap_ide::AssistKind::Refactor => "refactor",
                    ra_ap_ide::AssistKind::RefactorExtract => "refactor.extract",
                    ra_ap_ide::AssistKind::RefactorInline => "refactor.inline",
                    ra_ap_ide::AssistKind::RefactorRewrite => "refactor.rewrite",
                };
                json!({
                    "title": assist.label.to_string(),
                    "kind": kind,
                    "data": {
                        "uri": uri_of(&path),
                        "range": params["range"],
                        "id": assist.id.0.to_string(),
                        "subtype": assist.id.2,
                    },
                })
            })
            .collect();
        Ok(json!(actions))
    }

    /// The code action of `codeAction/resolve` with its edit.
    pub fn resolve_code_action(&self, action: &Value) -> Result<Value> {
        let mut action = action.clone();
        let data = action.get("data").cloned().unwrap_or(Value::Null);
        let request = json!({ "textDocument": { "uri": data["uri"] } });
        let (_, file_id, text) = document(self, &request)?;
        let lines = Lines::new(&text);
        let frange = FileRange {
            file_id,
            range: lines.range_of(&data["range"]),
        };
        let id = data["id"].as_str().unwrap_or_default();
        let subtype = data["subtype"].as_u64().map(|s| s as usize);
        let (assist_config, diagnostics_config) = Self::assist_configs();
        let offered = self.analysis.assists_with_fixes(
            &assist_config,
            &diagnostics_config,
            AssistResolveStrategy::None,
            frange,
        )?;
        let target = offered
            .iter()
            .find(|a| a.id.0 == id && (subtype.is_none() || a.id.2 == subtype))
            .with_context(|| format!("code action `{id}` is no longer offered here"))?;
        let resolve = AssistResolveStrategy::Single(SingleResolve {
            assist_id: target.id.0.to_string(),
            assist_kind: target.id.1,
            assist_subtype: target.id.2,
        });
        let change = self
            .analysis
            .assists_with_fixes(&assist_config, &diagnostics_config, resolve, frange)?
            .into_iter()
            .find(|a| a.id.0 == id && (subtype.is_none() || a.id.2 == subtype))
            .and_then(|a| a.source_change)
            .with_context(|| format!("code action `{id}` produced no edit"))?;
        action["edit"] = self.workspace_edit(&change)?;
        Ok(action)
    }

    /// A source change as an LSP workspace edit: text edits per file, and the files it
    /// creates and moves as document changes.
    fn workspace_edit(&self, change: &SourceChange) -> Result<Value> {
        let mut document_changes = Vec::new();
        for fs_edit in &self.outcome_from_change(change)?.created {
            let uri = uri_of(&fs_edit.path);
            document_changes.push(json!({ "kind": "create", "uri": uri }));
            document_changes.push(json!({
                "textDocument": { "uri": uri, "version": null },
                "edits": [{
                    "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                    "newText": fs_edit.new_text,
                }],
            }));
        }
        for (file_id, (edit, _snippet)) in change.source_file_edits.iter() {
            let Some(path) = self.path_for_file_id(*file_id) else {
                continue;
            };
            let text = self.analysis.file_text(*file_id)?;
            let lines = Lines::new(&text);
            document_changes.push(json!({
                "textDocument": { "uri": uri_of(&path), "version": null },
                "edits": text_edits(&lines, edit),
            }));
        }
        for moved in &self.outcome_from_change(change)?.moves {
            document_changes.push(json!({
                "kind": "rename",
                "oldUri": uri_of(&moved.from),
                "newUri": uri_of(&moved.to),
            }));
        }
        Ok(json!({ "documentChanges": document_changes }))
    }

    /// Dispatches one editor request by its LSP method; `None` for a method this module
    /// does not answer.
    pub fn editor_request(&self, method: &str, params: &Value) -> Option<Result<Value>> {
        Some(match method {
            "textDocument/completion" => self.completion(params),
            "completionItem/resolve" => self.resolve_completion(params),
            "textDocument/signatureHelp" => self.signature_help(params),
            "textDocument/inlayHint" => self.inlay_hints(params),
            "textDocument/documentHighlight" => self.document_highlight(params),
            "textDocument/codeAction" => self.code_actions(params),
            "codeAction/resolve" => self.resolve_code_action(params),
            "textDocument/formatting" => self.formatting(params),
            _ => return None,
        })
    }
}

/// The LSP methods [`RustEngineSnapshot::editor_request`] answers.
pub const EDITOR_METHODS: &[&str] = &[
    "textDocument/completion",
    "completionItem/resolve",
    "textDocument/signatureHelp",
    "textDocument/inlayHint",
    "textDocument/documentHighlight",
    "textDocument/codeAction",
    "codeAction/resolve",
    "textDocument/formatting",
];
