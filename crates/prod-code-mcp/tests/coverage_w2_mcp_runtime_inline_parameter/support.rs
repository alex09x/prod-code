/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What the inline-parameter tests share: the fixtures, the scripted analyzer and the request.

use prod_code_mcp::inline_parameter::{InlinedParameter, inline_parameter_polyglot};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;

pub const MATH_TS: &str =
    "export function clamp(x: number, max: number): number {\n    return Math.min(x, max);\n}\n";
pub const CLIENT_TS: &str = "import { clamp } from \"./math\";\n\nexport function compute() {\n    return clamp(10, 100) + clamp(20, 100);\n}\n";

pub fn location(file: &Path, line: u32, character: u32) -> Value {
    json!({
        "uri": url::Url::from_file_path(file).unwrap().to_string(),
        "range": { "start": { "line": line, "character": character } }
    })
}

/// The reference the analyzer would report at the `occurrence`-th appearance of `name`.
pub fn reference_at(file: &Path, source: &str, name: &str, occurrence: usize) -> Value {
    let at = source.match_indices(name).nth(occurrence).unwrap().0;
    let before = &source[..at];
    let line = before.matches('\n').count() as u32;
    let character = before.rsplit('\n').next().unwrap().encode_utf16().count() as u32;
    location(file, line, character)
}

pub async fn gateway(references: Vec<Value>, diagnostics: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/diagnostic" => diagnostics.clone(),
        "textDocument/references" => Value::Array(references.clone()),
        _ => Value::Null,
    })
    .await
}

/// A gateway that is never asked: the call must fail before it needs an analyzer.
pub fn dead_end() -> SocketAddr {
    "127.0.0.1:1".parse().unwrap()
}

pub struct Request<'a> {
    pub file: &'a str,
    pub line: Option<u32>,
    pub character: Option<u32>,
    pub function: Option<&'a str>,
    pub param: Option<&'a str>,
    pub apply: bool,
    pub force: bool,
}

impl<'a> Request<'a> {
    pub fn new(file: &'a str, function: &'a str, param: &'a str) -> Self {
        Self {
            file,
            line: None,
            character: None,
            function: Some(function),
            param: Some(param),
            apply: false,
            force: false,
        }
    }

    pub async fn run(&self, addr: SocketAddr, ws: &Workspace) -> anyhow::Result<InlinedParameter> {
        inline_parameter_polyglot(
            addr,
            &ws.root(),
            &ws.path(self.file),
            self.line,
            self.character,
            self.function,
            self.param,
            self.apply,
            self.force,
        )
        .await
    }
}

pub fn rewritten<'a>(res: &'a InlinedParameter, name: &str) -> &'a str {
    res.rewritten
        .iter()
        .find(|(path, _)| path.ends_with(name))
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("{name} not rewritten: {:?}", res.rewritten))
}

pub fn ts_workspace() -> Workspace {
    Workspace::new(&[("math.ts", MATH_TS), ("client.ts", CLIENT_TS)])
}

pub fn client_calls(ws: &Workspace) -> Vec<Value> {
    let client = ws.path("client.ts");
    vec![
        reference_at(&client, CLIENT_TS, "clamp", 1),
        reference_at(&client, CLIENT_TS, "clamp", 2),
    ]
}
