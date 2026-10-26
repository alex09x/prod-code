/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{available, compiled, convert, refuses};

#[tokio::test]
async fn repair_cpp_move_only_value_receiver() {
    let before = "#include <iostream>\n#include <memory>\nstruct C {\n    std::unique_ptr<int> value;\n    static int same(C c) { return *c.value; }\n};\nint main() { std::cout << C::same(C{std::make_unique<int>(7)}) << '\\n'; }\n";
    if available("c++") {
        assert_eq!(compiled("c++", "cpp", before), "7\n");
    } else {
        eprintln!("C++ compiler unavailable; no-write refusal source control only");
    }
    refuses(before, "cpp").await;
}

#[tokio::test]
async fn repair_cpp_value_receiver_construction_counts() {
    let before = "#include <iostream>\nstruct C {\n    int value;\n    C(int v): value(v) {}\n    C(const C& c): value(c.value + 10) {}\n    C(C&& c): value(c.value + 100) {}\n    static int same(C c) { return c.value; }\n};\nint main() { C c{7}; std::cout << C::same(c) << ':' << C::same(C{7}) << ':' << C::same(static_cast<C&&>(c)) << '\\n'; }\n";
    if available("c++") {
        assert_eq!(compiled("c++", "cpp", before), "17:7:107\n");
    } else {
        eprintln!("C++ compiler unavailable; no-write refusal source control only");
    }
    refuses(before, "cpp").await;
    refuses(&before.replace("same(C c)", "same(const C c)"), "cpp").await;

    // Even trivial values need construction analysis; do not silently copy them
    // just because their constructors are not visible in the method body.
    refuses(
        "struct C {\n    int value;\n    static int same(C c) { return c.value; }\n};\n",
        "cpp",
    )
    .await;
}

#[tokio::test]
async fn repair_cpp_reference_direct_identity_stays_supported() {
    let before = "#include <iostream>\nstruct C {\n    static bool same(C& c, C& other) { return &c == &other; }\n};\nint main() { C c; std::cout << C::same(c, c) << '\\n'; }\n";
    let (after, _) = convert(before, "cpp", true).await;
    assert!(after.contains("C& c = *this;"), "{after}");
    if available("c++") {
        assert_eq!(
            compiled("c++", "cpp", &after),
            compiled("c++", "cpp", before)
        );
    } else {
        eprintln!("C++ compiler unavailable; planner/apply source control only");
    }
}

#[tokio::test]
async fn review_default_initializer_and_signature_suffix_braces_are_not_method_bodies() {
    for suffix in ["", " noexcept(noexcept(C{0}))"] {
        let before = format!(
            "#include <iostream>\nstruct C {{\n    int value;\n    static int same(C& c, C other = C{{0}}){suffix}\n    {{\n        auto take = [](C& value) {{ return value.value; }};\n        return take(c) + other.value;\n    }}\n}};\nint main() {{ C value{{7}}; std::cout << C::same(value) << '\\n'; }}\n"
        );
        let expected = if available("c++") {
            Some(compiled("c++", "cpp", &before))
        } else {
            eprintln!("C++ compiler unavailable; planner/apply source control only");
            None
        };
        let (after, _) = convert(&before, "cpp", true).await;
        eprintln!("BEFORE:\n{before}AFTER:\n{after}");
        if let Some(expected) = expected {
            assert_eq!(compiled("c++", "cpp", &after), expected);
        }
        assert!(after.contains("other = C{0}"), "{after}");
        assert!(after.contains("C& c = *this;"), "{after}");
    }
}

#[tokio::test]
async fn cpp_reference_pointer_and_const_receiver_controls() {
    for (parameter, body, call) in [
        (
            "C& c",
            "auto capture = [&c] { return c.value; }; c = other; return capture();",
            "C::same(value, other)",
        ),
        (
            "C* c",
            "auto take = [](C* p) { return p->value; }; c = other; return take(c) + c[0].value;",
            "C::same(&value, &other)",
        ),
        (
            "const C& c",
            "auto capture = [c] { return c.value; }; return capture() + c.value;",
            "C::same(value, other)",
        ),
    ] {
        let other = if parameter.contains('*') {
            "C* other"
        } else {
            "C other"
        };
        let before = format!(
            "#include <iostream>\nstruct C {{\n    int value;\n    static int same({parameter}, {other}) {{ {body} }}\n}};\nint main() {{ C value{{3}}, other{{5}}; std::cout << {call} << ':' << value.value << '\\n'; }}\n"
        );
        let (after, _) = convert(&before, "cpp", true).await;
        if !available("c++") {
            eprintln!("C++ compiler unavailable; planner/apply source control only");
            continue;
        }
        assert_eq!(
            compiled("c++", "cpp", &after),
            compiled("c++", "cpp", &before),
            "{after}"
        );
    }
}

#[tokio::test]
async fn cpp_direct_reference_arguments_shadowing_literals_and_complete_receivers() {
    let before = r#"#include <iostream>
struct C {
    int value;
    static int same(C& c, C other) {
        auto take = [](const C& value) { return value.value; };
        auto shadow = [](C c) { return c.value; };
        const char* text = "c.value }";
        char brace = '}';
        char quote = '"';
        int thousand = 1'000;
        // c.value } is a comment
        return take(c) + shadow(other) + c.value + (text[0] == 'c') + (brace == '}') + (quote == '"') + (thousand == 1000);
    }
};
int main() { C values[2]{{3}, {5}}; bool choose = true; std::cout << C::same(choose ? values[0] : values[1], values[1]) << '\n'; }
"#;
    let (after, _) = convert(before, "cpp", true).await;
    assert!(after.contains("(choose ? values[0] : values[1]).same(values[1])"));
    assert!(after.contains("[](C c) { return c.value; }"));
    assert!(after.contains("\"c.value }\""));
    if available("c++") {
        assert_eq!(
            compiled("c++", "cpp", &after),
            compiled("c++", "cpp", before)
        );
    } else {
        eprintln!("C++ compiler unavailable; planner/apply source control only");
    }
}
