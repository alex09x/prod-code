/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "pull_up_literal_preservation/boundary_regressions.rs"]
mod boundary_regressions;
#[path = "pull_up_literal_preservation/brace_language.rs"]
mod brace_language;
#[path = "pull_up_literal_preservation/consumer_evidence.rs"]
mod consumer_evidence;
#[path = "pull_up_literal_preservation/push_down_regressions.rs"]
mod push_down_regressions;
#[path = "pull_up_literal_preservation/replacement.rs"]
mod replacement;
#[path = "pull_up_literal_preservation/review_regressions.rs"]
mod review_regressions;
#[path = "pull_up_literal_preservation/shared_consumers.rs"]
mod shared_consumers;
#[path = "pull_up_literal_preservation/support.rs"]
mod support;
#[path = "pull_up_literal_preservation/swift_default_forwarding.rs"]
mod swift_default_forwarding;
#[path = "pull_up_literal_preservation/swift_replacement.rs"]
mod swift_replacement;

use support::Case;

#[tokio::test]
async fn python_multiline_values_survive_sibling_cleanup() {
    // All literal lines deliberately remain inside the selected method's indentation.
    let source = concat!(
        "class Pet:\n    pass\n\n",
        "class Dog(Pet):\n    def speak(self):\n",
        "        return \"\"\"hello \n        world\n        \"\"\"\n\n",
        "class Cat(Pet):\n    def speak(self):\n",
        "        return \"\"\"hello\n        world\n        \"\"\"\n",
    );
    let case = Case::new("pets.py", source, false);
    case.assert_member("Dog", "speak", "hello \n        world\n        ");
    case.assert_member("Cat", "speak", "hello\n        world\n        ");
    let before = case.python_values(&["Dog", "Cat"]);
    assert_eq!(
        before,
        [
            "hello \n        world\n        ",
            "hello\n        world\n        "
        ]
    );
    case.pull().await;
    let after = case.python_values(&["Dog", "Cat"]);
    case.evidence(&before, &after);
    assert_eq!(after, before, "pull-up changed Python literal values");
    case.assert_member("Pet", "speak", "hello \n        world\n        ");
    case.assert_member("Cat", "speak", "hello\n        world\n        ");
    case.assert_no_member("Dog", "speak");
}

#[tokio::test]
async fn javascript_modifier_words_and_template_bytes_survive() {
    let source = concat!(
        "class Pet {}\n",
        "class Dog extends Pet {\n    speak() {\n",
        "        return `override final \n        tail`;\n    }\n}\n",
        "class Cat extends Pet {\n    speak() {\n",
        "        return `final \n        tail`;\n    }\n}\n",
    );
    let case = Case::new("pets.js", source, false);
    case.assert_member("Dog", "speak", "`override final \n        tail`");
    case.assert_member("Cat", "speak", "`final \n        tail`");
    let before = case.javascript_values(&["Dog", "Cat"]);
    assert_eq!(
        before,
        ["override final \n        tail", "final \n        tail"]
    );
    case.pull().await;
    let after = case.javascript_values(&["Dog", "Cat"]);
    case.evidence(&before, &after);
    assert_eq!(after, before, "pull-up changed JavaScript literal values");
    case.assert_member("Pet", "speak", "`override final \n        tail`");
    case.assert_member("Cat", "speak", "`final \n        tail`");
}

#[tokio::test]
async fn identical_siblings_are_removed_and_different_bodies_remain() {
    for multi_file in [false, true] {
        let source = concat!(
            "class Pet:\n    pass\n\n",
            "class Dog(Pet):\n    def speak(self):\n        return 7\n\n",
            "class Copy(Pet):\n    def speak(self):\n        return 7\n\n",
            "class Cat(Pet):\n    def speak(self):\n        return 8\n",
        );
        let case = Case::new("pets.py", source, multi_file);
        let before = case.python_values(&["Dog", "Copy", "Cat"]);
        assert_eq!(before, ["7", "7", "8"]);
        case.pull().await;
        let after = case.python_values(&["Dog", "Copy", "Cat"]);
        case.evidence(&before, &after);
        assert_eq!(after, before);
        case.assert_no_member("Dog", "speak");
        case.assert_no_member("Copy", "speak");
        case.assert_member("Cat", "speak", "return 8");
        case.assert_member("Pet", "speak", "return 7");
    }
}

#[tokio::test]
async fn moving_to_a_different_indent_preserves_multiline_data() {
    for multi_file in [false, true] {
        let source = concat!(
            "class Pet:\n    pass\n\n",
            "class Dog(Pet):\n        def speak(self):\n",
            "            return \"\"\"hello \n            \n            world\n            \"\"\"\n\n",
            "class Cat(Pet):\n        def speak(self):\n",
            "            return \"\"\"hello\n            \n            world\n            \"\"\"\n",
        );
        let case = Case::new("pets.py", source, multi_file);
        let before = case.python_values(&["Dog", "Cat"]);
        assert_eq!(
            before,
            [
                "hello \n            \n            world\n            ",
                "hello\n            \n            world\n            "
            ]
        );
        case.pull().await;
        let after = case.python_values(&["Dog", "Cat"]);
        case.evidence(&before, &after);
        assert_eq!(after, before);
        case.assert_member(
            "Pet",
            "speak",
            "\n            \n            world\n            \"\"\"",
        );
        case.assert_member("Cat", "speak", "hello\n");
    }
}
