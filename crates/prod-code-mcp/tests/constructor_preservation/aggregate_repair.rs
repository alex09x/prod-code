/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::aggregate_repair_support::{Case, check};

const MANIFEST: &str = "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n[[bin]]\nname='fixture'\npath='main.rs'\n";
const MAIN: &str = "mod model;\n#[derive(Default)] pub struct Config { pub value: i32 }\nimpl Config { pub fn new(value: i32) -> Self { let mut c = Self::default(); c.value = value + 10; c } }\nfn main() { println!(\"{} {}\", model::make().value, model::other().value); }\n";
const MODEL: &str = "pub struct Config { pub value: i32 }\npub fn make() -> Config { Config { value: 1 } }\npub fn other() -> super::Config { super::Config { value: 2 } }\n";

#[tokio::test]
async fn aggregate_repair_super_owner() {
    check(Case {
        id: "super-owner",
        files: &[
            ("main.rs", MAIN),
            ("model.rs", MODEL),
            ("Cargo.toml", MANIFEST),
        ],
        selected: "model.rs",
        target: "Config",
        count: 1,
        refuse: false,
        blocked_preview: false,
        references: MODEL
            .match_indices("Config")
            .skip(1)
            .take(2)
            .map(|(at, _)| ("model.rs", at, 6))
            .collect(),
        preserved: &["super::Config { value: 2 }"],
    })
    .await;
}

#[tokio::test]
async fn aggregate_repair_constructor_member() {
    for (id, source) in [
        (
            "member-prefix",
            "class Bag {\n constructorCount() { return 0; }\n constructor(value) { this.value = value; }\n}\nconst bag = new Bag(7);\nconsole.log(bag.value);\n",
        ),
        (
            "member-nested",
            "class Bag {\n helper() { class Nested { constructor(unrelated) {} } return Nested; }\n constructor(value = 7, ...rest) { this.value = value + rest.reduce((a,b) => a+b,0); }\n}\nconsole.log(new Bag().value, new Bag(7,2,3).value);\n",
        ),
        (
            "member-initializer",
            "function constructor(unrelated) { return unrelated; }\nclass Bag {\n cached = constructor(0);\n constructor(value = 7, ...rest) { this.value = value + rest.reduce((a,b) => a+b,0); }\n}\nconsole.log(new Bag().value, new Bag(7,2,3).value);\n",
        ),
    ] {
        check(Case {
            id,
            files: &[("main.mjs", source)],
            selected: "main.mjs",
            target: "Bag",
            count: if id == "member-prefix" { 1 } else { 2 },
            refuse: false,
            blocked_preview: false,
            references: source
                .match_indices("new Bag")
                .map(|(at, _)| ("main.mjs", at + 4, 3))
                .collect(),
            preserved: &[],
        })
        .await;
    }
}

#[tokio::test]
async fn aggregate_repair_constructor_after_string_field() {
    for (id, source) in [
        (
            "member-asi-string",
            "class Bag {\n label = 'bag'\n constructor(value, extra) { this.saved = value + extra; }\n}\nconsole.log(new Bag(7, 2).saved);\n",
        ),
        (
            "member-asi-comment",
            "class Bag {\n label = 'bag' /* constructor(fake) {} */\n // constructor(unrelated) {}\n constructor(value = 7, ...rest) { this.saved = value + rest.reduce((a,b) => a+b,0); }\n}\nconsole.log(new Bag().saved, new Bag(7,2,3).saved);\n",
        ),
    ] {
        check(Case {
            id,
            files: &[("main.mjs", source)],
            selected: "main.mjs",
            target: "Bag",
            count: if id == "member-asi-string" { 1 } else { 2 },
            refuse: false,
            blocked_preview: false,
            references: source
                .match_indices("new Bag")
                .map(|(at, _)| ("main.mjs", at + 4, 3))
                .collect(),
            preserved: &["label = 'bag'"],
        })
        .await;
    }
}

#[tokio::test]
async fn aggregate_repair_constructor_after_identifier_field() {
    for (id, source) in [
        (
            "member-asi-get",
            "const get = 0; class Bag {\n label = get\n constructor(value, extra) { this.saved = value + extra; }\n}\nconsole.log(new Bag(7, 2).saved);\n",
        ),
        (
            "member-asi-set",
            "const set = 0; class Bag {\n static\n constructor(unrelated) { return unrelated; }\n label = set\n constructor(value = 7, ...rest) { this.saved = value + rest.reduce((a,b) => a+b,0); }\n}\nconsole.log(new Bag().saved, new Bag(7,2,3).saved);\n",
        ),
        (
            "member-asi-async",
            "const async = 0; class Bag {\n label = async /* completed field */\n constructor(value = 7, ...rest) { this.saved = value + rest.reduce((a,b) => a+b,0); }\n}\nconsole.log(new Bag().saved, new Bag(7,2,3).saved);\n",
        ),
    ] {
        check(Case {
            id,
            files: &[("main.mjs", source)],
            selected: "main.mjs",
            target: "Bag",
            count: if id == "member-asi-get" { 1 } else { 2 },
            refuse: false,
            blocked_preview: false,
            references: source
                .match_indices("new Bag")
                .map(|(at, _)| ("main.mjs", at + 4, 3))
                .collect(),
            preserved: &["label ="],
        })
        .await;
    }
}

#[tokio::test]
async fn aggregate_repair_shift_field_type() {
    let source = "struct Packet { data: [u8; 8 >> 1], tag: u8 }\nfn main() { let p = Packet { data: [0; 4], tag: 1 }; println!(\"{} {}\", p.data.len(), p.tag); }\n";
    check(Case {
        id: "shift-type",
        files: &[("main.rs", source), ("Cargo.toml", MANIFEST)],
        selected: "main.rs",
        target: "Packet",
        count: 1,
        refuse: false,
        blocked_preview: false,
        references: vec![("main.rs", source.rfind("Packet").unwrap(), 6)],
        preserved: &["data: [u8; 8 >> 1]"],
    })
    .await;
}

#[tokio::test]
async fn aggregate_repair_metadata_fields() {
    for (id, source, count) in [
        (
            "self-field",
            "struct Config { __self__: i32 }\nfn main() { let c = Config { __self__: 7 }; println!(\"{}\", c.__self__); }\n",
            1,
        ),
        (
            "args-field",
            "struct Config { __type_args__: i32 }\nfn main() { let c = Config { __type_args__: 7 }; println!(\"{}\", c.__type_args__); }\n",
            1,
        ),
        (
            "self-generic-fields",
            "struct Config<T> { __self__: T, __type_args__: i32 }\nimpl Config<i32> { fn make() -> Self { Self { __self__: 7, __type_args__: 8 } } }\nfn main() { let a = Config::make(); let b = Config::<i32> { __self__: 9, __type_args__: 10 }; println!(\"{} {} {} {}\", a.__self__, a.__type_args__, b.__self__, b.__type_args__); }\n",
            2,
        ),
    ] {
        check(Case {
            id,
            files: &[("main.rs", source), ("Cargo.toml", MANIFEST)],
            selected: "main.rs",
            target: "Config",
            count,
            refuse: false,
            blocked_preview: false,
            references: source
                .match_indices("Config")
                .skip(1)
                .map(|(at, _)| ("main.rs", at, 6))
                .chain(
                    source
                        .match_indices("Self")
                        .map(|(at, _)| ("main.rs", at, 4)),
                )
                .collect(),
            preserved: &[],
        })
        .await;
    }
}

#[tokio::test]
async fn aggregate_repair_invalid_field_parse_refuses() {
    let source = "struct Packet { data: [u8; 8 >> 1], tag: }\nfn main() { let _ = Packet { data: [0; 4], tag: 1 }; }\n";
    check(Case {
        id: "invalid-field",
        files: &[("main.rs", source), ("Cargo.toml", MANIFEST)],
        selected: "main.rs",
        target: "Packet",
        count: 0,
        refuse: true,
        blocked_preview: false,
        references: vec![("main.rs", source.rfind("Packet").unwrap(), 6)],
        preserved: &[],
    })
    .await;
}

#[tokio::test]
async fn aggregate_repair_unknown_qualified_owner_refuses() {
    let main = MAIN.replace("mod model;", "#[path=\"model.rs\"] mod model;");
    check(Case {
        id: "unknown-super",
        files: &[
            ("main.rs", &main),
            ("model.rs", MODEL),
            ("Cargo.toml", MANIFEST),
        ],
        selected: "model.rs",
        target: "Config",
        count: 1,
        refuse: true,
        blocked_preview: true,
        references: MODEL
            .match_indices("Config")
            .skip(1)
            .take(2)
            .map(|(at, _)| ("model.rs", at, 6))
            .collect(),
        preserved: &[],
    })
    .await;
}

#[tokio::test]
async fn aggregate_repair_unknown_owner_exact_reference_works() {
    let main = "#[path=\"model.rs\"] mod model;\npub struct Config { pub value: i32 }\nfn main() { println!(\"{}\", model::make().value); }\n";
    let model = "pub fn make() -> super::Config { super::Config { value: 7 } }\n";
    check(Case {
        id: "confirmed-super",
        files: &[
            ("main.rs", main),
            ("model.rs", model),
            ("Cargo.toml", MANIFEST),
        ],
        selected: "main.rs",
        target: "Config",
        count: 1,
        refuse: false,
        blocked_preview: false,
        references: model
            .match_indices("Config")
            .map(|(at, _)| ("model.rs", at, 6))
            .collect(),
        preserved: &[],
    })
    .await;
}
