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
use std::process::Stdio;

#[tokio::test]
async fn stderr_tail_caps_lines_and_preserves_final_partial_line() {
    let mut child = tokio::process::Command::new("python3")
        .args(["-c", "import sys; sys.stderr.write(''.join(str(i)+'\\n' for i in range(35)) + 'x'*600 + '\\nfinal é')"])
        .stderr(Stdio::piped()).spawn().unwrap();
    let tail = Arc::new(StdMutex::new(VecDeque::new()));
    let (sender, mut done) = tokio::sync::watch::channel(false);
    spawn_stderr_reader(child.stderr.take().unwrap(), tail.clone(), sender);
    tokio::time::timeout(std::time::Duration::from_secs(2), done.wait_for(|v| *v))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child.wait().await.unwrap().code(), Some(0));
    let expected: VecDeque<_> = (7..35)
        .map(|i| i.to_string())
        .chain(["x".repeat(512), "final é".into()])
        .collect();
    assert_eq!(*tail.lock().unwrap(), expected);
}

#[tokio::test]
async fn writer_refuses_dead_process_before_sending() {
    let mut child = tokio::process::Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let writer = Arc::new(Mutex::new(child.stdin.take().unwrap()));
    let child = Arc::new(StdMutex::new(child));
    let alive = Arc::new(AtomicBool::new(false));
    let pending = Arc::new(StdMutex::new(HashMap::new()));
    let error = write_frame_until(
        &writer,
        &serde_json::json!({}),
        tokio::time::Instant::now() + std::time::Duration::from_secs(1),
        "test",
        &Arc::downgrade(&child),
        &Arc::downgrade(&pending),
        &Arc::downgrade(&alive),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Language server process has exited before message 'test'"
    );
    let mut process = child.lock().unwrap();
    process.start_kill().unwrap();
    drop(process);
    let mut process = Arc::try_unwrap(child).ok().unwrap().into_inner().unwrap();
    process.wait().await.unwrap();
}
