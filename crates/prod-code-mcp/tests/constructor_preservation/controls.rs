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
use prod_code_mcp::replace_constructor::ReplaceMode::{Builder, Factory};
use prod_code_testkit::Workspace;
use serde_json::json;
use std::fs;

#[tokio::test]
async fn refused_grammar_never_writes_even_with_force() {
    for (id, ext, target, source, argv) in [
        (
            "rust-update",
            "rs",
            "Config",
            "struct Config {\n value: i32,\n}\nfn main() { let old = Config { value: 1 }; let new = Config { ..old }; println!(\"{}\", new.value); }\n",
            vec!["rustc", "--edition=2021", "main.rs"],
        ),
        (
            "js-destructure",
            "mjs",
            "Config",
            "class Config { constructor({value}) { this.value = value; } }\nconst c = new Config({value: 1}); console.log(c.value);\n",
            vec!["node", "main.mjs"],
        ),
        (
            "python-positional-only",
            "py",
            "Config",
            "class Config:\n    def __init__(self, value, /):\n        self.value = value\nc = Config(1)\nprint(c.value)\n",
            vec!["python3", "main.py"],
        ),
    ] {
        for mode in [Factory, Builder] {
            let filename = format!("main.{ext}");
            let ws = Workspace::new(&[
                (filename.as_str(), source),
                (
                    "Cargo.toml",
                    "[package]\nname=\"fixture\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
                ),
            ]);
            let before = run(&ws.root(), &argv);
            let result = transform(&ws, ext, target, mode, true).await;
            let after = fs::read_to_string(ws.root().join(filename)).unwrap();
            record(
                &format!("refusal-{id}-{mode}"),
                &json!({"input":bytes(&ws.root(),source), "output":bytes(&ws.root(),&after),
                "before_execution":before,"public_error":result.as_ref().err().map(|e| format!("{e:#}")),
                "unchanged":source == after}),
            );
            assert_eq!(before["exit"], 0, "{before}");
            assert!(
                result.is_err(),
                "unknown grammar was silently accepted: {id}"
            );
            assert_eq!(source, after, "refused call wrote {id}");
        }
    }
}
