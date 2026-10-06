/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::replace_inheritance::replace_inheritance_impl;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::fs;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_replace_inheritance_python() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "collections_mod.py",
            r#"class List:
    def push(self, item):
        pass

    def pop(self):
        pass

class CustomQueue(List):
    def peek(self):
        return self._items[0]
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("collections_mod.py");
    let gw = fake_gateway().await;

    let res = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "CustomQueue",
        Some("List"),
        Some("list"),
        Some(&["push".to_string(), "pop".to_string()]),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.sub_type, "CustomQueue");
    assert_eq!(res.base_type, "List");
    assert_eq!(res.field_name, "list");
    assert_eq!(res.forwarded_methods, vec!["push", "pop"]);
    assert!(res.applied);

    let content = fs::read_to_string(&file).unwrap();
    assert!(!content.contains("class CustomQueue(List):"));
    assert!(content.contains("class CustomQueue:"));
    assert!(content.contains("self.list = List(*args, **kwargs)"));
    assert!(content.contains("def push(self, *args, **kwargs):"));
    assert!(content.contains("return self.list.push(*args, **kwargs)"));
    assert!(content.contains("def pop(self, *args, **kwargs):"));
    assert!(content.contains("return self.list.pop(*args, **kwargs)"));
    assert!(content.contains("def peek(self):"));
}

#[tokio::test]
async fn test_replace_inheritance_python_with_existing_init() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.py",
            r#"class BaseService:
    def execute(self):
        return 42

class CustomService(BaseService):
    def __init__(self, name):
        super().__init__()
        self.name = name
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.py");
    let gw = fake_gateway().await;

    let res = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "CustomService",
        None, // auto-detect BaseService
        Some("delegate"),
        None, // auto-discover execute
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.base_type, "BaseService");
    assert_eq!(res.field_name, "delegate");
    assert_eq!(res.forwarded_methods, vec!["execute"]);

    let content = fs::read_to_string(&file).unwrap();
    assert!(!content.contains("class CustomService(BaseService):"));
    assert!(content.contains("class CustomService:"));
    assert!(content.contains("self.delegate = BaseService()"));
    assert!(!content.contains("super().__init__()"));
    assert!(content.contains("def execute(self, *args, **kwargs):"));
    assert!(content.contains("return self.delegate.execute(*args, **kwargs)"));
}

#[tokio::test]
async fn test_replace_inheritance_typescript() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "model.ts",
            r#"export class Repository {
    find(id: string): any {
        return null;
    }
    save(entity: any): void {}
}

export class CachedRepository extends Repository {
    override find(id: string): any {
        return super.find(id);
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("model.ts");
    let gw = fake_gateway().await;

    let res = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "CachedRepository",
        Some("Repository"),
        Some("repository"),
        Some(&["save".to_string()]),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.sub_type, "CachedRepository");
    assert_eq!(res.base_type, "Repository");
    assert_eq!(res.field_name, "repository");
    assert_eq!(res.forwarded_methods, vec!["save"]);

    let content = fs::read_to_string(&file).unwrap();
    assert!(!content.contains("class CachedRepository extends Repository"));
    assert!(content.contains("class CachedRepository {"));
    assert!(content.contains("private repository: Repository;"));
    assert!(content.contains("this.repository = new Repository("));
    assert!(content.contains("return this.repository.save("));
    // override should be stripped from find
    assert!(!content.contains("override find"));
    assert!(content.contains("find(id: string): any"));
    // super.find rewritten to this.repository.find
    assert!(!content.contains("super.find(id)"));
    assert!(content.contains("this.repository.find(id)"));
}

#[tokio::test]
async fn test_replace_inheritance_cpp() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "shapes.hpp",
            r#"class Rectangle {
public:
    int area() const { return 100; }
};

class Square : public Rectangle {
public:
    void resize() override {
        Rectangle::area();
    }
};
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("shapes.hpp");
    let gw = fake_gateway().await;

    let res = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "Square",
        Some("Rectangle"),
        Some("rect"),
        Some(&["area".to_string()]),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.sub_type, "Square");
    assert_eq!(res.base_type, "Rectangle");
    assert_eq!(res.field_name, "rect");

    let content = fs::read_to_string(&file).unwrap();
    assert!(!content.contains("class Square : public Rectangle"));
    assert!(content.contains("class Square {"));
    assert!(content.contains("Rectangle rect;"));
    assert!(content.contains("return rect.area();"));
    // override stripped from resize
    assert!(!content.contains("override"));
    assert!(content.contains("void resize() {"));
    // Rectangle::area() rewritten to rect.area()
    assert!(!content.contains("Rectangle::area()"));
    assert!(content.contains("rect.area();"));
}

#[tokio::test]
async fn replace_inheritance_cpp_forwards_arguments_and_preserves_other_bases() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "base.hpp",
            r#"class Base {
public:
    explicit Base(int seed) {}
    virtual int find(int id) { return id; }
};

class Interface {
public:
    virtual void reset() {}
};

class Derived : public Base, public Interface {
public:
    explicit Derived(int seed) : Base(seed) {}
};
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("base.hpp");
    let gw = fake_gateway().await;

    let result = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "Derived",
        Some("Base"),
        Some("base_"),
        Some(&["find".to_string()]),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert!(result.applied);
    let content = fs::read_to_string(file).unwrap();
    assert!(
        content.contains("class Derived : public Interface"),
        "{content}"
    );
    assert!(
        !content.contains("class Derived : public Base"),
        "{content}"
    );
    assert!(
        content.contains("Derived(int seed): base_(seed)"),
        "{content}"
    );
    assert!(content.contains("int find(int id)"), "{content}");
    assert!(content.contains("return base_.find(id);"), "{content}");
}

#[tokio::test]
async fn test_replace_inheritance_swift() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "vehicle.swift",
            r#"class Vehicle {
    func start() {}
    func stop() {}
}

class Car: Vehicle {
    override func start() {
        super.start()
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("vehicle.swift");
    let gw = fake_gateway().await;

    let res = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "Car",
        None,
        Some("vehicle"),
        Some(&["stop".to_string()]),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.sub_type, "Car");
    assert_eq!(res.base_type, "Vehicle");
    assert_eq!(res.field_name, "vehicle");
    assert_eq!(res.forwarded_methods, vec!["stop"]);

    let content = fs::read_to_string(&file).unwrap();
    assert!(!content.contains("class Car: Vehicle"));
    assert!(content.contains("class Car {"));
    assert!(content.contains("private let vehicle: Vehicle"));
    assert!(content.contains("self.vehicle = Vehicle()"));
    assert!(content.contains("func stop() {"));
    assert!(content.contains("vehicle.stop()"));
    assert!(!content.contains("override func start()"));
    assert!(content.contains("func start()"));
    assert!(!content.contains("super.start()"));
    assert!(content.contains("self.vehicle.start()"));
}

#[tokio::test]
async fn replace_inheritance_swift_forwards_parameter_and_return_signature() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "vehicle.swift",
            r#"class Vehicle {
    func fetch(id: Int) -> String {
        return "item-\(id)"
    }
}

class Car: Vehicle {}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("vehicle.swift");
    let gw = fake_gateway().await;

    let result = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "Car",
        Some("Vehicle"),
        Some("vehicle"),
        Some(&["fetch".to_string()]),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert!(result.applied);
    let content = fs::read_to_string(file).unwrap();
    assert!(
        content.contains("func fetch(id: Int) -> String"),
        "{content}"
    );
    assert!(
        content.contains("return self.vehicle.fetch(id: id)"),
        "{content}"
    );
}

#[tokio::test]
async fn test_replace_inheritance_dry_run() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "stack.py",
            r#"class Container:
    def size(self):
        return 0

class Stack(Container):
    pass
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("stack.py");
    let gw = fake_gateway().await;

    let res = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "Stack",
        Some("Container"),
        Some("container"),
        Some(&["size".to_string()]),
        false, // dry-run
        false,
        None,
    )
    .await
    .unwrap();

    assert!(!res.applied);
    assert!(!res.diff.is_empty());
    assert!(!res.overlays.is_empty());

    // File on disk must NOT be modified
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("class Stack(Container):"));
}

#[tokio::test]
async fn replace_inheritance_does_not_apply_analyzer_rejected_edits_without_force() {
    let source = "class Base { foo(): string { return \"base\"; } }\nclass Derived extends Base { bar(): string { return \"derived\"; } }\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("service.ts", source)]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let diagnostic_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = diagnostic_count.clone();
    let gw = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/diagnostic" => {
            if count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                answers::no_diagnostics()
            } else {
                serde_json::json!({
                    "kind": "full",
                    "items": [{
                        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                        "severity": 1,
                        "message": "synthetic analyzer error"
                    }]
                })
            }
        }
        _ => serde_json::Value::Null,
    })
    .await;

    let err = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "Derived",
        Some("Base"),
        Some("base"),
        Some(&["foo".to_string()]),
        true,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("analyzer errors"), "{err:#}");
    assert_eq!(fs::read_to_string(file).unwrap(), source);
}

#[tokio::test]
async fn test_replace_inheritance_unknown_class_fails() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("empty.py", "x = 1\n")]);
    let root = ws.root().to_path_buf();
    let file = root.join("empty.py");
    let gw = fake_gateway().await;

    let err = replace_inheritance_impl(
        gw.addr(),
        &root,
        &file,
        "NonExistentClass",
        None,
        None,
        None,
        false,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("not found"));
}
