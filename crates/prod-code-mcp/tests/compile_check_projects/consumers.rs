/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::gateway::{CargoCompileGateway, assert_workspace_clean, consumer_proof as proof};
use super::{MEMBER_CARGO, MEMBER_LIB, ROOT_WORKSPACE_MANIFEST, render_tool_text};
use prod_code_mcp::pull_push::{pull_up_impl, push_down_impl};
use prod_code_mcp::replace_conditional::replace_conditional_impl;
use prod_code_mcp::replace_constructor::{ReplaceMode, replace_constructor_impl};
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::Workspace;
use serde_json::json;

const EXCLUDED_CARGO: &str =
    "[package]\nname = \"excluded\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";

const CONDITIONAL_EXCLUDED_SOURCE: &str = "\
pub enum BirdType {
    European,
    African,
}

pub fn calculate_speed(bird: &BirdType) -> u32 {
    match bird {
        BirdType::European => 10,
        BirdType::African => 8,
    }
}
";

const CONDITIONAL_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", ROOT_WORKSPACE_MANIFEST),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    ("crates/excluded/Cargo.toml", EXCLUDED_CARGO),
    ("crates/excluded/src/lib.rs", CONDITIONAL_EXCLUDED_SOURCE),
];

#[tokio::test]
async fn replace_conditional_consumer_checks_excluded_package_and_preserves_force_semantics() {
    let workspace = Workspace::new(CONDITIONAL_FILES);
    let gateway = CargoCompileGateway::with_files(CONDITIONAL_FILES).await;
    let file = workspace.path("crates/excluded/src/lib.rs");

    // 1. Without `force`, a compiler error in the excluded crate rejects the refactoring
    //    before writing to disk.
    let err = replace_conditional_impl(
        gateway.addr,
        &workspace.root(),
        &file,
        7,
        5,
        "Bird",
        "get_speed",
        &[],
        Some("u32"),
        Some("bird"),
        true,
        false,
        Some("compile"),
    )
    .await
    .expect_err("excluded crate compiler error must reject replace_conditional when force=false");
    let message = format!("{err:#}");
    proof(
        "replace_conditional/refused",
        CONDITIONAL_FILES,
        &message,
        &workspace,
    );
    assert!(
        message.contains("compiler verification failed") && message.contains("E0599"),
        "expected E0599 from excluded crate compiler check, got: {message}"
    );
    let runs = gateway.take_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].request.subdir.as_deref(), Some("crates/excluded"));
    assert_eq!(runs[0].exit_code, Some(101));
    assert_workspace_clean(
        &workspace,
        CONDITIONAL_FILES,
        &["crates/member/target", "crates/excluded/target"],
    );

    // 2. With `force: true`, caller force semantics are preserved: the failed verdict is
    //    recorded in `verified = false` and `diagnostics`, while `apply: true` writes the file.
    let forced = replace_conditional_impl(
        gateway.addr,
        &workspace.root(),
        &file,
        7,
        5,
        "Bird",
        "get_speed",
        &[],
        Some("u32"),
        Some("bird"),
        true,
        true,
        Some("compile"),
    )
    .await
    .expect("force=true must preserve caller override semantics");
    proof(
        "replace_conditional/forced",
        CONDITIONAL_FILES,
        &format!("{forced:?}"),
        &workspace,
    );
    assert!(!forced.verified);
    assert!(forced.applied);
    assert!(
        forced
            .diagnostics
            .iter()
            .any(|d| d.contains("compiler errors:") && d.contains("E0599")),
        "expected compiler error diagnostic in forced result: {:?}",
        forced.diagnostics
    );
    let forced_runs = gateway.take_runs();
    assert_eq!(forced_runs.len(), 1);
    assert_eq!(
        forced_runs[0].request.subdir.as_deref(),
        Some("crates/excluded")
    );
    assert!(
        workspace
            .read("crates/excluded/src/lib.rs")
            .contains("pub trait Bird")
    );
}

const CONSTRUCTOR_CONST_FN_SOURCE: &str = "\
pub struct Point {
    pub x: i32,
    pub y: i32,
}

pub const fn origin() -> Point {
    Point { x: 0, y: 0 }
}
";

const CONSTRUCTOR_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", ROOT_WORKSPACE_MANIFEST),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    ("crates/excluded/Cargo.toml", EXCLUDED_CARGO),
    ("crates/excluded/src/lib.rs", CONSTRUCTOR_CONST_FN_SOURCE),
];

#[tokio::test]
async fn replace_constructor_factory_and_builder_consumers_check_excluded_package() {
    let workspace = Workspace::new(CONSTRUCTOR_FILES);
    let gateway = CargoCompileGateway::with_files(CONSTRUCTOR_FILES).await;
    let file = workspace.path("crates/excluded/src/lib.rs");

    for mode in [ReplaceMode::Factory, ReplaceMode::Builder] {
        let err = replace_constructor_impl(
            gateway.addr,
            &workspace.root(),
            &file,
            "Point",
            mode,
            None,
            true,
            false,
            Some("compile"),
        )
        .await
        .expect_err("non-const factory/builder call inside const fn in excluded crate must fail");
        let message = format!("{err:#}");
        proof(
            &format!("replace_constructor/{mode:?}"),
            CONSTRUCTOR_FILES,
            &message,
            &workspace,
        );
        assert!(
            message.contains("cargo check failed on the workspace") && message.contains("E0015"),
            "mode={mode:?} expected E0015 from excluded crate compiler check, got: {message}"
        );
        let runs = gateway.take_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].request.subdir.as_deref(), Some("crates/excluded"));
        assert_eq!(runs[0].exit_code, Some(101));
        assert_workspace_clean(
            &workspace,
            CONSTRUCTOR_FILES,
            &["crates/member/target", "crates/excluded/target"],
        );
    }
}

const ENCAPSULATE_EXCLUDED_SOURCE: &str = "\
pub struct Holder {
    pub data: String,
}
";

const ENCAPSULATE_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", ROOT_WORKSPACE_MANIFEST),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    ("crates/excluded/Cargo.toml", EXCLUDED_CARGO),
    ("crates/excluded/src/lib.rs", ENCAPSULATE_EXCLUDED_SOURCE),
];

#[tokio::test]
async fn compile_gate_consumer_checks_excluded_package_and_preserves_force_semantics() {
    let workspace = Workspace::new(ENCAPSULATE_FILES);
    let gateway = CargoCompileGateway::with_files(ENCAPSULATE_FILES).await;

    // 1. `force: false, apply: true, verify: "compile"` refuses to write when the excluded
    //    crate fails `cargo check` (by-value getter `pub fn data(&self) -> String { self.data }`
    //    cannot move `String` out of `&self`, failing with E0507).
    let rejected = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_encapsulate_field",
        json!({
            "path": "crates/excluded/src/lib.rs",
            "line": 2,
            "character": 9,
            "by_value": true,
            "verify": "compile",
            "apply": true,
            "force": false
        }),
    )
    .await
    .unwrap();
    let rejected_text = render_tool_text(&rejected);
    proof(
        "encapsulate_field/refused",
        ENCAPSULATE_FILES,
        &rejected_text,
        &workspace,
    );
    assert!(rejected.is_error, "{rejected_text}");
    assert!(
        rejected_text.contains("the compiler rejects the result")
            && rejected_text.contains("E0507")
            && rejected_text.contains("nothing was written: the compiler rejects it"),
        "{rejected_text}"
    );
    let runs = gateway.take_runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].request.subdir.as_deref(), Some("crates/excluded"));
    assert_eq!(runs[0].exit_code, Some(101));
    assert_workspace_clean(
        &workspace,
        ENCAPSULATE_FILES,
        &["crates/member/target", "crates/excluded/target"],
    );

    // 2. `force: true, apply: true, verify: "compile"` preserves caller force semantics:
    //    reports the compiler rejection while writing the forced edit to disk.
    let forced = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_encapsulate_field",
        json!({
            "path": "crates/excluded/src/lib.rs",
            "line": 2,
            "character": 9,
            "by_value": true,
            "verify": "compile",
            "apply": true,
            "force": true
        }),
    )
    .await
    .unwrap();
    let forced_text = render_tool_text(&forced);
    proof(
        "encapsulate_field/forced",
        ENCAPSULATE_FILES,
        &forced_text,
        &workspace,
    );
    assert!(
        forced.is_error,
        "verdict remains failed even when forced: {forced_text}"
    );
    assert!(
        forced_text.contains("the compiler rejects the result") && forced_text.contains("E0507"),
        "{forced_text}"
    );
    assert!(
        !forced_text.contains("nothing was written"),
        "{forced_text}"
    );
    let forced_runs = gateway.take_runs();
    assert_eq!(forced_runs.len(), 1);
    assert_eq!(
        forced_runs[0].request.subdir.as_deref(),
        Some("crates/excluded")
    );
    assert!(
        workspace
            .read("crates/excluded/src/lib.rs")
            .contains("pub fn data(&self) -> String")
    );
}

const TRAIT_HIERARCHY_EXCLUDED_SOURCE: &str = "\
pub trait BaseTrait {
    fn shared(&self) -> u32;
}

pub trait SubTrait: BaseTrait {
    fn sub_only(&self) -> u32;
}

pub struct BaseImpl;
impl BaseTrait for BaseImpl {
    fn shared(&self) -> u32 {
        1
    }
}

pub fn call_shared<T: BaseTrait>(item: &T) -> u32 {
    item.shared()
}
";

const TRAIT_HIERARCHY_FILES: &[(&str, &str)] = &[
    ("Cargo.toml", ROOT_WORKSPACE_MANIFEST),
    ("crates/member/Cargo.toml", MEMBER_CARGO),
    ("crates/member/src/lib.rs", MEMBER_LIB),
    ("crates/excluded/Cargo.toml", EXCLUDED_CARGO),
    (
        "crates/excluded/src/lib.rs",
        TRAIT_HIERARCHY_EXCLUDED_SOURCE,
    ),
];

#[tokio::test]
async fn pull_up_and_push_down_consumers_check_excluded_package() {
    let workspace = Workspace::new(TRAIT_HIERARCHY_FILES);
    let gateway = CargoCompileGateway::with_files(TRAIT_HIERARCHY_FILES).await;
    let file = workspace.path("crates/excluded/src/lib.rs");

    // 1. `pull_up_impl` moving `sub_only` into `BaseTrait` breaks `impl BaseTrait for BaseImpl` (E0046).
    let pull_err = pull_up_impl(
        gateway.addr,
        &workspace.root(),
        &file,
        "SubTrait",
        Some("BaseTrait"),
        &["sub_only".to_string()],
        true,
        false,
        false,
        Some("compile"),
    )
    .await
    .expect_err("pull_up compiler failure in excluded crate must abort when force=false");
    let pull_msg = format!("{pull_err:#}");
    proof(
        "pull_up/refused",
        TRAIT_HIERARCHY_FILES,
        &pull_msg,
        &workspace,
    );
    assert!(
        pull_msg.contains("compiler verification failed") && pull_msg.contains("E0046"),
        "expected E0046 from pull_up excluded crate check, got: {pull_msg}"
    );
    let pull_runs = gateway.take_runs();
    assert_eq!(pull_runs.len(), 1);
    assert_eq!(
        pull_runs[0].request.subdir.as_deref(),
        Some("crates/excluded")
    );

    // 2. `push_down_impl` moving `shared` out of `BaseTrait` into `SubTrait` breaks `call_shared` (E0599).
    let push_err = push_down_impl(
        gateway.addr,
        &workspace.root(),
        &file,
        "BaseTrait",
        None,
        &["shared".to_string()],
        true,
        false,
        Some("compile"),
    )
    .await
    .expect_err("push_down compiler failure in excluded crate must abort when force=false");
    let push_msg = format!("{push_err:#}");
    proof(
        "push_down/refused",
        TRAIT_HIERARCHY_FILES,
        &push_msg,
        &workspace,
    );
    assert!(
        push_msg.contains("compiler verification failed") && push_msg.contains("E0599"),
        "expected E0599 from push_down excluded crate check, got: {push_msg}"
    );
    let push_runs = gateway.take_runs();
    assert_eq!(push_runs.len(), 1);
    assert_eq!(
        push_runs[0].request.subdir.as_deref(),
        Some("crates/excluded")
    );
    assert_workspace_clean(
        &workspace,
        TRAIT_HIERARCHY_FILES,
        &["crates/member/target", "crates/excluded/target"],
    );
}
