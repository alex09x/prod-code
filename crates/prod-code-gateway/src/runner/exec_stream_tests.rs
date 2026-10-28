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

fn process_alive(pid: i32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && let Some((_, fields)) = stat.rsplit_once(") ")
        && matches!(fields.split_whitespace().next(), Some("Z" | "X"))
    {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(target_os = "linux")]
#[test]
fn process_liveness_distinguishes_a_zombie_from_a_running_child() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    assert!(process_alive(pid));
    child.kill().unwrap();
    // WNOWAIT observes the exited child without reaping it, leaving a real zombie.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let observed = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOWAIT,
        )
    };
    assert_eq!(observed, 0, "waitid: {}", std::io::Error::last_os_error());
    let still_alive = process_alive(pid);
    child.wait().unwrap();
    assert!(!still_alive, "a zombie must not count as a live process");
    assert!(!process_alive(pid));
}

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

/// Dropping the stream's future without a Disconnect still kills the process group (#1536).
#[tokio::test]
async fn exec_cancelled_stream_kills_its_process_group() {
    let marks = tempfile::tempdir().unwrap();
    let pid_file = marks.path().join("pid");
    let command = format!("echo $$ > {}; sleep 30", pid_file.display());
    let (_workspace, _client, task) = exec_fixture(&command, 60, false).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|t| t.trim().parse::<i32>().ok())
        {
            break pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the child never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    task.abort();
    let _ = task.await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while process_alive(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "process {pid} outlived its cancelled stream"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
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

/// While the stream may still signal the group, its exited leader stays a zombie that holds
/// the PID and so the group's number; cancelling then kills the descendant and only after that
/// lets the leader be reaped (#1545, #1541).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_exited_leader_holds_its_group_until_the_stream_lets_go() {
    let marks = tempfile::tempdir().unwrap();
    let pid_file = marks.path().join("pids");
    let command = format!("sleep 30 & echo $$ $! > {}; exit 0", pid_file.display());
    let (_workspace, _client, task) = exec_fixture(&command, 60, false).await;
    let state = |pid: i32| {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?.1.trim_start().chars().next()
    };
    let alive = |pid: i32| {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let (leader, descendant) = loop {
        let pids: Vec<i32> = std::fs::read_to_string(&pid_file)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect();
        if let [leader, descendant] = pids[..] {
            break (leader, descendant);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the command never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    while state(leader) != Some('Z') {
        assert!(
            std::time::Instant::now() < deadline,
            "leader {leader} was reaped while its stream still runs: {:?}",
            state(leader)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The descendant keeps the pipes open, so the stream drains on and the zombie stays.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        state(leader),
        Some('Z'),
        "the zombie holds the group's number"
    );
    task.abort();
    let _ = task.await;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while alive(descendant) || state(leader) == Some('Z') {
        assert!(
            std::time::Instant::now() < deadline,
            "descendant {descendant} alive {} or leader {leader} unreaped {:?}",
            alive(descendant),
            state(leader)
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
