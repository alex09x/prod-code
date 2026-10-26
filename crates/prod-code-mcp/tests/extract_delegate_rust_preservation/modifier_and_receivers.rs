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
async fn raw_async_parameter_type_does_not_await_sync_forwarder() {
    let owner = format!(
        "{OWNER}pub mod r#async {{ pub struct Marker; }}\nimpl Account {{\n    pub fn read(&self, _marker: r#async::Marker) -> u32 {{ self.city }}\n}}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account;\nuse account::Account;\nfn main() { let a = Account { city: 31, balance: 2 }; println!(\"{}\", a.read(account::r#async::Marker)); }\n",
        ),
    ]);
    assert_eq!(run("raw-async", "before", &ws), "31\n");
    let (gw, _) = gateway(
        &ws,
        &[
            ("src/account.rs", &["self.city"]),
            ("src/main.rs", &["Account { city"]),
        ],
    )
    .await;
    extract(&ws, &gw, &["read"]).await.unwrap();
    assert_eq!(run("raw-async", "after", &ws), "31\n");
    let after = ws.read("src/account.rs");
    assert!(after.contains("self.address.read(_marker)\n"));
    assert!(!after.contains(".await"));
}

#[tokio::test]
async fn self_aliases_and_parentheses_keep_helper_origin() {
    let owner = format!(
        "{OWNER}\nimpl Account {{\n    pub fn read(&self, other: &Account) -> u32 {{\n        let this = self;\n        let alias = (this);\n        this.city + alias.city + (self).city + ((self)).city + other.city\n    }}\n}}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account;\nuse account::Account;\nfn main() { let a = Account { city: 7, balance: 2 }; let b = Account { city: 9, balance: 3 }; println!(\"{}\", a.read(&b)); }\n",
        ),
    ]);
    let case = "self-alias-v3-retry-observed";
    assert_eq!(run(case, "before", &ws), "37\n");
    let (gw, _) = gateway_case(
        &ws,
        &[
            (
                "src/account.rs",
                &[
                    "this.city",
                    "alias.city",
                    "(self).city",
                    "((self)).city",
                    "other.city",
                ],
            ),
            ("src/main.rs", &["a = Account { city", "b = Account { city"]),
        ],
        Some(case),
    )
    .await;
    extract(&ws, &gw, &["read"]).await.unwrap();
    assert_eq!(run(case, "after", &ws), "37\n");
    assert!(
        ws.read("src/account.rs")
            .contains("this.city + alias.city + (self).city + ((self)).city + other.address.city")
    );
}

#[tokio::test]
async fn local_shadowing_and_independent_aliases_keep_owner_origin() {
    let owner = format!(
        "{OWNER}\nimpl Account {{\n    pub fn read(&self, other: &Account) -> u32 {{\n        let this = self;\n        let independent = other;\n        let value = {{ let this = other; this.city }};\n        this.city + independent.city + (other).city + value\n    }}\n}}\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/account.rs", &owner),
        (
            "src/main.rs",
            "mod account;\nuse account::Account;\nfn main() { let a = Account { city: 7, balance: 2 }; let b = Account { city: 9, balance: 3 }; println!(\"{}\", a.read(&b)); }\n",
        ),
    ]);
    assert_eq!(run("alias-shadow", "before", &ws), "34\n");
    let (gw, _) = gateway(
        &ws,
        &[
            (
                "src/account.rs",
                &[
                    "other; this.city",
                    "};\n        this.city",
                    "independent.city",
                    "(other).city",
                ],
            ),
            ("src/main.rs", &["a = Account { city", "b = Account { city"]),
        ],
    )
    .await;
    extract(&ws, &gw, &["read"]).await.unwrap();
    assert_eq!(run("alias-shadow", "after", &ws), "34\n");
    let after = ws.read("src/account.rs");
    assert!(after.contains("let this = other; this.address.city"));
    assert!(after.contains("this.city + independent.address.city + (other).address.city"));
}

#[tokio::test]
async fn unsupported_alias_forms_refuse_without_writing() {
    for (index, (body, needle)) in [
        ("let this = { self }; this.city", "this.city"),
        ("let mut this = self; this = other; this.city", "this.city"),
        ("let mut this = other; this = self; this.city", "this.city"),
        ("let (this,) = (self,); this.city", "this.city"),
        (
            "let this = self; [other].iter().map(|this| this.city).sum()",
            "this.city",
        ),
        (
            "let mut this = self; let _ = { this = other; }; this.city",
            "this.city",
        ),
        (
            "let this = self; if let Some(this) = Some(other) { this.city } else { 0 }",
            "this.city",
        ),
        ("for this in [self] { return this.city; } 0", "this.city"),
    ]
    .into_iter()
    .enumerate()
    {
        let owner = format!(
            "{OWNER}\nimpl Account {{ pub fn read(&self, other: &Account) -> u32 {{ {body} }} }}\n"
        );
        let ws = Workspace::new(&[("Cargo.toml", MANIFEST), ("src/account.rs", &owner)]);
        let case = format!("refusal-alias-{index}");
        let (gw, _) = gateway_case(&ws, &[("src/account.rs", &[needle])], Some(&case)).await;
        artifact(&case, "before", &ws);
        let error = extract(&ws, &gw, &["read"]).await.unwrap_err().to_string();
        assert!(error.contains("unsupported self-alias"), "{error}");
        assert_eq!(ws.read("src/account.rs"), owner);
        refusal(&case, &error, &ws);
    }
}
