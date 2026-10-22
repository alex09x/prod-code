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

async fn exec_fixture(
    command: &str,
    seconds: u64,
    structured: bool,
) -> (
    tempfile::TempDir,
    Framed<tokio::net::TcpStream, ProdCodeCodec>,
    tokio::task::JoinHandle<Result<ExecCompletion>>,
) {
    use std::os::unix::process::CommandExt;
    let workspace = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let child = std::process::Command::new("sh")
        .args(["-c", command])
        .process_group(0)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let running = RunningEntry::start(workspace.path(), &[command.to_string()]);
    let task = tokio::spawn(async move {
        let mut framed = Framed::new(AnyStream::from(server), ProdCodeCodec::new());
        stream_exec(
            child,
            running,
            &mut framed,
            tokio::time::Instant::now() + Duration::from_secs(seconds),
            structured.then_some((RemoteExecLanguage::Rust, false)),
        )
        .await
    });
    (workspace, Framed::new(client, ProdCodeCodec::new()), task)
}

#[tokio::test]
async fn exec_completed_child_is_not_listed_while_inherited_pipes_are_open() {
    for structured in [false, true] {
        let (workspace, client, task) = exec_fixture("sleep 2 & exit 0", 5, structured).await;
        // The shell has exited, but its background child still owns both output pipes.
        tokio::time::sleep(Duration::from_millis(250)).await;
        let name = workspace.path().file_name().unwrap().to_string_lossy();
        let stale = running_commands().iter().any(|c| c.workspace == name);
        drop(client);
        let finished = tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!stale, "a reaped shell is still listed as running");
        assert!(
            finished.client_left,
            "disconnect after child exit must still be observed"
        );
    }
}

#[tokio::test]
async fn exec_silent_child_answers_ping_and_preserves_exit_status() {
    for structured in [false, true] {
        let (_workspace, mut client, task) = exec_fixture("sleep 0.2; exit 7", 5, structured).await;
        client.send(WireMessage::Ping).await.unwrap();
        assert!(matches!(client.next().await, Some(Ok(WireMessage::Pong))));
        let finished = task.await.unwrap().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            std::process::ExitStatus::from_raw(finished.status.unwrap().0).code(),
            Some(7)
        );
        assert!(!finished.timed_out);
    }
}

#[tokio::test]
async fn exec_disconnected_silent_child_is_killed_and_unlisted() {
    for structured in [false, true] {
        let (workspace, mut client, task) = exec_fixture("sleep 30", 40, structured).await;
        client.send(WireMessage::Ping).await.unwrap();
        assert!(matches!(client.next().await, Some(Ok(WireMessage::Pong))));
        drop(client);
        let finished = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(finished.client_left);
        let name = workspace.path().file_name().unwrap().to_string_lossy();
        assert!(!running_commands().iter().any(|c| c.workspace == name));
    }
}

#[tokio::test]
async fn exec_deadline_still_bounds_inherited_pipes_after_child_exit() {
    for structured in [false, true] {
        let (_workspace, _client, task) = exec_fixture("sleep 2 & exit 7", 1, structured).await;
        let finished = tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            finished.timed_out,
            "the deadline was disabled after the direct child exited"
        );
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            std::process::ExitStatus::from_raw(finished.status.unwrap().0).code(),
            Some(7)
        );
    }
}
