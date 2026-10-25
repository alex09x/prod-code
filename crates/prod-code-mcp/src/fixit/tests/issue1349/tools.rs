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

fn python() -> Workspace {
    Workspace::new(&[
        (
            "pyproject.toml",
            "[project]\nname='fixture'\nversion='0.1.0'\n",
        ),
        ("main.py", "value = 1\n"),
    ])
}

async fn failed_fixer(case: &str, changed: bool) {
    let ws = python();
    let mut failed = Step {
        changed,
        ..Step::green()
    };
    failed.stdout = format!("scripted {case} failure\n");
    let expected = match case {
        "start" => {
            failed.code = None;
            failed.error = Some("scripted spawn refusal".into());
            "failed to start"
        }
        "timeout" => {
            failed.timed_out = true;
            "timed out"
        }
        "exit" => {
            failed.code = Some(2);
            "exit"
        }
        "unknown" => {
            failed.code = None;
            "exit"
        }
        _ => unreachable!(),
    };
    let gateway = Gateway::start(vec![Step::green(), failed, Step::green()]).await;
    let result = check_and_fix(gateway.addr, &ws.root(), None, VerifyKind::Lint, 10).await;
    let requests = gateway.finish().await;
    println!(
        "fixer protocol: case={case}, changed={changed}, result={:?}, bytes={:?}, requests={}",
        result
            .as_ref()
            .map(|r| (r.ok(), r.render(10)))
            .map_err(|e| e.to_string()),
        ws.read("main.py"),
        requests.len()
    );
    let error = result
        .expect_err("a green initial lint cannot mask a failed requested fixer")
        .to_string();
    assert!(error.contains(expected), "{error}");
    assert!(error.contains("ruff"), "{error}");
    assert!(
        error.contains(&format!("scripted {case} failure")),
        "{error}"
    );
    assert_eq!(
        requests.len(),
        2,
        "do not hide fixer failure with a later green lint"
    );
    assert_eq!(
        requests[1].command,
        ["ruff", "check", ".", "--fix", "--output-format", "concise"]
    );
    assert!(requests[1].pull_changes);
    if changed {
        assert!(
            error.contains("main.py") && error.contains("1"),
            "report pulled partial changes: {error}"
        );
        assert_eq!(ws.read("main.py").as_bytes(), b"value = 2\n");
    } else {
        assert_eq!(ws.read("main.py").as_bytes(), b"value = 1\n");
    }
}

#[tokio::test]
async fn issue1349_fixer_start_failure() {
    failed_fixer("start", false).await;
}
#[tokio::test]
async fn issue1349_fixer_timeout() {
    failed_fixer("timeout", false).await;
}
#[tokio::test]
async fn issue1349_fixer_nonzero_exit() {
    failed_fixer("exit", false).await;
}
#[tokio::test]
async fn issue1349_fixer_missing_exit() {
    failed_fixer("unknown", false).await;
}
#[tokio::test]
async fn issue1349_fixer_partial_start_failure() {
    failed_fixer("start", true).await;
}
#[tokio::test]
async fn issue1349_fixer_partial_timeout() {
    failed_fixer("timeout", true).await;
}
#[tokio::test]
async fn issue1349_fixer_partial_nonzero_exit() {
    failed_fixer("exit", true).await;
}
#[tokio::test]
async fn issue1349_fixer_partial_missing_exit() {
    failed_fixer("unknown", true).await;
}

#[tokio::test]
async fn issue1349_successful_fixer_with_and_without_changes() {
    for (changed, initial) in [(false, 0), (true, 0), (false, 1), (true, 1)] {
        let ws = python();
        let gateway = Gateway::start(vec![
            Step {
                code: Some(initial),
                ..Step::green()
            },
            Step {
                changed,
                ..Step::green()
            },
            Step::green(),
        ])
        .await;
        let result = check_and_fix(gateway.addr, &ws.root(), None, VerifyKind::Lint, 10).await;
        let requests = gateway.finish().await;
        let fixed = result.unwrap();
        assert_eq!(fixed.ok(), changed || initial == 0);
        assert_eq!(requests.len(), if changed { 3 } else { 2 });
        assert_eq!(fixed.outcomes.len(), usize::from(changed));
        assert_eq!(fixed.after.is_some(), changed);
        assert_eq!(
            ws.read("main.py").as_bytes(),
            if changed {
                b"value = 2\n"
            } else {
                b"value = 1\n"
            }
        );
    }
}

#[tokio::test]
async fn issue1349_no_fix_mode_and_non_lint_preserve_initial_report() {
    let ws = Workspace::new(&[
        ("go.mod", "module example.invalid/fixture\n\ngo 1.23\n"),
        ("main.go", "package fixture\n"),
    ]);
    for (kind, initial) in [
        (VerifyKind::Lint, 0),
        (VerifyKind::Check, 0),
        (VerifyKind::Lint, 1),
        (VerifyKind::Check, 1),
    ] {
        let gateway = Gateway::start(vec![Step {
            code: Some(initial),
            ..Step::green()
        }])
        .await;
        let result = check_and_fix(gateway.addr, &ws.root(), None, kind, 10).await;
        let requests = gateway.finish().await;
        let fixed = result.unwrap();
        assert_eq!(fixed.ok(), initial == 0);
        assert_eq!(requests.len(), 1);
        assert!(fixed.after.is_none() && fixed.outcomes.is_empty());
        assert!(
            fixed
                .note
                .as_ref()
                .unwrap()
                .contains(if kind == VerifyKind::Lint {
                    "no fix mode"
                } else {
                    "only `lint --fix`"
                })
        );
        assert_eq!(ws.read("main.go").as_bytes(), b"package fixture\n");
    }
}
