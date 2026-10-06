/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use ra_ap_ide::{
    AdjustmentHints, AdjustmentHintsMode, CallableSnippets, ClosureReturnTypeHints,
    CompletionConfig, CompletionFieldsToResolve, CompletionItem, CompletionItemKind,
    DiagnosticsConfig, DiscriminantHints, GenericParameterHints, InlayFieldsToResolve,
    InlayHintsConfig, LifetimeElisionHints, SymbolKind, TypeHintsPlacement,
};
use ra_ap_ide_db::SnippetCap;
use ra_ap_ide_db::ra_fixture::RaFixtureConfig;
use serde_json::{Value, json};

use super::lines::Lines;

pub(crate) const COMPLETION_LIMIT: usize = 256;

pub(crate) fn completion_config() -> CompletionConfig<'static> {
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
pub(crate) fn completion_kind(kind: CompletionItemKind) -> u8 {
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
pub(crate) fn completion_item(lines: &Lines, item: CompletionItem, at: &Value) -> Value {
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

pub(crate) fn inlay_config() -> InlayHintsConfig<'static> {
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
pub(crate) fn text_edits(lines: &Lines, edit: &ra_ap_ide::TextEdit) -> Vec<Value> {
    edit.iter()
        .map(|indel| json!({ "range": lines.range(indel.delete), "newText": indel.insert }))
        .collect()
}
