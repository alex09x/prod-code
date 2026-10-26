/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::nominal_support::preserve;

const ROOT: &str = "mod account; #[path = \"account.rs\"] mod alternate; mod client; fn main() { println!(\"{}\", client::value()); }\n";
const ALIASED: &str = "pub fn value() -> u32 { let a = model::Account { city: 7, balance: 2 }; let model::Account { city, balance } = a; city + balance }\n";
const IMPORTED: &str = "pub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n";

#[tokio::test]
async fn nominal_module_alias_constructor_and_pattern() {
    preserve(
        "module-alias",
        ROOT,
        None,
        &format!("use crate::alternate as model;\n{ALIASED}"),
        &["model::Account { city", "let model::Account { city"],
        "crate::alternate::Address",
    )
    .await;
}

#[tokio::test]
async fn nominal_grouped_block_module_alias() {
    preserve(
        "grouped-block-alias", ROOT, None,
        "use crate::account as model;\npub fn value() -> u32 { use crate::{alternate as model}; let a = model::Account { city: 7, balance: 2 }; let model::Account { city, balance } = a; city + balance }\n",
        &["model::Account { city", "let model::Account { city"],
        "crate::alternate::Address",
    ).await;
}

#[tokio::test]
async fn nominal_absolute_external_constructor_under_local_root() {
    preserve(
        "absolute-constructor",
        "mod preservation {} mod client; fn main() { println!(\"{}\", client::value()); }\n",
        Some("pub mod account;\n"),
        "use crate::preservation;\npub fn value() -> u32 { let a = ::preservation::account::Account { city: 7, balance: 2 }; let ::preservation::account::Account { city, balance } = a; city + balance }\n",
        &["::preservation::account::Account { city", "let ::preservation::account::Account { city"],
        "::preservation::account::Address",
    ).await;
}

#[tokio::test]
async fn nominal_absolute_external_owner_import_under_local_root() {
    preserve(
        "absolute-import",
        "mod preservation {} mod client; fn main() { println!(\"{}\", client::value()); }\n",
        Some("pub mod account;\n"),
        &format!("use crate::preservation; use ::preservation::account::Account;\n{IMPORTED}"),
        &["Account { city", "let Account { city"],
        "::preservation::account::Address",
    )
    .await;
}

#[tokio::test]
async fn nominal_constant_cfg_attr_module_path() {
    cfg_path("constant-path", "#[cfg_attr(all(), path = \"account.rs\")]").await;
}

#[tokio::test]
async fn nominal_nested_cfg_attr_module_path_and_cfg() {
    cfg_path(
        "nested-path",
        "#[cfg_attr(all(), cfg_attr(not(any()), path = \"account.rs\", cfg(all())), cfg_attr(any(), cfg(any()), path = \"missing.rs\"))]",
    ).await;
}

#[tokio::test]
async fn nominal_target_dependent_cfg_attr_keeps_possible_path() {
    cfg_path("possible-path", "#[cfg_attr(unix, path = \"account.rs\")]").await;
}

#[tokio::test]
async fn nominal_unknown_cfg_attr_without_module_effect_is_definite() {
    cfg_path(
        "harmless-unknown",
        "#[cfg_attr(unix, cfg_attr(any(), cfg(any()), path = \"missing.rs\"))] #[cfg_attr(all(), path = \"account.rs\")]",
    ).await;
}

#[tokio::test]
async fn nominal_chained_module_alias_and_self_import() {
    preserve(
        "chained-alias",
        ROOT,
        None,
        &format!("use crate::alternate::{{self as actual}}; use actual as model;\n{ALIASED}"),
        &["model::Account { city", "let model::Account { city"],
        "crate::alternate::Address",
    )
    .await;
}

#[tokio::test]
async fn nominal_chain_keeps_alternate_declaration_scope() {
    preserve(
        "chain-alternate-scope", ROOT, None,
        "use crate::alternate as actual; use actual as model;\npub fn value() -> u32 { use crate::account as actual; let a = model::Account { city: 7, balance: 2 }; let model::Account { city, balance } = a; city + balance }\n",
        &["model::Account { city", "let model::Account { city"],
        "crate::alternate::Address",
    ).await;
}

#[tokio::test]
async fn nominal_chain_keeps_account_declaration_scope() {
    preserve(
        "chain-account-scope", ROOT, None,
        "use crate::account as actual; use actual as model;\npub fn value() -> u32 { use crate::alternate as actual; let a = model::Account { city: 7, balance: 2 }; let model::Account { city, balance } = a; city + balance }\n",
        &["model::Account { city", "let model::Account { city"],
        "crate::account::Address",
    ).await;
}

#[tokio::test]
async fn nominal_conditional_alias_keeps_external_alternative() {
    super::nominal_support::preserve_files(
        "conditional-extern",
        "mod local; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        Some("pub mod account;\n"),
        "#[cfg(windows)] use crate::local as preservation;\npub fn value() -> u32 { let a = preservation::account::Account { city: 7, balance: 2 }; let preservation::account::Account { city, balance } = a; city + balance }\n",
        "pub struct Account { pub city: u32, pub balance: u32 }\n",
        "src/client.rs",
        &["preservation::account::Account { city", "let preservation::account::Account { city"],
        "::preservation::account::Address",
        &[("src/local.rs", "#[path = \"account.rs\"] pub mod account;\n")],
    ).await;
}

#[tokio::test]
async fn nominal_relative_local_module_is_distinct_from_extern_root() {
    preserve(
        "relative-local", ROOT, Some("pub mod account;\n"),
        "#[path = \"account.rs\"] mod preservation;\npub fn value() -> u32 { let a = preservation::Account { city: 7, balance: 2 }; let preservation::Account { city, balance } = a; city + balance }\n",
        &["preservation::Account { city", "let preservation::Account { city"],
        "self::preservation::Address",
    ).await;
}

#[tokio::test]
async fn nominal_unresolved_explicit_constructor_refuses_before_writes() {
    super::nominal_support::unresolved_refuses().await;
}

#[tokio::test]
async fn nominal_alias_inside_shared_owner_file() {
    super::nominal_support::preserve_owner(
        "owner-file-alias", ROOT, None,
        "pub fn value() -> u32 { crate::account::value() }\n",
        &format!("pub struct Account {{ pub city: u32, pub balance: u32 }}\nuse crate::alternate as model;\n{ALIASED}"),
        "src/account.rs",
        &["model::Account { city", "let model::Account { city"],
        "crate::alternate::Address",
    ).await;
}

async fn cfg_path(case: &str, attributes: &str) {
    preserve(
        case,
        &format!("mod account; {attributes} mod alternate; mod client; fn main() {{ println!(\"{{}}\", client::value()); }}\n"),
        None,
        &format!("use crate::alternate::Account;\n{IMPORTED}"),
        &["Account { city", "let Account { city"],
        "crate::alternate::Address",
    ).await;
}
