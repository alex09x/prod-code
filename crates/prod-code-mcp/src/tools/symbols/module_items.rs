/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What a module path can name among the declarations read from source (#2020).

use super::nested_projects::read_name_scan_text;
use super::types::SymbolHit;

/// The Rust keywords that declare an item a module path can name.
const RUST_ITEM_KEYWORDS: &[&str] = &[
    "fn",
    "struct",
    "enum",
    "trait",
    "type",
    "union",
    "mod",
    "const",
    "static",
    "macro_rules!",
];

/// Whether `line` declares `name` as a Rust item (`fn name`, `struct name`, `static mut name`).
/// A local, a parameter and a field (`let name`, `name: T`) are declarations too, but no module
/// path names them.
pub(crate) fn declares_rust_item(line: &str, name: &str) -> bool {
    let is_name = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(name).any(|(at, _)| {
        let (before, after) = (&line[..at], &line[at + name.len()..]);
        if before.chars().next_back().is_some_and(is_name)
            || after.chars().next().is_some_and(is_name)
        {
            return false;
        }
        let mut words = before
            .split(|c: char| c.is_whitespace() || c == '(')
            .filter(|word| !word.is_empty())
            .rev();
        match words.next() {
            Some("mut") => words.next() == Some("static"),
            Some(word) => RUST_ITEM_KEYWORDS.contains(&word),
            None => false,
        }
    })
}

/// Whether a declaration found by reading source, with no owner the index would give it, can
/// be what a module-qualified name (`ledger_math::payable`) asks for. In Rust that is an
/// item: the parameter `payable: i64` of a function in `ledger_math.rs` was answered as
/// the definition of `ledger_math::payable`. Other languages keep their rules.
pub(crate) fn names_module_item(hit: &SymbolHit) -> bool {
    if hit.path.extension().and_then(|e| e.to_str()) != Some("rs") {
        return true;
    }
    use crate::tools::references::across::{mask_executable_code, masked_code_string};
    read_name_scan_text(&hit.path).is_some_and(|text| {
        masked_code_string(&text, &mask_executable_code(&text, &hit.path))
            .lines()
            .nth(hit.line.saturating_sub(1) as usize)
            .is_some_and(|line| declares_rust_item(line, &hit.name))
    })
}

/// The 0-based lines of `code` (comments and literals already blanked) whose declaration of
/// `name` an owner can qualify: an item anywhere, and, when `owner_is_struct`, a field: a
/// `name: T` line that is in no parameter list. A `let` and a parameter are owned by their
/// function, which a qualified name does not spell.
pub(crate) fn qualified_declaration_lines(
    code: &str,
    name: &str,
    owner_is_struct: bool,
) -> Vec<usize> {
    let mut lines = Vec::new();
    // How many parentheses are open where the line starts: a parameter list, a call, a tuple.
    let mut open = 0usize;
    for (index, line) in code.lines().enumerate() {
        let local = line.trim_start().starts_with("let ");
        if declares_rust_item(line, name)
            || (owner_is_struct
                && open == 0
                && !local
                && super::declared_at::declared_at(line, name).is_some())
        {
            lines.push(index);
        }
        for c in line.chars() {
            match c {
                '(' => open += 1,
                ')' => open = open.saturating_sub(1),
                _ => {}
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_qualifies_an_item_or_a_field_never_a_parameter_or_a_local() {
        let code = "pub struct Ledger {\n    pub fee: i64,\n}\n\npub fn settle(\n    total: i64,\n    fee: i64,\n) -> i64 {\n    let fee = fee + 1;\n    total - fee\n}\n";
        // The module's name finds no `fee`: a field belongs to its struct, not to the module.
        assert!(qualified_declaration_lines(code, "fee", false).is_empty());
        // The struct's name finds the field alone.
        assert_eq!(qualified_declaration_lines(code, "fee", true), vec![1]);
        assert_eq!(qualified_declaration_lines(code, "settle", false), vec![4]);
    }

    #[test]
    fn an_item_is_told_from_a_local_a_parameter_and_a_field() {
        for (line, name) in [
            ("pub fn settle(", "settle"),
            ("pub(crate) async fn settle() {}", "settle"),
            ("    const fn settle() -> u8 { 0 }", "settle"),
            ("pub struct Ledger {", "Ledger"),
            ("pub(super) enum Side { Buy }", "Side"),
            ("type Amount = i64;", "Amount"),
            ("pub const LIMIT: i64 = 1;", "LIMIT"),
            ("static mut COUNTER: u32 = 0;", "COUNTER"),
            ("pub mod ledger_math;", "ledger_math"),
            ("macro_rules! settle {", "settle"),
        ] {
            assert!(declares_rust_item(line, name), "{line}");
        }
        for (line, name) in [
            ("    payable: i64,", "payable"),
            ("    pub payable: i64,", "payable"),
            ("    let reserved = amount / 2;", "reserved"),
            ("    let mut reserved = 0;", "reserved"),
            ("    total - payable", "payable"),
        ] {
            assert!(!declares_rust_item(line, name), "{line}");
        }
    }

    #[test]
    fn a_source_declaration_names_a_module_item_only_when_it_is_one() {
        let dir = tempfile::Builder::new()
            .prefix("ModuleItems")
            .tempdir()
            .unwrap();
        let file = dir.path().join("ledger_math.rs");
        std::fs::write(
            &file,
            "pub fn settle(\n    total: i64,\n    payable: i64,\n) -> i64 {\n    total - payable\n}\n",
        )
        .unwrap();
        let hit = |name: &str, line: u32| SymbolHit {
            path: file.clone(),
            name: name.to_string(),
            kind: "Declaration",
            container: None,
            line,
            col: 5,
        };
        assert!(names_module_item(&hit("settle", 1)));
        assert!(!names_module_item(&hit("payable", 3)));
        // Another language's declaration is left to that language's rules.
        let mut go = hit("payable", 3);
        go.path = dir.path().join("ledger_math.go");
        assert!(names_module_item(&go));
    }

    /// `ledger_math::payable` resolved to the parameter of `settle` in `ledger_math.rs`:
    /// the index lists no such symbol, the analyzer hovers at the parameter, and the file's
    /// module path was all that qualified it.
    #[tokio::test]
    async fn a_module_path_resolves_to_the_function_and_not_to_its_parameter() {
        use crate::tools::symbols::resolve_symbol;
        use prod_code_testkit::{ScriptedGateway, Workspace, answers};

        let ws = Workspace::new(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("src/lib.rs", "pub mod ledger_math;\n"),
            (
                "src/ledger_math.rs",
                "pub fn settle(\n    total: i64,\n    payable: i64,\n) -> i64 {\n    total - payable\n}\n",
            ),
        ]);
        let gateway = ScriptedGateway::start(|method, params| {
            let in_module = params["textDocument"]["uri"]
                .as_str()
                .is_some_and(|uri| uri.ends_with("ledger_math.rs"));
            match method {
                "textDocument/documentSymbol" if in_module => serde_json::json!([answers::nested(
                    answers::document_symbol("settle", 12, 1, 6, 8),
                    vec![answers::document_symbol("payable", 13, 3, 3, 5)],
                )]),
                "textDocument/hover" if in_module => answers::hover("i64"),
                "workspace/symbol" => serde_json::json!([]),
                _ => serde_json::Value::Null,
            }
        })
        .await;
        let root = ws.root();

        let parameter = resolve_symbol(gateway.addr(), &root, "ledger_math::payable", None)
            .await
            .map(|hit| (hit.line, hit.col));
        let error = parameter.expect_err("a parameter is not an item of the module");
        assert!(
            format!("{error:#}").contains("ledger_math::payable"),
            "{error:#}"
        );

        let function = resolve_symbol(gateway.addr(), &root, "ledger_math::settle", None)
            .await
            .unwrap();
        assert_eq!((function.line, function.col), (1, 8));
    }

    /// The same question to `code_definition` on a file the analyzer answers nothing for: the
    /// source fallback of that tool named the parameter too.
    #[tokio::test]
    async fn code_definition_does_not_answer_a_module_path_with_a_parameter() {
        use crate::protocol::McpContentItem;
        use prod_code_testkit::{ScriptedGateway, Workspace};

        let ws = Workspace::new(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("src/lib.rs", "pub mod ledger_math;\n"),
            (
                "src/ledger_math.rs",
                "pub fn settle(\n    total: i64,\n    payable: i64,\n) -> i64 {\n    let reserved = total / 2;\n    total - payable - reserved\n}\n",
            ),
        ]);
        let gateway = ScriptedGateway::start(|_, _| serde_json::Value::Null).await;
        let root = ws.root();
        let answer = |symbol: &'static str| {
            let root = root.clone();
            let addr = gateway.addr();
            async move {
                let args = serde_json::json!({ "symbol": symbol });
                match crate::tools::execute_tool(addr, &root, "code_definition", args).await {
                    Ok(result) => {
                        let McpContentItem::Text { text } = &result.content[0];
                        text.clone()
                    }
                    Err(error) => format!("{error:#}"),
                }
            }
        };

        for symbol in ["ledger_math::payable", "ledger_math::reserved"] {
            let text = answer(symbol).await;
            assert!(!text.contains("ledger_math.rs:"), "{symbol}: {text}");
            assert!(text.contains(symbol), "{symbol}: {text}");
        }
        let text = answer("ledger_math::settle").await;
        assert!(text.contains("ledger_math.rs:1:8"), "{text}");
    }
}
