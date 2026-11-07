/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::time::{Duration, Instant};

use prod_code_protocol::transport::ScrubSecrets;

use crate::editor_proxy::child::OwnedChild;

/// How a prebuild command ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Prebuilt {
    /// Exited with status 0.
    Done,
    /// Exited with another status (`None`: ended by a signal).
    Failed(Option<i32>),
    /// Was still running when its budget ended, and was stopped.
    TimedOut,
    /// Could not be started (no such program, no such directory).
    NotStarted(String),
}

impl Prebuilt {
    /// The low-cardinality name a metrics event carries; `None` when it succeeded.
    pub fn error_class(&self) -> Option<&'static str> {
        match self {
            Prebuilt::Done => None,
            Prebuilt::Failed(_) => Some("exit_non_zero"),
            Prebuilt::TimedOut => Some("timeout"),
            Prebuilt::NotStarted(_) => Some("not_started"),
        }
    }
}

/// Runs `command` (a program and its arguments, no shell) in `dir`, standard streams closed and
/// the daemon's cluster secrets removed from its environment, for at most `budget`, and says how
/// long it ran. The command leads its own process group, and the whole group is killed when the
/// budget ends or this future is dropped, so that the compilers a build driver started do not
/// outlive it. The build that writes the index a language server answers from is the one use of
/// this (#1976).
pub async fn run_prebuild(
    dir: &Path,
    command: &[String],
    budget: Duration,
) -> (Prebuilt, Duration) {
    let Some((program, args)) = command.split_first() else {
        return (
            Prebuilt::NotStarted("an empty command".to_string()),
            Duration::ZERO,
        );
    };
    let started = Instant::now();
    let mut process = tokio::process::Command::new(program);
    process
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    process.scrub_cluster_secrets();
    #[cfg(unix)]
    process.process_group(0);
    let mut child = match process.spawn() {
        Ok(child) => OwnedChild::new(child),
        Err(error) => return (Prebuilt::NotStarted(error.to_string()), started.elapsed()),
    };
    let outcome = match tokio::time::timeout(budget, child.wait()).await {
        Ok(Ok(status)) if status.success() => Prebuilt::Done,
        Ok(Ok(status)) => Prebuilt::Failed(status.code()),
        Ok(Err(error)) => Prebuilt::NotStarted(error.to_string()),
        Err(_) => Prebuilt::TimedOut,
    };
    let took = started.elapsed();
    // On every path the group goes with its leader; `Drop` does the same if this is cancelled.
    child
        .retire(tokio::time::Instant::now() + Duration::from_secs(5))
        .await;
    (outcome, took)
}

#[cfg(test)]
mod tests {
    use super::{Prebuilt, run_prebuild};
    use std::time::Duration;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    #[tokio::test]
    async fn a_command_that_succeeds_runs_in_the_directory_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        // `touch` writes into the working directory: the file shows where the command ran.
        let (outcome, _) = run_prebuild(
            dir.path(),
            &argv(&["touch", "ran"]),
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(outcome, Prebuilt::Done);
        assert!(dir.path().join("ran").exists());
        assert_eq!(outcome.error_class(), None);
    }

    #[tokio::test]
    async fn a_failing_a_missing_and_a_slow_command_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let (failed, _) =
            run_prebuild(dir.path(), &argv(&["false"]), Duration::from_secs(10)).await;
        assert_eq!(failed, Prebuilt::Failed(Some(1)));
        assert_eq!(failed.error_class(), Some("exit_non_zero"));

        let (missing, _) = run_prebuild(
            dir.path(),
            &argv(&["no-such-program-1976"]),
            Duration::from_secs(10),
        )
        .await;
        assert!(matches!(missing, Prebuilt::NotStarted(_)), "{missing:?}");
        assert_eq!(missing.error_class(), Some("not_started"));

        let started = std::time::Instant::now();
        let (slow, took) = run_prebuild(
            dir.path(),
            &argv(&["sleep", "30"]),
            Duration::from_millis(200),
        )
        .await;
        assert_eq!(slow, Prebuilt::TimedOut);
        assert_eq!(slow.error_class(), Some("timeout"));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it was stopped at its budget"
        );
        assert!(
            took >= Duration::from_millis(200),
            "a timed-out run reports the time it ran, not 0: {took:?}"
        );

        let (empty, _) = run_prebuild(dir.path(), &[], Duration::from_secs(1)).await;
        assert!(matches!(empty, Prebuilt::NotStarted(_)));
    }

    /// A failed run says how long it ran too (a build that fails after ten minutes is not 0 ms).
    #[tokio::test]
    async fn a_failed_run_reports_its_duration() {
        let dir = tempfile::tempdir().unwrap();
        let (outcome, took) = run_prebuild(
            dir.path(),
            &argv(&["sh", "-c", "sleep 0.3; exit 3"]),
            Duration::from_secs(10),
        )
        .await;
        assert_eq!(outcome, Prebuilt::Failed(Some(3)));
        assert!(took >= Duration::from_millis(250), "{took:?}");
    }

    /// The driver's own children go with it when the budget ends (#1976): a stopped `swift build`
    /// must not leave its compilers running under init.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_timeout_kills_the_processes_the_command_started() {
        let dir = tempfile::tempdir().unwrap();
        let (outcome, _) = run_prebuild(
            dir.path(),
            &argv(&["sh", "-c", "sleep 300 & echo $! > child.pid; wait"]),
            Duration::from_millis(600),
        )
        .await;
        assert_eq!(outcome, Prebuilt::TimedOut);
        let pid: i32 = std::fs::read_to_string(dir.path().join("child.pid"))
            .expect("the child wrote its pid")
            .trim()
            .parse()
            .unwrap();
        // Gone, or a zombie that init has not reaped yet.
        let gone = || match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(_) => true,
            Ok(stat) => stat
                .rsplit(')')
                .next()
                .is_some_and(|rest| rest.trim_start().starts_with('Z')),
        };
        for _ in 0..50 {
            if gone() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        unsafe { libc::kill(pid, libc::SIGKILL) };
        panic!("the grandchild {pid} outlived the timed-out command");
    }
}
