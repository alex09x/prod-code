/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_testkit::Workspace;
use std::fs;

#[tokio::test]
async fn python_callers_before_after_and_remaining_method() {
    let source = "def before(a):\n    return a.city\nclass Account:\n    def __init__(self, city):\n        self.city = city\n    def read(self):\n        return self.city\ndef after(a):\n    return a.city\na = Account('Paris')\nprint(before(a), a.read(), after(a))\n";
    let ws = Workspace::new(&[("account.py", source)]);
    let file = ws.path("account.py");
    let refs = vec![
        at(&file, source, "return a.city\nclass", "city"),
        at(&file, source, "self.city = city", "city"),
        at(&file, source, "return self.city", "city"),
        at(&file, source, "return a.city\na =", "city"),
    ];
    let gw = gateway(&file, source, "Account", "city", "self.city = city", refs).await;
    let before = run(&ws, "python3", &["account.py"]);
    extract(
        &ws,
        &gw,
        "account.py",
        "Account",
        "city",
        &[],
        "Address",
        "address",
        true,
    )
    .await
    .unwrap();
    let after = fs::read_to_string(file).unwrap();
    assert_eq!(after.matches("a.address.city").count(), 2, "{after}");
    assert_eq!(run(&ws, "python3", &["account.py"]), before);
    evidence("declaring.py", source, &after, &before);
}

#[tokio::test]
async fn go_exported_literal_uses_actual_import_alias() {
    let owner = "package server\ntype Server struct {\n    Host string `json:\"host\"`\n}\nfunc (s Server) Read() string { return s.Host }\n";
    let caller = "package main\nimport (\n    \"fmt\"\n    srv \"example/server\"\n)\nfunc main() {\n    s := srv.Server{Host: \"Paris\"}\n    other := struct{ Host string }{Host: \"Rome\"}\n    fmt.Println(s.Host, other.Host, s.Read())\n}\n";
    let ws = Workspace::new(&[
        ("go.mod", "module example\n\ngo 1.20\n"),
        ("server/server.go", owner),
        ("main.go", caller),
    ]);
    let file = ws.path("server/server.go");
    let refs = vec![
        at(&file, owner, "s.Host", "Host"),
        at(&ws.path("main.go"), caller, "Host: \"Paris\"", "Host"),
        at(&ws.path("main.go"), caller, "s.Host", "Host"),
    ];
    let gw = gateway(&file, owner, "Server", "Host", "Host string", refs).await;
    let before = run(&ws, "go", &["run", "."]);
    extract(
        &ws,
        &gw,
        "server/server.go",
        "Server",
        "Host",
        &[],
        "Endpoint",
        "Endpoint",
        true,
    )
    .await
    .unwrap();
    let after_owner = fs::read_to_string(file).unwrap();
    let after = fs::read_to_string(ws.path("main.go")).unwrap();
    assert!(after.contains("Endpoint: srv.Endpoint{"), "{after}");
    assert!(after.contains("other.Host"), "{after}");
    assert!(
        after_owner.contains("Host string `json:\"host\"`"),
        "{after_owner}"
    );
    assert_eq!(run(&ws, "go", &["run", "."]), before);
    evidence("server.go", owner, &after_owner, &before);
    evidence("main.go", caller, &after, &before);
}

#[tokio::test]
async fn go_default_import_and_same_package_literals_are_preserved() {
    let owner = "package server\ntype Server struct {\n    Host string\n}\nfunc New() Server { return Server{Host: \"Paris\"} }\n";
    let caller = "package main\nimport (\n    \"fmt\"\n    \"example/server\"\n)\nfunc main() {\n    s := server.Server{Host: \"Rome\"}\n    fmt.Println(s.Host, server.New().Host)\n}\n";
    let ws = Workspace::new(&[
        ("go.mod", "module example\n\ngo 1.20\n"),
        ("server/server.go", owner),
        ("main.go", caller),
    ]);
    let file = ws.path("server/server.go");
    let refs = vec![
        at(&file, owner, "Host: \"Paris\"", "Host"),
        at(&ws.path("main.go"), caller, "Host: \"Rome\"", "Host"),
        at(&ws.path("main.go"), caller, "s.Host", "Host"),
        at(&ws.path("main.go"), caller, "New().Host", "Host"),
    ];
    let gw = gateway(&file, owner, "Server", "Host", "Host string", refs).await;
    let before = run(&ws, "go", &["run", "."]);
    extract(
        &ws,
        &gw,
        "server/server.go",
        "Server",
        "Host",
        &[],
        "Endpoint",
        "Endpoint",
        true,
    )
    .await
    .unwrap();
    let after = fs::read_to_string(ws.path("main.go")).unwrap();
    assert!(after.contains("Endpoint: server.Endpoint{"), "{after}");
    assert_eq!(run(&ws, "go", &["run", "."]), before);
    evidence("default-main.go", caller, &after, &before);
    evidence(
        "default-server.go",
        owner,
        &fs::read_to_string(file).unwrap(),
        &before,
    );
}
