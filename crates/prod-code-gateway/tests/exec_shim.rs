//! The gateway binary's exec shim (#255): it reports the peak memory of the command it runs,
//! not the high-water mark its parent carried into it across fork, and it passes the command's
//! exit code or terminating signal on as its own.

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus};

use prod_code_gateway::exec_shim::{SHIM_FLAG, parse_report};
use prod_code_protocol::ExecUsage;

/// Runs `command` through the shim and returns how the shim ended and the report it wrote.
fn run_shim(command: &[&str]) -> (ExitStatus, i32, ExecUsage) {
    let dir = tempfile::tempdir().expect("temp dir");
    let report = dir.path().join("report");
    let status = Command::new(env!("CARGO_BIN_EXE_prod-code-server"))
        .arg(SHIM_FLAG)
        .arg(&report)
        .arg("--")
        .args(command)
        .status()
        .expect("the shim starts");
    let text = std::fs::read_to_string(&report).expect("the shim wrote a report");
    let (raw, usage) = parse_report(&text).unwrap_or_else(|| panic!("a report line: {text:?}"));
    (status, raw, usage)
}

/// This process's own peak resident set, in KiB.
fn own_max_rss_kb() -> u64 {
    // SAFETY: an all-zero `rusage` is a valid value for `getrusage` to fill in.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `usage` is valid for writes for the duration of the call.
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
    // Linux reports the peak in KiB, macOS in bytes.
    if cfg!(target_os = "macos") {
        usage.ru_maxrss as u64 / 1024
    } else {
        usage.ru_maxrss as u64
    }
}

/// Reaps `child` with `wait4` and returns the peak resident set it reported, in KiB.
#[cfg(target_os = "linux")]
fn reap_peak_kb(child: std::process::Child) -> u64 {
    let pid = child.id() as libc::pid_t;
    let mut status: libc::c_int = 0;
    // SAFETY: an all-zero `rusage` is a valid value for `wait4` to fill in.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `status` and `usage` are valid for writes for the duration of the call, and
    // `pid` is a child of this process that nothing else waits for.
    assert_eq!(unsafe { libc::wait4(pid, &mut status, 0, &mut usage) }, pid);
    usage.ru_maxrss as u64
}

#[test]
fn the_reported_peak_is_the_commands_not_the_parents() {
    // A parent with a large resident set, as the gateway is: every page is written, so that it
    // is resident and not merely reserved.
    let mut ballast = vec![0u8; 512 << 20];
    for page in ballast.chunks_mut(4096) {
        page[0] = 1;
    }
    std::hint::black_box(&ballast);
    assert!(
        own_max_rss_kb() >= 512 << 10,
        "the test process holds 512 MiB: {} KiB",
        own_max_rss_kb()
    );

    // Without the shim, Linux hands the parent's peak to the command, which is the bug this
    // test would otherwise not be able to see.
    #[cfg(target_os = "linux")]
    {
        let direct = reap_peak_kb(Command::new("true").spawn().expect("`true` starts"));
        assert!(
            direct >= 256 << 10,
            "`true` started directly inherits the parent's peak: {direct} KiB"
        );
    }

    let (status, raw, usage) = run_shim(&["true"]);
    drop(ballast);

    assert_eq!(status.code(), Some(0));
    assert!(libc::WIFEXITED(raw) && libc::WEXITSTATUS(raw) == 0, "{raw}");
    assert!(
        usage.max_rss_kb < 64 << 10,
        "`true` reported a peak of {} KiB",
        usage.max_rss_kb
    );
}

#[test]
fn the_commands_exit_code_is_passed_on() {
    let (status, raw, _) = run_shim(&["sh", "-c", "exit 3"]);
    assert_eq!(status.code(), Some(3));
    assert!(libc::WIFEXITED(raw) && libc::WEXITSTATUS(raw) == 3, "{raw}");
}

#[test]
fn the_shim_dies_of_the_signal_that_killed_the_command() {
    let (status, raw, _) = run_shim(&["sh", "-c", "kill -TERM $$"]);
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?}");
    assert!(
        libc::WIFSIGNALED(raw) && libc::WTERMSIG(raw) == libc::SIGTERM,
        "{raw}"
    );
}
