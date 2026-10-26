/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{available, compiled, convert};

#[tokio::test]
async fn swift_direct_identity_and_implicit_return_controls() {
    for body in [
        "return c === other",
        "c === other",
        "\n        c === other\n    ",
    ] {
        let before = format!(
            "class C {{\n    static func same(_ c: C, other: C) -> Bool {{ {body} }}\n}}\nlet value = C()\nprint(C.same(value, other: value))\n"
        );
        let (after, _) = convert(&before, "swift", true).await;
        assert!(after.contains("let c: C = self;"), "{after}");
        assert!(after.contains("return c === other"), "{after}");
        if available("swiftc") {
            assert_eq!(
                compiled("swiftc", "swift", &after),
                compiled("swiftc", "swift", &before)
            );
        } else {
            eprintln!(
                "Swift compiler unavailable; public planner/apply source controls only; no Swift runtime claim"
            );
        }
    }
}

#[tokio::test]
async fn swift_argument_capture_shadowing_and_subscript_source_controls() {
    let before = r#"class C {
    var value = 3
    subscript(index: Int) -> Int { value }
    static func same(c: C, other: C) -> Int {
        let take = { (value: C) in value.value }
        let capture = { [c] in c.value }
        let shadow = { (c: C) in c.value }
        let text = "c.value }"
        // c.value } stays a comment
        return take(c) + capture() + shadow(other) + c[0] + text.count
    }
}
let value = C()
print(C.same(c: value, other: value))
"#;
    let (after, _) = convert(before, "swift", false).await;
    assert!(after.contains("let c: C = self;"));
    assert!(after.contains("{ [c] in c.value }"));
    assert!(after.contains("{ (c: C) in c.value }"));
    assert!(after.contains("\"c.value }\""));
    if available("swiftc") {
        assert_eq!(
            compiled("swiftc", "swift", &after),
            compiled("swiftc", "swift", before)
        );
    } else {
        eprintln!(
            "Swift compiler unavailable; public planner/apply source controls only; no Swift runtime claim"
        );
    }
}
