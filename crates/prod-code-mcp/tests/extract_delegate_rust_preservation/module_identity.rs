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
const CLIENT: &str = "use crate::account::Account;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n";

async fn preserves_module(root: &str, client_file: &str, case: &str) {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", OWNER),
        (client_file, CLIENT),
        ("src/main.rs", root),
    ]);
    assert_eq!(run(case, "before", &ws), "9\n");
    let (gw, _) = gateway_case(
        &ws,
        &[(client_file, &["a = Account { city", "let Account { city"])],
        Some(case),
    )
    .await;
    let result = extract(&ws, &gw, &[]).await;
    if let Err(error) = &result {
        refusal(case, &format!("{error:#}"), &ws);
    }
    result.unwrap();
    assert_eq!(run(case, "after", &ws), "9\n");
    assert!(ws.read(client_file).contains("crate::account::Address"));
}

#[tokio::test]
async fn module_identity_raw_external_consumer() {
    preserves_module(
        "mod account; mod r#async; fn main() { println!(\"{}\", r#async::value()); }\n",
        "src/async.rs",
        "module-raw-consumer",
    )
    .await;
}

#[tokio::test]
async fn module_identity_disabled_path_alias() {
    preserves_module(
        "#[cfg(any())] #[path = \"account.rs\"] mod disabled; mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        "src/client.rs",
        "module-disabled-alias",
    )
    .await;
}

#[tokio::test]
async fn module_identity_conditional_alias_before_ordinary_module() {
    preserves_module(
        "#[cfg(feature = \"unused\")] #[path = \"account.rs\"] mod disabled; mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        "src/client.rs",
        "module-conditional-alias",
    )
    .await;
}

#[tokio::test]
async fn module_identity_conditional_only_owner() {
    // The fixture runner invokes rustc without optimization, enabling this cfg.
    preserves_module(
        "#[cfg(debug_assertions)] mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        "src/client.rs",
        "module-conditional-owner",
    )
    .await;
}

#[tokio::test]
async fn module_identity_raw_directory_consumer() {
    preserves_module(
        "mod account; mod r#async; fn main() { println!(\"{}\", r#async::value()); }\n",
        "src/async/mod.rs",
        "module-raw-directory",
    )
    .await;
}

#[tokio::test]
async fn module_identity_raw_inline_ancestor() {
    preserves_module(
        "mod account; mod r#async { pub mod client; } fn main() { println!(\"{}\", r#async::client::value()); }\n",
        "src/async/client.rs",
        "module-raw-inline",
    )
    .await;
}

#[tokio::test]
async fn module_identity_nested_false_cfg_and_attribute_scope() {
    preserves_module(
        "#[cfg(any())] fn absent() {} #[cfg(all(any(), feature = \"unused\"))] #[path = \"account.rs\"] mod disabled; #[cfg(not(all()))] mod missing; #[cfg(all())] mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        "src/client.rs",
        "module-nested-cfg",
    )
    .await;
}

#[tokio::test]
async fn module_identity_retains_raw_owner_spelling() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", OWNER),
        (
            "src/client.rs",
            &CLIENT.replace("::account::", "::r#async::"),
        ),
        (
            "src/main.rs",
            "#[path = \"account.rs\"] mod r#async; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        ),
    ]);
    let case = "module-raw-owner";
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
    extract(&ws, &gw, &[]).await.unwrap();
    assert_eq!(run(case, "after", &ws), "9\n");
    assert!(ws.read("src/client.rs").contains("crate::r#async::Address"));
}
