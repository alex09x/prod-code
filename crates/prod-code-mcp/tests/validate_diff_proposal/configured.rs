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

const MAKE: &[(&str, &str)] = &[
    (
        "Makefile",
        "CXX = c++\nall:\n\t$(CXX) -c main.cpp -o main.o\n",
    ),
    ("main.cpp", "int main() { return 0; }\n"),
    ("unused.cpp", "int unused = 1;\n"),
];
const CMAKE: &[(&str, &str)] = &[
    (
        "CMakeLists.txt",
        "cmake_minimum_required(VERSION 3.16)\nproject(proposal LANGUAGES CXX)\nset(target_name app)\nset(source_file main.cpp)\nadd_executable(${target_name} ${source_file})\n",
    ),
    ("main.cpp", "int main() { return 0; }\n"),
    ("unused.cpp", "int unused = 1;\n"),
];

fn unchanged(workspace: &Workspace, files: &[(&str, &str)]) {
    for (path, bytes) in files {
        assert_eq!(workspace.read(path), *bytes);
    }
    for path in ["Cargo.lock", "target", "build", "main.o"] {
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
async fn variable_make_and_dynamic_cmake_use_existing_configured_commands() {
    // Real host Make/C++ and CMake/C++ execution. This does not assert CMake 4.2 support.
    for files in [MAKE, CMAKE] {
        let workspace = Workspace::new(files);
        let gateway = CompilerGateway::with_files(files).await;
        let diff = "--- a/main.cpp\n+++ b/main.cpp\n@@ -1 +1 @@\n-int main() { return 0; }\n+int main() { return 1; }\n--- a/unused.cpp\n+++ /dev/null\n@@ -1 +0,0 @@\n-int unused = 1;\n";
        let result = execute_tool(
            gateway.addr,
            &workspace.root(),
            "code_validate_edits",
            json!({"diff":diff,"compile":true}),
        )
        .await
        .unwrap();
        let report = text(&result);
        assert!(!result.is_error, "{report}");
        assert!(
            report.contains("configured compiler check passed"),
            "{report}"
        );
        assert!(
            report.contains("coverage is limited to the displayed command"),
            "{report}"
        );
        assert!(
            report.contains("exit=0 error=null timed_out=false"),
            "{report}"
        );
        assert!(report.contains("cwd=\".\""), "{report}");
        assert!(!report.contains("proof"), "{report}");
        let requests = gateway.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].subdir, None);
        assert_eq!(requests[0].hypotheses[0].files.len(), 2);
        assert_eq!(
            requests[0].hypotheses[0].files[0].content.as_deref(),
            Some(&b"int main() { return 1; }\n"[..])
        );
        assert!(requests[0].hypotheses[0].files[1].content.is_none());
        assert!(
            report.contains(&format!(
                "argv={}",
                serde_json::to_string(&requests[0].command).unwrap()
            )),
            "{report}"
        );
        unchanged(&workspace, files);
    }
}

const TWO: &[(&str, &str)] = &[
    (
        "one/Cargo.toml",
        "[package]\nname=\"one\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    ),
    ("one/src/lib.rs", "pub const ONE:u32=1;\n"),
    (
        "two/Cargo.toml",
        "[package]\nname=\"two\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    ),
    ("two/src/lib.rs", "pub const TWO:u32=2;\n"),
    ("two/src/unused.rs", "pub const UNUSED:u32=3;\n"),
];

#[tokio::test]
async fn independent_subdirectories_receive_the_identical_workspace_delta() {
    let workspace = Workspace::new(TWO);
    let gateway = CompilerGateway::with_files(TWO).await;
    let diff = "--- a/one/src/lib.rs\n+++ b/one/src/lib.rs\n@@ -1 +1 @@\n-pub const ONE:u32=1;\n+pub const ONE:u32=9;\n--- a/two/src/unused.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-pub const UNUSED:u32=3;\n";
    let result = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_validate_edits",
        json!({"diff":diff,"borrow_check":true}),
    )
    .await
    .unwrap();
    assert!(!result.is_error, "{}", text(&result));
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].subdir.as_deref(), Some("one"));
    assert_eq!(requests[1].subdir.as_deref(), Some("two"));
    for request in requests.iter() {
        let deltas = &request.hypotheses[0].files;
        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas[0].relative_path, "one/src/lib.rs");
        assert_eq!(
            deltas[0].content.as_deref(),
            Some(&b"pub const ONE:u32=9;\n"[..])
        );
        assert_eq!(deltas[1].relative_path, "two/src/unused.rs");
        assert!(deltas[1].content.is_none());
    }
    unchanged(&workspace, TWO);
    for path in [
        "one/target",
        "one/Cargo.lock",
        "two/target",
        "two/Cargo.lock",
    ] {
        assert!(!workspace.path(path).exists());
    }
}

const NESTED: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname=\"outer\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    ),
    ("src/lib.rs", "pub const OUTER:u32=1;\n"),
    (
        "nested/Cargo.toml",
        "[package]\nname=\"inner\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    ),
    ("nested/src/lib.rs", "pub const INNER:u32=2;\n"),
];

const NESTED_MAKE: &[(&str, &str)] = &[
    (
        "Makefile",
        "CXX = c++\nall:\n\t$(CXX) -c main.cpp -o main.o\n",
    ),
    ("main.cpp", "int main() { return 0; }\n"),
    (
        "nested/Makefile",
        "CXX = c++\nall:\n\t$(CXX) -c main.cpp -o main.o\n",
    ),
    ("nested/main.cpp", "int main() { return 0; }\n"),
];

#[tokio::test]
async fn unselected_additional_make_root_is_explicitly_incomplete() {
    let workspace = Workspace::new(NESTED_MAKE);
    let gateway = CompilerGateway::with_files(NESTED_MAKE).await;
    let result = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_validate_edits",
        json!({"edits":[
        {"path":"main.cpp","new_text":"int main() { return 1; }\n"},
        {"path":"nested/main.cpp","new_text":"#error omitted root must be visible\n"}
    ],"compile":true}),
    )
    .await
    .unwrap();
    let report = text(&result);
    assert!(result.is_error, "{report}");
    assert!(
        report.contains("additional command root nested is unverified/incomplete"),
        "{report}"
    );
    assert!(
        report.contains("configured compiler check passed"),
        "{report}"
    );
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].command, ["make"]);
    assert_eq!(requests[0].hypotheses[0].files.len(), 2);
    unchanged(&workspace, NESTED_MAKE);
}

#[tokio::test]
async fn same_language_does_not_certify_an_additional_command_root() {
    let workspace = Workspace::new(NESTED);
    let gateway = CompilerGateway::with_files(NESTED).await;
    let result=execute_tool(gateway.addr,&workspace.root(),"code_validate_edits",json!({"edits":[{"path":"src/lib.rs","new_text":"pub const OUTER:u32=9;\n"},{"path":"nested/src/lib.rs","new_text":"compile_error!(\"must not be claimed compiled\");\n"}],"compile":true})).await.unwrap();
    let report = text(&result);
    assert!(result.is_error, "{report}");
    assert!(report.contains("cwd=\"nested\""), "{report}");
    assert!(report.contains("must not be claimed compiled"), "{report}");
    assert!(
        report.contains("configured compiler check passed"),
        "{report}"
    );
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_eq!(request.hypotheses[0].files.len(), 2);
        assert_eq!(request.hypotheses[0].files[0].relative_path, "src/lib.rs");
        assert_eq!(
            request.hypotheses[0].files[1].relative_path,
            "nested/src/lib.rs"
        );
    }
    unchanged(&workspace, NESTED);
}

#[tokio::test]
async fn changed_definitions_and_unsupported_manifest_text_are_visible() {
    let workspace = Workspace::new(MAKE);
    let gateway = CompilerGateway::with_files(MAKE).await;
    let result=execute_tool(gateway.addr,&workspace.root(),"code_validate_edits",json!({"diff":"--- a/Makefile\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-CXX = c++\n-all:\n-\t$(CXX) -c main.cpp -o main.o\n","compile":true})).await.unwrap();
    assert!(result.is_error);
    assert!(
        text(&result).contains("route selection is unestablished"),
        "{}",
        text(&result)
    );
    assert!(gateway.requests.lock().unwrap().is_empty());
    unchanged(&workspace, MAKE);

    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let result=execute_tool(gateway.addr,&workspace.root(),"code_validate_edits",json!({"edits":[{"path":"Cargo.toml","new_text":"[workspace]\n"},{"path":"src/lib.rs","new_text":FILES[1].1}],"compile":true})).await;
    let report = match result {
        Ok(r) => {
            assert!(r.is_error);
            text(&r)
        }
        Err(e) => format!("{e:#}"),
    };
    assert!(
        report.contains("source")
            || report.contains("manifest")
            || report.contains("unestablished"),
        "{report}"
    );
    assert!(gateway.requests.lock().unwrap().is_empty());
    assert_unchanged(&workspace);
}
