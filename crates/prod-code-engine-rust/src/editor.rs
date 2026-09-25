//! What an editor asks the in-memory engine for, answered in LSP's own shapes (#310):
//! completion, signature help, inlay hints, document highlights, code actions, formatting and
//! the diagnostics the gateway pushes after an edit.
//!
//! Positions are LSP's: 0-based lines and UTF-16 columns. The rest of the engine speaks 1-based
//! lines and character columns to the agent tools, which is not what an editor sends.

use crate::{RustEngine, RustEngineSnapshot, line_col_to_offset};
use anyhow::{Context, Result};
use ra_ap_ide::{
    AdjustmentHints, AdjustmentHintsMode, AssistResolveStrategy, CallableSnippets,
    ClosureReturnTypeHints, CompletionConfig, CompletionFieldsToResolve, CompletionItem,
    CompletionItemImport, CompletionItemKind, DiagnosticsConfig, DiscriminantHints, FileId,
    FilePosition, FileRange, GenericParameterHints, HighlightRelatedConfig, InlayFieldsToResolve,
    InlayHintPosition, InlayHintsConfig, InlayKind, LifetimeElisionHints, RaFixtureConfig,
    SingleResolve, SourceChange, SymbolKind, TextRange, TextSize, TypeHintsPlacement,
};
use ra_ap_ide_db::SnippetCap;
use ra_ap_ide_db::search::ReferenceCategory;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The most completion items one answer carries; a longer list is marked incomplete, so the
/// editor asks again as the user types.
const COMPLETION_LIMIT: usize = 256;

/// Line starts of a text, for conversions between byte offsets and LSP positions.
pub struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { text, starts }
    }

    /// The LSP position of a byte offset.
    pub fn position(&self, offset: TextSize) -> Value {
        let offset = usize::from(offset).min(self.text.len());
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        let start = self.starts[line];
        let character: usize = self
            .text
            .get(start..offset)
            .map(|before| before.chars().map(char::len_utf16).sum())
            .unwrap_or(0);
        json!({ "line": line, "character": character })
    }

    pub fn range(&self, range: TextRange) -> Value {
        json!({ "start": self.position(range.start()), "end": self.position(range.end()) })
    }

    /// The byte offset of an LSP position. A column past the end of its line is the line's
    /// end; a line past the end of the text is the text's end.
    pub fn offset(&self, line: u32, character: u32) -> TextSize {
        let Some(&start) = self.starts.get(line as usize) else {
            return TextSize::of(self.text);
        };
        let end = self
            .starts
            .get(line as usize + 1)
            .map_or(self.text.len(), |&next| next - 1);
        let mut units = 0u32;
        for (byte, ch) in self.text[start..end].char_indices() {
            if units >= character {
                return TextSize::from((start + byte) as u32);
            }
            units += ch.len_utf16() as u32;
        }
        TextSize::from(end as u32)
    }

    /// The byte offset of the LSP position a JSON `position` object names.
    fn offset_of(&self, position: &Value) -> TextSize {
        let field = |name: &str| position.get(name).and_then(Value::as_u64).unwrap_or(0) as u32;
        self.offset(field("line"), field("character"))
    }

    /// The byte range of a JSON LSP `range` object; a reversed range is put in order.
    fn range_of(&self, range: &Value) -> TextRange {
        let start = self.offset_of(&range["start"]);
        let end = self.offset_of(&range["end"]);
        TextRange::new(start.min(end), start.max(end))
    }
}

/// The server path a `file://` URI names.
fn path_of(uri: &str) -> PathBuf {
    PathBuf::from(uri.trim_start_matches("file://"))
}

fn uri_of(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// The document a request is about: its path, file id and text.
fn document(snapshot: &RustEngineSnapshot, params: &Value) -> Result<(PathBuf, FileId, String)> {
    let uri = params
        .pointer("/textDocument/uri")
        .and_then(Value::as_str)
        .context("the request names no textDocument.uri")?;
    let path = path_of(uri);
    let file_id = snapshot
        .file_id_for_path(&path)
        .with_context(|| format!("File not found in VFS: {}", path.display()))?;
    let text = snapshot.analysis.file_text(file_id)?.to_string();
    Ok((path, file_id, text))
}

fn completion_config() -> CompletionConfig<'static> {
    CompletionConfig {
        enable_postfix_completions: true,
        enable_imports_on_the_fly: true,
        enable_self_on_the_fly: true,
        enable_auto_iter: true,
        enable_auto_await: true,
        enable_private_editable: false,
        enable_term_search: false,
        term_search_fuel: 1000,
        full_function_signatures: false,
        callable: Some(CallableSnippets::FillArguments),
        add_colons_to_module: true,
        add_semicolon_to_unit: true,
        snippet_cap: SnippetCap::new(true),
        insert_use: DiagnosticsConfig::test_sample().insert_use,
        prefer_no_std: false,
        prefer_prelude: true,
        prefer_absolute: false,
        snippets: Vec::new(),
        limit: Some(COMPLETION_LIMIT),
        fields_to_resolve: CompletionFieldsToResolve {
            resolve_label_details: false,
            resolve_tags: false,
            resolve_detail: false,
            resolve_documentation: false,
            resolve_filter_text: false,
            resolve_text_edit: false,
            resolve_command: false,
        },
        exclude_flyimport: Vec::new(),
        exclude_traits: &[],
        ra_fixture: RaFixtureConfig::default(),
    }
}

/// LSP's `CompletionItemKind` for rust-analyzer's, as rust-analyzer's own server maps it.
fn completion_kind(kind: CompletionItemKind) -> u8 {
    const TEXT: u8 = 1;
    const METHOD: u8 = 2;
    const FUNCTION: u8 = 3;
    const FIELD: u8 = 5;
    const VARIABLE: u8 = 6;
    const INTERFACE: u8 = 8;
    const MODULE: u8 = 9;
    const VALUE: u8 = 12;
    const ENUM: u8 = 13;
    const KEYWORD: u8 = 14;
    const SNIPPET: u8 = 15;
    const REFERENCE: u8 = 18;
    const ENUM_MEMBER: u8 = 20;
    const CONSTANT: u8 = 21;
    const STRUCT: u8 = 22;
    const TYPE_PARAMETER: u8 = 25;
    match kind {
        CompletionItemKind::Binding => VARIABLE,
        CompletionItemKind::BuiltinType | CompletionItemKind::InferredType => STRUCT,
        CompletionItemKind::Keyword => KEYWORD,
        CompletionItemKind::Snippet | CompletionItemKind::Expression => SNIPPET,
        CompletionItemKind::UnresolvedReference => REFERENCE,
        CompletionItemKind::SymbolKind(symbol) => match symbol {
            SymbolKind::Attribute
            | SymbolKind::BuiltinAttr
            | SymbolKind::Derive
            | SymbolKind::DeriveHelper
            | SymbolKind::Function
            | SymbolKind::Macro
            | SymbolKind::ProcMacro => FUNCTION,
            SymbolKind::Method => METHOD,
            SymbolKind::Const => CONSTANT,
            SymbolKind::ConstParam
            | SymbolKind::LifetimeParam
            | SymbolKind::SelfType
            | SymbolKind::TypeParam => TYPE_PARAMETER,
            SymbolKind::CrateRoot | SymbolKind::Module | SymbolKind::ToolModule => MODULE,
            SymbolKind::Enum => ENUM,
            SymbolKind::Field => FIELD,
            SymbolKind::Impl => TEXT,
            SymbolKind::InlineAsmRegOrRegClass => KEYWORD,
            SymbolKind::Label | SymbolKind::Local => VARIABLE,
            SymbolKind::SelfParam | SymbolKind::Static | SymbolKind::ValueParam => VALUE,
            SymbolKind::Struct | SymbolKind::TypeAlias | SymbolKind::Union => STRUCT,
            SymbolKind::Trait => INTERFACE,
            SymbolKind::Variant => ENUM_MEMBER,
        },
    }
}

/// One completion item. The edit that covers the completed identifier is the item's own; any
/// other goes with it as an additional edit. An item that needs an import carries what
/// `completionItem/resolve` needs to compute it in `data`.
fn completion_item(lines: &Lines, item: CompletionItem, at: &Value) -> Value {
    let mut out = json!({
        "label": item.label.primary.as_str(),
        "kind": completion_kind(item.kind),
        // The highest relevance sorts first.
        "sortText": format!("{:08x}", u32::MAX - item.relevance.score()),
        "filterText": item.lookup(),
        "insertTextFormat": if item.is_snippet { 2 } else { 1 },
    });
    if item.label.detail_left.is_some() || item.label.detail_right.is_some() {
        let mut details = json!({});
        if let Some(detail) = &item.label.detail_left {
            details["detail"] = json!(detail);
        }
        if let Some(description) = &item.label.detail_right {
            details["description"] = json!(description);
        }
        out["labelDetails"] = details;
    }
    if let Some(detail) = &item.detail {
        out["detail"] = json!(detail);
    }
    if let Some(docs) = &item.documentation {
        out["documentation"] = json!({ "kind": "markdown", "value": docs.as_str() });
    }
    if item.deprecated {
        out["tags"] = json!([1]);
    }
    if !item.import_to_add.is_empty() {
        let imports: Vec<Value> = item
            .import_to_add
            .iter()
            .map(|import| json!({ "path": import.path, "as_underscore": import.as_underscore }))
            .collect();
        let mut data = at.clone();
        data["imports"] = json!(imports);
        out["data"] = data;
    }
    let source_range = item.source_range;
    let mut own = None;
    let mut additional = Vec::new();
    for indel in item.text_edit {
        let edit = json!({ "range": lines.range(indel.delete), "newText": indel.insert });
        if own.is_none() && indel.delete.contains_range(source_range) {
            own = Some(edit);
        } else {
            additional.push(edit);
        }
    }
    if let Some(edit) = own {
        out["textEdit"] = edit;
    } else if let Some(first) = additional.first().cloned() {
        additional.remove(0);
        out["textEdit"] = first;
    }
    if !additional.is_empty() {
        out["additionalTextEdits"] = json!(additional);
    }
    out
}

fn inlay_config() -> InlayHintsConfig<'static> {
    InlayHintsConfig {
        render_colons: true,
        type_hints: true,
        type_hints_placement: TypeHintsPlacement::Inline,
        sized_bound: false,
        discriminant_hints: DiscriminantHints::Never,
        parameter_hints: true,
        parameter_hints_for_missing_arguments: false,
        generic_parameter_hints: GenericParameterHints {
            type_hints: false,
            lifetime_hints: false,
            const_hints: true,
        },
        chaining_hints: true,
        adjustment_hints: AdjustmentHints::Never,
        adjustment_hints_disable_reborrows: true,
        adjustment_hints_mode: AdjustmentHintsMode::Prefix,
        adjustment_hints_hide_outside_unsafe: false,
        closure_return_type_hints: ClosureReturnTypeHints::Never,
        closure_capture_hints: false,
        binding_mode_hints: false,
        implicit_drop_hints: false,
        implied_dyn_trait_hints: true,
        lifetime_elision_hints: LifetimeElisionHints::Never,
        param_names_for_lifetime_elision_hints: false,
        hide_inferred_type_hints: false,
        hide_named_constructor_hints: false,
        hide_closure_initialization_hints: false,
        hide_closure_parameter_hints: false,
        range_exclusive_hints: false,
        closure_style: ra_ap_hir::ClosureStyle::ImplFn,
        max_length: Some(25),
        closing_brace_hints_min_lines: Some(25),
        fields_to_resolve: InlayFieldsToResolve {
            resolve_text_edits: false,
            resolve_hint_tooltip: false,
            resolve_label_tooltip: false,
            resolve_label_location: false,
            resolve_label_command: false,
        },
        ra_fixture: RaFixtureConfig::default(),
    }
}

/// The LSP text edits of one file's part of a source change.
fn text_edits(lines: &Lines, edit: &ra_ap_ide::TextEdit) -> Vec<Value> {
    edit.iter()
        .map(|indel| json!({ "range": lines.range(indel.delete), "newText": indel.insert }))
        .collect()
}

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

    /// The edition the crate of `file_id` is written in, as rustfmt spells it.
    fn edition_of(&self, file_id: FileId) -> Option<String> {
        let krate = self.analysis.crates_for(file_id).ok()?.into_iter().next()?;
        Some(self.analysis.crate_edition(krate).ok()?.to_string())
    }

    /// The whole document formatted by rustfmt on this node, under the checkout's
    /// `rustfmt.toml`, as one edit; no edit when it is formatted already.
    pub fn formatting(&self, params: &Value) -> Result<Value> {
        let (path, file_id, text) = document(self, params)?;
        let mut command = std::process::Command::new("rustfmt");
        command.args(["--emit", "stdout", "--quiet"]);
        if let Some(edition) = self.edition_of(file_id) {
            command.args(["--edition", &edition]);
        }
        if let Some(dir) = path.parent().filter(|dir| dir.is_dir()) {
            command.current_dir(dir);
        }
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("rustfmt did not start")?;
        let mut stdin = child.stdin.take().context("rustfmt has no stdin")?;
        let input = text.clone();
        let writer = std::thread::spawn(move || {
            use std::io::Write;
            stdin.write_all(input.as_bytes())
        });
        let output = child.wait_with_output().context("rustfmt did not finish")?;
        let _ = writer.join();
        if !output.status.success() {
            anyhow::bail!(
                "rustfmt failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let formatted = String::from_utf8(output.stdout).context("rustfmt printed no UTF-8")?;
        if formatted == text {
            return Ok(json!([]));
        }
        let lines = Lines::new(&text);
        Ok(json!([{
            "range": lines.range(TextRange::up_to(TextSize::of(text.as_str()))),
            "newText": formatted,
        }]))
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

impl RustEngine {
    /// One editor request, answered on the current state of the engine (see
    /// [`RustEngineSnapshot::editor_request`]).
    pub fn editor_request(&self, method: &str, params: &Value) -> Option<Result<Value>> {
        self.snapshot().editor_request(method, params)
    }

    /// The diagnostics of `path` as LSP `Diagnostic`s, for `textDocument/publishDiagnostics`:
    /// what [`RustEngine::diagnostics`] reports, placed in UTF-16 columns.
    pub fn editor_diagnostics(&self, path: &Path) -> Result<Value> {
        let diagnostics = self.diagnostics(path)?;
        let snapshot = self.snapshot();
        let file_id = snapshot
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {}", path.display()))?;
        let text = snapshot.analysis.file_text(file_id)?;
        let lines = Lines::new(&text);
        let items: Vec<Value> = diagnostics
            .into_iter()
            .filter(|d| d.severity != "allow")
            .map(|d| {
                let start = line_col_to_offset(&text, d.line, d.col).unwrap_or_default();
                let end = line_col_to_offset(&text, d.end_line, d.end_col).unwrap_or(start);
                let severity = match d.severity.as_str() {
                    "error" => 1,
                    "warning" => 2,
                    "weak" => 4,
                    _ => 3,
                };
                let mut out = json!({
                    "range": lines.range(TextRange::new(start, end.max(start))),
                    "severity": severity,
                    "code": d.code,
                    "source": "prod-code",
                    "message": d.message,
                });
                if d.unused {
                    out["tags"] = json!([1]);
                }
                out
            })
            .collect();
        Ok(json!(items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_utf16_units_and_clamp_to_their_line() {
        // "é" is two bytes and one UTF-16 unit; "𝄞" is four bytes and two units.
        let text = "fn a() {}\nlet é = \"𝄞x\";\n";
        let lines = Lines::new(text);
        let x = text.find('x').unwrap();
        assert_eq!(
            lines.position(TextSize::from(x as u32)),
            json!({ "line": 1, "character": 11 })
        );
        assert_eq!(usize::from(lines.offset(1, 11)), x);
        // Past the end of a line is its end, before the newline; past the last line is the end.
        assert_eq!(usize::from(lines.offset(0, 99)), 9);
        assert_eq!(usize::from(lines.offset(9, 0)), text.len());
        assert_eq!(
            lines.range_of(&json!({
                "start": { "line": 1, "character": 11 },
                "end": { "line": 0, "character": 0 }
            })),
            TextRange::new(TextSize::from(0), TextSize::from(x as u32))
        );
    }

    #[test]
    fn a_completion_kind_is_the_one_rust_analyzer_gives_an_editor() {
        assert_eq!(
            completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Method)),
            2
        );
        assert_eq!(
            completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Function)),
            3
        );
        assert_eq!(
            completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Struct)),
            22
        );
        assert_eq!(
            completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Variant)),
            20
        );
        assert_eq!(
            completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Trait)),
            8
        );
        assert_eq!(completion_kind(CompletionItemKind::Binding), 6);
        assert_eq!(completion_kind(CompletionItemKind::Keyword), 14);
    }
}
