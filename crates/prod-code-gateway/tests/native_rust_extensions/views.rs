/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Fixture, SOURCE, check_tree, expanded};
use prod_code_gateway::admission::Admission;
use prod_code_gateway::workspace::SharedWorkspace;
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
async fn native_extensions_respect_worktree_and_validation_views() {
    let fixture = Fixture::new(SOURCE, "2021").await;
    let worktree = fixture.temp.path().join("worktree");
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    std::fs::copy(
        fixture.workspace.root.join("Cargo.toml"),
        worktree.join("Cargo.toml"),
    )
    .unwrap();
    std::fs::copy(
        fixture.workspace.root.join("prod-code.toml"),
        worktree.join("prod-code.toml"),
    )
    .unwrap();
    let worktree_source = SOURCE.replace("+ 10", "+ 50");
    std::fs::write(worktree.join("src/lib.rs"), &worktree_source).unwrap();
    fixture
        .workspace
        .rust_engine
        .as_ref()
        .unwrap()
        .lock()
        .await
        .attach_worktree(&worktree)
        .unwrap();
    let workspace = Arc::new(SharedWorkspace::with_base(
        worktree.clone(),
        "rust".into(),
        fixture.workspace.rust_engine.clone(),
        None,
        None,
        None,
        Some(Arc::clone(&fixture.workspace)),
    ));
    fixture
        .manager
        .insert_ready_for_test(Arc::clone(&workspace))
        .await;
    let mut base = fixture.session(41).await;
    let mut wt = fixture.session_in(42, &workspace).await;
    let validation = workspace
        .validation_view(&Arc::new(Admission::unbounded()))
        .await
        .unwrap();
    assert!(validation.backend.is_none());
    assert!(!Arc::ptr_eq(
        validation.rust_engine.as_ref().unwrap(),
        workspace.rust_engine.as_ref().unwrap()
    ));
    let mut a = fixture.session_in(43, &workspace).await;
    let mut b = fixture.session_in(44, &workspace).await;
    a.view.workspace = Arc::clone(&validation);
    b.view.workspace = validation;
    let proposal = worktree_source.replace("+ 50", "+ 71");
    a.open(&proposal).await;
    check_tree(&a.tree(json!("validation-proposal")).await, &proposal);
    expanded(
        a.expand(json!("validation-macro"), &proposal, "choose!(1)")
            .await,
        "1+71",
    );
    check_tree(&b.tree(json!("validation-base")).await, &worktree_source);
    expanded(
        b.expand(
            json!("validation-base-macro"),
            &worktree_source,
            "choose!(1)",
        )
        .await,
        "1+50",
    );
    check_tree(&wt.tree(json!("worktree")).await, &worktree_source);
    expanded(
        wt.expand(json!("worktree-macro"), &worktree_source, "choose!(1)")
            .await,
        "1+50",
    );
    check_tree(&base.tree(json!("base")).await, SOURCE);
    expanded(
        base.expand(json!("base-macro"), SOURCE, "choose!(1)").await,
        "1+10",
    );
    fixture.finish(vec![a, b, wt, base]).await;
}

#[tokio::test]
async fn native_extensions_use_the_consumers_edition() {
    let source = concat!(
        "macro_rules! choose { ($x:expr) => { $x }; }\n",
        "pub fn gen() {}\n",
        "pub fn example() -> i32 { choose!(const { 7 }) }\n",
    );
    for edition in ["2021", "2024"] {
        let fixture = Fixture::new(source, edition).await;
        let mut session = fixture.session(51).await;
        let tree = session.tree(json!(edition)).await;
        check_tree(&tree, source);
        assert_eq!(has_kind(&tree, "GEN_KW"), edition == "2024");
        if edition == "2024" {
            expanded(
                session
                    .expand(json!("edition-expansion"), source, "choose!(const")
                    .await,
                "(const{7})",
            );
        }
        fixture.finish(vec![session]).await;
    }
}

fn has_kind(tree: &Value, kind: &str) -> bool {
    tree["kind"] == kind
        || tree["children"]
            .as_array()
            .is_some_and(|children| children.iter().any(|child| has_kind(child, kind)))
}
