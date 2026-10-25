/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::io::Write;
use std::process::Command;
use std::sync::{Arc, Mutex};

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct Compiled {
    pub status: i32,
    pub diagnostics: Value,
    pub stdout: String,
    pub stderr: String,
}

/// A real compiler is mandatory: an absent toolchain fails this target rather than skipping it.
pub fn compile(source: &str, run: bool) -> Compiled {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fixture.rs");
    let binary = dir.path().join("fixture");
    std::fs::write(&file, source).unwrap();
    let output = Command::new("rustc")
        .args([
            "--edition=2021",
            "--crate-name=fixture",
            "--error-format=json",
            "--color=never",
        ])
        .arg(&file)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("rustc must be installed on the build node");
    let messages: Vec<Value> = String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(|line| serde_json::from_str(line).expect("rustc JSON diagnostic"))
        .collect();
    let mut items: Vec<Value> = messages
        .iter()
        .filter(|d| d["level"] == "error")
        .filter_map(|d| {
            let span = d["spans"]
                .as_array()?
                .iter()
                .find(|s| s["is_primary"] == true)?;
            let line = span["line_start"].as_u64()? - 1;
            let col = span["column_start"].as_u64()? - 1;
            Some(json!({
                "severity": 1, "message": d["message"], "code": d["code"]["code"],
                "range": { "start": { "line": line, "character": col },
                           "end": { "line": line, "character": col + 1 } }
            }))
        })
        .collect();
    if !output.status.success() && items.is_empty() {
        items.push(json!({
            "severity": 1, "message": "rustc failed without a source span",
            "range": { "start": { "line": 0, "character": 0 },
                       "end": { "line": 0, "character": 1 } }
        }));
    }
    let stderr = messages
        .iter()
        .filter_map(|d| d["rendered"].as_str())
        .collect::<String>()
        .replace(&dir.path().to_string_lossy().to_string(), "<fixture>");
    let mut stdout = String::new();
    if output.status.success() && run {
        let result = Command::new(binary).output().unwrap();
        assert!(result.status.success(), "program failed: {result:?}");
        stdout = String::from_utf8(result.stdout).unwrap();
    }
    Compiled {
        status: output.status.code().unwrap_or(-1),
        diagnostics: json!({ "kind": "full", "items": items }),
        stdout,
        stderr,
    }
}

pub fn evidence(label: &str, source: &str) -> Compiled {
    let result = compile(source, true);
    // Write directly so the required unmodified cargo-test command retains compiler evidence.
    writeln!(
        std::io::stdout().lock(),
        "{label}: rustc exit={}\nsource:\n{source}\n{}runtime stdout={:?}",
        result.status,
        result.stderr,
        result.stdout
    )
    .unwrap();
    result
}

pub fn position(source: &str, at: usize) -> (u32, u32) {
    let line = source[..at].bytes().filter(|b| *b == b'\n').count() as u32 + 1;
    let start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let col = source[start..at].encode_utf16().count() as u32 + 1;
    (line, col)
}

/// The fixtures have unique parameter bindings and no identifier occurrences in literals/docs.
/// Reference answers are explicit identifier occurrences in the original source, never a
/// fabricated empty list. Unexpected reference/hover queries fail the test.
fn occurrences(source: &str, name: &str) -> Vec<usize> {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    source
        .match_indices(name)
        .filter_map(|(at, _)| {
            let before = at.checked_sub(1).and_then(|i| source.as_bytes().get(i));
            let after = source.as_bytes().get(at + name.len());
            (!before.is_some_and(|b| ident(*b)) && !after.is_some_and(|b| ident(*b))).then_some(at)
        })
        .collect()
}

pub struct Fixture {
    pub ws: Workspace,
    pub gateway: ScriptedGateway,
    pub at: (u32, u32),
    pub params: Vec<String>,
}

impl Fixture {
    pub async fn new(source: &str, params: &[&str], unmatched: bool, failed_refs: bool) -> Self {
        let ws = Workspace::new(&[
            (
                "Cargo.toml",
                "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
            ),
            ("src/main.rs", source),
        ]);
        let file = ws.path("src/main.rs");
        let decl = source.find("fn sum").unwrap() + 3;
        let at = position(source, decl);
        let mut references = HashMap::new();
        let mut hovers = HashMap::new();
        let mut calls: Vec<_> = occurrences(source, "sum")
            .into_iter()
            .filter(|o| *o != decl)
            .map(|o| position(source, o))
            .collect();
        if unmatched {
            calls.push((1, 1));
        }
        references.insert(at, answers::locations(&file, &calls));
        let (_, open, close) = prod_code_mcp::signature::param_span(source, decl).unwrap();
        let (_, declared) = prod_code_mcp::signature::parse_declared(&source[open..close]);
        for parameter in declared {
            let name = &parameter.name;
            let offsets = occurrences(source, name);
            let declaration = offsets[0];
            let ty = prod_code_mcp::parameter_object::type_of(&parameter.raw).unwrap();
            hovers.insert(
                position(source, declaration),
                answers::hover(&format!("```rust\n{name}: {ty}\n```\nno Drop")),
            );
            references.insert(
                position(source, declaration),
                if failed_refs {
                    answers::failure("controlled reference failure")
                } else {
                    answers::locations(
                        &file,
                        &offsets[1..]
                            .iter()
                            .map(|o| position(source, *o))
                            .collect::<Vec<_>>(),
                    )
                },
            );
        }
        let overlay = Arc::new(Mutex::new(source.to_string()));
        let cache = Arc::new(Mutex::new(HashMap::<String, Value>::new()));
        let gateway = ScriptedGateway::start(move |method, p| {
            let point = (
                p["position"]["line"].as_u64().unwrap_or(0) as u32 + 1,
                p["position"]["character"].as_u64().unwrap_or(0) as u32 + 1,
            );
            match method {
                "textDocument/references" => references
                    .get(&point)
                    .unwrap_or_else(|| panic!("unexpected references at {point:?}"))
                    .clone(),
                "textDocument/hover" => hovers
                    .get(&point)
                    .unwrap_or_else(|| panic!("unexpected hover at {point:?}"))
                    .clone(),
                "textDocument/didOpen" | "textDocument/didChange" => {
                    let text = p
                        .pointer("/textDocument/text")
                        .or_else(|| p.pointer("/contentChanges/0/text"))
                        .and_then(Value::as_str)
                        .unwrap();
                    *overlay.lock().unwrap() = text.to_string();
                    Value::Null
                }
                "textDocument/diagnostic" => {
                    let source = overlay.lock().unwrap().clone();
                    cache
                        .lock()
                        .unwrap()
                        .entry(source.clone())
                        .or_insert_with(|| compile(&source, false).diagnostics)
                        .clone()
                }
                _ => Value::Null,
            }
        })
        .await;
        Self {
            ws,
            gateway,
            at,
            params: params.iter().map(|p| p.to_string()).collect(),
        }
    }

    pub async fn introduce(
        &self,
        apply: bool,
    ) -> anyhow::Result<prod_code_mcp::parameter_object::ParameterObject> {
        prod_code_mcp::parameter_object::introduce(
            self.gateway.addr(),
            &self.ws.root(),
            &self.ws.path("src/main.rs"),
            self.at.0,
            self.at.1,
            &self.params,
            "Pair",
            "pair",
            apply,
            false,
        )
        .await
    }
}

pub async fn preserves(
    label: &str,
    source: &str,
    params: &[&str],
    output: &str,
    fragments: &[&str],
) {
    let before = evidence(&format!("{label} original"), source);
    assert_eq!(before.status, 0);
    assert_eq!(before.stdout, output);
    let fixture = Fixture::new(source, params, false, false).await;
    let planned = fixture.introduce(false).await.expect("planning succeeds");
    assert!(!planned.applied);
    assert_eq!(fixture.ws.read("src/main.rs"), source);
    let generated = &planned
        .rewritten
        .iter()
        .find(|(path, _)| path == fixture.ws.path("src/main.rs").to_str().unwrap())
        .unwrap()
        .1;
    let after = evidence(&format!("{label} generated"), generated);
    assert_eq!(after.status, 0, "generated program must compile");
    assert_eq!(after.stdout, before.stdout);
    assert!(planned.diagnostics.is_empty(), "{:?}", planned.diagnostics);
    for fragment in fragments {
        assert!(
            generated.contains(fragment),
            "missing {fragment:?}:\n{generated}"
        );
    }
    let applied = fixture
        .introduce(true)
        .await
        .expect("valid plan applies without force");
    assert!(applied.applied);
    assert_eq!(fixture.ws.read("src/main.rs"), *generated);
}
