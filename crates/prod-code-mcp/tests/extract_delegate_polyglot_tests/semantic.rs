/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, answers};
use serde_json::{Value, json};
use std::path::Path;

pub struct Field {
    name: String,
    position: Value,
    references: Vec<Value>,
}

pub fn references(file: &Path, source: &str, field: &str, anchors: &[&str]) -> Vec<Value> {
    anchors
        .iter()
        .map(|anchor| location(file, source, field, anchor))
        .collect()
}

fn location(file: &Path, source: &str, field: &str, anchor: &str) -> Value {
    assert_eq!(
        source.matches(anchor).count(),
        1,
        "ambiguous anchor: {anchor}"
    );
    let offset = source.find(anchor).unwrap() + anchor.find(field).unwrap();
    let (line, col) = prod_code_mcp::signature::position_at(source, offset).unwrap();
    json!({"uri": url::Url::from_file_path(file).unwrap().to_string(),
        "range": {"start": {"line": line - 1, "character": col - 1}}})
}

pub fn field(
    file: &Path,
    source: &str,
    name: &str,
    declaration: &str,
    mut references: Vec<Value>,
) -> Field {
    let declaration = location(file, source, name, declaration);
    let position = declaration["range"]["start"].clone();
    // Include the declaration, as real includeDeclaration=true answers may do.
    references.push(declaration);
    Field {
        name: name.into(),
        position,
        references,
    }
}

pub async fn semantic_gateway(file: &Path, owner: &str, fields: Vec<Field>) -> ScriptedGateway {
    let uri = url::Url::from_file_path(file).unwrap().to_string();
    let children: Vec<_> = fields
        .iter()
        .map(|f| {
            json!({
                "name": f.name, "kind": 8,
                "selectionRange": {"start": f.position, "end": f.position}
            })
        })
        .collect();
    let symbols = json!([{"name": owner, "kind": 5, "children": children}]);
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" if params["textDocument"]["uri"] == uri => symbols.clone(),
        "textDocument/references" if params["textDocument"]["uri"] == uri => {
            let f = fields
                .iter()
                .find(|f| f.position == params["position"])
                .expect("reference query must use the exact declared field identity");
            json!(f.references)
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
}
