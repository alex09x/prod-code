/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{bytes, record, run};
use prod_code_mcp::replace_constructor::{ReplaceMode, replace_constructor_impl};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};

const MAIN: &str = "mod model;\n#[derive(Default)] pub struct Config { pub value: i32 }\nimpl Config {\n pub fn new(value: i32) -> Self { let mut c = Self::default(); c.value = value + 10; c }\n}\nfn main() { println!(\"{} {} {}\", model::make().value, model::Config::selected().value, Config::other().value); }\n";
const MODEL: &str = "pub struct Config { pub value: i32 }\npub fn make() -> Config { Config { value: 1 } }\nimpl Config { pub fn selected() -> Self { Self { value: 3 } } }\nimpl crate::Config { pub fn other() -> Self { Self { value: 2 } } }\n";

fn reference(source: &str, offset: usize, file: &str, uri: &str, length: usize) -> Value {
    let line = source[..offset].bytes().filter(|&b| b == b'\n').count();
    let character = source[..offset]
        .rsplit('\n')
        .next()
        .unwrap()
        .encode_utf16()
        .count();
    json!({"file":file,"uri":uri,"range":{"start":{"line":line,"character":character},
        "end":{"line":line,"character":character+length}}})
}

async fn module_owner(mode: ReplaceMode, entry: &str, ambiguous: bool) {
    let manifest = format!(
        "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n[[bin]]\nname='fixture'\npath='{entry}'\n"
    );
    let ws = Workspace::new(&[
        (entry, MAIN),
        ("model.rs", MODEL),
        ("Cargo.toml", &manifest),
    ]);
    let root = ws.root();
    if ambiguous {
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub struct Config;\n").unwrap();
    }
    let main_uri = url::Url::from_file_path(root.join(entry))
        .unwrap()
        .to_string();
    let model_uri = url::Url::from_file_path(root.join("model.rs"))
        .unwrap()
        .to_string();
    // All non-declaration references to model::Config, including both Self tokens.
    // No crate-root Config token is a reference to the selected declaration.
    let mut references: Vec<Value> = MODEL
        .match_indices("Config")
        .skip(1)
        .take(3)
        .map(|(at, _)| reference(MODEL, at, "model.rs", &model_uri, 6))
        .collect();
    for (at, _) in MODEL.match_indices("Self").take(2) {
        references.push(reference(MODEL, at, "model.rs", &model_uri, 4));
    }
    references.push(reference(
        MAIN,
        MAIN.find("model::Config").unwrap() + 7,
        entry,
        &main_uri,
        6,
    ));
    let response = Value::Array(references.clone());
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => response.clone(),
        "textDocument/diagnostic" => json!({"kind":"full","items":[]}),
        _ => Value::Null,
    })
    .await;
    let before_compile = run(&root, &["rustc", "--edition=2021", entry, "-o", "program"]);
    let before = run(&root, &["./program"]);
    let result = replace_constructor_impl(
        gateway.addr(),
        &root,
        &root.join("model.rs"),
        "Config",
        mode,
        None,
        true,
        true,
        None,
    )
    .await;
    let main_after = std::fs::read_to_string(root.join(entry)).unwrap();
    let model_after = std::fs::read_to_string(root.join("model.rs")).unwrap();
    let after_compile = run(&root, &["rustc", "--edition=2021", entry, "-o", "program"]);
    let after = if after_compile["exit"] == 0 {
        run(&root, &["./program"])
    } else {
        Value::Null
    };
    // Paths are transport data only; retain portable, exact semantic coordinates.
    for reference in &mut references {
        reference.as_object_mut().unwrap().remove("uri");
    }
    let fixture = serde_json::to_string(&references).unwrap();
    let suffix = if ambiguous {
        "-ambiguous"
    } else if entry == "main.rs" {
        ""
    } else {
        "-custom"
    };
    record(
        &format!("module-owner-{mode}{suffix}"),
        &json!({
            "input_main":bytes(&root, MAIN),"input_model":bytes(&root, MODEL),
            "output_main":bytes(&root, &main_after),"output_model":bytes(&root, &model_after),
            "entry":entry,"manifest":bytes(&root, &manifest),"ambiguous":ambiguous,
            "references":references,"reference_bytes":bytes(&root, &fixture),
            "scripted_transport":"complete exact selected-declaration reference fixture; scripted diagnostics, force=true; no live analyzer certification",
            "before_compile":before_compile,"before":before,"after_compile":after_compile,"after":after,
            "public_result":result.as_ref().map(|r| json!({"count":r.instantiations_rewritten,"blocked":r.blocked}))
                .unwrap_or_else(|e| json!({"error":format!("{e:#}").replace(&*root.to_string_lossy(), "<fixture>")}))
        }),
    );
    assert_eq!(before_compile["exit"], 0, "{before_compile}");
    assert_eq!(before["exit"], 0);
    assert_eq!(before["stdout"], "1 3 2\n");
    if ambiguous {
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot resolve Rust constructor owner")
        );
        assert_eq!(main_after, MAIN);
        assert_eq!(model_after, MODEL);
        return;
    }
    assert_eq!(result.unwrap().instantiations_rewritten, 2);
    assert_eq!(after_compile["exit"], 0, "{after_compile}");
    assert_eq!(after["exit"], 0);
    assert_eq!(before["stdout"], after["stdout"]);
    assert_eq!(main_after, MAIN);
    assert!(
        model_after.contains("impl crate::Config { pub fn other() -> Self { Self { value: 2 } } }")
    );
}

#[tokio::test]
async fn module_owner_factory() {
    module_owner(ReplaceMode::Factory, "main.rs", false).await;
}

#[tokio::test]
async fn module_owner_builder() {
    module_owner(ReplaceMode::Builder, "main.rs", false).await;
}

#[tokio::test]
async fn module_owner_custom_target() {
    for mode in [ReplaceMode::Factory, ReplaceMode::Builder] {
        module_owner(mode, "entry.rs", false).await;
    }
}

#[tokio::test]
async fn module_owner_ambiguous_crate_refuses_application() {
    for mode in [ReplaceMode::Factory, ReplaceMode::Builder] {
        module_owner(mode, "main.rs", true).await;
    }
}

#[test]
fn module_owner_source_only_requires_crate_context() {
    let (sites, blocked) = prod_code_mcp::replace_constructor::find_rust_instantiations(
        MODEL,
        "Config",
        MODEL.find("struct").unwrap(),
        MODEL.find('}').unwrap() + 1,
    );
    record(
        "module-owner-source-only",
        &json!({"count":sites.len(),"blocked":blocked}),
    );
    assert_eq!(sites.len(), 2);
    assert_eq!(blocked.len(), 1);
    assert!(blocked[0].contains("crate identity"));
}
