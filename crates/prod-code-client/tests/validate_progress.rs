/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

// TABLE: reported size and nearest variants. Each slow row withholds one file's
// proposal diagnostic; the last row also withholds that file's baseline diagnostic.
// files | missing | exact result                         | progress
// 14    | 7       | 13 checked, file7.rs unchecked, exit 1 | 10 s then every 15 s
// 3     | 0       | 2 checked, file0.rs unchecked, exit 1  | 0/3 done
// 3     | 1       | 2 checked, file1.rs unchecked, exit 1  | 1/3 done
// 3     | 2       | 2 checked, file2.rs unchecked, exit 1  | 2/3 done
// 3     | none    | 3 checked, exit 0                     | none
// 3     | 1, also baseline | 2 checked, file1.rs unchecked | 0/3 during baseline
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validate_progress_and_partial_results_table() {
    let cases = [
        (14, Some(7), false),
        (3, Some(0), false),
        (3, Some(1), false),
        (3, Some(2), false),
        (3, None, false),
        (3, Some(1), true),
    ];
    let results = futures_util::future::join_all(
        cases
            .into_iter()
            .map(|(count, missing, baseline)| validate_case(count, missing, baseline)),
    )
    .await;
    for result in results {
        result.unwrap();
    }
}

async fn validate_case(count: usize, missing: Option<usize>, baseline: bool) -> anyhow::Result<()> {
    let ws = Workspace::new(&[(
        "Cargo.toml",
        "[package]\nname='batch_fixture'\nversion='0.1.0'\nedition='2021'\n",
    )]);
    let proposals = tempfile::tempdir()?;
    let home = tempfile::tempdir()?;
    let opened = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let gateway = ScriptedGateway::start(move |method, params| {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        if method == "textDocument/didOpen"
            && params["textDocument"]["text"]
                .as_str()
                .is_some_and(|s| s.contains("proposed"))
        {
            opened.lock().unwrap().insert(uri.to_string());
        }
        if method == "textDocument/diagnostic" {
            if missing.is_some_and(|i| uri.ends_with(&format!("/file{i}.rs")))
                && (baseline || opened.lock().unwrap().contains(uri))
            {
                return json!({"prod-code/no-response": true});
            }
            return answers::no_diagnostics();
        }
        json!([])
    })
    .await;
    let mut args = vec!["validate".to_string(), "file0.rs".into(), "--from".into()];
    for i in 0..count {
        let name = format!("file{i}.rs");
        ws.write(&name, "pub fn original() {}\n");
        let proposal = proposals.path().join(&name);
        std::fs::write(&proposal, "pub fn proposed() {}\n")?;
        if i == 0 {
            args.push(proposal.to_string_lossy().into_owned());
        } else {
            args.extend(["--with".into(), format!("{name}={}", proposal.display())]);
        }
    }
    args.push("--json".into());
    let started = Instant::now();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(gateway.addr().to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", gateway.addr().to_string())
        .env("HOME", home.path())
        .current_dir(ws.root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut first = String::new();
    if missing.is_some() {
        let line =
            tokio::time::timeout(Duration::from_secs(14), stderr.read_line(&mut first)).await;
        if line.is_err() {
            child.kill().await?;
            let output = child.wait_with_output().await?;
            anyhow::bail!(
                "{count}-file batch: no progress within 14s; stdout={} bytes, stderr={} bytes, interrupted={}",
                output.stdout.len(),
                first.len(),
                output.status
            );
        }
        line??;
        assert!(started.elapsed() >= Duration::from_secs(9), "{first}");
        assert!(first.contains("[prod-code validate]"), "{first}");
        assert!(first.contains("textDocument/diagnostic"), "{first}");
        let done = if baseline { 0 } else { missing.unwrap() };
        assert!(
            first.contains(&format!("{done}/{count} files done")),
            "{first}"
        );
    }
    let mut rest = String::new();
    let (output, read) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(115), child.wait_with_output()),
        stderr.read_to_string(&mut rest),
    );
    read?;
    let output = output??;
    assert!(started.elapsed() < Duration::from_secs(123));
    assert_eq!(
        output.status.code(),
        Some(if missing.is_some() { 1 } else { 0 })
    );
    let reports: Vec<Value> = serde_json::from_slice(&output.stdout)?;
    assert_eq!(reports.len(), count);
    for (i, report) in reports.iter().enumerate() {
        assert_eq!(report["file"], format!("file{i}.rs"));
        if Some(i) == missing {
            assert_eq!(
                report["items"][0]["code"],
                "prod-code-validation-incomplete"
            );
            let message = report["items"][0]["message"].as_str().unwrap();
            assert!(
                message.contains(&format!("file{i}.rs was not validated")),
                "{message}"
            );
            assert!(message.contains("retry this file"), "{message}");
        } else {
            assert_eq!(report["items"], json!([]));
        }
    }
    let progress = format!("{first}{rest}");
    if missing.is_some() {
        let expected = if baseline { 6 } else { 4 };
        assert_eq!(
            progress.lines().count(),
            expected,
            "expected reports at 10s then every 15s: {progress}"
        );
    } else {
        assert!(
            progress.is_empty(),
            "fast batch printed progress: {progress}"
        );
    }
    Ok(())
}
