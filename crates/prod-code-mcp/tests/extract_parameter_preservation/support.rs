/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_parameter::{ExtractedParameter, extract};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::json;
use std::process::Command;

pub fn workspace(source: &str, ext: &str) -> Workspace {
    // This target creates many synchronized fixtures and compiler subprocesses.
    // Raise only its own soft limit; never change the host or the hard limit.
    #[cfg(unix)]
    {
        static LIMIT: std::sync::Once = std::sync::Once::new();
        LIMIT.call_once(|| {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: both calls receive a valid initialized rlimit pointer.
            assert_eq!(
                unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
                0
            );
            let before = limit.rlim_cur;
            if before < 8192 {
                limit.rlim_cur = limit.rlim_max.min(8192);
                assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) }, 0);
            }
            println!(
                "fixture RLIMIT_NOFILE soft={before}->{} hard={}",
                limit.rlim_cur, limit.rlim_max
            );
        });
    }
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (&format!("main.{ext}"), source),
    ])
}

pub fn position(source: &str, at: usize) -> (u32, u32) {
    prod_code_mcp::signature::position_at(source, at).unwrap()
}

#[allow(clippy::too_many_arguments)]
pub async fn change(
    ws: &Workspace,
    source: &str,
    ext: &str,
    needle: &str,
    all: bool,
    apply: bool,
    force: bool,
    unmatched: bool,
) -> anyhow::Result<ExtractedParameter> {
    change_typed(
        ws,
        source,
        ext,
        needle,
        all,
        apply,
        force,
        unmatched,
        if ext == "go" { "int" } else { "i32" },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn change_typed(
    ws: &Workspace,
    source: &str,
    ext: &str,
    needle: &str,
    all: bool,
    apply: bool,
    force: bool,
    unmatched: bool,
    ty: &str,
) -> anyhow::Result<ExtractedParameter> {
    change_resolved(
        ws,
        source,
        ext,
        needle,
        all,
        apply,
        force,
        unmatched,
        ty,
        &[],
    )
    .await
}

/// Explicit fixture use/declaration byte offsets. These are scripted analyzer
/// answers, not a replacement C/C++ parser or evidence of a live clangd session.
#[allow(clippy::too_many_arguments)]
pub async fn change_resolved(
    ws: &Workspace,
    source: &str,
    ext: &str,
    needle: &str,
    all: bool,
    apply: bool,
    force: bool,
    unmatched: bool,
    ty: &str,
    definitions: &[(usize, usize)],
) -> anyhow::Result<ExtractedParameter> {
    change_analyzed(
        ws,
        source,
        ext,
        needle,
        all,
        apply,
        force,
        unmatched,
        ty,
        definitions,
        &[],
    )
    .await
}

/// Explicit hover answers, keyed by byte position, for declaration/macro controls.
/// These deliberately simulate clangd; the compiler checks remain independent.
#[allow(clippy::too_many_arguments)]
pub async fn change_analyzed(
    ws: &Workspace,
    source: &str,
    ext: &str,
    needle: &str,
    all: bool,
    apply: bool,
    force: bool,
    unmatched: bool,
    ty: &str,
    definitions: &[(usize, usize)],
    hovers: &[(usize, &str)],
) -> anyhow::Result<ExtractedParameter> {
    change_with_ast(
        ws,
        source,
        ext,
        needle,
        all,
        apply,
        force,
        unmatched,
        ty,
        definitions,
        hovers,
        &[],
    )
    .await
}

/// Clangd AST replies are explicit fixtures, not live analyzer evidence.
#[allow(clippy::too_many_arguments)]
pub async fn change_with_ast(
    ws: &Workspace,
    source: &str,
    ext: &str,
    needle: &str,
    all: bool,
    apply: bool,
    force: bool,
    unmatched: bool,
    ty: &str,
    definitions: &[(usize, usize)],
    hovers: &[(usize, &str)],
    asts: &[(usize, serde_json::Value)],
) -> anyhow::Result<ExtractedParameter> {
    let file = ws.path(&format!("main.{ext}"));
    let name_at = source.find("target").unwrap();
    let body_end = if ext == "py" {
        // The declaration ends before the line terminator, including CRLF.
        source[..source.find("\nprint(").unwrap()]
            .trim_end_matches('\r')
            .len()
    } else if source.starts_with("class C {") {
        // Class fixtures put target (the first multiline member) before the
        // class closing brace. Report the method boundary, not its class scope.
        source[name_at..].find("\n}\n").unwrap() + name_at + 2
    } else {
        source
            .rfind("\n}\n")
            .map(|at| at + 2)
            .unwrap_or_else(|| source.find('}').unwrap() + 1)
    };
    let (el, ec) = position(source, body_end);
    let (nl, nc) = position(source, name_at);
    let symbols = json!([{"name":"target", "kind":12,
        "range":{"start":{"line":0,"character":0},"end":{"line":el-1,"character":ec-1}},
        "selectionRange":{"start":{"line":nl-1,"character":nc-1},"end":{"line":nl-1,"character":nc+5}}}]);
    let mut refs = source
        .match_indices("target")
        .skip(1)
        .map(|(at, _)| position(source, at))
        .collect::<Vec<_>>();
    if unmatched {
        refs.push(position(source, source.len() - 2));
    }
    let refs = answers::locations(&file, &refs);
    let definitions = definitions
        .iter()
        .map(|(at, declaration)| {
            let (line, col) = position(source, *at);
            (
                (line - 1, col - 1),
                answers::locations(&file, &[position(source, *declaration)]),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let hovers = hovers
        .iter()
        .map(|(at, value)| {
            let (line, col) = position(source, *at);
            (
                (line - 1, col - 1),
                json!({"contents": {"kind": "markdown", "value": value}}),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let asts = asts
        .iter()
        .map(|(at, ast)| {
            let (line, col) = position(source, *at);
            ((line - 1, col - 1), ast.clone())
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => symbols.clone(),
        "textDocument/references" => refs.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/definition" => {
            let at = (
                params["position"]["line"].as_u64().unwrap() as u32,
                params["position"]["character"].as_u64().unwrap() as u32,
            );
            definitions
                .get(&at)
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        }
        "textDocument/hover" => hovers
            .get(&(
                params["position"]["line"].as_u64().unwrap() as u32,
                params["position"]["character"].as_u64().unwrap() as u32,
            ))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        "textDocument/ast" => asts
            .get(&(
                params["range"]["start"]["line"].as_u64().unwrap() as u32,
                params["range"]["start"]["character"].as_u64().unwrap() as u32,
            ))
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    })
    .await;
    let (needle, len) = needle
        .strip_prefix("select:")
        .map_or((needle, 2), |s| (s, s.len()));
    let at = source.find(needle).unwrap();
    extract(
        gateway.addr(),
        &ws.root(),
        &file,
        position(source, at),
        position(source, at + len),
        "limit",
        Some(ty),
        all,
        apply,
        force,
    )
    .await
}

/// Runs on the build node, independently of the scripted analyzer diagnostics.
pub fn run(source: &str, ext: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join(format!("main.{ext}"));
    std::fs::write(&input, source).unwrap();
    let output = if ext == "rs" {
        let bin = dir.path().join("program");
        let compiled = Command::new("rustc")
            .args(["--edition=2021", "-A", "warnings"])
            .arg(&input)
            .arg("-o")
            .arg(&bin)
            .output()
            .unwrap();
        println!(
            "rustc: {}\n{}",
            compiled.status,
            String::from_utf8_lossy(&compiled.stderr)
        );
        assert!(compiled.status.success());
        Command::new(bin).output().unwrap()
    } else if matches!(ext, "c" | "cpp" | "h") {
        let bin = dir.path().join("program");
        let compiler = if ext == "c" { "cc" } else { "c++" };
        let standard = if ext == "c" { "-std=c11" } else { "-std=c++17" };
        let mut command = Command::new(compiler);
        command.args([standard, "-Wall", "-Wextra"]);
        if ext == "h" {
            command.args(["-x", "c++"]);
        }
        let compiled = command.arg(&input).arg("-o").arg(&bin).output().unwrap();
        println!(
            "{compiler}: {}\n{}",
            compiled.status,
            String::from_utf8_lossy(&compiled.stderr)
        );
        assert!(compiled.status.success());
        Command::new(bin).output().unwrap()
    } else if ext == "ts" {
        let compiled = Command::new("tsc")
            .args([
                "--target", "es2022", "--module", "commonjs", "--pretty", "false", "--outDir",
            ])
            .arg(dir.path().join("out"))
            .arg(&input)
            .output()
            .unwrap();
        println!(
            "tsc: {}\n{}{}",
            compiled.status,
            String::from_utf8_lossy(&compiled.stdout),
            String::from_utf8_lossy(&compiled.stderr)
        );
        assert!(compiled.status.success());
        Command::new("node")
            .arg(dir.path().join("out/main.js"))
            .output()
            .unwrap()
    } else if ext == "go" {
        Command::new("go")
            .arg("run")
            .arg(input)
            .env("GOWORK", "off")
            .output()
            .unwrap()
    } else if ext == "py" {
        Command::new("python3").arg(input).output().unwrap()
    } else {
        Command::new("node").arg(input).output().unwrap()
    };
    println!(
        "runtime: {}\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}
