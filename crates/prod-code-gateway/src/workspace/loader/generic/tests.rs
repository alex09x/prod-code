/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn every_managed_language_has_its_label_and_nothing_else_does() {
    let labels = [
        ("python", "Python"),
        ("cpp", "C/C++ clangd"),
        ("swift", "Swift sourcekit-lsp"),
        ("typescript", "TypeScript"),
        ("java", "Java"),
        ("kotlin", "Kotlin"),
        ("csharp", "C#"),
        ("php", "PHP"),
        ("ruby", "Ruby"),
        ("dart", "Dart"),
        ("zig", "Zig"),
        ("elixir", "Elixir"),
        ("scala", "Scala"),
        ("lua", "Lua"),
        ("haskell", "Haskell"),
        ("ocaml", "OCaml"),
        ("clojure", "Clojure"),
        ("julia", "Julia"),
        ("shell", "Shell"),
        ("r", "R"),
        ("erlang", "Erlang"),
        ("fsharp", "F#"),
        ("perl", "Perl"),
        ("solidity", "Solidity"),
        ("nim", "Nim"),
        ("d", "D"),
        ("fortran", "Fortran"),
        ("sql", "SQL"),
        ("graphql", "GraphQL"),
        ("protobuf", "Protobuf"),
        ("crystal", "Crystal"),
        ("groovy", "Groovy"),
        ("ada", "Ada"),
        ("v", "V"),
        ("racket", "Racket"),
        ("terraform", "Terraform"),
        ("nix", "Nix"),
        ("markdown", "Markdown"),
        ("yaml", "YAML"),
        ("toml", "TOML"),
        ("json", "JSON"),
        ("html", "HTML"),
        ("css", "CSS"),
        ("dockerfile", "Dockerfile"),
        ("svelte", "Svelte"),
        ("vue", "Vue"),
        ("assembly", "Assembly"),
    ];
    for (engine, label) in labels {
        let (_, got) = generic_lsp_config(engine).unwrap_or_else(|| panic!("{engine} unmanaged"));
        assert_eq!(got, label, "{engine}");
    }
    // The Rust and Go engines have their own loaders, and names are matched exactly.
    for other in ["rust", "go", "", "Python", "python ", "unknown", "c++"] {
        assert!(generic_lsp_config(other).is_none(), "{other:?}");
    }
}

#[tokio::test]
async fn an_unmanaged_engine_loads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_generic(dir.path(), "rust").await.is_none());
    assert!(load_generic(dir.path(), "").await.is_none());
}

/// A language server that is not installed fails the load with its startup error, instead of
/// leaving a workspace with no engine: the subprocess fallback only exists for the Rust and Go
/// servers (#1427).
#[tokio::test]
async fn a_server_that_cannot_start_fails_the_load() {
    let dir = tempfile::tempdir().unwrap();
    match load_generic(dir.path(), "ada").await {
        Some(Err(err)) => {
            let text = format!("{err:#}");
            assert!(
                text.contains("the Ada language server did not start"),
                "{text}"
            );
            assert!(
                text.contains("no managed backend language server"),
                "{text}"
            );
        }
        Some(Ok(_)) => panic!("no ada language server on a build node"),
        None => panic!("ada is a managed language"),
    }
}

/// Once the cause is fixed, the next load starts the server: a failed load leaves nothing
/// behind to reuse (#1427).
#[tokio::test]
async fn a_load_after_the_server_is_fixed_starts_it() {
    let dir = tempfile::tempdir().unwrap();
    let broken = GenericLspConfig {
        command: dir
            .path()
            .join("missing-server")
            .to_string_lossy()
            .into_owned(),
        ..Default::default()
    };
    let Err(err) = load_generic_with(dir.path(), "python", broken, "Python").await else {
        panic!("a server that is not there starts nothing");
    };
    assert!(format!("{err:#}").contains("did not start"), "{err:#}");

    let script = dir.path().join("server.py");
    std::fs::write(
        &script,
        r#"import json, sys
def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.strip()
        if not line: break
        if line.lower().startswith(b"content-length:"): length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length)) if length else None
while True:
    message = read()
    if message is None: break
    if message.get("method") == "initialize":
        body = json.dumps({"jsonrpc":"2.0","id":message["id"],"result":{"capabilities":{}}}).encode()
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        sys.stdout.buffer.flush()
"#,
    )
    .unwrap();
    let fixed = GenericLspConfig {
        command: "python3".to_string(),
        args: vec![script.to_string_lossy().into_owned()],
        ..Default::default()
    };
    let (engine, worker) = load_generic_with(dir.path(), "python", fixed, "Python")
        .await
        .expect("the fixed server starts");
    assert!(engine.is_some_and(|e| e.is_alive()));
    assert!(worker.is_none());
}
