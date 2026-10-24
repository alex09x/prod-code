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
fn defaults_match_target_language_literals() {
    for (ty, expected) in [
        ("bool", "false"),
        ("int", "0"),
        ("long", "0"),
        ("size_t", "0"),
        ("uint32_t", "0"),
        ("int64_t", "0"),
        ("float", "0.0"),
        ("double", "0.0"),
        ("std::string", "\"\""),
        ("string", "\"\""),
        ("std::vector<int>", "{}"),
        ("std::map<int,int>", "{}"),
        ("Widget", "{}"),
        (" int ", "0"),
    ] {
        assert_eq!(cpp_default_for_type(ty), expected, "{ty}");
    }
    for (ty, expected) in [
        ("String", "\"\""),
        ("Int", "0"),
        ("Int8", "0"),
        ("Int16", "0"),
        ("Int32", "0"),
        ("Int64", "0"),
        ("UInt", "0"),
        ("UInt8", "0"),
        ("UInt16", "0"),
        ("UInt32", "0"),
        ("UInt64", "0"),
        ("Double", "0.0"),
        ("Float", "0.0"),
        ("Bool", "false"),
        ("[String]", "[]"),
        ("String?", "nil"),
        ("Widget", "nil"),
        (" Bool ", "false"),
    ] {
        assert_eq!(swift_default_for_type(ty), expected, "{ty}");
    }
    for (ty, expected) in [
        ("boolean", "false"),
        ("byte", "0"),
        ("short", "0"),
        ("int", "0"),
        ("long", "0"),
        ("float", "0.0f"),
        ("double", "0.0"),
        ("char", "'\\0'"),
        ("String", "\"\""),
        ("List<String>", "new java.util.ArrayList<>()"),
        ("java.util.List<String>", "new java.util.ArrayList<>()"),
        ("Map<String,String>", "new java.util.HashMap<>()"),
        ("java.util.Map<String,String>", "new java.util.HashMap<>()"),
        ("Set<String>", "new java.util.HashSet<>()"),
        ("java.util.Set<String>", "new java.util.HashSet<>()"),
        ("Widget", "null"),
    ] {
        assert_eq!(java_default_for_type(ty), expected, "{ty}");
    }
}

fn method(ret: Option<&str>) -> MethodSignature {
    MethodSignature {
        name: "read".into(),
        params: vec![("key".into(), "String".into())],
        return_type: ret.map(str::to_owned),
    }
}

#[test]
fn cpp_records_calls_and_initializes_fields() {
    assert_eq!(
        generate_cpp_mock(
            "Store",
            &[method(Some("bool")), method(None)],
            &[("ready".into(), "bool".into())]
        ),
        "class MockStore : public Store {\npublic:\n    std::vector<std::string> calls;\n\n    bool ready{false};\n    bool read(String key) override {\n        calls.push_back(\"read\");\n        return false;\n    }\n\n    void read(String key) override {\n        calls.push_back(\"read\");\n    }\n\n};"
    );
}

#[test]
fn swift_records_calls_and_omits_void_returns() {
    assert_eq!(
        generate_swift_mock(
            "Store",
            &[
                method(Some("Bool")),
                method(Some("Void")),
                method(Some("()")),
                method(None)
            ],
            &[("ready".into(), "Bool".into())]
        ),
        "final class MockStore: Store {\n    var calls: [String] = []\n\n    var ready: Bool = false\n    func read(key: String) -> Bool {\n        calls.append(\"read\")\n        return false\n    }\n\n    func read(key: String) -> Void {\n        calls.append(\"read\")\n    }\n\n    func read(key: String) -> () {\n        calls.append(\"read\")\n    }\n\n    func read(key: String) {\n        calls.append(\"read\")\n    }\n\n}"
    );
}

#[test]
fn java_records_calls_and_omits_void_returns() {
    assert_eq!(
        generate_java_mock(
            "Store",
            &[method(Some("boolean")), method(Some("Void")), method(None)],
            &[("ready".into(), "boolean".into())]
        ),
        "public class MockStore implements Store {\n    public java.util.List<String> calls = new java.util.ArrayList<>();\n\n    public boolean ready = false;\n    @Override\n    public boolean read(String key) {\n        calls.add(\"read\");\n        return false;\n    }\n\n    @Override\n    public Void read(String key) {\n        calls.add(\"read\");\n        return null;\n    }\n\n    @Override\n    public void read(String key) {\n        calls.add(\"read\");\n    }\n\n}\n"
    );
}
