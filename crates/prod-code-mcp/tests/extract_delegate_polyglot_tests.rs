/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_delegate::extract_delegate_polyglot;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::fs;

#[path = "extract_delegate_polyglot_tests/classes.rs"]
mod classes;
#[path = "extract_delegate_polyglot_tests/semantic.rs"]
mod semantic;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_extract_delegate_exported_go_literals_from_another_package() {
    use semantic::*;
    let owner = "package server\n\ntype Server struct {\n    Host string\n    Port int\n    MaxClients int\n}\n\nfunc (s *Server) Address() string {\n    return s.Host\n}\n";
    let caller = "package main\n\nimport \"example/server\"\n\nfunc run() {\n    srv := &server.Server{\n        Host: \"localhost\",\n        Port: 8080,\n        MaxClients: 100,\n    }\n    println(srv.Host)\n    println(srv.Port)\n    println(srv.Address())\n}\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("go.mod", "module example\n\ngo 1.20\n"),
        ("server/server.go", owner),
        ("main.go", caller),
    ]);
    let file = ws.path("server/server.go");
    let client = ws.path("main.go");
    let fields = ["Host", "Port"]
        .iter()
        .map(|name| {
            let mut refs = references(
                &client,
                caller,
                name,
                &[&format!("{name}:"), &format!("srv.{name}")],
            );
            if *name == "Host" {
                refs.extend(references(&file, owner, name, &["s.Host"]));
            }
            field(
                &file,
                owner,
                name,
                &format!("{name} {}", if *name == "Host" { "string" } else { "int" }),
                refs,
            )
        })
        .collect();
    let gw = semantic_gateway(&file, "Server", fields).await;
    let res = extract_delegate_polyglot(
        gw.addr(),
        &ws.root(),
        &file,
        Some("Server"),
        None,
        None,
        &["Host".into(), "Port".into()],
        &["Address".into()],
        "Endpoint",
        "Endpoint",
        true,
        false,
        None,
    )
    .await
    .unwrap();
    assert!(res.applied);
    assert_eq!(res.helper, "Endpoint");
    assert_eq!(res.field, "Endpoint");
    assert_eq!(res.accesses, 2);
    let owner = fs::read_to_string(file).unwrap();
    assert!(owner.contains("type Endpoint struct"), "{owner}");
    assert!(owner.contains("return s.Endpoint.Address()"), "{owner}");
    let caller = fs::read_to_string(client).unwrap();
    for expected in [
        "Endpoint: server.Endpoint{",
        "Host: \"localhost\"",
        "Port: 8080",
        "MaxClients: 100",
        "println(srv.Endpoint.Host)",
        "println(srv.Endpoint.Port)",
        "println(srv.Address())",
    ] {
        assert!(caller.contains(expected), "{expected}: {caller}");
    }
}

#[tokio::test]
async fn test_extract_delegate_safety_checks_refused() {
    let source = "export class Model {\n    id: string;\n    title: string;\n    score: number;\n\n    display(): string {\n        return `${this.id}: ${this.title} (${this.score})`;\n    }\n}\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("model.ts", source)]);
    let file = ws.path("model.ts");
    let gw = fake_gateway().await;
    for (fields, methods, delegate, expected) in [
        (
            vec!["id".into(), "title".into()],
            vec!["display".into()],
            "meta",
            "uses `score`, which does not move",
        ),
        (
            vec!["nonexistent".into()],
            vec![],
            "meta",
            "has no field `nonexistent`",
        ),
        (
            vec!["id".into()],
            vec!["nonexistentMethod".into()],
            "meta",
            "has no method `nonexistentMethod`",
        ),
        (
            vec!["id".into()],
            vec![],
            "title",
            "already has a field `title`",
        ),
    ] {
        let error = extract_delegate_polyglot(
            gw.addr(),
            &ws.root(),
            &file,
            Some("Model"),
            None,
            None,
            &fields,
            &methods,
            "Metadata",
            delegate,
            false,
            false,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error:#}");
        assert_eq!(fs::read_to_string(&file).unwrap(), source);
    }
}

#[tokio::test]
async fn test_extract_delegate_via_mcp_execute_tool() {
    use semantic::*;
    let source = "class Service:\n    def __init__(self, host: str, port: int, name: str):\n        self.host = host\n        self.port = port\n        self.name = name\n\n    def url(self) -> str:\n        return f\"{self.host}:{self.port}\"\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("service.py", source)]);
    let file = ws.path("service.py");
    let fields = ["host", "port"]
        .iter()
        .map(|name| {
            field(
                &file,
                source,
                name,
                &format!("self.{name} ="),
                references(&file, source, name, &[&format!("{{self.{name}}}")]),
            )
        })
        .collect();
    let gw = semantic_gateway(&file, "Service", fields).await;
    let args = serde_json::json!({"path": "service.py", "symbol": "Service", "fields": ["host", "port"],
        "methods": ["url"], "name": "Endpoint", "field": "endpoint", "apply": true});
    let result =
        prod_code_mcp::tools::execute_tool(gw.addr(), &ws.root(), "code_extract_delegate", args)
            .await
            .unwrap();
    assert!(!result.is_error, "{result:?}");
    let content = fs::read_to_string(file).unwrap();
    assert!(content.contains("self.endpoint = Endpoint(host, port)"));
    assert!(content.contains("class Endpoint:"));
    assert!(content.contains("return self.endpoint.url()"));
}
