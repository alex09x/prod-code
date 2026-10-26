/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::check;
use prod_code_mcp::replace_constructor::ReplaceMode::{Builder, Factory};

const GLOB_OWNER: &str = r#"struct Config { value: i32 }
mod inner {
    #[derive(Default)] pub struct Config { pub value: i32 }
    impl Config {
        pub fn new(value: i32) -> Self {
            let mut result = Self::default(); result.value = value + 10; result
        }
    }
}
fn main() {
    let inner = {
        use inner::*;
        impl Config { fn make() -> Self { Self { value: 2 } } }
        Config::make().value
    };
    let outer = Config { value: 1 };
    println!("{} {}", outer.value, inner);
}
"#;

#[tokio::test]
async fn glob_imported_owner_factory() {
    check(
        "glob-imported-owner-factory",
        "rs",
        "Config",
        GLOB_OWNER,
        Factory,
        1,
        &["Self { value: 2 }", "result.value = value + 10"],
    )
    .await;
}

#[tokio::test]
async fn glob_imported_owner_builder() {
    check(
        "glob-imported-owner-builder",
        "rs",
        "Config",
        GLOB_OWNER,
        Builder,
        1,
        &["Self { value: 2 }", "result.value = value + 10"],
    )
    .await;
}

#[tokio::test]
async fn glob_grouped_and_reexported_owners() {
    for (id, prefix, import) in [
        ("grouped", "", "use crate::{inner::*};"),
        (
            "reexported",
            "mod reexport { pub use crate::inner::*; }\n",
            "use reexport::*;",
        ),
        (
            "aliased-module",
            "",
            "use crate::inner as module; use module::*;",
        ),
    ] {
        let source = format!("{prefix}{GLOB_OWNER}").replace("use inner::*;", import);
        for mode in [Factory, Builder] {
            check(
                &format!("glob-{id}-owner-{mode}"),
                "rs",
                "Config",
                &source,
                mode,
                1,
                &["Self { value: 2 }", "result.value = value + 10"],
            )
            .await;
        }
    }
}

#[tokio::test]
async fn glob_selected_owners_and_shadowing() {
    for (id, prefix, import) in [
        ("explicit", "", "use inner::*; use crate::Config;"),
        (
            "irrelevant",
            "mod irrelevant { pub struct Other; }\n",
            "use irrelevant::*;",
        ),
        (
            "selected",
            "mod selected { pub use crate::Config; }\n",
            "use selected::*;",
        ),
        ("crate", "", "use crate::*;"),
        (
            "duplicate",
            "mod selected { pub use crate::Config; }\nmod second { pub use crate::Config; }\n",
            "use selected::*; use second::*;",
        ),
        (
            "cyclic",
            "mod a { pub use crate::b::*; }\nmod b { pub use crate::a::*; pub use crate::Config; }\n",
            "use a::*;",
        ),
    ] {
        // Reexporting the selected owner requires public visibility in Rust.
        let owner = GLOB_OWNER.replacen("struct Config", "pub struct Config", 1);
        let source = format!("{prefix}{owner}").replace("use inner::*;", import);
        for mode in [Factory, Builder] {
            check(
                &format!("glob-{id}-owner-{mode}"),
                "rs",
                "Config",
                &source,
                mode,
                2,
                &["result.value = value + 10"],
            )
            .await;
        }
    }
}

#[tokio::test]
async fn glob_shadowed_by_local_owner() {
    let source = GLOB_OWNER.replace(
        "use inner::*;",
        "use inner::*;\n        struct Config { value: i32 }",
    );
    for mode in [Factory, Builder] {
        check(
            &format!("glob-local-owner-{mode}"),
            "rs",
            "Config",
            &source,
            mode,
            1,
            &["Self { value: 2 }", "result.value = value + 10"],
        )
        .await;
    }
}

#[tokio::test]
async fn glob_unavailable_owner_refuses_partial_application() {
    use super::support::{bytes, record, run, transform};
    use prod_code_testkit::Workspace;
    use serde_json::json;

    let source = "struct Config { value: i32 }\nmod inner;\nfn main() {\n let inner = {\n  use inner::*;\n  impl Config { fn make() -> Self { Self { value: 2 } } }\n  Config::make().value\n };\n let outer = Config { value: 1 };\n println!(\"{} {}\", outer.value, inner);\n}\n";
    let inner = "pub struct Config { pub value: i32 }\nimpl Config { pub fn new(value: i32) -> Self { let mut result = Self { value: 0 }; result.value = value + 10; result } }\n";
    for mode in [Factory, Builder] {
        let ws = Workspace::new(&[("main.rs", source), ("inner.rs", inner)]);
        let compiler = run(
            &ws.root(),
            &["rustc", "--edition=2021", "main.rs", "-o", "program"],
        );
        let runtime = run(&ws.root(), &["./program"]);
        let preview = transform(&ws, "rs", "Config", mode, false).await.unwrap();
        let result = transform(&ws, "rs", "Config", mode, true).await;
        let after = std::fs::read_to_string(ws.root().join("main.rs")).unwrap();
        let after_inner = std::fs::read_to_string(ws.root().join("inner.rs")).unwrap();
        let at = source.rfind("Config").unwrap();
        let error = result
            .err()
            .map(|e| format!("{e:#}").replace(&*ws.root().to_string_lossy(), "<fixture>"));
        record(
            &format!("glob-unavailable-owner-{mode}"),
            &json!({
                "input":bytes(&ws.root(), source), "output":bytes(&ws.root(), &after),
                "input_inner":bytes(&ws.root(), inner), "output_inner":bytes(&ws.root(), &after_inner),
                "reference_fixture":{"file":"main.rs","line":source[..at].bytes().filter(|b| *b == b'\n').count(),
                    "character":source[..at].rsplit('\n').next().unwrap().encode_utf16().count(),"length":6},
                "scripted_transport":"exact selected outer reference, diagnostic sentinel, force=true; not live analyzer certification",
                "compiler":compiler,"runtime":runtime,
                "preview":{"count":preview.instantiations_rewritten,"blocked":preview.blocked},
                "apply_error":error
            }),
        );
        assert_eq!(compiler["exit"], 0, "{compiler}");
        assert_eq!(runtime["exit"], 0, "{runtime}");
        assert_eq!(runtime["stdout"], "1 2\n");
        assert_eq!(preview.instantiations_rewritten, 1);
        assert_eq!(preview.blocked.len(), 1, "{preview:?}");
        assert!(
            error
                .unwrap()
                .contains("cannot resolve Rust constructor owner")
        );
        assert_eq!(after, source);
        assert_eq!(after_inner, inner);
    }
}
