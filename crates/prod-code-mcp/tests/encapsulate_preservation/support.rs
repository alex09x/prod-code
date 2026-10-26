/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Clone, Copy)]
pub struct TestGateway(SocketAddr);

impl TestGateway {
    pub fn addr(&self) -> SocketAddr {
        self.0
    }
}

type GatewayAnswers = Arc<Mutex<(Value, Value)>>;
type GatewayMap = Mutex<HashMap<PathBuf, (SocketAddr, GatewayAnswers)>>;

// Write evidence directly so the exact acceptance command also retains successful probes.
pub fn emit(text: String) {
    std::io::stdout().lock().write_all(text.as_bytes()).unwrap();
}

// Each anchor names a particular fixture expression, never all same-spelled properties.
// The scripted semantic locations/diagnostics are not evidence of a real LSP's behavior.
pub fn spot(text: &str, expression: &str, field: &str) -> (u32, u32) {
    let start = text.find(expression).expect("explicit expression anchor");
    assert_eq!(text.matches(expression).count(), 1, "unique fixture anchor");
    let at = start + expression.find(field).unwrap();
    let prefix = &text[..at];
    (
        prefix.bytes().filter(|b| *b == b'\n').count() as u32 + 1,
        prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32 + 1,
    )
}

pub async fn gateway(
    ws: &Workspace,
    owner: &str,
    field: &str,
    declaration: (&str, (u32, u32)),
    refs: &[(&str, (u32, u32))],
) -> TestGateway {
    static BY_ROOT: OnceLock<GatewayMap> = OnceLock::new();
    let locations: Vec<Value> = refs
        .iter()
        .flat_map(|(file, pos)| {
            answers::locations(&ws.path(file), &[*pos])
                .as_array()
                .unwrap()
                .clone()
        })
        .collect();
    let (line, col) = declaration.1;
    let symbols = json!([answers::nested(
        answers::document_symbol(owner, 5, 1, 100, 1),
        vec![answers::document_symbol(field, 8, line, line, col)],
    )]);
    let map = BY_ROOT.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((addr, state)) = map.lock().unwrap().get(&ws.root()).cloned() {
        *state.lock().unwrap() = (symbols, json!(locations));
        return TestGateway(addr);
    }
    let state: GatewayAnswers = Arc::new(Mutex::new((symbols, json!(locations))));
    let shared = Arc::clone(&state);
    let gw = ScriptedGateway::start(move |method, _| {
        let (symbols, locations) = shared.lock().unwrap().clone();
        match method {
            "textDocument/documentSymbol" => symbols,
            "textDocument/references" => locations,
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        }
    })
    .await;
    let addr = gw.addr();
    map.lock().unwrap().insert(ws.root(), (addr, state));
    TestGateway(addr)
}

pub fn run(ws: &Workspace, program: &str, args: &[&str]) -> std::process::Output {
    let out = Command::new(program)
        .args(args)
        .current_dir(ws.root())
        .output()
        .expect("required real runtime/compiler");
    emit(format!(
        "COMMAND {program} {}\nSTATUS {}\nSTDOUT {}\nSTDERR {}\n",
        args.join(" "),
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ));
    out
}

pub fn rust_runtime(ws: &Workspace) {
    emit(format!("EMITTED main.rs:\n{}\n", ws.read("main.rs")));
    if ws.path("model.rs").exists() {
        emit(format!("EMITTED model.rs:\n{}\n", ws.read("model.rs")));
    }
    let compile = run(ws, "rustc", &["--edition=2024", "main.rs", "-o", "probe"]);
    assert!(
        compile.status.success(),
        "real rustc must accept generated source"
    );
    let output = run(ws, "./probe", &[]);
    assert!(output.status.success(), "runtime assertions must hold");
}
