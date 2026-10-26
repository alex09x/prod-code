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
use std::{collections::BTreeMap, fs, process::Command};

fn execute(ws: &Workspace, program: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(program)
        .args(args)
        .current_dir(ws.root())
        .output()
        .unwrap();
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .replace(ws.root().to_str().unwrap(), "<fixture>");
    println!("{program} {args:?} exit {:?}: {output}", out.status.code());
    (out.status.success(), output)
}

pub(super) async fn check(
    name: &str,
    source: &str,
    anchors: &[&str],
    expected: &str,
    refusal: Option<&str>,
) {
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let declaration = if source.contains("readonly city:") {
        "readonly city:"
    } else if source.contains("city: Reader") {
        "city: Reader"
    } else {
        "city ="
    };
    let refs = anchors
        .iter()
        .map(|anchor| at(&file, source, anchor, "city"))
        .collect();
    let gw = gateway(&file, source, "Account", "city", declaration, refs).await;
    println!("CASE {name}\nBEFORE SOURCE\n{source}END SOURCE");
    let args = ["--target", "ES2022", "--module", "commonjs", "account.ts"];
    assert!(execute(&ws, "tsc", &args).0);
    let before = execute(&ws, "node", &["account.js"]);
    assert!(before.0);
    assert_eq!(before.1, expected);
    let snapshot = || {
        fs::read_dir(ws.root())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name(),
                    fs::read(entry.path()).unwrap_or_default(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let before_files = snapshot();
    let result = extract(
        &ws,
        &gw,
        "account.ts",
        "Account",
        "city",
        &[],
        "Address",
        "address",
        true,
    )
    .await;
    let after = fs::read_to_string(&file).unwrap();
    println!("AFTER SOURCE\n{after}END SOURCE");
    if result.is_ok() {
        let compiled = execute(&ws, "tsc", &args);
        let runtime = execute(&ws, "node", &["account.js"]);
        if refusal.is_none() {
            assert!(compiled.0 && runtime.0);
            assert_eq!(runtime.1, expected);
            assert_ne!(after, source);
        }
    }
    if let Some(reason) = refusal {
        assert!(
            result.is_err(),
            "unsafe extraction must refuse before writes"
        );
        let error = result.unwrap_err();
        assert!(error.to_string().contains(reason), "{error:#}");
        println!("REFUSED: {error}");
        assert_eq!(after.as_bytes(), source.as_bytes());
        assert_eq!(snapshot(), before_files);
    } else {
        result.unwrap();
    }
}

#[tokio::test]
async fn readonly_super_prefix_refuses_before_writes() {
    check(
        "readonly-super",
        "class Base {\n    constructor() { console.log(this.current()); }\n    current(): string | undefined { return 'base'; }\n}\nclass Account extends Base {\n    readonly city: string;\n    constructor(city: string) {\n        super();\n        this.city = city;\n    }\n    current(): string | undefined { return this.city; }\n}\nconsole.log(new Account('Paris').city);\n",
        &["this.city =", "return this.city", ").city)"],
        "undefined\nParis\n",
        Some("readonly initialization has unproved earlier owner/control-flow effects"),
    )
    .await;
}

#[tokio::test]
async fn readonly_template_initializer_refuses_before_writes() {
    check(
        "readonly-template-initializer",
        "class Account {\n    readonly city: string;\n    label = `${this.current()}`;\n    constructor(city: string) {\n        this.city = city;\n    }\n    current(): string | undefined { return this.city; }\n}\nconst a = new Account('Paris');\nconsole.log(a.label, a.city);\n",
        &["this.city =", "return this.city", "a.city)"],
        "undefined Paris\n",
        Some("readonly initialization has unproved owner field-initializer effects"),
    )
    .await;
}

#[tokio::test]
async fn grouped_calls_after_blocks_refuse_before_writes() {
    for statement in [
        "if (true) {} (a.city)();",
        "if (true) {} /* completed block */ ((a.city))();",
        "{} (a.city)();",
        "while (false) {} (a.city)();",
        "try {} finally {} (a.city)();",
        "label: {} (a.city)();",
        "function completed() {} (a.city)();",
        "class Completed {} (a.city)();",
        "if (true) {} (a.city)`tag`;",
    ] {
        let source = format!(
            "class Account {{\n    city = function(this: any, ..._args: unknown[]) {{ console.log(this.id); }};\n    id = 7;\n}}\nconst a = new Account();\n{statement}\n"
        );
        check(
            statement,
            &source,
            &["a.city"],
            "7\n",
            Some("callable field"),
        )
        .await;
    }
}

#[tokio::test]
async fn readonly_literal_and_template_data_remain_supported() {
    for initializer in [
        "'this super'",
        "`this.current() super()`",
        "`${'this'} ${'super'} ${1 + 2}`",
        "`${'`this.current()`'} ${`super.current()`}`",
        "`${`outer ${'`this.current()`'}`}`",
        "`\\${this.current()} \\${super.current()}`",
    ] {
        let source = format!(
            "class Account {{\n    readonly city: string;\n    label = {initializer};\n    constructor(city: string) {{\n        const thisValue = 'this';\n        const superValue = `super`;\n        this.city = city;\n    }}\n}}\nconsole.log(new Account('Paris').city);\n"
        );
        check(
            initializer,
            &source,
            &["this.city =", ").city)"],
            "Paris\n",
            None,
        )
        .await;
    }
}

#[tokio::test]
async fn scalar_reads_and_real_braced_call_arguments_remain_supported() {
    for expression in [
        "a.city!",
        "(a.city! as string)",
        "(a.city satisfies string)",
        "(function(value: string) { return () => value; }(a.city)())",
        "(function named(value: string) { return () => value; }(a.city)())",
        "(function named(value: string): () => string { return () => value; }(a.city)())",
        "(new class { constructor(value: string) { return (() => value) as any; } }(a.city) as any)()",
        "(new class extends Object { constructor(value: string) { super(); return (() => value) as any; } }(a.city) as any)()",
    ] {
        let source = format!(
            "class Account {{\n    city = 'Paris';\n}}\nconst a = new Account();\nconsole.log({expression});\n"
        );
        check(expression, &source, &["a.city"], "Paris\n", None).await;
    }
}

#[tokio::test]
async fn non_null_nested_generic_calls_refuse_before_writes() {
    for ty in [
        "Array<string>",
        "() => string",
        "() => Array<string>",
        "Array<() => string>",
        "{ read: () => Array<string> }",
    ] {
        let source = format!(
            "type Reader = {{ <T>(): number; }};\nclass Account {{\n    city: Reader = function<T>(this: any) {{ return this.id; }};\n    id = 7;\n}}\nconst a = new Account();\nconsole.log(a.city!<{ty}>());\n"
        );
        check(ty, &source, &["a.city!"], "7\n", Some("callable field")).await;
    }
}

#[tokio::test]
async fn review_followup_member_control_names_are_scalar_arguments() {
    for method in ["catch", "switch", "if", "while", "for", "with"] {
        let source = format!(
            "class Account {{\n    city = 'Paris';\n}}\nconst a = new Account();\nconst factory = {{ {method}() {{ return (value: string) => () => value; }} }};\nconsole.log(factory.{method}()(a.city)());\n"
        );
        check(method, &source, &["a.city"], "Paris\n", None).await;
    }
}

pub(super) async fn declaration_call(statement: &str) {
    let source = format!(
        "class Account {{\n    city = function(this: any) {{ console.log(this.id); }};\n    id = 7;\n}}\nconst a = new Account();\n{statement} (a.city)();\n"
    );
    check(
        statement,
        &source,
        &["a.city"],
        "7\n",
        Some("callable field"),
    )
    .await;
}

#[tokio::test]
async fn review_followup_typed_function_declaration_refuses() {
    declaration_call("function completed(): void {}").await;
}

#[tokio::test]
async fn review_followup_inherited_class_declaration_refuses() {
    declaration_call("class Completed extends Object {}").await;
}

#[tokio::test]
async fn review_followup_quoted_backticks_in_interpolation_refuse() {
    for initializer in [
        "`${'`' + this.current() + '`'}`",
        "`${`outer ${'`' + this.current() + '`'}`}`",
        "`${'`'}${this.current()}${'`'}`",
    ] {
        let source = format!(
            "class Account {{\n    readonly city: string;\n    label = {initializer};\n    constructor(city: string) {{\n        this.city = city;\n    }}\n    current(): string | undefined {{ return this.city; }}\n}}\nconst a = new Account('Paris');\nconsole.log(a.city);\n"
        );
        check(
            initializer,
            &source,
            &["this.city =", "return this.city", "a.city)"],
            "Paris\n",
            Some("readonly initialization has unproved owner field-initializer effects"),
        )
        .await;
    }
}

#[tokio::test]
async fn final_review_optional_binding_catch_refuses() {
    declaration_call("try {} catch {}").await;
}

#[tokio::test]
async fn final_review_asi_declarations_refuse() {
    for declaration in [
        "function completed(): void {}",
        "class Completed extends Object {}",
        "async function completed(): Promise<void> {}",
    ] {
        let source = format!(
            "class Account {{\n    city = function(this: any) {{ console.log(this.id); }};\n    id = 7;\n}}\nconst a = new Account()\n{declaration} (a.city)();\n"
        );
        check(
            declaration,
            &source,
            &["a.city"],
            "7\n",
            Some("callable field"),
        )
        .await;
    }
}

#[tokio::test]
async fn final_review_multiline_expression_arguments_remain_supported() {
    for expression in [
        "const read =\nfunction(value: string) { return () => value; }(a.city)();",
        "const read = (\nfunction(value: string) { return () => value; }(a.city)());",
        "const read = true ?\nfunction(value: string) { return () => value; }(a.city)() : '';",
        "const read = (new\nclass { constructor(value: string) { return (() => value) as any; } }(a.city) as any)();",
    ] {
        let source = format!(
            "class Account {{\n    city = 'Paris';\n}}\nconst a = new Account();\n{expression}\nconsole.log(read);\n"
        );
        check(expression, &source, &["a.city"], "Paris\n", None).await;
    }
}

#[tokio::test]
async fn latest_review_super_property_data_remains_supported() {
    for initializer in [
        "{ super: 1 }",
        "({ super: 1 }).super",
        "({ super() { return 1; } }).super()",
        "`${({ super: 1 }).super}`",
    ] {
        let source = format!(
            "class Account {{\n    readonly city: string;\n    metadata = {initializer};\n    constructor(city: string) {{\n        this.city = city;\n    }}\n}}\nconsole.log(new Account('Paris').city);\n"
        );
        check(
            initializer,
            &source,
            &["this.city =", ").city)"],
            "Paris\n",
            None,
        )
        .await;
    }
}

#[tokio::test]
async fn latest_review_object_return_annotation_arguments_remain_supported() {
    for expression in [
        "const read = function(value: string): { (): string } { return () => value; }(a.city)();",
        "const read = function named(value: string): { (): string } { return () => value; }(a.city)();",
        "const read = function(value: string): { (): string } | (() => string) { return () => value; }(a.city)();",
        "const read = function(value: string): { (): string; label?: string } { return () => value; }(a.city)();",
    ] {
        let source = format!(
            "class Account {{\n    city = 'Paris';\n}}\nconst a = new Account();\n{expression}\nconsole.log(read);\n"
        );
        check(expression, &source, &["a.city"], "Paris\n", None).await;
    }
}
