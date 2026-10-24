/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::Workspace;
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;

// TABLE against real rust-analyzer. All rows share one loaded fixture project:
// files | changed file | exact CLI result
// 14    | none         | exit 0, all 14 files clean (reported batch size)
// 3     | file1.rs     | exit 1, only file1.rs has E0308
// 14    | file13.rs    | exit 1, only file13.rs has E0308
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running gateway"]
async fn validate_live_rust_batch_table() {
    let remote: SocketAddr = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("live gateway prerequisite")
        .parse()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(170), live_table(remote))
        .await
        .expect("shared fixture table must finish within three minutes");
}

async fn live_table(remote: SocketAddr) {
    let ws = Workspace::new(&[(
        "Cargo.toml",
        "[package]\nname='batch_fixture'\nversion='0.1.0'\nedition='2021'\n",
    )]);
    let home = tempfile::tempdir().unwrap();
    let proposals = tempfile::tempdir().unwrap();
    let mut lib = String::new();
    for i in 0..14 {
        lib.push_str(&format!("pub mod file{i};\n"));
        ws.write(&format!("src/file{i}.rs"), "pub fn value() -> u32 { 1 }\n");
    }
    ws.write("src/lib.rs", &lib);
    ws.commit();
    for (count, broken) in [(14, None), (3, Some(1)), (14, Some(13))] {
        let mut args = vec![
            "validate".to_string(),
            "src/file0.rs".into(),
            "--from".into(),
        ];
        for i in 0..count {
            let proposal = proposals.path().join(format!("file{i}.rs"));
            std::fs::write(
                &proposal,
                if Some(i) == broken {
                    "pub fn value() -> u32 { \"wrong type\" }\n"
                } else {
                    "pub fn value() -> u32 { 2 }\n"
                },
            )
            .unwrap();
            if i == 0 {
                args.push(proposal.to_string_lossy().into_owned());
            } else {
                args.extend([
                    "--with".into(),
                    format!("src/file{i}.rs={}", proposal.display()),
                ]);
            }
        }
        args.push("--json".into());
        let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
            .arg("--remote")
            .arg(remote.to_string())
            .args(args)
            .env("PROD_CODE_REMOTE", remote.to_string())
            .env("HOME", home.path())
            .current_dir(ws.root())
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if broken.is_some() { 1 } else { 0 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reports: Vec<Value> = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)));
        assert_eq!(reports.len(), count);
        for (i, report) in reports.iter().enumerate() {
            assert_eq!(report["file"], format!("src/file{i}.rs"));
            assert_eq!(
                report["errors"].as_u64(),
                Some(u64::from(Some(i) == broken)),
                "{report}"
            );
            if Some(i) == broken {
                assert_eq!(report["items"][0]["code"], "E0308", "{report}");
            } else {
                assert_eq!(report["items"], serde_json::json!([]), "{report}");
            }
        }
    }
}
