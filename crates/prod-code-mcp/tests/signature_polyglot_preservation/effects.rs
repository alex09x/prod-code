/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::Workspace;

#[tokio::test]
async fn refuses_numeric_prefix_effects_even_with_force() {
    refuses("function f(a,b) { return [a,b]; }\nlet i=0; console.log(JSON.stringify(f(1+i++,1+i++)));\n",
        vec![Param::Keep("b".into()), Param::Keep("a".into())]).await;
}

#[tokio::test]
async fn refuses_quoted_concatenation_effects_even_with_force() {
    refuses("function f(a,b) { return a; }\nfunction log() { console.log('effect'); return ''; }\nconsole.log(f(1,\"a\" + log() + \"b\"));\n",
        vec![Param::Keep("a".into())]).await;
}

async fn refuses(source: &str, params: Vec<Param>) {
    for force in [false, true] {
        let ws = Workspace::new(&[("main.js", source)]);
        let root = &ws.root();
        evidence(root, "main.js", "original");
        let original = run(root, "node", &["main.js"]);
        let call = if source.contains("i++") {
            "f(1+i++"
        } else {
            "f(1,"
        };
        let gw = gateway(vec![location(root, "main.js", source, call)]).await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.js"),
            1,
            10,
            &params,
            &Modifiers::default(),
            true,
            force,
        )
        .await;
        evidence(root, "main.js", "generated");
        let generated = run(root, "node", &["main.js"]);
        assert_eq!(generated, original, "runtime effects/values changed");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("nothing was written")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("main.js")).unwrap(),
            source
        );
    }
}
