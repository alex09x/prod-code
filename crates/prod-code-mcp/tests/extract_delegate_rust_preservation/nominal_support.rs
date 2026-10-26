/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{MANIFEST, artifact, gateway_case, gateway_case_with_owner, refusal};
use prod_code_mcp::extract_delegate::extract_delegate_rust;
use prod_code_testkit::Workspace;
use std::process::Command;

const OWNER: &str = "pub struct Account { pub city: u32, pub balance: u32 }\n";

pub(super) fn record(case: &str, name: &str, text: &str) {
    if let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") {
        let path = std::path::Path::new(&root).join(case).join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

// Native compiler and executable observations, independent of the scripted LSP
// references, owner hovers and empty diagnostics returned by gateway_case.
fn observe(case: &str, stage: &str, ws: &Workspace) -> bool {
    artifact(case, stage, ws);
    if ws.path("src/local.rs").exists() {
        record(case, &format!("{stage}/local.rs"), &ws.read("src/local.rs"));
    }
    let mut compiler = Command::new("rustc");
    compiler.args(["--edition=2021", "src/main.rs", "-o", "program"]);
    if ws.path("src/lib.rs").exists() {
        let output = Command::new("rustc")
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
            .unwrap();
        record_output(case, stage, "library-rustc", &output, ws);
        if !output.status.success() {
            return false;
        }
        compiler.args(["--extern", "preservation=libpreservation.rlib"]);
    }
    let output = compiler.current_dir(ws.root()).output().unwrap();
    record_output(case, stage, "rustc", &output, ws);
    if !output.status.success() {
        record(
            case,
            &format!("{stage}/runtime.txt"),
            "not run: compiler failed\n",
        );
        return false;
    }
    let output = Command::new(ws.path("program")).output().unwrap();
    record_output(case, stage, "runtime", &output, ws);
    output.status.success() && output.stdout == b"9\n"
}

fn record_output(
    case: &str,
    stage: &str,
    tool: &str,
    output: &std::process::Output,
    ws: &Workspace,
) {
    let stderr = String::from_utf8_lossy(&output.stderr)
        .replace(&ws.root().to_string_lossy().to_string(), "<fixture>");
    let text = format!(
        "exit: {}\nstderr:\n{stderr}\nstdout:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout)
    );
    record(case, &format!("{stage}/{tool}.txt"), &text);
    println!("{case}/{stage}/{tool}: {}", output.status);
}

pub(super) async fn preserve(
    case: &str,
    root: &str,
    library: Option<&str>,
    client: &str,
    needles: &[&str],
    expected: &str,
) {
    preserve_owner(
        case,
        root,
        library,
        client,
        OWNER,
        "src/client.rs",
        needles,
        expected,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn preserve_owner(
    case: &str,
    root: &str,
    library: Option<&str>,
    client: &str,
    owner: &str,
    reference_file: &str,
    needles: &[&str],
    expected: &str,
) {
    preserve_files(
        case,
        root,
        library,
        client,
        owner,
        reference_file,
        needles,
        expected,
        &[],
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn preserve_files(
    case: &str,
    root: &str,
    library: Option<&str>,
    client: &str,
    owner: &str,
    reference_file: &str,
    needles: &[&str],
    expected: &str,
    extra: &[(&str, &str)],
) {
    let mut failures = Vec::new();
    for mode in ["preview", "apply", "force"] {
        let case = format!("nominal-{case}-{mode}");
        let mut files = vec![
            ("Cargo.toml", MANIFEST),
            ("src/account.rs", owner),
            ("src/main.rs", root),
            ("src/client.rs", client),
        ];
        if let Some(lib) = library {
            files.push(("src/lib.rs", lib));
        }
        files.extend_from_slice(extra);
        let ws = Workspace::new(&files);
        assert!(observe(&case, "before", &ws), "invalid original {case}");
        let (gw, _) = gateway_case_with_owner(
            &ws,
            &[(reference_file, needles)],
            Some(&case),
            library.map(|_| "account"),
        )
        .await;
        let apply = mode != "preview";
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
            apply,
            mode == "force",
        )
        .await;
        match result {
            Ok(mut result) => {
                assert_eq!(result.applied, apply);
                if !apply {
                    for (path, original) in &files {
                        assert_eq!(ws.read(path), *original, "preview wrote {path}");
                    }
                }
                for (path, text) in &mut result.rewritten {
                    let relative = std::path::Path::new(path)
                        .strip_prefix(ws.root())
                        .unwrap()
                        .to_path_buf();
                    if apply {
                        assert_eq!(*text, ws.read(relative.to_str().unwrap()));
                    } else {
                        std::fs::write(ws.path(relative.to_str().unwrap()), &*text).unwrap();
                    }
                    *path = relative.to_string_lossy().into_owned();
                }
                record(
                    &case,
                    "returned.json",
                    &serde_json::to_string_pretty(&result).unwrap(),
                );
                if !observe(&case, "after", &ws) {
                    failures.push(format!("{case}: returned bytes failed native observation"));
                }
                if !ws.read(reference_file).contains(expected) {
                    failures.push(format!("{case}: missing {expected}"));
                }
            }
            Err(error) => {
                refusal(&case, &format!("{error:#}"), &ws);
                for (path, original) in &files {
                    assert_eq!(ws.read(path), *original, "refusal wrote {path}");
                }
                failures.push(format!("{case}: {error:#}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

pub(super) async fn unresolved_refuses() {
    let client = "mod model { pub struct Account { pub city: u32, pub balance: u32 } }\npub fn value() -> u32 { let a = model::Account { city: 7, balance: 2 }; a.city + a.balance }\n";
    let files = [
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", OWNER),
        (
            "src/main.rs",
            "mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        ),
        ("src/client.rs", client),
    ];
    for (apply, force, mode) in [
        (false, false, "preview"),
        (true, false, "apply"),
        (true, true, "force"),
    ] {
        let case = format!("nominal-unresolved-{mode}");
        let ws = Workspace::new(&files);
        assert!(observe(&case, "before", &ws));
        // Deliberately inconsistent semantic reference: an explicit constructor
        // must never fall back to the other physical-file owner on this input.
        let (gw, _) = gateway_case(
            &ws,
            &[("src/client.rs", &["model::Account { city"])],
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
            apply,
            force,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("explicit constructor binding"),
            "{error:#}"
        );
        refusal(&case, &format!("{error:#}"), &ws);
        for (path, original) in &files {
            assert_eq!(ws.read(path), *original);
        }
        assert!(observe(&case, "after", &ws));
    }
}
