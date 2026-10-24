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
use crate::parameter_object::Language;

#[test]
fn language_aliases_and_empty_templates() {
    assert_eq!(
        generate_mock(Language::Swift, "Store", &[], &[]),
        "final class MockStore: Store {\n    var calls: [String] = []\n\n}"
    );
    assert_eq!(
        generate_mock(Language::Java, "Store", &[], &[]),
        "public class MockStore implements Store {\n    public java.util.List<String> calls = new java.util.ArrayList<>();\n\n}\n"
    );
    for (language, canonical) in [
        (Language::C, Language::Cpp),
        (Language::JavaScript, Language::TypeScript),
    ] {
        assert_eq!(
            generate_mock(language, "Store", &[], &[]),
            generate_mock(canonical, "Store", &[], &[])
        );
    }
    assert_eq!(
        generate_mock(
            Language::Go,
            "Store",
            &[],
            &[("Ready".into(), "bool".into())]
        ),
        "// MockStore is a mock implementation of Store for testing.\ntype MockStore struct {\n    Ready bool\n    Calls []string\n}"
    );
}

#[test]
fn generates_go_mock_for_interface() {
    let methods = vec![
        MethodSignature {
            name: "GetUser".to_string(),
            params: vec![
                ("ctx".to_string(), "context.Context".to_string()),
                ("id".to_string(), "string".to_string()),
            ],
            return_type: Some("*User, error".to_string()),
        },
        MethodSignature {
            name: "DeleteUser".to_string(),
            params: vec![("id".to_string(), "string".to_string())],
            return_type: Some("error".to_string()),
        },
    ];
    let mock = generate_mock(Language::Go, "UserService", &methods, &[]);
    assert!(mock.contains("type MockUserService struct"));
    assert!(mock.contains("GetUserFunc func(ctx context.Context, id string) (*User, error)"));
    assert!(mock.contains("DeleteUserFunc func(id string) error"));
    assert!(mock.contains(
        "func (m *MockUserService) GetUser(ctx context.Context, id string) (*User, error)"
    ));
    assert!(mock.contains("m.Calls = append(m.Calls, \"GetUser\")"));
    assert!(mock.contains("return nil, nil"));
}

#[test]
fn generates_ts_mock_for_interface() {
    let methods = vec![MethodSignature {
        name: "fetch".to_string(),
        params: vec![("url".to_string(), "string".to_string())],
        return_type: Some("Promise<string>".to_string()),
    }];
    let mock = generate_mock(Language::TypeScript, "Client", &methods, &[]);
    assert!(mock.contains("export class MockClient implements Client"));
    assert!(mock.contains("fetchHandler?: (url: string) => Promise<string>"));
    assert!(mock.contains("async fetch(url: string): Promise<string>"));
    assert!(mock.contains("this.calls.push({ method: \"fetch\", args: [url] })"));
    assert!(mock.contains("export const createMockClient"));
}

#[test]
fn generates_python_mock_for_protocol() {
    let methods = vec![MethodSignature {
        name: "process".to_string(),
        params: vec![("item".to_string(), "str".to_string())],
        return_type: Some("bool".to_string()),
    }];
    let mock = generate_mock(Language::Python, "Processor", &methods, &[]);
    assert!(mock.contains("class MockProcessor:"));
    assert!(mock.contains("def process(self, item: str) -> bool:"));
    assert!(mock.contains("self.calls.append((\"process\", (item,), {}))"));
    assert!(mock.contains("return False"));
}

#[test]
fn generates_rust_mock_for_trait() {
    let methods = vec![MethodSignature {
        name: "execute".to_string(),
        params: vec![
            ("&self".to_string(), String::new()),
            ("code".to_string(), "u32".to_string()),
        ],
        return_type: Some("bool".to_string()),
    }];
    let mock = generate_mock(Language::Rust, "Executor", &methods, &[]);
    assert!(mock.contains("pub struct MockExecutor"));
    assert!(mock.contains("impl Executor for MockExecutor"));
    assert!(mock.contains("fn execute(&self, code: u32) -> bool"));
    assert!(mock.contains("self.calls.lock().unwrap().push(\"execute\".to_string())"));
}
