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
async fn external_literal_only_files_preserve_complete_values() {
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", OWNER),
        (
            "src/client.rs",
            "use crate::account::Account;\npub fn value() -> u32 {\n    let city = 17;\n    let a = Account { city, balance: 9 };\n    let Account { city: home, balance } = a;\n    home * 100 + balance\n}\n",
        ),
        (
            "src/main.rs",
            "mod account;\nmod client;\nfn main() { println!(\"{}\", client::value()); }\n",
        ),
    ]);
    assert_eq!(run("external", "before", &ws), "1709\n");
    let (gw, seen) = gateway(
        &ws,
        &[(
            "src/client.rs",
            &["a = Account { city", "let Account { city"],
        )],
    )
    .await;
    let result = extract(&ws, &gw, &[]).await.unwrap();
    assert!(result.applied);
    assert_eq!(run("external", "after", &ws), "1709\n");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|uri| uri.ends_with("/src/client.rs"))
    );
}

#[tokio::test]
async fn moved_method_rewrites_other_owner_but_keeps_helper_self() {
    let owner = format!(
        "{OWNER}\nimpl Account {{\n    pub fn same(&self, other: &Account) -> bool {{ self.city == other.city }}\n}}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account;\nuse account::Account;\nfn main() {\n    let a = Account { city: 7, balance: 1 };\n    let b = Account { city: 7, balance: 2 };\n    let c = Account { city: 8, balance: 3 };\n    println!(\"{} {}\", a.same(&b), a.same(&c));\n}\n",
        ),
    ]);
    assert_eq!(run("other", "before", &ws), "true false\n");
    let (gw, _) = gateway(
        &ws,
        &[
            ("src/account.rs", &["self.city", "other.city"]),
            (
                "src/main.rs",
                &[
                    "a = Account { city",
                    "b = Account { city",
                    "c = Account { city",
                ],
            ),
        ],
    )
    .await;
    extract(&ws, &gw, &["same"]).await.unwrap();
    assert_eq!(run("other", "after", &ws), "true false\n");
    assert!(
        ws.read("src/account.rs")
            .contains("self.city == other.address.city")
    );
}

#[tokio::test]
async fn async_forwarder_is_actually_polled_and_sync_stays_sync() {
    let owner = format!(
        "{OWNER}\nimpl Account {{\n    pub async fn value(&self, extra: u32) -> u32 {{ self.city + extra }}\n    pub fn sync_value(&self) -> u32 {{ self.city }}\n}}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            r#"mod account;
use account::Account;
use std::{future::Future, pin::pin, sync::Arc, task::{Context, Poll, Wake, Waker}};
struct Noop;
impl Wake for Noop { fn wake(self: Arc<Self>) {} }
fn main() {
    let a = Account { city: 23, balance: 4 };
    let waker = Waker::from(Arc::new(Noop));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(a.value(5));
    let Poll::Ready(value) = future.as_mut().poll(&mut context) else { panic!("not ready") };
    println!("{} {}", value, a.sync_value());
}
"#,
        ),
    ]);
    assert_eq!(run("async", "before", &ws), "28 23\n");
    let (gw, _) = gateway(
        &ws,
        &[
            (
                "src/account.rs",
                &["{ self.city", "sync_value(&self) -> u32 { self.city"],
            ),
            ("src/main.rs", &["a = Account { city"]),
        ],
    )
    .await;
    extract(&ws, &gw, &["value", "sync_value"]).await.unwrap();
    assert_eq!(run("async", "after", &ws), "28 23\n");
    let after = ws.read("src/account.rs");
    assert!(after.contains("self.address.value(extra).await"));
    assert!(after.contains("self.address.sync_value()\n"));
}

#[tokio::test]
async fn same_file_self_constructor_and_mutating_sync_method_remain_usable() {
    let owner = format!(
        "{OWNER}\nimpl Account {{\n    pub fn new(city: u32) -> Self {{ Self {{ city, balance: 9 }} }}\n    pub fn add(&mut self, value: u32) -> u32 {{ self.city += value; self.city }}\n    pub fn read(&self) -> u32 {{ self.city }}\n}}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account;\nfn main() { let mut a = account::Account::new(11); println!(\"{} {}\", a.add(3), a.read()); }\n",
        ),
    ]);
    assert_eq!(run("sync", "before", &ws), "14 14\n");
    let (gw, _) = gateway(
        &ws,
        &[(
            "src/account.rs",
            &[
                "Self { city",
                "{ self.city",
                "; self.city",
                "read(&self) -> u32 { self.city",
            ],
        )],
    )
    .await;
    extract(&ws, &gw, &["add", "read"]).await.unwrap();
    assert_eq!(run("sync", "after", &ws), "14 14\n");
    assert!(
        ws.read("src/account.rs")
            .contains("pub fn add(&mut self, value: u32) -> u32")
    );
}

#[tokio::test]
async fn semantic_entries_preserve_unrelated_names_literals_patterns_and_access_order() {
    let client = r###"use crate::account::Account as Owner;
mod unrelated { pub struct Account { pub city: u32 } }
pub fn value() -> String {
    let unchanged = r#"Account { city: 91 } self.balance other.city"#;
    // Account { city: 92 } self.balance other.city
    let untouched = unrelated::Account { city: 93 };
    let city = "Q, Account { city: 94 }".to_owned();
    let mut a = Owner { city: { city }, balance: 5 };
    a.city.push_str("!");
    let Owner { city: ref mut home, balance } = a;
    home.push_str("?");
    format!("{}|{}|{}|{}", home, balance, untouched.city, unchanged)
}
"###;
    let owner = "pub struct Account { pub city: String, pub balance: u32 }\n";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", owner),
        ("src/client.rs", client),
        (
            "src/main.rs",
            "mod account;\nmod client;\nfn main() { println!(\"{}\", client::value()); }\n",
        ),
    ]);
    let expected = "Q, Account { city: 94 }!?|5|93|Account { city: 91 } self.balance other.city\n";
    assert_eq!(run("lexical", "before", &ws), expected);
    let (gw, _) = gateway(
        &ws,
        &[(
            "src/client.rs",
            &["Owner { city", "a.city", "let Owner { city"],
        )],
    )
    .await;
    let result = extract(&ws, &gw, &[]).await.unwrap();
    assert_eq!(result.accesses, 1);
    assert_eq!(run("lexical", "after", &ws), expected);
    let after = ws.read("src/client.rs");
    for line in client.lines().filter(|line| {
        line.contains("unchanged =")
            || line.contains("// Account")
            || line.contains("let untouched")
    }) {
        assert!(
            after.lines().any(|candidate| candidate == line),
            "changed unrelated bytes: {line}"
        );
    }
    assert!(after.contains("a.address.city.push_str"));
    assert!(after.contains("city: ref mut home"));
}
