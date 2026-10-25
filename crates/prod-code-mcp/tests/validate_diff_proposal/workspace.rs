/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

const MEMBER_MANIFEST: &str = "[package]\nname=\"member\"\nversion=\"0.1.0\"\nedition=\"2024\"\n";
const MEMBER: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers=[\"crates/member\"]\nresolver=\"3\"\n",
    ),
    ("crates/member/Cargo.toml", MEMBER_MANIFEST),
    ("crates/member/src/lib.rs", "pub mod child;\n"),
    ("crates/member/src/child.rs", "pub const CHILD:u32=1;\n"),
    ("crates/member/src/unused.rs", "pub const UNUSED:u32=2;\n"),
];

fn unchanged(workspace: &Workspace, files: &[(&str, &str)]) {
    for (path, bytes) in files {
        assert_eq!(workspace.read(path), *bytes);
    }
    for path in [
        "Cargo.lock",
        "target",
        "crates/member/Cargo.lock",
        "crates/member/target",
    ] {
        assert!(!workspace.path(path).exists());
    }
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(workspace.root())
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(status.stdout.is_empty());
}

#[tokio::test]
async fn cargo_workspace_member_uses_root_command_for_complete_proposals() {
    // Actual Cargo runs on the complete serialized proposal, including member deletions.
    let workspace = Workspace::new(MEMBER);
    let gateway = CompilerGateway::with_files(MEMBER).await;
    let unused = "--- a/crates/member/src/unused.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-pub const UNUSED:u32=2;\n";
    let remove_declaration = "--- a/crates/member/src/lib.rs\n+++ b/crates/member/src/lib.rs\n@@ -1 +1 @@\n-pub mod child;\n+pub const VALUE:u32=3;\n";
    let child =
        "--- a/crates/member/src/child.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-pub const CHILD:u32=1;\n";
    for (tool, args, expected, fails) in [
        (
            "code_validate_edit",
            json!({"path":"crates/member/src/lib.rs","new_text":"pub mod child;\npub const VALUE:u32=3;\n","compile":true}),
            vec![(
                "crates/member/src/lib.rs",
                Some("pub mod child;\npub const VALUE:u32=3;\n"),
            )],
            false,
        ),
        (
            "code_validate_edits",
            json!({"diff":format!("{remove_declaration}{child}"),"compile":true}),
            vec![
                ("crates/member/src/lib.rs", Some("pub const VALUE:u32=3;\n")),
                ("crates/member/src/child.rs", None),
            ],
            false,
        ),
        (
            "code_validate_edits",
            json!({"diff":unused,"borrow_check":true}),
            vec![("crates/member/src/unused.rs", None)],
            false,
        ),
        (
            "code_validate_edits",
            json!({"diff":child,"compile":true}),
            vec![("crates/member/src/child.rs", None)],
            true,
        ),
    ] {
        let before = gateway.requests.lock().unwrap().len();
        let result = execute_tool(gateway.addr, &workspace.root(), tool, args)
            .await
            .unwrap();
        let report = text(&result);
        assert_eq!(result.is_error, fails, "{report}");
        assert!(!report.contains("unverified/incomplete"), "{report}");
        assert!(report.contains("cwd=\".\""), "{report}");
        if fails {
            assert!(report.contains("E0583"), "{report}");
        } else {
            assert!(
                report.contains("exit=0 error=null timed_out=false"),
                "{report}"
            );
            assert!(
                report.contains("coverage is limited to the displayed command"),
                "{report}"
            );
        }
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(requests.len(), before + 1);
        let request = requests.last().unwrap();
        assert_eq!(request.subdir, None);
        assert_eq!(
            request.command,
            [
                "cargo",
                "check",
                "--workspace",
                "--all-targets",
                "--message-format=json"
            ]
        );
        let deltas = &request.hypotheses[0].files;
        assert_eq!(deltas.len(), expected.len());
        for (delta, (path, content)) in deltas.iter().zip(expected) {
            assert_eq!(delta.relative_path, path);
            assert_eq!(delta.content.as_deref(), content.map(str::as_bytes));
        }
        unchanged(&workspace, MEMBER);
    }
}

const GLOB_MEMBER: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers=[\"crates/*\"]\nresolver=\"3\"\n",
    ),
    MEMBER[1],
    MEMBER[2],
    MEMBER[3],
    MEMBER[4],
];
const IMPLICIT_MEMBER: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname=\"outer\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n[dependencies]\nmember={path=\"crates/member\"}\n",
    ),
    ("src/lib.rs", "pub use member::child::CHILD;\n"),
    MEMBER[1],
    MEMBER[2],
    MEMBER[3],
    MEMBER[4],
];

#[tokio::test]
async fn glob_and_implicit_cargo_members_reuse_workspace_ownership() {
    for files in [GLOB_MEMBER, IMPLICIT_MEMBER] {
        let workspace = Workspace::new(files);
        let gateway = CompilerGateway::with_files(files).await;
        let result = execute_tool(
            gateway.addr, &workspace.root(), "code_validate_edits",
            json!({"edits":[{"path":"crates/member/src/child.rs","new_text":"pub const CHILD:u32=9;\n"}],"compile":true}),
        ).await.unwrap();
        let report = text(&result);
        assert!(!result.is_error, "{report}");
        assert!(!report.contains("unverified/incomplete"), "{report}");
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].subdir, None);
        assert_eq!(
            requests[0].hypotheses[0].files[0].relative_path,
            "crates/member/src/child.rs"
        );
        assert_eq!(
            requests[0].hypotheses[0].files[0].content.as_deref(),
            Some(&b"pub const CHILD:u32=9;\n"[..])
        );
        unchanged(&workspace, files);
    }
}

const EXCLUDED: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers=[\"crates/member\"]\nexclude=[\"independent\"]\nresolver=\"3\"\n",
    ),
    MEMBER[1],
    MEMBER[2],
    MEMBER[3],
    MEMBER[4],
    (
        "independent/Cargo.toml",
        "[package]\nname=\"independent\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    ),
    ("independent/src/lib.rs", "pub const VALUE:u32=1;\n"),
];

#[tokio::test]
async fn excluded_cargo_root_still_receives_complete_proposal() {
    let workspace = Workspace::new(EXCLUDED);
    let gateway = CompilerGateway::with_files(EXCLUDED).await;
    let result = execute_tool(
        gateway.addr, &workspace.root(), "code_validate_edits",
        json!({"edits":[
            {"path":"crates/member/src/child.rs","new_text":"pub const CHILD:u32=9;\n"},
            {"path":"independent/src/lib.rs","new_text":"compile_error!(\"excluded root checked\");\n"}
        ],"compile":true}),
    ).await.unwrap();
    let report = text(&result);
    assert!(result.is_error, "{report}");
    assert!(report.contains("excluded root checked"), "{report}");
    assert!(report.contains("cwd=\"independent\""), "{report}");
    assert!(
        !report.contains("additional command root crates/member"),
        "{report}"
    );
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].subdir, None);
    assert_eq!(requests[1].subdir.as_deref(), Some("independent"));
    let first = serde_json::to_value(&requests[0].hypotheses[0].files).unwrap();
    let second = serde_json::to_value(&requests[1].hypotheses[0].files).unwrap();
    assert_eq!(first, second);
    assert_eq!(requests[1].hypotheses[0].files.len(), 2);
    unchanged(&workspace, EXCLUDED);
    for path in ["independent/target", "independent/Cargo.lock"] {
        assert!(!workspace.path(path).exists());
    }
}

const MEMBER_COMPANION: &[(&str, &str)] = &[
    MEMBER[0],
    MEMBER[1],
    MEMBER[2],
    MEMBER[3],
    MEMBER[4],
    ("crates/member/Makefile", "all:\n\tfalse\n"),
];

#[tokio::test]
async fn cargo_member_ownership_does_not_cover_companion_make_root() {
    let workspace = Workspace::new(MEMBER_COMPANION);
    let gateway = CompilerGateway::with_files(MEMBER_COMPANION).await;
    let result = execute_tool(
        gateway.addr, &workspace.root(), "code_validate_edit",
        json!({"path":"crates/member/src/child.rs","new_text":"pub const CHILD:u32=9;\n","compile":true}),
    ).await.unwrap();
    let report = text(&result);
    assert!(result.is_error, "{report}");
    assert!(
        report.contains("additional command root crates/member is unverified/incomplete"),
        "{report}"
    );
    assert!(
        report.contains("configured compiler check passed"),
        "{report}"
    );
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].subdir, None);
    unchanged(&workspace, MEMBER_COMPANION);
}
