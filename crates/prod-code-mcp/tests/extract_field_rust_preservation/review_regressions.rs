/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::evidence;
use super::support::*;
use prod_code_mcp::extract_field::extract;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::process::Command;

#[tokio::test]
async fn numeric_exponents_are_whole_tokens() {
    let ws = fixture(
        "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet selected = 3;\nlet f = 1e-3;\nlet g = 1.0E+3f64;\nlet h = 1_e-3_f64;\nlet hex = 0x1e - 3;\nassert_eq!(f, 0.001); assert_eq!(g, 1000.0); assert_eq!(h, f); assert_eq!(hex, 27);\nselected\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 3); }\n",
    );
    assert!(execute(&ws.root(), "exponent before").0);
    let result = promote_initialized(&ws, "3", "3", true, false, false)
        .await
        .unwrap();
    returned(&ws, &result);
    assert!(execute(&ws.root(), "exponent returned").0);
    assert_eq!(result.replaced, 2); // selected and the separate subtraction operand
    for literal in ["1e-3", "1.0E+3f64", "1_e-3_f64"] {
        assert!(ws.read("src/main.rs").contains(literal));
    }
}

#[tokio::test]
async fn complete_exponent_expressions_remain_selectable_but_partial_digits_do_not() {
    let ws = fixture(
        "struct Store {}\nimpl Store {\nfn limit(&self) -> usize { let a = 1e-3 as usize; let b = 1e-3 as usize; a + b }\n}\nfn main() { assert_eq!(Store {}.limit(), 0); }\n",
    );
    assert!(execute(&ws.root(), "complete exponent before").0);
    assert!(
        promote_initialized(&ws, "3", "3", false, false, true)
            .await
            .is_err()
    );
    let result = promote_initialized(&ws, "1e-3 as usize", "0", true, false, false)
        .await
        .unwrap();
    returned(&ws, &result);
    assert!(execute(&ws.root(), "complete exponent returned").0);
    assert_eq!(result.replaced, 2);
}

#[tokio::test]
async fn condition_operands_before_blocks_remain_selectable() {
    let ws = fixture(
        "struct Store { entries: usize }\nimpl Store {\nfn limit(&self) -> usize { if self.entries == 64 { 1 } else { 0 } }\n}\nfn main() { assert_eq!(Store { entries: 64 }.limit(), 1); }\n",
    );
    assert!(execute(&ws.root(), "condition before").0);
    let result = promote(&ws, "64", false, false, false).await.unwrap();
    returned(&ws, &result);
    assert!(execute(&ws.root(), "condition returned").0);
    assert_eq!(result.replaced, 1);
}

#[tokio::test]
async fn condition_replace_all_keeps_patterns_untouched() {
    let ws = fixture(
        "struct Store { entries: usize }\nimpl Store {\nfn limit(&self) -> usize {\nlet n = if self.entries == 64 { 1 } else { 0 };\nlet mut i = 0; while i < 64 { i += 1; }\nlet m = match 64 { 64 => 1, _ => 0 };\nlet partial = 64 * 2;\nn + i + m + partial\n}\n}\nfn main() { assert_eq!(Store { entries: 64 }.limit(), 194); }\n",
    );
    assert!(execute(&ws.root(), "condition duplicates before").0);
    let result = promote(&ws, "64", true, false, false).await.unwrap();
    returned(&ws, &result);
    assert!(execute(&ws.root(), "condition duplicates returned").0);
    assert_eq!(result.replaced, 4); // if/while/match operands and the multiplication operand
    assert!(ws.read("src/main.rs").contains("match self.cap { 64 =>"));
}

#[tokio::test]
async fn const_generic_comparisons_do_not_poison_later_expressions() {
    let ws = fixture(
        "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet _ = std::mem::size_of::<[u8; { if 1 < 2 { 1 } else { 2 } }]>();\n64\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 64); }\n",
    );
    assert!(execute(&ws.root(), "generic comparison before").0);
    let result = promote(&ws, "64", false, false, false).await.unwrap();
    returned(&ws, &result);
    assert!(execute(&ws.root(), "generic comparison returned").0);
}

#[tokio::test]
async fn const_generic_contents_remain_excluded() {
    let source = "struct Store {}\nimpl Store {\nfn limit(&self) -> usize {\nlet _ = std::mem::size_of::<[u8; { if 64 < 65 { 1 } else { 2 } }]>();\n64\n}\n}\nfn main() { assert_eq!(Store {}.limit(), 64); }\n";
    let ws = fixture(source);
    assert!(execute(&ws.root(), "generic contents before").0);
    assert!(promote(&ws, "64", false, true, true).await.is_err());
    assert_eq!(ws.read("src/main.rs"), source);
}

fn execute_crates(ws: &Workspace, label: &str) -> bool {
    let mut ok = true;
    println!("=== {label} ===");
    for path in ["Cargo.toml", "src/lib.rs", "src/main.rs"] {
        println!("// {path}\n{}", ws.read(path));
    }
    for args in [
        vec![
            "--crate-type=lib",
            "--crate-name=preservation",
            "src/lib.rs",
            "-o",
            "libpreservation.rlib",
        ],
        vec![
            "src/main.rs",
            "--extern",
            "preservation=libpreservation.rlib",
            "-o",
            "program",
        ],
    ] {
        let output = Command::new("rustc")
            .current_dir(ws.root())
            .arg("--edition=2021")
            .args(&args)
            .output()
            .unwrap();
        let argv = std::iter::once("rustc".to_string())
            .chain(std::iter::once("--edition=2021".to_string()))
            .chain(args.iter().map(|s| s.to_string()))
            .collect::<Vec<_>>();
        evidence::native(&ws.root(), label, "compile", &argv, &output);
        println!(
            "rustc {args:?} exit={}\n{}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        );
        ok &= output.status.success();
    }
    if ok {
        let output = Command::new(ws.root().join("program")).output().unwrap();
        evidence::native(&ws.root(), label, "runtime", &["./program".into()], &output);
        println!(
            "runtime exit={}\nstdout={}\nstderr={}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ok &= output.status.success();
    }
    ok
}

#[tokio::test]
async fn library_and_binary_in_one_package_are_distinct_crates() {
    let main =
        "use preservation::Store;\nfn main() { assert_eq!(Store { entries: 1 }.limit(), 64); }\n";
    let lib = "pub struct Store { pub entries: usize }\nimpl Store {\npub fn limit(&self) -> usize { 64 }\n}\nstruct Private { hidden: usize }\n";
    let ws = fixture(main);
    let lib_path = ws.write("src/lib.rs", lib);
    assert!(execute_crates(&ws, "two crates before"));
    let refs: Vec<_> = [(ws.path("src/lib.rs"), lib), (ws.path("src/main.rs"), main)]
        .into_iter()
        .flat_map(|(file, text)| {
            text.match_indices("Store").flat_map(move |(at, _)| {
                answers::locations(&file, &[position(text, at)])
                    .as_array()
                    .unwrap()
                    .clone()
            })
        })
        .collect();
    println!(
        "SCRIPTED definition=src/lib.rs:(1,12); references={}",
        serde_json::to_string(&refs)
            .unwrap()
            .replace(&ws.root().display().to_string(), "<fixture>")
    );
    let def = lib_path.clone();
    let root = ws.root();
    let gateway = ScriptedGateway::start(move |method, request| {
        evidence::reply(
            &root,
            method,
            request,
            match method {
                "textDocument/definition" => answers::locations(&def, &[(1, 12)]),
                "textDocument/references" => serde_json::Value::Array(refs.clone()),
                "textDocument/diagnostic" => answers::no_diagnostics(),
                _ => serde_json::Value::Null,
            },
        )
    })
    .await;
    let at = lib.find("64").unwrap();
    evidence::operation(
        &ws.root(),
        &lib_path,
        at,
        at + 2,
        serde_json::json!({"all": false, "apply": false, "force": false}),
    );
    let result = extract(
        gateway.addr(),
        &ws.root(),
        &lib_path,
        position(lib, at),
        position(lib, at + 2),
        "cap",
        Some("usize"),
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap();
    evidence::result(&ws.root(), &result);
    returned(&ws, &result);
    assert!(execute_crates(&ws, "two crates returned"));
    assert!(ws.read("src/lib.rs").contains("pub cap: usize"));
    assert!(
        ws.read("src/lib.rs")
            .contains("struct Private { hidden: usize }")
    );
}
