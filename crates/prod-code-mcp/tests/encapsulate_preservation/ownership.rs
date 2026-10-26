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
use prod_code_mcp::encapsulate_field::{encapsulate, encapsulate_polyglot};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;

#[tokio::test]
async fn cpp_internal_callable_occurrences_follow_storage_without_touching_other_owners() {
    let source = "using Fn = int(*)();\nint answer() { return 3; }\nclass Box {\npublic:\n    Fn count;\n    int value() { return count() + this -> count(); }\n};\nstruct Other { Fn count = answer; };\nint main() {\n    Box b; b.count = answer;\n    Other other;\n    const char *quoted = \"count()\"; // count()\n    return b.value() == 6 && other.count() == 3 && quoted[0] == 'c' ? 0 : 1;\n}\n";
    let ws = Workspace::new(&[("main.cpp", source)]);
    assert!(
        run(&ws, "c++", &["main.cpp", "-o", "probe"])
            .status
            .success()
    );
    assert!(run(&ws, "./probe", &[]).status.success());
    let refs = ["return count()", "this -> count()", "b.count ="]
        .map(|anchor| ("main.cpp", spot(source, anchor, "count")));
    let gw = gateway(
        &ws,
        "Box",
        "count",
        ("main.cpp", spot(source, "Fn count;", "count")),
        &refs,
    )
    .await;
    let preview = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("main.cpp"),
        Some("Box"),
        "count",
        None,
        false,
        true,
    )
    .await
    .unwrap();
    assert!(!preview.applied);
    assert_eq!(ws.read("main.cpp"), source);
    let done = encapsulate_polyglot(
        gw.addr(),
        &ws.root(),
        &ws.path("main.cpp"),
        Some("Box"),
        "count",
        None,
        true,
        true,
    )
    .await
    .unwrap();
    emit(format!(
        "CPP INTERNAL applied={} unmatched={:?}\nEMITTED main.cpp:\n{}\n",
        done.applied,
        done.unmatched,
        ws.read("main.cpp")
    ));
    assert!(
        run(&ws, "c++", &["main.cpp", "-o", "probe"])
            .status
            .success(),
        "selected internal callable must compile or refuse before writes"
    );
    assert!(run(&ws, "./probe", &[]).status.success());
    assert!(
        done.applied,
        "ordinary internal function-pointer access is supported"
    );
    assert!(done.unmatched.is_empty());
    assert!(
        ws.read("main.cpp")
            .contains("return count_() + this -> count_();")
    );
    assert!(ws.read("main.cpp").contains("other.count()"));
    assert!(ws.read("main.cpp").contains("\"count()\"; // count()"));
}

async fn rust_collision_case(target: &str, force: bool) {
    let source = format!(
        "mod inner {{\n    pub struct Config {{\n        pub retries: i32,\n    }}\n    pub fn new() -> Config {{ Config {{ retries: 3 }} }}\n    pub mod child {{\n        use super::Config as Alias;\n        use super::Config;\n        impl {target} {{\n            pub fn retries(&self) -> i32 {{ self.retries }}\n        }}\n    }}\n}}\nfn main() {{\n    let cfg = inner::new();\n    assert_eq!(cfg.retries(), 3);\n    println!(\"existing descendant accessor=3\");\n}}\n"
    );
    // Qualified ownership also works when the impl follows a use on the same line.
    let source = if target == "super::Config" {
        source.replace(
            "use super::Config;\n        impl",
            "use super::Config; impl",
        )
    } else {
        source
    };
    let ws = Workspace::new(&[("main.rs", &source)]);
    rust_runtime(&ws);
    let decl = spot(&source, "pub retries:", "retries");
    let owner = spot(&source, "struct Config", "Config");
    let locations = answers::locations(
        &ws.path("main.rs"),
        &[
            spot(&source, "self.retries", "retries"),
            spot(&source, "Config { retries:", "retries"),
        ],
    );
    let definition = answers::locations(&ws.path("main.rs"), &[owner]);
    let terminal = target.rsplit("::").next().unwrap();
    let impl_owner = spot(&source, &format!("impl {target}"), terminal);
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => locations.clone(),
        "textDocument/definition" => {
            assert_eq!(params["position"]["line"], impl_owner.0 - 1);
            assert_eq!(params["position"]["character"], impl_owner.1 - 1);
            definition.clone()
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    for apply in [false, true] {
        let result = encapsulate(
            gw.addr(),
            &ws.root(),
            &ws.path("main.rs"),
            decl.0,
            decl.1,
            None,
            apply,
            force,
        )
        .await;
        emit(format!(
            "RUST COLLISION target={target} force={force} apply={apply} result={result:?}\nEMITTED main.rs:\n{}\n",
            ws.read("main.rs")
        ));
        if let Ok(plan) = &result {
            ws.write("candidate.rs", &plan.rewritten[0].1);
            assert!(
                run(
                    &ws,
                    "rustc",
                    &["--edition=2024", "candidate.rs", "-o", "candidate"]
                )
                .status
                .success(),
                "unsafe collision proposal must be rejected before generation"
            );
        }
        assert!(
            result.is_err(),
            "existing descendant accessor must refuse before writes"
        );
        assert!(format!("{:#}", result.unwrap_err()).contains("already has a `fn retries`"));
        assert_eq!(ws.read("main.rs"), source);
    }
}

#[tokio::test]
async fn rust_descendant_accessor_collision_refuses() {
    rust_collision_case("Config", true).await;
}

#[tokio::test]
async fn rust_aliased_and_qualified_descendant_accessor_collisions_refuse() {
    rust_collision_case("Alias", false).await;
    rust_collision_case("super::Config", true).await;
}

#[tokio::test]
async fn rust_unrelated_descendant_type_does_not_collide() {
    let source = "mod inner {\n    pub struct Config {\n        pub retries: i32,\n    }\n    pub fn new() -> Config { Config { retries: 3 } }\n    pub mod child {\n        pub struct Config;\n        impl Config {\n            pub fn retries(&self) -> i32 { 99 }\n        }\n    }\n}\nfn main() {\n    let cfg = inner::new();\n    assert_eq!(cfg.retries, 3);\n    assert_eq!(inner::child::Config.retries(), 99);\n}\n";
    let ws = Workspace::new(&[("main.rs", source)]);
    rust_runtime(&ws);
    let decl = spot(source, "pub retries:", "retries");
    let locations = answers::locations(
        &ws.path("main.rs"),
        &[
            spot(source, "cfg.retries,", "retries"),
            spot(source, "Config { retries:", "retries"),
        ],
    );
    let definition = answers::locations(
        &ws.path("main.rs"),
        &[spot(source, "struct Config;", "Config")],
    );
    let gw = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => locations.clone(),
        "textDocument/definition" => definition.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let done = encapsulate(
        gw.addr(),
        &ws.root(),
        &ws.path("main.rs"),
        decl.0,
        decl.1,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(done.applied);
    rust_runtime(&ws);
    assert!(ws.read("main.rs").contains("cfg.retries(), 3"));
}
