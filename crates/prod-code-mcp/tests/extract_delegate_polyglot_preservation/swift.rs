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
use std::{fs, process::Command};

#[tokio::test]
async fn swift_interpolation_references_move_and_rewrite() {
    let source = "class Account {\n    var city: String\n    init(city: String) {\n        self.city = city\n    }\n    func read() -> String {\n        return \"city: \\(city)\"\n    }\n    func retained() -> String {\n        return \"other: \\(self.city)\"\n    }\n}\nlet a = Account(city: \"Paris\")\nprint(a.read(), a.retained(), \"external: \\(a.city)\")\n";
    let ws = Workspace::new(&[("account.swift", source)]);
    let file = ws.path("account.swift");
    let refs = ["\\(city)", "self.city =", "self.city)", "a.city"]
        .iter()
        .map(|anchor| at(&file, source, anchor, "city"))
        .collect();
    let gw = gateway(&file, source, "Account", "city", "var city:", refs).await;
    let swift = Command::new("swift")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    let before = swift.then(|| run(&ws, "swift", &["account.swift"]));
    println!("Swift execution available: {swift}; otherwise source-only assertions");
    extract(
        &ws,
        &gw,
        "account.swift",
        "Account",
        "city",
        &["read"],
        "Address",
        "address",
        true,
    )
    .await
    .unwrap();
    let after = fs::read_to_string(file).unwrap();
    assert!(after.contains("return \"city: \\(city)\""), "{after}");
    assert!(after.contains("\\(self.address.city)"), "{after}");
    assert!(after.contains("\\(a.address.city)"), "{after}");
    assert!(after.contains("return address.read()"), "{after}");
    if let Some(before) = before {
        assert_eq!(run(&ws, "swift", &["account.swift"]), before);
    }
    evidence(
        "interpolation.swift",
        source,
        &after,
        if swift {
            "Actual Swift before/after execution matched\n"
        } else {
            "Source-only: Swift unavailable on the build node\n"
        },
    );
}

#[tokio::test]
async fn swift_raw_multiline_and_nested_interpolation_are_executable() {
    for (expression, anchor) in [
        (r##"#"raw: \#(city)"#"##, r"\#(city)"),
        ("\"\"\"\nvalue: \\(city)\n\"\"\"", r"\(city)"),
        (r#""nested: \("[" + city + "]")""#, "+ city +"),
    ] {
        let source = format!(
            "class Account {{\n    var city: String\n    init(city: String) {{\n        self.city = city\n    }}\n    func read() -> String {{\n        return {expression}\n    }}\n}}\n"
        );
        let ws = Workspace::new(&[("account.swift", &source)]);
        let file = ws.path("account.swift");
        let gw = gateway(
            &file,
            &source,
            "Account",
            "city",
            "var city:",
            vec![
                at(&file, &source, "self.city =", "city"),
                at(&file, &source, anchor, "city"),
            ],
        )
        .await;
        extract(
            &ws,
            &gw,
            "account.swift",
            "Account",
            "city",
            &["read"],
            "Address",
            "address",
            true,
        )
        .await
        .unwrap();
        let after = fs::read_to_string(file).unwrap();
        // Swift removes the closing delimiter's indentation from every content
        // line. Moving this method adds four spaces to both, preserving the text.
        let expected = if expression.starts_with("\"\"\"") {
            expression.replace('\n', "\n    ")
        } else {
            expression.to_string()
        };
        assert!(after.contains(&expected), "{after}");
        assert!(after.contains("return address.read()"), "{after}");
    }
}

#[tokio::test]
async fn swift_literal_or_escaped_interpolation_identity_is_refused() {
    for expression in [r#""city""#, r#""\\(city)""#, r##"#"\(city)"#"##] {
        let source = format!("class Account {{\n    var city: String\n}}\nprint({expression})\n");
        let ws = Workspace::new(&[("account.swift", &source)]);
        let file = ws.path("account.swift");
        let gw = gateway(
            &file,
            &source,
            "Account",
            "city",
            "var city:",
            vec![at(&file, &source, expression, "city")],
        )
        .await;
        let error = extract(
            &ws,
            &gw,
            "account.swift",
            "Account",
            "city",
            &[],
            "Address",
            "address",
            true,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("literal/comment text"),
            "{error:#}"
        );
        assert_eq!(fs::read_to_string(file).unwrap(), source);
    }
}
