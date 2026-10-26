/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::remaining_bindings::preserve;
use super::support::{MANIFEST, artifact, gateway_case, refusal};
use prod_code_mcp::extract_delegate::extract_delegate_rust;
use prod_code_testkit::Workspace;

const OWNER: &str = "pub struct Account { pub city: u32, pub balance: u32 }\n";

#[tokio::test]
async fn final_self_constructor_with_const_generic_trait_header() {
    const_trait_header("braced", "{ 1 }").await;
}

#[tokio::test]
async fn final_self_constructor_with_const_generic_trait_header_nested() {
    const_trait_header(
        "nested",
        "{ let values = [1, 2]; (values[0] < values[1]) as usize }",
    )
    .await;
}

#[tokio::test]
async fn final_self_lookup_after_file_inner_attribute() {
    self_lookup(
        "inner-attribute",
        "#![allow(dead_code)]\nimpl crate::account::Account { pub fn new() -> Self { use crate::alternate::*; Self { city: 7, balance: 2 } } }\npub fn value() -> u32 { let a = crate::account::Account::new(); a.city + a.balance }\n",
    )
    .await;
}

#[tokio::test]
async fn final_self_lookup_with_parenthesized_impl_type() {
    self_lookup(
        "parenthesized-type",
        "use crate::alternate::*;\nimpl (crate::account::Account) { pub fn new() -> Self { Self { city: 7, balance: 2 } } }\npub fn value() -> u32 { let a = crate::account::Account::new(); a.city + a.balance }\n",
    )
    .await;
}

#[tokio::test]
async fn final_self_lookup_inside_comparison_block() {
    self_lookup(
        "comparison-block",
        "use crate::alternate::*;\nimpl crate::account::Account { pub fn new() -> Self { if 1 < 2 { return Self { city: 7, balance: 2 }; } unreachable!() } }\npub fn value() -> u32 { let a = crate::account::Account::new(); a.city + a.balance }\n",
    )
    .await;
}

#[tokio::test]
async fn final_self_lookup_refuses_invalid_inner_attribute_or_non_struct_target() {
    for (name, client) in [
        (
            "fn-impl-trait",
            "#![allow(dead_code)]\nfn f() -> impl Into<u32> { let _ = Self { city: 7, balance: 2 }; 9u32 }\n",
        ),
        (
            "outer-then-inner",
            "#[allow(dead_code)]\n#![allow(unused)]\nimpl crate::account::Account { pub fn new() -> Self { Self { city: 7, balance: 2 } } }\n",
        ),
        (
            "tuple-target",
            "impl (crate::account::Account, u32) { fn new() -> Self { Self { city: 7, balance: 2 } } }\n",
        ),
    ] {
        for force in [false, true] {
            let case = format!(
                "final-self-refusal-{name}-{}",
                if force { "force" } else { "default" }
            );
            let files = [
                ("Cargo.toml", MANIFEST),
                ("src/account.rs", OWNER),
                ("src/client.rs", client),
            ];
            let ws = Workspace::new(&files);
            artifact(&case, "before", &ws);
            let (gw, _) =
                gateway_case(&ws, &[("src/client.rs", &["Self { city"])], Some(&case)).await;
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
                error.contains("cannot resolve the enclosing impl type for Self"),
                "{case}: {error}"
            );
            for (file, original) in files {
                assert_eq!(ws.read(file), original, "{case}: partial write to {file}");
            }
            refusal(&case, &error, &ws);
        }
    }
}

async fn const_trait_header(case: &str, argument: &str) {
    let client = format!(
        "use crate::alternate::*;\ntrait Build<const N: usize> {{ fn new() -> Self; }}\nimpl Build<{argument}> for crate::account::Account {{ fn new() -> Self {{ Self {{ city: 7, balance: 2 }} }} }}\npub fn value() -> u32 {{ let a = crate::account::Account::new(); a.city + a.balance }}\n"
    );
    self_lookup(&format!("const-{case}"), &client).await;
}

async fn self_lookup(case: &str, client: &str) {
    preserve(
        &format!("final-self-{case}"),
        MANIFEST,
        "mod account; #[path = \"account.rs\"] mod alternate; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        None,
        client,
        &["Self { city", "a.city"],
        None,
        "crate::account::Address",
        "preservation",
    )
    .await;
}
