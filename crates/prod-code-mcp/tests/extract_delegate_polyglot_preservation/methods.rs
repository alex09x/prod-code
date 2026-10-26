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
use std::fs;

#[tokio::test]
async fn moved_javascript_method_does_not_delete_same_named_unrelated_method() {
    let source = "class Other {\n    read() { return 'other'; }\n}\nclass Account {\n    city = 'Paris';\n    read() { return this.city; }\n    remaining() { return this.city; }\n}\nconst a = new Account();\nconsole.log(a.read(), a.remaining(), new Other().read());\n";
    let ws = Workspace::new(&[("account.js", source)]);
    let file = ws.path("account.js");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "city = 'Paris'",
        vec![
            at(&file, source, "read() { return this.city; }", "city"),
            at(&file, source, "remaining() { return this.city; }", "city"),
        ],
    )
    .await;
    let before = run(&ws, "node", &["account.js"]);
    extract(
        &ws,
        &gw,
        "account.js",
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
    assert!(after.contains("read() { return 'other'; }"), "{after}");
    assert_eq!(run(&ws, "node", &["account.js"]), before);
    evidence("methods.js", source, &after, &before);
}

#[tokio::test]
async fn python_keyword_and_variadic_forwarders_preserve_runtime() {
    let source = "class Account:\n    def __init__(self, city):\n        self.city = city\n    def describe(self, *parts, prefix='', **options):\n        return prefix + self.city + ''.join(parts) + options['tail']\na = Account('Paris')\nprint(a.describe('!', prefix='City:', tail='?'))\n";
    let ws = Workspace::new(&[("account.py", source)]);
    let file = ws.path("account.py");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "self.city = city",
        vec![
            at(&file, source, "self.city = city", "city"),
            at(&file, source, "prefix + self.city", "city"),
        ],
    )
    .await;
    let before = run(&ws, "python3", &["account.py"]);
    extract(
        &ws,
        &gw,
        "account.py",
        "Account",
        "city",
        &["describe"],
        "Address",
        "address",
        true,
    )
    .await
    .unwrap();
    let after = fs::read_to_string(file).unwrap();
    assert_eq!(run(&ws, "python3", &["account.py"]), before);
    evidence("forwarder.py", source, &after, &before);
}

#[tokio::test]
async fn cpp_includes_precede_helper_and_remaining_accesses_are_rewritten() {
    let source = "#include <string>\n#include <iostream>\nclass Other {\npublic:\n    int read() { return 9; }\n};\nclass Account {\npublic:\n    std::string city;\n    std::string read() { return city; }\n    std::string remaining() { return this->city; }\n};\nint main() {\n    Account a;\n    a.city = \"Paris\";\n    std::cout << a.read() << ' ' << a.remaining() << ' ' << Other().read() << '\\n';\n}\n";
    let ws = Workspace::new(&[("account.cpp", source)]);
    let file = ws.path("account.cpp");
    let gw = gateway(
        &file,
        source,
        "Account",
        "city",
        "std::string city;",
        vec![
            at(&file, source, "return city;", "city"),
            at(&file, source, "this->city", "city"),
            at(&file, source, "a.city =", "city"),
        ],
    )
    .await;
    run(&ws, "c++", &["-std=c++17", "account.cpp", "-o", "app"]);
    let before = run(&ws, "./app", &[]);
    extract(
        &ws,
        &gw,
        "account.cpp",
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
    assert!(after.find("#include <iostream>").unwrap() < after.find("class Address").unwrap());
    assert!(after.contains("int read() { return 9; }"));
    run(&ws, "c++", &["-std=c++17", "account.cpp", "-o", "app"]);
    assert_eq!(run(&ws, "./app", &[]), before);
    evidence("methods.cpp", source, &after, &before);
}
