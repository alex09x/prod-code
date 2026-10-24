/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::*;

#[test]
fn python_defaults() {
    for (ty, expected) in [
        ("list[int]", "[]"),
        ("List[str]", "[]"),
        ("dict[str,int]", "{}"),
        ("Dict[str,int]", "{}"),
        ("set[int]", "set()"),
        ("Set[str]", "set()"),
        ("Optional[str]", "None"),
        ("str", "\"\""),
        ("int", "0"),
        ("float", "0.0"),
        ("bool", "False"),
        ("None", "None"),
        ("datetime", "datetime.datetime(2026, 1, 1)"),
        ("datetime.datetime", "datetime.datetime(2026, 1, 1)"),
        ("Widget", "None"),
    ] {
        assert_eq!(py_default_for_type(ty), expected, "{ty}");
    }
}

#[test]
fn rust_defaults() {
    for (ty, expected) in [
        ("()", "()"),
        ("bool", "false"),
        ("i8", "0"),
        ("i16", "0"),
        ("i32", "0"),
        ("i64", "0"),
        ("i128", "0"),
        ("isize", "0"),
        ("u8", "0"),
        ("u16", "0"),
        ("u32", "0"),
        ("u64", "0"),
        ("u128", "0"),
        ("usize", "0"),
        ("f32", "0.0"),
        ("f64", "0.0"),
        ("String", "String::new()"),
        ("&str", "\"\""),
        ("str", "\"\""),
        ("Option<u8>", "None"),
        ("Vec<u8>", "Vec::new()"),
        ("HashMap<u8,u8>", "std::collections::HashMap::new()"),
        ("Result<u8, E>", "Ok(Default::default())"),
        ("Widget", "Default::default()"),
    ] {
        assert_eq!(rust_default_for_type(ty), expected, "{ty}");
    }
}

#[test]
fn python_empty_and_multiple_arguments() {
    assert_eq!(
        generate_python_mock("Store", &[], &[]),
        "class MockStore:\n    \"\"\"Mock implementation of Store for testing.\"\"\"\n\n    def __init__(self):\n        self.calls: list[tuple[str, tuple, dict]] = []\n        self._stubs: dict[str, any] = {}\n        pass"
    );
    let methods = [MethodSignature {
        name: "read".into(),
        params: vec![("key".into(), "".into()), ("limit".into(), "int".into())],
        return_type: None,
    }];
    assert_eq!(
        generate_python_mock("Store", &methods, &[("ready".into(), "bool".into())]),
        "class MockStore:\n    \"\"\"Mock implementation of Store for testing.\"\"\"\n\n    def __init__(self):\n        self.calls: list[tuple[str, tuple, dict]] = []\n        self._stubs: dict[str, any] = {}\n        self.ready: bool = False\n\n    def read(self, key, limit: int):\n        self.calls.append((\"read\", (key, limit), {}))\n        if \"read\" in self._stubs:\n            handler = self._stubs[\"read\"]\n            return handler(key, limit) if callable(handler) else handler\n        return None"
    );
}
