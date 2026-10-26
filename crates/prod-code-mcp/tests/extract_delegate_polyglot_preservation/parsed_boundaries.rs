/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{remaining_findings::check, support::*};
use prod_code_mcp::extract_delegate::extract_delegate_polyglot;
use prod_code_testkit::Workspace;
use std::{collections::BTreeMap, fs, process::Command};

fn run_cmd(ws: &Workspace, program: &str, args: &[&str]) -> (bool, String) {
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

async fn division_case(name: &str, preamble: &str, value: &str, refusal: bool) {
    let source = format!(
        "{preamble}\nclass Account {{\n    readonly city: number;\n    metadata = {value};\n    constructor(city: number) {{\n        this.city = city;\n    }}\n    current(): number {{ return this.city; }}\n}}\nconst a = new Account(7);\nconsole.log(a.metadata, a.city);\n"
    );
    check(
        name,
        &source,
        &["this.city =", "return this.city", "a.city)"],
        if refusal { "NaN 7\n" } else { "1 7\n" },
        refusal.then_some("readonly initialization has unproved owner field-initializer effects"),
    )
    .await;
}

#[tokio::test]
async fn parsed_boundaries_contextual_identifier_of_division_refuses() {
    division_case(
        "contextual-of-division",
        "const of = 2;",
        "of / this.current()",
        true,
    )
    .await;
}

#[tokio::test]
async fn parsed_boundaries_generic_assertion_division_refuses() {
    for value in [
        "2 as Numeric<string> / this.current()",
        "2 satisfies Numeric<string> / this.current()",
        "2 as Numeric<Array<string>> / this.current()",
    ] {
        division_case(value, "type Numeric<T> = number;", value, true).await;
    }
}

#[tokio::test]
async fn parsed_boundaries_contextual_and_generic_division_data_remain_supported() {
    division_case("of / 2", "const of = 2;", "of / 2", false).await;
    for value in [
        "2 as Numeric<string> / 2",
        "2 satisfies Numeric<string> / 2",
        "2 as Numeric<Array<string>> / 2",
    ] {
        division_case(value, "type Numeric<T> = number;", value, false).await;
    }
}

#[tokio::test]
async fn parsed_boundaries_later_class_selected_by_line_after_regex_brace_class() {
    for (name, extra, target_line) in [
        ("single-line-other", "", 4u32),
        ("multiline-other-after-regex", "    extra = 1;\n", 5u32),
    ] {
        let source = format!(
            "class Other {{\n    pattern = /}}/;\n{extra}}}\nclass Account {{\n    city = 'Paris';\n}}\nconst other = new Other();\nconst a = new Account();\nconsole.log(other.pattern.test('}}'), a.city);\n"
        );
        let ws = Workspace::new(&[("account.ts", &source)]);
        let file = ws.path("account.ts");
        let gw = gateway(
            &file,
            &source,
            "Account",
            "city",
            "city = 'Paris'",
            vec![at(&file, &source, "a.city)", "city")],
        )
        .await;
        println!("CASE {name}\nBEFORE SOURCE\n{source}END SOURCE");
        let args = ["--target", "ES2022", "--module", "commonjs", "account.ts"];
        assert!(run_cmd(&ws, "tsc", &args).0);
        let before = run_cmd(&ws, "node", &["account.js"]);
        assert!(before.0);
        assert_eq!(before.1, "true Paris\n");
        let res = extract_delegate_polyglot(
            gw.addr(),
            &ws.root(),
            &file,
            None,
            Some(target_line),
            None,
            &["city".into()],
            &[],
            "Address",
            "address",
            true,
            true,
            None,
        )
        .await
        .expect(
            "selecting a later class by line must ignore earlier unselected class regex braces",
        );
        assert!(res.applied);
        let after = fs::read_to_string(&file).unwrap();
        println!("AFTER SOURCE\n{after}END SOURCE");
        assert!(after.starts_with("class Other {\n    pattern = /}/;\n"));
        assert!(after.contains("class Address {"));
        assert!(after.contains("a.address.city"));
        assert!(run_cmd(&ws, "tsc", &args).0);
        let after_run = run_cmd(&ws, "node", &["account.js"]);
        assert!(after_run.0);
        assert_eq!(after_run.1, "true Paris\n");
    }
}

#[tokio::test]
async fn parsed_boundaries_selected_class_with_regex_brace_still_refuses_atomically() {
    let source = "class Other {\n    pattern = /}/;\n    city = 'Paris';\n}\nclass Account {\n    city = 'London';\n}\nconst other = new Other();\nconsole.log(other.pattern.test('}'), other.city);\n";
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let gw = gateway(
        &file,
        source,
        "Other",
        "city",
        "city = 'Paris'",
        vec![at(&file, source, "other.city)", "city")],
    )
    .await;
    let args = ["--target", "ES2022", "--module", "commonjs", "account.ts"];
    assert!(run_cmd(&ws, "tsc", &args).0);
    let before = run_cmd(&ws, "node", &["account.js"]);
    assert!(before.0);
    assert_eq!(before.1, "true Paris\n");
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
    let err = extract_delegate_polyglot(
        gw.addr(),
        &ws.root(),
        &file,
        None,
        Some(3),
        None,
        &["city".into()],
        &[],
        "Address",
        "address",
        true,
        true,
        None,
    )
    .await
    .expect_err("selecting a class whose own body contains an unproved regex brace must refuse");
    assert!(
        err.to_string()
            .contains("class has unproved template/regex lexical boundaries"),
        "{err:#}"
    );
    assert_eq!(
        fs::read_to_string(&file).unwrap().as_bytes(),
        source.as_bytes()
    );
    assert_eq!(snapshot(), before_files);
}

#[tokio::test]
async fn parsed_boundaries_division_after_contextual_or_generic_preserves_field_reference() {
    let source = "type Numeric<T> = number;\nconst of = 14;\nclass Account {\n    city = 7;\n}\nconst a = new Account();\nconsole.log(of / a.city / 2, (14 as Numeric<string>) / a.city / 2);\n";
    let ws = Workspace::new(&[("account.ts", source)]);
    let file = ws.path("account.ts");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "city = 7",
        vec![
            at(&file, source, "of / a.city", "city"),
            at(&file, source, ") / a.city", "city"),
        ],
    )
    .await;
    println!("CASE division-field-reference\nBEFORE SOURCE\n{source}END SOURCE");
    let args = ["--target", "ES2022", "--module", "commonjs", "account.ts"];
    assert!(run_cmd(&ws, "tsc", &args).0);
    let before = run_cmd(&ws, "node", &["account.js"]);
    assert!(before.0);
    assert_eq!(before.1, "1 1\n");
    let res = extract(
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
    .await
    .expect("division after contextual identifier or generic assertion must keep field references visible");
    assert!(res.applied);
    let after = fs::read_to_string(&file).unwrap();
    println!("AFTER SOURCE\n{after}END SOURCE");
    assert!(after.contains("of / a.address.city / 2"));
    assert!(after.contains("(14 as Numeric<string>) / a.address.city / 2"));
    assert!(run_cmd(&ws, "tsc", &args).0);
    let after_run = run_cmd(&ws, "node", &["account.js"]);
    assert!(after_run.0);
    assert_eq!(after_run.1, "1 1\n");
}

#[tokio::test]
async fn parsed_boundaries_jsx_and_tsx_preserve_class_selection_and_initializer_guards() {
    let source = "declare namespace JSX { interface IntrinsicElements { div: any; } }\nconst h = (tag: string, props: any) => ({ tag, props });\nclass Account {\n    readonly city: string;\n    badge = <div data-label=\"this.current()\" />;\n    constructor(city: string) {\n        this.city = city;\n    }\n}\nconst view = <div />;\nconst a = new Account('Paris');\nconsole.log(view.tag, a.badge.tag, a.city);\n";
    let ws = Workspace::new(&[("account.tsx", source)]);
    let file = ws.path("account.tsx");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "readonly city:",
        vec![
            at(&file, source, "this.city = city", "city"),
            at(&file, source, "a.city)", "city"),
        ],
    )
    .await;
    let args = [
        "--target",
        "ES2022",
        "--module",
        "commonjs",
        "--jsx",
        "react",
        "--jsxFactory",
        "h",
        "account.tsx",
    ];
    assert!(run_cmd(&ws, "tsc", &args).0);
    assert_eq!(run_cmd(&ws, "node", &["account.js"]).1, "div div Paris\n");
    let res = extract(
        &ws,
        &gw,
        "account.tsx",
        "Account",
        "city",
        &[],
        "Address",
        "address",
        true,
    )
    .await
    .expect("TSX file with unrelated JSX and safe JSX field initializer must succeed");
    assert!(res.applied);
    let after = fs::read_to_string(&file).unwrap();
    assert!(after.contains("class Address {"));
    assert!(after.contains("const view = <div />;"));
    assert!(after.contains("a.address.city"));
    assert!(run_cmd(&ws, "tsc", &args).0);
    assert_eq!(run_cmd(&ws, "node", &["account.js"]).1, "div div Paris\n");

    let unsafe_tsx = "declare namespace JSX { interface IntrinsicElements { div: any; } }\nconst h = (tag: string, props: any) => ({ tag, props });\nclass Account {\n    readonly city: number;\n    badge = <div data-city={this.current()} />;\n    constructor(city: number) {\n        this.city = city;\n    }\n    current(): number { return this.city; }\n}\n";
    let ws2 = Workspace::new(&[("account.tsx", unsafe_tsx)]);
    let file2 = ws2.path("account.tsx");
    let gw2 = gateway(
        &file2,
        unsafe_tsx,
        "Account",
        "city",
        "readonly city:",
        vec![
            at(&file2, unsafe_tsx, "this.city = city", "city"),
            at(&file2, unsafe_tsx, "return this.city", "city"),
        ],
    )
    .await;
    let err = extract(
        &ws2,
        &gw2,
        "account.tsx",
        "Account",
        "city",
        &[],
        "Address",
        "address",
        true,
    )
    .await
    .expect_err("JSX expression initializer reading this must refuse readonly deferral");
    assert!(
        err.to_string()
            .contains("readonly initialization has unproved owner field-initializer effects"),
        "{err:#}"
    );
}

#[tokio::test]
async fn parsed_boundaries_exported_class_with_class_literal_in_extends_preserves_exports() {
    for (prefix, header, expected_prefix) in [
        (
            "function base(_label: string) {\n    return class {};\n}\n",
            "export class Account extends base(\"class Other\")",
            "function base(_label: string) {\n    return class {};\n}\nexport class Address {",
        ),
        (
            "function base(_label: string) {\n    return class {};\n}\n",
            "export class Account extends base(\"class Account\")",
            "function base(_label: string) {\n    return class {};\n}\nexport class Address {",
        ),
        (
            "const sentinel = 1\n",
            "export class Account",
            "const sentinel = 1\nexport class Address {",
        ),
        (
            "// preceding line comment\n",
            "export class Account",
            "// preceding line comment\nexport class Address {",
        ),
        (
            "const sentinel = 1\n// preceding line comment\n",
            "export default class Account",
            "const sentinel = 1\n// preceding line comment\nexport class Address {",
        ),
    ] {
        let source = format!(
            "{prefix}{header} {{\n    city = 'Paris';\n}}\nconst a = new Account();\nconsole.log(a.city);\n"
        );
        let ws = Workspace::new(&[("account.ts", &source)]);
        let file = ws.path("account.ts");
        let gw = gateway(
            &file,
            &source,
            "Account",
            "city",
            "city = 'Paris'",
            vec![at(&file, &source, "a.city)", "city")],
        )
        .await;
        let args = ["--target", "ES2022", "--module", "commonjs", "account.ts"];
        assert!(run_cmd(&ws, "tsc", &args).0);
        assert_eq!(run_cmd(&ws, "node", &["account.js"]).1, "Paris\n");
        let res = extract(
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
        .await
        .expect("exported class must preserve preceding trivia and extract cleanly");
        assert!(res.applied);
        let after = fs::read_to_string(&file).unwrap();
        assert!(after.starts_with(expected_prefix), "after:\n{after}");
        assert!(after.contains(&format!("{header} {{")), "after:\n{after}");
        assert!(run_cmd(&ws, "tsc", &args).0);
        assert_eq!(run_cmd(&ws, "node", &["account.js"]).1, "Paris\n");
    }
}
