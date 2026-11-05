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
use prod_code_mcp::extract_delegate::extract_delegate_rust;
use prod_code_testkit::Workspace;
use std::process::Command;

const OWNER: &str = "pub struct Account { pub city: u32, pub balance: u32 }\n";
const ROOT: &str = "mod account; #[path = \"account.rs\"] mod alternate; mod client; fn main() { println!(\"{}\", client::value()); }\n";
const VALUE: &str = "pub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n";

fn record(case: &str, file: &str, value: &str) {
    if let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") {
        let path = std::path::Path::new(&root).join(case).join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value).unwrap();
    }
}

// Cargo supplies each target's real crate name, including an implicit binary
// beside a differently named library. The compiler and runtime are real;
// references/owner hovers use the existing scripted gateway at exact field tokens.
fn cargo_run(case: &str, stage: &str, ws: &Workspace, bin: &str) -> Result<String, String> {
    artifact(case, stage, ws);
    let output = Command::new("cargo")
        .args(["run", "--offline", "--quiet", "--bin", bin])
        .current_dir(ws.root())
        .output()
        .expect("cargo fixture compiler/runtime");
    let diagnostics = String::from_utf8_lossy(&output.stderr)
        .replace(&ws.root().to_string_lossy().to_string(), "<fixture>");
    let stdout = String::from_utf8(output.stdout).unwrap();
    record(
        case,
        &format!("{stage}/cargo-run.txt"),
        &format!(
            "cargo run --offline --quiet --bin {bin}\nexit: {}\n{diagnostics}\nstdout:\n{stdout}",
            output.status
        ),
    );
    println!(
        "{case}/{stage}: cargo run --offline --quiet --bin {bin}: {}",
        output.status
    );
    if output.status.success() {
        Ok(stdout)
    } else {
        Err(diagnostics)
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn preserve(
    case: &str,
    manifest: &str,
    root: &str,
    library: Option<&str>,
    client: &str,
    needles: &[&str],
    binding: Option<&str>,
    expected_path: &str,
    bin: &str,
) {
    let mut failures = Vec::new();
    for force in [false, true] {
        let case = format!("{case}-{}", if force { "force" } else { "default" });
        let mut files = vec![
            ("Cargo.toml", manifest),
            ("src/account.rs", OWNER),
            ("src/main.rs", root),
            ("src/client.rs", client),
        ];
        if let Some(library) = library {
            files.push(("src/lib.rs", library));
        }
        let ws = Workspace::new(&files);
        assert_eq!(cargo_run(&case, "before", &ws, bin).unwrap(), "9\n");
        let identity =
            binding.map(|module| format!("{}::{module}::Account", bin.replace('-', "_")));
        let (gw, _) = gateway_case_with_identity(
            &ws,
            &[("src/client.rs", needles)],
            Some(&case),
            identity.as_deref(),
        )
        .await;
        let result = extract_delegate_rust(
            gw.addr(),
            &ws.root(),
            &ws.path("src/account.rs"),
            Some("Account"),
            None,
            None,
            &["city".into()],
            &[],
            "Address",
            "address",
            true,
            force,
        )
        .await;
        match result {
            Ok(mut result) => {
                assert!(result.applied);
                for (path, text) in &mut result.rewritten {
                    let relative = std::path::Path::new(path)
                        .strip_prefix(ws.root())
                        .unwrap()
                        .to_path_buf();
                    assert_eq!(*text, ws.read(relative.to_str().unwrap()));
                    *path = relative.to_string_lossy().into_owned();
                }
                record(
                    &case,
                    "returned.json",
                    &serde_json::to_string_pretty(&result).unwrap(),
                );
                let after = cargo_run(&case, "after", &ws, bin);
                if after.as_deref() != Ok("9\n") {
                    failures.push(format!("{case}: after compiler/runtime: {after:?}"));
                }
                if !ws.read("src/client.rs").contains(expected_path) {
                    failures.push(format!("{case}: missing {expected_path}"));
                }
            }
            Err(error) => {
                let error = format!("{error:#}");
                refusal(&case, &error, &ws);
                // A refusal is never allowed to leave partially transformed files.
                for (file, original) in &files {
                    assert_eq!(ws.read(file), *original, "{case}: partial write to {file}");
                }
                failures.push(format!("{case}: public API: {error}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn final_self_uses_enclosing_impl_not_unrelated_glob() {
    preserve(
        "final-self-impl", MANIFEST, ROOT, None,
        "use crate::alternate::*;\nimpl crate::account::Account { pub fn new() -> Self { Self { city: 7, balance: 2 } } }\npub fn value() -> u32 { let a = crate::account::Account::new(); a.city + a.balance }\n",
        &["Self { city", "a.city"], None, "crate::account::Address", "preservation",
    ).await;
}

#[tokio::test]
async fn final_cfg_attr_disabled_explicit_cannot_override_glob() {
    let client = format!(
        "#[cfg_attr(all(), cfg(any()))] use crate::alternate::Account; use crate::account::*;\n{VALUE}"
    );
    preserve(
        "final-cfg-attr",
        MANIFEST,
        ROOT,
        None,
        &client,
        &["a = Account { city", "let Account { city"],
        None,
        "crate::account::Address",
        "preservation",
    )
    .await;
}

#[tokio::test]
async fn final_implicit_binary_name_is_independent_of_library() {
    let manifest = format!("{MANIFEST}\n[lib]\nname = \"custom_library\"\n");
    let client =
        format!("#[cfg(windows)] use crate::alternate::Account; use crate::account::*;\n{VALUE}");
    preserve(
        "final-implicit-bin",
        &manifest,
        ROOT,
        Some("pub fn library() {}\n"),
        &client,
        &["a = Account { city", "let Account { city"],
        Some("account"),
        "crate::account::Address",
        "preservation",
    )
    .await;
}

#[tokio::test]
async fn final_self_alias_is_bound_at_impl_not_inside_method() {
    preserve(
        "final-self-alias", MANIFEST, ROOT, None,
        "use crate::account::Account as Owner;\nimpl Owner { pub fn new() -> Self { use crate::alternate::*; Self { city: 7, balance: 2 } } }\npub fn value() -> u32 { let a = Owner::new(); a.city + a.balance }\n",
        &["Self { city", "a.city"], None, "crate::account::Address", "preservation",
    ).await;
}

#[tokio::test]
async fn final_self_constructor_with_impl_trait_argument() {
    preserve(
        "final-self-impl-trait", MANIFEST, ROOT, None,
        "use crate::account::*;\nimpl Account { pub fn new(city: impl Into<u32>) -> Self { Self { city: city.into(), balance: 2 } } }\npub fn value() -> u32 { let a = Account::new(7u32); a.city + a.balance }\n",
        &["Self { city", "a.city"], None, "crate::account::Address", "preservation",
    ).await;
}

#[tokio::test]
async fn final_cfg_attr_nested_constants_and_unknown_semantics() {
    for (case, attribute, binding) in [
        (
            "nested",
            "cfg_attr(all(), cfg_attr(not(any()), cfg(any())), allow(unused))",
            None,
        ),
        ("unknown", "cfg_attr(unix, cfg(any()))", Some("account")),
        ("false-predicate", "cfg_attr(any(), cfg(any()))", None),
    ] {
        let client = format!(
            "#[{attribute}] use crate::alternate::Account; use crate::account::*;\n{VALUE}"
        );
        let expected = if case == "false-predicate" {
            "crate::alternate::Address"
        } else {
            "crate::account::Address"
        };
        preserve(
            &format!("final-cfg-attr-{case}"),
            MANIFEST,
            ROOT,
            None,
            &client,
            &["a = Account { city", "let Account { city"],
            binding,
            expected,
            "preservation",
        )
        .await;
    }
}

#[tokio::test]
async fn final_unknown_cfg_attr_refuses_without_identity_or_writes() {
    for force in [false, true] {
        let case = format!("final-cfg-attr-refusal-{force}");
        let client = format!(
            "#[cfg_attr(unix, cfg(any()))] use crate::alternate::Account; use crate::account::*;\n{VALUE}"
        );
        let files = [
            ("Cargo.toml", MANIFEST),
            ("src/account.rs", OWNER),
            ("src/main.rs", ROOT),
            ("src/client.rs", &client),
        ];
        let ws = Workspace::new(&files);
        assert_eq!(
            cargo_run(&case, "before", &ws, "preservation").unwrap(),
            "9\n"
        );
        let (gw, _) = gateway_case(
            &ws,
            &[(
                "src/client.rs",
                &["a = Account { city", "let Account { city"],
            )],
            Some(&case),
        )
        .await;
        let error = extract_delegate_rust(
            gw.addr(),
            &ws.root(),
            &ws.path("src/account.rs"),
            Some("Account"),
            None,
            None,
            &["city".into()],
            &[],
            "Address",
            "address",
            true,
            force,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("active conditional owner binding"),
            "{error}"
        );
        refusal(&case, &error, &ws);
        for (file, original) in files {
            assert_eq!(ws.read(file), original);
        }
        assert_eq!(
            cargo_run(&case, "after", &ws, "preservation").unwrap(),
            "9\n"
        );
    }
}

#[tokio::test]
async fn final_binary_target_names_and_hyphen_normalization() {
    for (case, package, bin, target) in [
        (
            "normalized-implicit",
            "preservation-app",
            "preservation-app",
            "",
        ),
        (
            "explicit-main",
            "preservation",
            "custom-runner",
            "\n[[bin]]\nname = \"custom-runner\"\npath = \"src/main.rs\"\n",
        ),
    ] {
        let manifest = format!(
            "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\nname = \"custom_library\"\n{target}"
        );
        let root = ROOT.replace(
            "fn main() {",
            &format!(
                "fn main() {{ assert_eq!(env!(\"CARGO_CRATE_NAME\"), \"{}\");",
                bin.replace('-', "_")
            ),
        );
        let client = format!(
            "#[cfg(windows)] use crate::alternate::Account; use crate::account::*;\n{VALUE}"
        );
        preserve(
            &format!("final-bin-{case}"),
            &manifest,
            &root,
            Some("pub fn library() {}\n"),
            &client,
            &["a = Account { city", "let Account { city"],
            Some("account"),
            "crate::account::Address",
            bin,
        )
        .await;
    }
}
