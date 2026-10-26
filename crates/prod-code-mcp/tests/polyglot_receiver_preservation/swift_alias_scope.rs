/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[tokio::test]
async fn review14_swift_alias_declaration_uses_canonical_scope() {
    for declarations in [
        "enum Names {}\ntypealias NamesAlias = Names\nextension NamesAlias { typealias Alias = C }",
        "enum Names {}\ntypealias NamesAlias = Names\nextension NamesAlias { typealias Target = C }\nextension Names { typealias Alias = Target }",
        "enum Names {}\ntypealias Prefix = Names\ntypealias NamesAlias = Prefix\nextension NamesAlias { typealias Alias = C }",
    ] {
        for (members, result) in [
            ("typealias Void = Int", "Void"),
            ("enum Swift { typealias Void = Int }", "((Swift.Void))"),
        ] {
            let before = format!(
                "class C {{\n    var value = 7\n    static func same(c: C) -> {result} {{ c.value }}\n}}\n{declarations}\nextension NamesAlias.Alias {{ {members} }}\nprint(C.same(c: C()))\n"
            );
            super::check(&before, true, "7\n").await;
        }
    }
}

#[tokio::test]
async fn review14_swift_canonical_scope_excludes_unrelated_alias_targets() {
    for declarations in [
        "enum Names { class C {} }\ntypealias NamesAlias = Names\nextension NamesAlias { typealias Alias = C }",
        "enum Names {}\ntypealias NamesAlias = Names\nextension NamesAlias { class C {} }\nextension Names { typealias Alias = C }",
        "class Other {}\nenum Names {}\ntypealias NamesAlias = Names\nextension NamesAlias { typealias Target = Other }\nextension Names { typealias Alias = Target }",
    ] {
        for (members, result) in [
            ("typealias Void = Int", "Void"),
            ("enum Swift { typealias Void = Int }", "Swift.Void"),
        ] {
            super::statements(
                &format!("{declarations}\nextension NamesAlias.Alias {{ {members} }}\n"),
                "",
                result,
            )
            .await;
            // A single multiline call also requires definite exclusion: the
            // separate-statement body proof must not hide unresolved lookup.
            let before = format!(
                "{declarations}\nextension NamesAlias.Alias {{ {members} }}\nclass C {{\n    var value = 7\n    static func same(c: C) -> {result} {{\n        print(\n            c.value\n        )\n    }}\n}}\nC.same(c: C())\n"
            );
            let (after, _) = super::convert(&before, "swift", true).await;
            assert!(
                after.contains("let c: C") && after.contains(" = self;"),
                "{after}"
            );
            assert!(
                after.contains("print(\n            c.value\n        )"),
                "{after}"
            );
            assert!(!after.contains("return print("), "{after}");
            if super::available("swiftc") {
                assert_eq!(super::compiled("swiftc", "swift", &before), "7\n");
                assert_eq!(super::compiled("swiftc", "swift", &after), "7\n");
            }
        }
    }
}

#[tokio::test]
async fn review14_swift_canonical_scope_keeps_unknown_alias_uncertain() {
    for target in ["External", "Missing.Nested", "Alias"] {
        let before = format!(
            "class C {{\n    var value = 7\n    static func same(c: C) -> Void {{\n        print(\n            c.value\n        )\n    }}\n}}\nenum Names {{}}\ntypealias NamesAlias = Names\nextension NamesAlias {{ typealias Alias = {target} }}\nextension NamesAlias.Alias {{ typealias Void = Int }}\n"
        );
        super::super::support::refuses(&before, "swift").await;
    }
}
