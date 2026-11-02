/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn invalid_literal_verification_is_rejected_before_execution() {
    for args in [
        json!({"kind":"check","path":"."}),
        json!({"kind":"check","path":"./."}),
        json!({"kind":"check","path":" . "}),
        json!({"kind":"check","path":"src","filter":"unexpected"}),
        json!({"kind":"test","path":"src","filter":" "}),
        json!({"kind":"check","path":"src","timeout_secs":999}),
        json!({"kind":"check","path":"src","extra":true}),
    ] {
        let request = json!({"apply":true,"steps":[
            {"id":"write","tool":"apply"},
            {"id":"check","tool":"verify","args":args}
        ]});
        assert!(plan::Plan::parse(request).is_err(), "accepted {args}");
    }
}

#[tokio::test]
async fn invalid_literal_scope_is_rejected_before_earlier_apply() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("sample.py"), "old\n").unwrap();
    let preview = edits::preview(
        root,
        &[edits::BlockEdit {
            path: "sample.py".into(),
            old: "old".into(),
            new: "new".into(),
        }],
    )
    .unwrap();
    let mut paths = vec!["missing.py".to_string(), root.to_str().unwrap().to_string()];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root, root.join("root_alias")).unwrap();
        paths.push("root_alias".into());
    }
    for path in paths {
        let result = execute(
            "127.0.0.1:1".parse().unwrap(),
            root,
            json!({"apply":true,"steps":[
                {"id":"write","tool":"apply","args":{"files":preview.files}},
                {"id":"check","tool":"verify","args":{"kind":"check","path":path}}
            ]}),
        )
        .await;
        assert!(result.is_err(), "accepted scope {path}");
        assert_eq!(
            std::fs::read_to_string(root.join("sample.py")).unwrap(),
            "old\n"
        );
    }
}

#[tokio::test]
async fn missing_reference_and_call_budget_refuse_before_starting_apply() {
    for tool in ["apply", "verify"] {
        let args = if tool == "apply" {
            json!({"files":{"$ref":"read#/missing"}})
        } else {
            json!({"kind":"check","path":{"$ref":"read#/missing"}})
        };
        let plan = plan::Plan::parse(json!({"apply":true,"steps":[
            {"id":"read","tool":"code_hover"}, {"id":"change","tool":tool,"args":args}
        ]}))
        .unwrap();
        let report = runner::run(plan, |_, _| async { Ok(json!({})) }).await;
        assert_eq!(report["executed_calls"], 1);
        assert_eq!(
            report["steps"][1]["effects"]["status"], "refused",
            "{report}"
        );
        let field = if tool == "apply" { "writes" } else { "started" };
        assert_eq!(
            report["steps"][1]["effects"][field],
            if tool == "apply" {
                json!(0)
            } else {
                json!(false)
            }
        );
    }
    let mut steps = (0..4)
        .map(|i| json!({"id":format!("read{i}"),"tool":"code_hover","for_each":[1,2,3,4,5,6,7,8]}))
        .collect::<Vec<_>>();
    steps.push(json!({"id":"write","tool":"apply"}));
    let plan = plan::Plan::parse(json!({"apply":true,"steps":steps})).unwrap();
    let report = runner::run(plan, |_, _| async { Ok(json!({})) }).await;
    assert_eq!(report["executed_calls"], 32);
    assert_eq!(report["steps"][4]["effects"]["writes"], 0, "{report}");
}

#[tokio::test]
async fn dynamic_external_verification_scope_is_a_pre_start_refusal() {
    let temp = tempfile::tempdir().unwrap();
    let remote = "127.0.0.1:1".parse().unwrap();
    let plan = plan::Plan::parse(json!({"apply":true,"steps":[
        {"id":"read","tool":"code_hover"},
        {"id":"check","tool":"verify","args":{"kind":"check","path":{"$ref":"read#/path"}}}
    ]}))
    .unwrap();
    let root = temp.path();
    let report = runner::run(plan, |tool, args| async move {
        if tool == "code_hover" {
            Ok(json!({"path":"../outside"}))
        } else {
            invoke(remote, root, &tool, args).await
        }
    })
    .await;
    assert_eq!(
        report["steps"][1]["effects"]["status"], "refused",
        "{report}"
    );
    assert_eq!(report["steps"][1]["effects"]["started"], false);
}
