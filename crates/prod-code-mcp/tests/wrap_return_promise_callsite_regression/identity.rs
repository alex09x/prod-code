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

#[tokio::test]
async fn empty_references_do_not_prove_an_unknown_same_file_member() {
    let source = "export function retry(): number { return 7; }\nexport async function run(obj: any) {\n    return obj.retry();\n}\n";
    let ws = workspace(source, "ts");
    let (gw, diagnostics) = gateway(&ws, source, &[], "ts").await;
    let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
    eprintln!(
        "empty-reference member plan: propagated={} unmatched={:?}",
        plan.propagated, plan.unmatched
    );
    let write = tool(gw.addr(), &ws, "ts", true, false).await;
    eprintln!(
        "empty-reference member apply: refused={}, unchanged={}; {}",
        write.is_err(),
        ws.read("selected.ts") == source,
        ws.read("selected.ts")
    );
    assert!(
        write.is_err(),
        "an ambiguous receiver must prevent tool application"
    );
    assert_eq!(ws.read("selected.ts"), source);
    assert_eq!(
        plan.propagated, 0,
        "unproven receiver must not be rewritten"
    );
    assert!(
        plan.unmatched
            .iter()
            .any(|s| s.contains("selected.ts:3:") && s.contains("receiver")),
        "{plan:?}"
    );
    assert!(!planned(&plan, "ts").contains("await obj.retry"));
    for force in [false, true] {
        let error = wrap(gw.addr(), &ws, "ts", true, force).await.unwrap_err();
        assert!(
            error.to_string().contains("nothing was written"),
            "{error:#}"
        );
        let error = tool(gw.addr(), &ws, "ts", true, force).await.unwrap_err();
        assert!(
            error.contains("not rewritten") || error.contains("incomplete"),
            "{error}"
        );
        assert_eq!(ws.read("selected.ts"), source);
    }
    validated(&diagnostics);
}

#[tokio::test]
async fn lexical_same_file_and_namespace_callers_survive_unrelated_and_shadowed_calls() {
    let source = "import * as own from './selected';\nexport function retry(): number { return 7; }\nexport async function run() {\n    const a = retry();\n    const b = own.retry();\n    return a + b;\n}\nexport function shadow(retry: () => number) {\n    return retry();\n}\n";
    let ws = workspace(source, "ts");
    std::fs::write(
        ws.root().join("other.ts"),
        "export function retry() { return 99; }\n",
    )
    .unwrap();
    let unrelated =
        "import * as other from './other';\nexport function run() { return other.retry(); }\n";
    std::fs::write(ws.root().join("unrelated.ts"), unrelated).unwrap();
    let shadowed = "import * as own from './selected';\nexport function run(own: any) {\n    return own.retry();\n}\n";
    std::fs::write(ws.root().join("shadowed.ts"), shadowed).unwrap();
    let (gw, diagnostics) = gateway(&ws, source, &[], "ts").await;
    let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
    assert_eq!(plan.propagated, 2, "{plan:?}");
    assert!(
        plan.unmatched.is_empty() && plan.blocked.is_empty(),
        "{plan:?}"
    );
    assert_eq!(ws.read("selected.ts"), source);
    tool(gw.addr(), &ws, "ts", true, false).await.unwrap();
    let after = ws.read("selected.ts");
    assert!(after.contains("const a = await retry();"), "{after}");
    assert!(after.contains("const b = await own.retry();"), "{after}");
    assert!(after.contains("return retry();\n}"), "{after}");
    assert_eq!(ws.read("unrelated.ts"), unrelated);
    assert_eq!(ws.read("shadowed.ts"), shadowed);
    validated(&diagnostics);
}

#[tokio::test]
async fn nonempty_semantic_references_prove_selected_member_and_exclude_other_members() {
    let source = "export function retry(): number { return 7; }\nexport async function run(obj: any, other: any) {\n    const value = obj.retry(1);\n    return other.retry(2) + value;\n}\n";
    let ws = workspace(source, "ts");
    let (gw, diagnostics) = gateway(&ws, source, &["retry(1)"], "ts").await;
    let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
    assert_eq!(plan.propagated, 1);
    assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
    tool(gw.addr(), &ws, "ts", true, false).await.unwrap();
    let after = ws.read("selected.ts");
    assert!(after.contains("await obj.retry(1)"), "{after}");
    assert!(after.contains("return other.retry(2) + value"), "{after}");
    validated(&diagnostics);
}

#[tokio::test]
async fn unmatched_semantic_evidence_is_never_suppressed() {
    let source = "export function retry(): number { return 7; }\nexport const alias = retry;\nexport async function run() { return alias(); }\n";
    let ws = workspace(source, "ts");
    let (gw, diagnostics) = gateway(&ws, source, &["retry;"], "ts").await;
    let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
    assert!(!plan.unmatched.is_empty());
    assert!(tool(gw.addr(), &ws, "ts", true, true).await.is_err());
    assert_eq!(ws.read("selected.ts"), source);
    validated(&diagnostics);
}

#[tokio::test]
async fn a_named_import_alias_is_not_proof_of_a_namespace_receiver() {
    let source = "import { holder as own } from './selected';\nexport function retry(): number { return 7; }\nexport const holder = { retry };\nexport async function run() {\n    return own.retry();\n}\n";
    let ws = workspace(source, "ts");
    let (gw, diagnostics) = gateway(&ws, source, &[], "ts").await;
    let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
    assert_eq!(plan.propagated, 0);
    assert!(
        plan.unmatched.iter().any(|s| s.contains("receiver")),
        "{plan:?}"
    );
    assert!(tool(gw.addr(), &ws, "ts", true, true).await.is_err());
    assert_eq!(ws.read("selected.ts"), source);
    validated(&diagnostics);
}
