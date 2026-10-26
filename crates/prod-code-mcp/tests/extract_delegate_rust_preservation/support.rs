/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_delegate::{Extracted, extract_delegate_rust};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::process::Command;
use std::sync::{Arc, Mutex};

pub const MANIFEST: &str =
    "[package]\nname = \"preservation\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
pub type Seen = Arc<Mutex<Vec<String>>>;

// Each needle ends at the semantic field token. No string/comment/name lookalikes are
// reported. The returned ranges and query positions are genuine zero-based UTF-16 LSP.
pub async fn gateway(ws: &Workspace, uses: &[(&str, &[&str])]) -> (ScriptedGateway, Seen) {
    gateway_case(ws, uses, None).await
}

pub async fn gateway_case(
    ws: &Workspace,
    uses: &[(&str, &[&str])],
    case: Option<&str>,
) -> (ScriptedGateway, Seen) {
    gateway_case_with_owner(ws, uses, case, None).await
}

pub async fn gateway_case_with_owner(
    ws: &Workspace,
    uses: &[(&str, &[&str])],
    case: Option<&str>,
    owner_module: Option<&str>,
) -> (ScriptedGateway, Seen) {
    let identity = owner_module.map(|module| format!("preservation::{module}::Account"));
    gateway_case_with_identity(ws, uses, case, identity.as_deref()).await
}

pub async fn gateway_case_with_identity(
    ws: &Workspace,
    uses: &[(&str, &[&str])],
    case: Option<&str>,
    owner_identity: Option<&str>,
) -> (ScriptedGateway, Seen) {
    let mut locations = Vec::new();
    for (file, needles) in uses {
        let source = ws.read(file);
        for needle in *needles {
            let start = source.find(needle).expect("reference needle") + needle.len() - 4;
            let line = source[..start].bytes().filter(|b| *b == b'\n').count();
            let column = source[..start]
                .rsplit('\n')
                .next()
                .unwrap()
                .encode_utf16()
                .count();
            locations.push(json!({
                "uri": url::Url::from_file_path(ws.path(file)).unwrap().to_string(),
                "range": { "start": { "line": line, "character": column },
                    "end": { "line": line, "character": column + 4 } }
            }));
        }
    }
    if let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") {
        // Paths are fixture-relative in the durable evidence, while the wire uses file URIs.
        let public: Vec<Value> = locations
            .iter()
            .cloned()
            .map(|mut location| {
                let uri = location["uri"].as_str().unwrap().to_owned();
                location["uri"] =
                    Value::String(uri.replace(&format!("file://{}/", ws.root().display()), ""));
                location
            })
            .collect();
        let id = if ws.read("src/account.rs").contains("mod r#async") {
            "raw-async"
        } else if ws
            .read("src/account.rs")
            .contains("let independent = other")
        {
            "alias-shadow"
        } else if ws.read("src/account.rs").contains("let this = self") {
            "self-alias"
        } else if ws.read("src/account.rs").contains("async fn") {
            "async"
        } else if ws.read("src/account.rs").contains("fn same") {
            "other"
        } else if ws.read("src/account.rs").contains("fn add") {
            "sync"
        } else if ws.read("src/account.rs").contains("String") {
            "lexical"
        } else if ws.path("src/main.rs").exists() {
            "external"
        } else {
            "refusals"
        };
        let dir = std::path::Path::new(&root).join(case.unwrap_or(id));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("references.json"),
            serde_json::to_string_pretty(&public).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.join("provenance.json"),
            serde_json::to_string_pretty(&json!({
                "case_id": case.unwrap_or(id),
                "kind": "fresh observed scripted gateway reference response; fixture-relative URI normalization only",
                "reference_count": public.len(),
                "reconstructed": false,
            })).unwrap(),
        ).unwrap();
    }
    let owner = ws.read("src/account.rs");
    let declaration = owner.find("city:").unwrap();
    let line = owner[..declaration].bytes().filter(|b| *b == b'\n').count();
    let column = owner[..declaration]
        .rsplit('\n')
        .next()
        .unwrap()
        .encode_utf16()
        .count();
    let uri = url::Url::from_file_path(ws.path("src/account.rs"))
        .unwrap()
        .to_string();
    let seen: Seen = Arc::default();
    let capture = seen.clone();
    let hover = owner_identity.map(|identity| {
        json!({
            "contents": {"kind": "markdown", "value": format!(
                "```rust\n{identity}\n```\n\n```rust\npub city: u32\n```"
            )}
        })
    });
    let hover_evidence = std::env::var("EXTRACT_DELEGATE_EVIDENCE")
        .ok()
        .and_then(|root| {
            case.map(|case| std::path::Path::new(&root).join(case).join("hover.json"))
        });
    let hover_records = Mutex::new(Vec::new());
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => {
            assert_eq!(params["textDocument"]["uri"], uri);
            assert_eq!(
                params["position"],
                json!({"line": line, "character": column})
            );
            assert_eq!(params["context"]["includeDeclaration"], false);
            Value::Array(locations.clone())
        }
        "textDocument/hover" => {
            assert!(locations.iter().any(|l| {
                l["uri"] == params["textDocument"]["uri"]
                    && l["range"]["start"] == params["position"]
            }), "hover must query an exact semantic field coordinate");
            let response = hover.clone().unwrap_or(Value::Null);
            if let Some(path) = &hover_evidence {
                let mut records = hover_records.lock().unwrap();
                records.push(json!({
                    "method": method,
                    "file": "src/client.rs",
                    "position": params["position"],
                    "response": response,
                    "provenance": "scripted owner identity, native-compiled fixture; not a live analyzer capture"
                }));
                std::fs::write(path, serde_json::to_string_pretty(&*records).unwrap()).unwrap();
            }
            response
        }
        "textDocument/diagnostic" => {
            capture
                .lock()
                .unwrap()
                .push(params["textDocument"]["uri"].as_str().unwrap().to_owned());
            answers::no_diagnostics()
        }
        _ => Value::Null,
    })
    .await;
    (gw, seen)
}

pub async fn extract(
    ws: &Workspace,
    gw: &ScriptedGateway,
    methods: &[&str],
) -> anyhow::Result<Extracted> {
    extract_delegate_rust(
        gw.addr(),
        &ws.root(),
        &ws.path("src/account.rs"),
        Some("Account"),
        None,
        None,
        &["city".into()],
        &methods.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
        "Address",
        "address",
        true,
        false,
    )
    .await
}

pub fn artifact(case: &str, stage: &str, ws: &Workspace) {
    let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") else {
        return;
    };
    let destination = std::path::Path::new(&root).join(case).join(stage);
    std::fs::create_dir_all(&destination).unwrap();
    std::fs::copy(ws.path("Cargo.toml"), destination.join("Cargo.toml")).unwrap();
    for file in [
        "account.rs",
        "main.rs",
        "client.rs",
        "lib.rs",
        "async.rs",
        "async/mod.rs",
        "async/client.rs",
    ] {
        let path = ws.path(&format!("src/{file}"));
        if path.exists() {
            std::fs::create_dir_all(destination.join(file).parent().unwrap()).unwrap();
            std::fs::copy(path, destination.join(file)).unwrap();
        }
    }
}

pub fn refusal(case: &str, error: &str, ws: &Workspace) {
    artifact(case, "after", ws);
    if let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") {
        let message = error.replace(&ws.root().to_string_lossy().to_string(), "<fixture>");
        std::fs::write(
            std::path::Path::new(&root).join(case).join("error.txt"),
            message,
        )
        .unwrap();
    }
}

pub fn run(case: &str, stage: &str, ws: &Workspace) -> String {
    artifact(case, stage, ws);
    let mut compiler = Command::new("rustc");
    compiler.args(["--edition=2021", "src/main.rs", "-o", "program"]);
    if ws.path("src/lib.rs").exists() {
        let library = Command::new("rustc")
            .args([
                "--edition=2021",
                "--crate-name",
                "preservation",
                "--crate-type",
                "lib",
                "src/lib.rs",
                "-o",
                "libpreservation.rlib",
            ])
            .current_dir(ws.root())
            .output()
            .expect("rustc library");
        if let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") {
            let diagnostics = String::from_utf8_lossy(&library.stderr)
                .replace(&ws.root().to_string_lossy().to_string(), "<fixture>");
            std::fs::write(
                std::path::Path::new(&root)
                    .join(case)
                    .join(stage)
                    .join("library-rustc.txt"),
                format!("exit: {}\n{diagnostics}", library.status),
            )
            .unwrap();
        }
        assert!(
            library.status.success(),
            "{}",
            String::from_utf8_lossy(&library.stderr)
        );
        compiler.args(["--extern", "preservation=libpreservation.rlib"]);
    }
    let output = compiler.current_dir(ws.root()).output().expect("rustc");
    let diagnostics = String::from_utf8_lossy(&output.stderr)
        .replace(&ws.root().to_string_lossy().to_string(), "<fixture>");
    if let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") {
        std::fs::write(
            std::path::Path::new(&root)
                .join(case)
                .join(stage)
                .join("rustc.txt"),
            format!("exit: {}\n{diagnostics}", output.status),
        )
        .unwrap();
    }
    assert!(output.status.success(), "{case}/{stage}: {diagnostics}");
    let result = Command::new(ws.path("program"))
        .output()
        .expect("run compiled fixture");
    assert!(result.status.success(), "{result:?}");
    let value = String::from_utf8(result.stdout).unwrap();
    if let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") {
        std::fs::write(
            std::path::Path::new(&root)
                .join(case)
                .join(stage)
                .join("runtime.txt"),
            format!("exit: {}\n", result.status),
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&root)
                .join(case)
                .join(stage)
                .join("stdout.txt"),
            &value,
        )
        .unwrap();
    }
    value
}
