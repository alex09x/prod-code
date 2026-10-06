/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::pull_push::{find_matching_brace, pull_up_impl, push_down_impl};
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

async fn gateway_with_overlay_error() -> ScriptedGateway {
    let diagnostics = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = diagnostics.clone();
    ScriptedGateway::start(move |method, _params| match method {
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
    .await
}

#[test]
fn test_find_matching_brace_complex() {
    let code = r#"class Foo {
    bar() {
        let x = "{ not a brace }";
        // comment with {
        /* block comment { */
        return x;
    }
}"#;
    let open_idx = code.find('{').unwrap();
    let close_idx = find_matching_brace(code, open_idx).unwrap();
    assert_eq!(close_idx, code.len() - 1);
}

#[tokio::test]
async fn test_pull_up_python_same_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "animals.py",
            r#"class Animal:
    def eat(self):
        print("eating")

class Dog(Animal):
    def bark(self):
        print("woof")
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("animals.py");
    let gw = fake_gateway().await;

    let res = pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "Dog",
        Some("Animal"),
        &["bark".to_string()],
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.operation, "pull_up");
    assert_eq!(res.source_class, "Dog");
    assert_eq!(res.target_classes, vec!["Animal"]);
    assert_eq!(res.members, vec!["bark"]);

    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("class Animal:"));
    assert!(content.contains("def bark(self):"));
    assert!(content.contains("class Dog(Animal):"));
    // Dog should have pass inserted because its body became empty
    assert!(content.contains("pass"));
}

#[tokio::test]
async fn test_push_down_python_same_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "shapes.py",
            r#"class Shape:
    def draw(self):
        print("drawing")

    def resize(self):
        print("resizing")

class Circle(Shape):
    pass
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("shapes.py");
    let gw = fake_gateway().await;

    let res = push_down_impl(
        gw.addr(),
        &root,
        &file,
        "Shape",
        Some(&["Circle".to_string()]),
        &["draw".to_string()],
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.operation, "push_down");
    assert_eq!(res.source_class, "Shape");
    assert_eq!(res.target_classes, vec!["Circle"]);

    let content = fs::read_to_string(&file).unwrap();
    // Shape keeps resize, but loses draw
    let shape_decl = content.find("class Shape:").unwrap();
    let circle_decl = content.find("class Circle(Shape):").unwrap();
    let shape_part = &content[shape_decl..circle_decl];
    assert!(!shape_part.contains("def draw(self):"));
    assert!(shape_part.contains("def resize(self):"));

    // Circle gains draw (replacing pass)
    let circle_part = &content[circle_decl..];
    assert!(circle_part.contains("def draw(self):"));
}

#[tokio::test]
async fn test_pull_up_ts_with_override_stripping() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "vehicle.ts",
            r#"export class Vehicle {
    speed: number;
}

export class Car extends Vehicle {
    override honk(): void {
        console.log("beep");
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("vehicle.ts");
    let gw = fake_gateway().await;

    let res = pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "Car",
        None, // auto-detect superclass
        &["honk".to_string()],
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.target_classes, vec!["Vehicle"]);

    let content = fs::read_to_string(&file).unwrap();
    let vehicle_part =
        &content[content.find("class Vehicle").unwrap()..content.find("class Car").unwrap()];
    // In Vehicle, override should be stripped
    assert!(vehicle_part.contains("honk(): void"));
    assert!(!vehicle_part.contains("override honk"));

    // Car should not have honk anymore
    let car_part = &content[content.find("class Car").unwrap()..];
    assert!(!car_part.contains("honk(): void"));
}

#[tokio::test]
async fn test_push_down_ts() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"export class BaseCalculator {
    compute(): number {
        return 42;
    }
}

export class ScientificCalculator extends BaseCalculator {
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.ts");
    let gw = fake_gateway().await;

    let res = push_down_impl(
        gw.addr(),
        &root,
        &file,
        "BaseCalculator",
        None, // auto-detect ScientificCalculator
        &["compute".to_string()],
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.target_classes, vec!["ScientificCalculator"]);

    let content = fs::read_to_string(&file).unwrap();
    let base_part = &content[content.find("class BaseCalculator").unwrap()
        ..content.find("class ScientificCalculator").unwrap()];
    assert!(!base_part.contains("compute(): number"));

    let sub_part = &content[content.find("class ScientificCalculator").unwrap()..];
    assert!(sub_part.contains("compute(): number"));
}

#[tokio::test]
async fn test_pull_up_cpp() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "widget.cpp",
            r#"class BaseWidget {
public:
    int width;
};

class Button : public BaseWidget {
public:
    void click() override {
        // click
    }
};
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("widget.cpp");
    let gw = fake_gateway().await;

    let res = pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "Button",
        Some("BaseWidget"),
        &["click".to_string()],
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.target_classes, vec!["BaseWidget"]);

    let content = fs::read_to_string(&file).unwrap();
    let base_part =
        &content[content.find("class BaseWidget").unwrap()..content.find("class Button").unwrap()];
    assert!(base_part.contains("void click()"));
    assert!(!base_part.contains("override"));

    let button_part = &content[content.find("class Button").unwrap()..];
    assert!(!button_part.contains("void click()"));
}

#[tokio::test]
async fn test_pull_up_swift() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "nodes.swift",
            r#"class BaseNode {
    var id: String = ""
}

class LeafNode: BaseNode {
    override func render() {
        print("rendering")
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("nodes.swift");
    let gw = fake_gateway().await;

    let _res = pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "LeafNode",
        None,
        &["render".to_string()],
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    let content = fs::read_to_string(&file).unwrap();
    let base_part =
        &content[content.find("class BaseNode").unwrap()..content.find("class LeafNode").unwrap()];
    assert!(base_part.contains("func render()"));
    assert!(!base_part.contains("override func render"));

    let leaf_part = &content[content.find("class LeafNode").unwrap()..];
    assert!(!leaf_part.contains("func render()"));
}

#[tokio::test]
async fn test_pull_up_rust_trait() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            r#"pub trait BaseService {
    fn status(&self) -> bool;
}

pub trait AdvancedService: BaseService {
    fn metrics(&self) -> u64;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("src/lib.rs");
    let gw = fake_gateway().await;

    let _res = pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "AdvancedService",
        Some("BaseService"),
        &["metrics".to_string()],
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    let content = fs::read_to_string(&file).unwrap();
    let base_part = &content[content.find("trait BaseService").unwrap()
        ..content.find("trait AdvancedService").unwrap()];
    assert!(base_part.contains("fn metrics(&self) -> u64;"));

    let adv_part = &content[content.find("trait AdvancedService").unwrap()..];
    assert!(!adv_part.contains("fn metrics(&self) -> u64;"));
}

#[tokio::test]
async fn test_push_down_rust_trait() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            r#"pub trait BaseService {
    fn status(&self) -> bool;
    fn legacy_ping(&self);
}

pub trait AdvancedService: BaseService {
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("src/lib.rs");
    let gw = fake_gateway().await;

    let _res = push_down_impl(
        gw.addr(),
        &root,
        &file,
        "BaseService",
        Some(&["AdvancedService".to_string()]),
        &["legacy_ping".to_string()],
        true,
        false,
        None,
    )
    .await
    .unwrap();

    let content = fs::read_to_string(&file).unwrap();
    let base_part = &content[content.find("trait BaseService").unwrap()
        ..content.find("trait AdvancedService").unwrap()];
    assert!(!base_part.contains("legacy_ping"));

    let adv_part = &content[content.find("trait AdvancedService").unwrap()..];
    assert!(adv_part.contains("fn legacy_ping(&self);"));
}

#[tokio::test]
async fn test_multi_file_python_pull_up() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "base.py",
            r#"class Entity:
    pass
"#,
        ),
        (
            "user.py",
            r#"from base import Entity

class User(Entity):
    def get_id(self):
        return self._id
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let user_file = root.join("user.py");
    let base_file = root.join("base.py");
    let gw = fake_gateway().await;

    let res = pull_up_impl(
        gw.addr(),
        &root,
        &user_file,
        "User",
        Some("Entity"),
        &["get_id".to_string()],
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(res.files_modified.len(), 2);

    let base_content = fs::read_to_string(&base_file).unwrap();
    assert!(base_content.contains("class Entity:\n    def get_id(self):"));

    let user_content = fs::read_to_string(&user_file).unwrap();
    assert!(!user_content.contains("def get_id(self):"));
    assert!(user_content.contains("pass"));
}

#[tokio::test]
async fn test_pull_up_detects_collision() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "collision.py",
            r#"class Base:
    def action(self):
        print("base action")

class Derived(Base):
    def action(self):
        print("derived action")
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("collision.py");
    let gw = fake_gateway().await;

    let res = pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "Derived",
        Some("Base"),
        &["action".to_string()],
        true,
        false,
        false,
        None,
    )
    .await;

    assert!(res.is_err());
    let err_msg = res.unwrap_err().to_string();
    assert!(err_msg.contains("superclass 'Base' already defines member 'action'"));
}

#[tokio::test]
async fn test_pull_up_clean_siblings() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "pets.py",
            r#"class Pet:
    pass

class Dog(Pet):
    def sleep(self):
        print("sleeping")

class Cat(Pet):
    def sleep(self):
        print("sleeping")
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("pets.py");
    let gw = fake_gateway().await;

    let _res = pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "Dog",
        Some("Pet"),
        &["sleep".to_string()],
        true, // clean_siblings
        true, // apply
        false,
        None,
    )
    .await
    .unwrap();

    let content = fs::read_to_string(&file).unwrap();
    // Pet has sleep
    let pet_part =
        &content[content.find("class Pet:").unwrap()..content.find("class Dog").unwrap()];
    assert!(pet_part.contains("def sleep(self):"));

    // Both Dog and Cat had sleep removed and pass inserted
    let dog_part = &content[content.find("class Dog").unwrap()..content.find("class Cat").unwrap()];
    assert!(!dog_part.contains("def sleep(self):"));
    assert!(dog_part.contains("pass"));

    let cat_part = &content[content.find("class Cat").unwrap()..];
    assert!(!cat_part.contains("def sleep(self):"));
    assert!(cat_part.contains("pass"));
}

#[tokio::test]
async fn pull_up_removes_only_semantically_identical_sibling_overrides() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "pets.py",
            r#"class Pet:
    pass

class Dog(Pet):
    def speak(self):
        return "woof"

class Cat(Pet):
    def speak(self):
        return "meow"
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("pets.py");
    let gw = fake_gateway().await;

    pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "Dog",
        Some("Pet"),
        &["speak".to_string()],
        true,
        true,
        false,
        None,
    )
    .await
    .unwrap();

    let content = fs::read_to_string(file).unwrap();
    let cat = &content[content.find("class Cat").unwrap()..];
    assert!(cat.contains("def speak(self):"), "{cat}");
    assert!(cat.contains("return \"meow\""), "{cat}");
    let pet = &content[content.find("class Pet:").unwrap()..content.find("class Dog").unwrap()];
    assert!(pet.contains("return \"woof\""), "{pet}");
}

#[tokio::test]
async fn pull_up_rejects_analyzer_errors_before_writing() {
    let source =
        "class Base {}\nclass Derived extends Base { foo(): string { return \"derived\"; } }\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("service.ts", source)]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let gw = gateway_with_overlay_error().await;

    let err = pull_up_impl(
        gw.addr(),
        &root,
        &file,
        "Derived",
        Some("Base"),
        &["foo".to_string()],
        false,
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
async fn push_down_rejects_analyzer_errors_before_writing() {
    let source =
        "class Base { foo(): string { return \"base\"; } }\nclass Derived extends Base {}\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("service.ts", source)]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let gw = gateway_with_overlay_error().await;

    let err = push_down_impl(
        gw.addr(),
        &root,
        &file,
        "Base",
        Some(&["Derived".to_string()]),
        &["foo".to_string()],
        true,
        false,
        None,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("analyzer errors"), "{err:#}");
    assert_eq!(fs::read_to_string(file).unwrap(), source);
}
