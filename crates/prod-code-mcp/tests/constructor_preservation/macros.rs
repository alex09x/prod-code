/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{bytes, record, run, transform};
use prod_code_mcp::replace_constructor::ReplaceMode::{self, Builder, Factory};
use prod_code_testkit::Workspace;
use serde_json::json;

const SOURCE: &str = "struct Config { value: i32 }\nfn main() { let xs = vec![Config { value: 1 }]; println!(\"{}\", xs[0].value); }\n";

async fn refuse_macro(id: &str, source: &str, mode: ReplaceMode, expected: &str, count: usize) {
    let ws = Workspace::new(&[
        ("main.rs", source),
        (
            "Cargo.toml",
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        ),
    ]);
    let compiler = run(
        &ws.root(),
        &["rustc", "--edition=2021", "main.rs", "-o", "program"],
    );
    let runtime = run(&ws.root(), &["./program"]);
    let preview = transform(&ws, "rs", "Config", mode, false).await.unwrap();
    let result = transform(&ws, "rs", "Config", mode, true).await;
    let after = std::fs::read_to_string(ws.root().join("main.rs")).unwrap();
    let error = result.as_ref().err().map(|e| format!("{e:#}"));
    let at = source.rfind("Config").unwrap();
    record(
        &format!("{id}-{mode}"),
        &json!({
            "input": bytes(&ws.root(), source), "output": bytes(&ws.root(), &after),
            "reference_fixture": {"line":source[..at].bytes().filter(|b| *b == b'\n').count(),
                "character":source[..at].rsplit('\n').next().unwrap().encode_utf16().count(),"length":6},
            "scripted_transport":"exact reference, diagnostic sentinel and force=true; not live analyzer certification",
            "compiler":compiler, "runtime":runtime,
            "preview": {"count":preview.instantiations_rewritten,"blocked":preview.blocked},
            "apply_error":error
        }),
    );
    assert_eq!(compiler["exit"], 0, "{compiler}");
    assert_eq!(runtime["exit"], 0, "{runtime}");
    assert_eq!(runtime["stdout"], expected);
    assert_eq!(preview.instantiations_rewritten, count);
    assert_eq!(preview.blocked.len(), 1, "{preview:?}");
    assert!(error.unwrap().contains("opaque macro arguments"));
    assert_eq!(after, source, "refusal must leave the file byte-identical");
}

#[tokio::test]
async fn macro_constructor_factory() {
    refuse_macro("macro-opaque", SOURCE, Factory, "1\n", 0).await;
}

#[tokio::test]
async fn macro_constructor_builder() {
    refuse_macro("macro-opaque", SOURCE, Builder, "1\n", 0).await;
}

#[tokio::test]
async fn macro_selected_self_refuses_partial_application() {
    let source = "struct Config { value: i32 }\nimpl Config { fn make() -> Self { let _ = Self { value: 0 }; let xs = vec![Self { value: 2 }]; xs.into_iter().next().unwrap() } }\nfn main() { let x = Config::make(); println!(\"{}\", x.value); }\n";
    for mode in [Factory, Builder] {
        refuse_macro("macro-self-partial", source, mode, "2\n", 1).await;
    }
}

#[tokio::test]
async fn macro_qualified_selected_owner() {
    let source = "struct Config { value: i32 }\nmod inner { pub struct Config; pub fn make() -> super::Config { let xs = vec![super::Config { value: 4 }]; xs.into_iter().next().unwrap() } }\nfn main() { let x = inner::make(); println!(\"{}\", x.value); }\n";
    for mode in [Factory, Builder] {
        refuse_macro("macro-qualified", source, mode, "4\n", 0).await;
    }
}

#[tokio::test]
async fn macro_definition_selected_owner() {
    let source = "struct Config { value: i32 }\nmacro_rules! make { () => { Config { value: 3 } } }\nfn main() { let x = make!(); println!(\"{}\", x.value); }\n";
    for mode in [Factory, Builder] {
        refuse_macro("macro-definition", source, mode, "3\n", 0).await;
    }
}

#[tokio::test]
async fn macro_generic_constructor_is_opaque() {
    let source = "struct Config<'a, T, const N: usize> { value: &'a T, bytes: [u8; N] }\nfn main() { let x = 7u8; let π = vec![Config::<u8, { 1 << 1 }> { value: &x, bytes: [1, 2] }]; println!(\"{} {}\", π[0].value, π[0].bytes.len()); }\n";
    for mode in [Factory, Builder] {
        refuse_macro("macro-generics", source, mode, "7 2\n", 0).await;
    }
}

#[tokio::test]
async fn macro_unrelated_owner_and_literal_data_are_preserved() {
    let source = r#"struct Config { value: i32 }
mod inner {
    pub struct Config { pub value: i32 }
    impl Config {
        pub fn make() -> Self { let xs = vec![Self { value: 2 }]; xs.into_iter().next().unwrap() }
    }
}
fn main() {
    let selected = Config { value: 1 };
    let xs = vec![inner::Config { value: 3 }];
    println!("Config {{ value: 99 }} {} {} {}", /* Config /* nested */ */ selected.value, inner::Config::make().value, xs[0].value);
    let _: Option<Config> = None;
}
"#;
    for mode in [Factory, Builder] {
        super::support::check(
            &format!("macro-unrelated-{mode}"),
            "rs",
            "Config",
            source,
            mode,
            1,
            &[
                "vec![Self { value: 2 }]",
                "vec![inner::Config { value: 3 }]",
                "/* Config /* nested */ */",
            ],
        )
        .await;
    }
}
