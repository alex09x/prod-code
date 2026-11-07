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

/// How a prebuild command ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Prebuilt {
    /// Exited with status 0, after this long.
    Done(Duration),
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
            Prebuilt::Done(_) => None,
            Prebuilt::Failed(_) => Some("exit_non_zero"),
            Prebuilt::TimedOut => Some("timeout"),
            Prebuilt::NotStarted(_) => Some("not_started"),
        }
    }
}

/// Runs `command` (a program and its arguments, no shell) in `dir`, standard streams closed, for
/// at most `budget`; the process is killed when the budget ends. The build that writes the index
/// a language server answers from is the one use of this (#1976).
pub async fn run_prebuild(dir: &Path, command: &[String], budget: Duration) -> Prebuilt {
    let Some((program, args)) = command.split_first() else {
        return Prebuilt::NotStarted("an empty command".to_string());
    };
    let started = Instant::now();
    let mut child = match tokio::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return Prebuilt::NotStarted(error.to_string()),
    };
    match tokio::time::timeout(budget, child.wait()).await {
        Ok(Ok(status)) if status.success() => Prebuilt::Done(started.elapsed()),
        Ok(Ok(status)) => Prebuilt::Failed(status.code()),
        Ok(Err(error)) => Prebuilt::NotStarted(error.to_string()),
        Err(_) => {
            let _ = child.kill().await;
            Prebuilt::TimedOut
        }
    }
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
        let outcome = run_prebuild(
            dir.path(),
            &argv(&["touch", "ran"]),
            Duration::from_secs(10),
        )
        .await;
        assert!(matches!(outcome, Prebuilt::Done(_)), "{outcome:?}");
        assert!(dir.path().join("ran").exists());
        assert_eq!(outcome.error_class(), None);
    }

    #[tokio::test]
    async fn a_failing_a_missing_and_a_slow_command_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let failed = run_prebuild(dir.path(), &argv(&["false"]), Duration::from_secs(10)).await;
        assert_eq!(failed, Prebuilt::Failed(Some(1)));
        assert_eq!(failed.error_class(), Some("exit_non_zero"));

        let missing = run_prebuild(
            dir.path(),
            &argv(&["no-such-program-1976"]),
            Duration::from_secs(10),
        )
        .await;
        assert!(matches!(missing, Prebuilt::NotStarted(_)), "{missing:?}");
        assert_eq!(missing.error_class(), Some("not_started"));

        let started = std::time::Instant::now();
        let slow = run_prebuild(
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

        let empty = run_prebuild(dir.path(), &[], Duration::from_secs(1)).await;
        assert!(matches!(empty, Prebuilt::NotStarted(_)));
    }
}
