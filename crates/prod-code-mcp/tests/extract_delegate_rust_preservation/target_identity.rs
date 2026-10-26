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

const OWNER: &str = "pub struct Account { pub city: u32, pub balance: u32 }\n";

async fn preserve(case: &str, root: &str, library: Option<&str>, client: &str) {
    preserve_owner(case, root, library, client, OWNER).await;
}

async fn preserve_owner(case: &str, root: &str, library: Option<&str>, client: &str, owner: &str) {
    preserve_binding(case, root, library, client, owner, None).await;
}

async fn preserve_binding(
    case: &str,
    root: &str,
    library: Option<&str>,
    client: &str,
    owner: &str,
    binding: Option<&str>,
) {
    let mut files = vec![
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", owner),
        ("src/main.rs", root),
        ("src/client.rs", client),
    ];
    if let Some(library) = library {
        files.push(("src/lib.rs", library));
    }
    let ws = Workspace::new(&files);
    assert_eq!(run(case, "before", &ws), "9\n");
    let (gw, _) = gateway_case_with_owner(
        &ws,
        &[(
            "src/client.rs",
            &["a = Account { city", "let Account { city"],
        )],
        Some(case),
        binding,
    )
    .await;
    let result = extract(&ws, &gw, &[]).await;
    if let Err(error) = &result {
        refusal(case, &format!("{error:#}"), &ws);
    }
    let mut result = result.unwrap();
    assert!(result.applied);
    assert!(ws.read("src/account.rs").contains("struct Address"));
    assert_eq!(ws.read("src/client.rs").matches("address:").count(), 2);
    for (path, text) in &mut result.rewritten {
        let relative = std::path::Path::new(path)
            .strip_prefix(ws.root())
            .unwrap()
            .to_path_buf();
        assert_eq!(*text, ws.read(relative.to_str().unwrap()));
        *path = relative.to_string_lossy().into_owned();
    }
    if let Ok(root) = std::env::var("EXTRACT_DELEGATE_EVIDENCE") {
        std::fs::write(
            std::path::Path::new(&root).join(case).join("returned.json"),
            serde_json::to_string_pretty(&result).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(run(case, "after", &ws), "9\n");
}

#[tokio::test]
async fn occurrence_target_shared_library_binary_glob() {
    preserve(
        "target-shared-glob",
        "mod client; fn main() { println!(\"{}\", client::value()); }\n",
        Some("extern crate self as preservation; pub mod account; pub mod client;\n"),
        "use preservation::account::*;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
    )
    .await;
}

#[tokio::test]
async fn occurrence_target_platform_module_glob() {
    preserve(
        "target-platform-glob",
        "#[cfg(windows)] #[path = \"account.rs\"] mod disabled; #[cfg(unix)] mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "use crate::account::*;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
    )
    .await;
}

#[tokio::test]
async fn occurrence_binding_explicit_import_overrides_earlier_glob() {
    preserve(
        "binding-explicit-over-glob",
        "mod account; #[path = \"account.rs\"] mod alternate; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "use crate::alternate::*; use crate::account::Account;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
    )
    .await;
}

#[tokio::test]
async fn occurrence_binding_inactive_glob_before_explicit_import() {
    preserve(
        "binding-inactive-glob",
        "#[cfg(windows)] #[path = \"account.rs\"] mod disabled; mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "#[cfg(windows)] use crate::disabled::*; use crate::account::Account;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
    )
    .await;
}

#[tokio::test]
async fn occurrence_binding_false_explicit_does_not_override_glob() {
    preserve(
        "binding-false-explicit",
        "mod account; #[path = \"account.rs\"] mod alternate; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "#[cfg(any())] use crate::alternate::Account; use crate::account::*;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
    )
    .await;
}

#[tokio::test]
async fn occurrence_binding_grouped_explicit_after_false_attribute() {
    preserve(
        "binding-grouped-explicit",
        "mod account; #[path = \"account.rs\"] mod alternate; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "#[cfg(any())] fn absent() {} use crate::alternate::*; #[cfg(all())] use crate::account::{Account};\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
    )
    .await;
}

#[tokio::test]
async fn occurrence_binding_conditional_only_explicit() {
    preserve(
        "binding-conditional-explicit",
        "#[cfg(unix)] mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "#[cfg(unix)] use crate::account::Account;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
    )
    .await;
}

#[tokio::test]
async fn remaining_conditional_explicit_before_active_glob() {
    preserve_binding(
        "remaining-conditional-import",
        "#[cfg(windows)] #[path = \"account.rs\"] mod disabled; mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "#[cfg(windows)] use crate::disabled::Account; use crate::account::*;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
        OWNER,
        Some("account"),
    )
    .await;
}

#[tokio::test]
async fn remaining_active_conditional_explicit_overrides_glob() {
    preserve_binding(
        "remaining-active-conditional-import",
        "#[cfg(unix)] #[path = \"account.rs\"] mod alternate; mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "#[cfg(unix)] use crate::alternate::Account; use crate::account::*;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
        OWNER,
        Some("alternate"),
    )
    .await;
}

#[tokio::test]
async fn remaining_unresolved_conditional_binding_refuses_without_writes() {
    let case = "remaining-unresolved-conditional";
    let root = "#[cfg(windows)] #[path = \"account.rs\"] mod disabled; mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n";
    let client = "#[cfg(windows)] use crate::disabled::Account; use crate::account::*;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/main.rs", root),
        ("src/account.rs", OWNER),
        ("src/client.rs", client),
    ]);
    assert_eq!(run(case, "before", &ws), "9\n");
    let (gw, _) = gateway_case(
        &ws,
        &[(
            "src/client.rs",
            &["a = Account { city", "let Account { city"],
        )],
        Some(case),
    )
    .await;
    let error = extract(&ws, &gw, &[]).await.unwrap_err().to_string();
    assert!(
        error.contains("active conditional owner binding"),
        "{error}"
    );
    for (path, expected) in [
        ("Cargo.toml", MANIFEST),
        ("src/main.rs", root),
        ("src/account.rs", OWNER),
        ("src/client.rs", client),
    ] {
        assert_eq!(ws.read(path), expected);
    }
    refusal(case, &error, &ws);
    assert_eq!(run(case, "after", &ws), "9\n");
}

#[tokio::test]
async fn remaining_block_import_shadows_outer_explicit() {
    preserve(
        "remaining-block-import",
        "mod account; #[path = \"account.rs\"] mod alternate; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "use crate::alternate::Account;\npub fn value() -> u32 { use crate::account::Account; let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
    )
    .await;
}

#[tokio::test]
async fn remaining_attributed_owner() {
    preserve_owner(
        "remaining-attributed-owner",
        "mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "use crate::account::*;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
        "#[derive(Debug)]\npub struct Account { pub city: u32, pub balance: u32 }\n",
    )
    .await;
}

#[tokio::test]
async fn remaining_documented_owner() {
    preserve_owner(
        "remaining-documented-owner",
        "mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        "use crate::account::*;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
        "/// An account.\npub struct Account { pub city: u32, pub balance: u32 }\n",
    )
    .await;
}
