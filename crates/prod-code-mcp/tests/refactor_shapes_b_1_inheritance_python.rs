/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Replacing a Python base class with a field: the header, the initializer in each of its
//! shapes, the forwarding methods and the leftover `super()` calls.

use prod_code_mcp::pull_push::parse_classes_in_text;
use prod_code_mcp::replace_inheritance::languages::transform_python;
use std::path::Path;

const BASE: &str = "class Animal:\n    def speak(self):\n        pass\n\n";

/// Transforms the second class of `BASE` + `sub` into a holder of an `Animal` named `_a`.
fn delegate(sub: &str, methods: &[&str]) -> anyhow::Result<String> {
    let text = format!("{BASE}{sub}");
    let classes = parse_classes_in_text(&text, "python", Path::new("zoo.py"));
    let methods: Vec<String> = methods.iter().map(|m| m.to_string()).collect();
    transform_python(
        &text,
        &classes[1],
        "Animal",
        "_a",
        &methods,
        Some(&classes[0]),
    )
}

#[test]
fn the_base_leaves_the_header_and_a_new_initializer_and_forwards_are_added() {
    // Another base stays in the header; the body gains an initializer and one forward per method.
    assert_eq!(
        delegate(
            "class Dog(Animal, Mixin):\n    def bark(self):\n        return 1\n",
            &["speak"]
        )
        .unwrap(),
        format!(
            "{BASE}class Dog(Mixin):\n    def bark(self):\n        return 1\n\n    def __init__(self, *args, **kwargs):\n        self._a = Animal(*args, **kwargs)\n\n    def speak(self, *args, **kwargs):\n        return self._a.speak(*args, **kwargs)\n"
        )
    );
    // A body of `pass` is replaced rather than extended.
    assert_eq!(
        delegate("class Cat(Animal):\n    pass\n", &["speak"]).unwrap(),
        format!(
            "{BASE}class Cat:\n    def __init__(self, *args, **kwargs):\n        self._a = Animal(*args, **kwargs)\n\n    def speak(self, *args, **kwargs):\n        return self._a.speak(*args, **kwargs)\n"
        )
    );
    // A class without a parenthesised header has no base to drop.
    assert_eq!(
        delegate("class Fish:\n    pass\n", &[]).unwrap(),
        format!(
            "{BASE}class Fish:\n    def __init__(self, *args, **kwargs):\n        self._a = Animal(*args, **kwargs)\n"
        )
    );
}

#[test]
fn an_existing_initializer_keeps_its_arguments_in_every_shape_it_is_written() {
    // `super().__init__(…)` becomes the assignment; any other `super().` call becomes a field call.
    assert_eq!(
        delegate(
            "class Dog(Animal):\n    def __init__(self, name):\n        super().__init__(name)\n\n    def speak(self):\n        return super().speak()\n",
            &[]
        )
        .unwrap(),
        format!(
            "{BASE}class Dog:\n    def __init__(self, name):\n        self._a = Animal(name)\n\n    def speak(self):\n        return self._a.speak()\n"
        )
    );
    // The explicit form drops `self`; a call with only `self` has no arguments to forward, and
    // a mention in a comment or without a call is left alone.
    assert_eq!(
        delegate(
            "class Dog(Animal):\n    def __init__(self, name):\n        # Animal.__init__(self, ignored)\n        Animal.__init__(self, name, 3)\n        Animal.__init__(self)\n        use = Animal.__init__\n",
            &[]
        )
        .unwrap(),
        format!(
            "{BASE}class Dog:\n    def __init__(self, name):\n        # Animal.__init__(self, ignored)\n        self._a = Animal(name, 3)\n        self._a = Animal()\n        use = Animal.__init__\n"
        )
    );
    // An initializer that never calls the base builds the field first.
    assert_eq!(
        delegate(
            "class Dog(Animal):\n    def __init__(self):\n        self.n = 1\n",
            &[]
        )
        .unwrap(),
        format!(
            "{BASE}class Dog:\n    def __init__(self):\n        self._a = Animal()\n        self.n = 1\n"
        )
    );
}

#[test]
fn an_explicit_base_initializer_without_self_is_refused() {
    let err = delegate(
        "class Dog(Animal):\n    def __init__(self, name):\n        Animal.__init__(selfish, name)\n",
        &[],
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "explicit base initializer does not start with `self`; nothing was rewritten"
    );
}
