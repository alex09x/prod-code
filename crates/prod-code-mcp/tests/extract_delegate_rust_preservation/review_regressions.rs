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

#[tokio::test]
async fn review_inline_module_helper_path() {
    let owner = format!("pub mod inner {{\n{OWNER}}}\n");
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/client.rs",
            "use crate::account::inner::Account;\npub fn value() -> u32 { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; city + balance }\n",
        ),
        (
            "src/main.rs",
            "mod account; mod client; fn main() { println!(\"{}\", client::value()); }\n",
        ),
    ]);
    let case = "review-inline-module";
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
}

#[tokio::test]
async fn review_same_file_nested_import() {
    let owner = format!(
        "{OWNER}pub fn top() -> u32 {{ let a = Account {{ city: 3, balance: 4 }}; let Account {{ city: c, balance: b }} = a; c + b }}\npub mod client {{\nuse super::Account;\npub fn value() -> u32 {{ let a = Account {{ city: 7, balance: 2 }}; let Account {{ city, balance }} = a; city + balance }}\n}}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account; fn main() { println!(\"{}\", account::client::value() + account::top()); }\n",
        ),
    ]);
    let case = "review-same-file-nested";
    assert_eq!(run(case, "before", &ws), "16\n");
    let (gw, _) = gateway_case(
        &ws,
        &[(
            "src/account.rs",
            &[
                "top() -> u32 { let a = Account { city",
                "balance: 4 }; let Account { city",
                "value() -> u32 { let a = Account { city",
                "balance: 2 }; let Account { city",
            ],
        )],
        Some(case),
    )
    .await;
    extract(&ws, &gw, &[]).await.unwrap();
    assert_eq!(run(case, "after", &ws), "16\n");
}

#[tokio::test]
async fn review_library_consumed_by_binary() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", OWNER),
        ("src/lib.rs", "pub mod account;\n"),
        (
            "src/main.rs",
            "use preservation::account::Account; fn main() { let a = Account { city: 7, balance: 2 }; let Account { city, balance } = a; println!(\"{}\", city + balance); }\n",
        ),
    ]);
    let case = "review-lib-bin";
    assert_eq!(run(case, "before", &ws), "9\n");
    let (gw, _) = gateway_case(
        &ws,
        &[("src/main.rs", &["a = Account { city", "let Account { city"])],
        Some(case),
    )
    .await;
    extract(&ws, &gw, &[]).await.unwrap();
    assert_eq!(run(case, "after", &ws), "9\n");
}

#[tokio::test]
async fn review_member_receiver_alias_collision() {
    let owner = format!(
        "{OWNER}pub struct Holder {{ pub this: Account }}\nimpl Account {{ pub fn read(&self, holder: &Holder) -> u32 {{ let this = self; this.city + holder.this.city }} }}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account; use account::{Account, Holder}; fn main() { let a = Account { city: 7, balance: 2 }; let h = Holder { this: Account { city: 9, balance: 3 } }; println!(\"{}\", a.read(&h)); }\n",
        ),
    ]);
    let case = "review-member-alias";
    assert_eq!(run(case, "before", &ws), "16\n");
    let (gw, _) = gateway_case(
        &ws,
        &[
            ("src/account.rs", &["self; this.city", "holder.this.city"]),
            (
                "src/main.rs",
                &["a = Account { city", "this: Account { city"],
            ),
        ],
        Some(case),
    )
    .await;
    extract(&ws, &gw, &["read"]).await.unwrap();
    assert_eq!(run(case, "after", &ws), "16\n");
    assert!(
        ws.read("src/account.rs")
            .contains("this.city + holder.this.address.city")
    );
}

#[tokio::test]
async fn review_unmoved_turbofish_value() {
    let owner = format!(
        "{OWNER}impl Account {{ pub fn new() -> Self {{ Self {{ city: 7, balance: std::mem::size_of::<u64>() as u32 }} }} }}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account; fn main() { let a = account::Account::new(); println!(\"{}\", a.city + a.balance); }\n",
        ),
    ]);
    let case = "review-turbofish";
    assert_eq!(run(case, "before", &ws), "15\n");
    let (gw, _) = gateway_case(
        &ws,
        &[
            ("src/account.rs", &["Self { city"]),
            ("src/main.rs", &["a.city"]),
        ],
        Some(case),
    )
    .await;
    let result = extract(&ws, &gw, &[]).await;
    if let Err(error) = &result {
        refusal(case, &error.to_string(), &ws);
    }
    result.unwrap();
    assert_eq!(run(case, "after", &ws), "15\n");
}

#[tokio::test]
async fn review_parenthesized_member_receiver() {
    let owner = format!(
        "{OWNER}pub struct Holder {{ pub this: Account }}\nimpl Account {{ pub fn read(&self, holder: &Holder) -> u32 {{ let this = self; this.city + (holder.this).city }} }}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account; use account::{Account, Holder}; fn main() { let a = Account { city: 7, balance: 2 }; let h = Holder { this: Account { city: 9, balance: 3 } }; println!(\"{}\", a.read(&h)); }\n",
        ),
    ]);
    let case = "review-parenthesized-member";
    assert_eq!(run(case, "before", &ws), "16\n");
    let (gw, _) = gateway_case(
        &ws,
        &[
            ("src/account.rs", &["self; this.city", "(holder.this).city"]),
            (
                "src/main.rs",
                &["a = Account { city", "this: Account { city"],
            ),
        ],
        Some(case),
    )
    .await;
    extract(&ws, &gw, &["read"]).await.unwrap();
    assert_eq!(run(case, "after", &ws), "16\n");
}

#[tokio::test]
async fn review_balanced_turbofish_commas() {
    let owner = format!(
        "{OWNER}impl Account {{ pub fn new() -> Self {{ Self {{ city: 7, balance: std::mem::size_of::<Result<u64, u32>>() as u32 }} }} }}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account; fn main() { let a = account::Account::new(); println!(\"{}\", a.city + a.balance); }\n",
        ),
    ]);
    let case = "review-turbofish-commas";
    assert_eq!(run(case, "before", &ws), "23\n");
    let (gw, _) = gateway_case(
        &ws,
        &[
            ("src/account.rs", &["Self { city"]),
            ("src/main.rs", &["a.city"]),
        ],
        Some(case),
    )
    .await;
    extract(&ws, &gw, &[]).await.unwrap();
    assert_eq!(run(case, "after", &ws), "23\n");
}
