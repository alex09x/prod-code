//! The exec shim: the gateway binary started as `prod-code-server --exec-shim REPORT -- COMMAND`
//! runs COMMAND as its own child, reaps it with `wait4`, and writes what the kernel reported
//! about it to REPORT (#255).
//!
//! Linux carries a forking process's resident-set high-water mark into the child across `exec`.
//! A command the gateway started directly therefore reported the gateway's own tens of gigabytes
//! as its peak memory. The shim is small when it forks, so the command it starts begins with a
//! small high-water mark, and `wait4` on it reports the largest resident set of the command and
//! of the descendants it waited for.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use prod_code_protocol::ExecUsage;

/// The first argument that turns the gateway binary into the exec shim.
pub const SHIM_FLAG: &str = "--exec-shim";

/// The file name the gateway binary starts with. Anything else, such as a test binary that
/// links the library, has no shim mode and starts commands directly.
const BINARY_NAME: &str = "prod-code-server";

/// Runs the shim with the arguments that follow [`SHIM_FLAG`]: `REPORT -- COMMAND [ARG]...`.
///
/// Returns the exit code the shim should exit with: the command's own, or 127 when it could not
/// be started. A command killed by a signal kills the shim with the same signal, and then this
/// does not return.
pub fn run(args: &[OsString]) -> i32 {
    let (report, program, command_args) = match args {
        [report, dashes, program, rest @ ..] if dashes == "--" => {
            (Path::new(report), program, rest)
        }
        _ => {
            eprintln!("usage: {BINARY_NAME} {SHIM_FLAG} REPORT -- COMMAND [ARG]...");
            return 2;
        }
    };
    // The command inherits the shim's standard streams, working directory, environment and
    // process group, which are the ones the gateway set up for it.
    let child = match std::process::Command::new(program)
        .args(command_args)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("failed to start {}: {e}", program.to_string_lossy());
            return 127;
        }
    };
    let Some((status, usage)) = reap_child(child) else {
        eprintln!(
            "{BINARY_NAME} {SHIM_FLAG}: waiting for {} failed: {}",
            program.to_string_lossy(),
            std::io::Error::last_os_error()
        );
        return 1;
    };
    if let Err(e) = std::fs::write(report, format_report(status, &usage)) {
        eprintln!(
            "{BINARY_NAME} {SHIM_FLAG}: cannot write {}: {e}",
            report.display()
        );
    }
    if libc::WIFEXITED(status) {
        return libc::WEXITSTATUS(status);
    }
    if libc::WIFSIGNALED(status) {
        let signal = libc::WTERMSIG(status);
        die_of(signal);
        return 128 + signal;
    }
    1
}

/// Reaps the shim's command. Taking the `Child` by value keeps it from being reaped anywhere
/// else, so `wait4` is the only wait it gets.
fn reap_child(child: std::process::Child) -> Option<(i32, ExecUsage)> {
    reap_with_usage(child.id() as i32)
}

/// Terminates the shim with `signal`, the way its command was terminated, so that the gateway
/// and a shell see the same death as without the shim.
fn die_of(signal: libc::c_int) {
    // The command has already dumped its core if the signal asks for one; a second core file,
    // of the shim, would only be noise.
    let no_core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `no_core` is a valid `rlimit` for the duration of the call.
    unsafe { libc::setrlimit(libc::RLIMIT_CORE, &no_core) };
    // SAFETY: restoring the default action installs no handler, and the shim is single-threaded
    // at this point, so nothing else depends on the signal's disposition.
    unsafe { libc::signal(signal, libc::SIG_DFL) };
    // SAFETY: an all-zero `sigset_t` is a valid value for `sigemptyset` to initialise.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: `set` is valid for writes, and a null old set asks for none to be returned.
    unsafe {
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, signal);
        libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
    }
    // SAFETY: raising a signal whose action is the default has no memory-safety requirements.
    unsafe { libc::raise(signal) };
}

/// Reaps the child `pid` with `wait4`, retrying on `EINTR`, and returns its raw wait status and
/// what it and the descendants it waited for used.
pub(crate) fn reap_with_usage(pid: i32) -> Option<(i32, ExecUsage)> {
    let mut status: libc::c_int = 0;
    // SAFETY: an all-zero `rusage` is a valid value for `wait4` to fill in.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    loop {
        // SAFETY: `status` and `usage` are valid for writes for the duration of the call, and
        // `pid` is a child of this process that nothing else waits for.
        let got = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
        if got == pid {
            break;
        }
        if got == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return None;
    }
    let ms = |t: libc::timeval| t.tv_sec as u64 * 1000 + t.tv_usec as u64 / 1000;
    // Linux reports the peak in KiB, macOS in bytes.
    let max_rss_kb = if cfg!(target_os = "macos") {
        usage.ru_maxrss as u64 / 1024
    } else {
        usage.ru_maxrss as u64
    };
    Some((
        status,
        ExecUsage {
            cpu_user_ms: ms(usage.ru_utime),
            cpu_sys_ms: ms(usage.ru_stime),
            max_rss_kb,
        },
    ))
}

/// The report line the shim writes: the raw wait status and the command's resource use.
pub fn format_report(status: i32, usage: &ExecUsage) -> String {
    format!(
        "status={status} user_ms={} sys_ms={} max_rss_kb={}\n",
        usage.cpu_user_ms, usage.cpu_sys_ms, usage.max_rss_kb
    )
}

/// Reads a report line back. A line without its final newline is rejected: the shim was killed
/// while writing it.
pub fn parse_report(text: &str) -> Option<(i32, ExecUsage)> {
    let line = text.strip_suffix('\n')?;
    let (mut status, mut user, mut sys, mut rss) = (None, None, None, None);
    for field in line.split(' ') {
        let (key, value) = field.split_once('=')?;
        match key {
            "status" => status = Some(value.parse().ok()?),
            "user_ms" => user = Some(value.parse().ok()?),
            "sys_ms" => sys = Some(value.parse().ok()?),
            "max_rss_kb" => rss = Some(value.parse().ok()?),
            _ => {}
        }
    }
    Some((
        status?,
        ExecUsage {
            cpu_user_ms: user?,
            cpu_sys_ms: sys?,
            max_rss_kb: rss?,
        },
    ))
}

/// The executable to start the shim from, when this process is the gateway binary.
///
/// On Linux that is `/proc/self/exe`, which still names the running binary after a deploy has
/// replaced the file on disk.
fn shim_executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if !exe.file_name()?.to_string_lossy().starts_with(BINARY_NAME) {
        return None;
    }
    if cfg!(target_os = "linux") {
        Some(PathBuf::from("/proc/self/exe"))
    } else {
        Some(exe)
    }
}

/// A command for `program`, run through the shim when this process is the gateway binary, and
/// the report file the shim will write. The caller adds the program's arguments.
pub fn command(program: &str) -> (std::process::Command, Option<ReportFile>) {
    match shim_executable() {
        Some(exe) => {
            let report = ReportFile::new();
            let mut cmd = std::process::Command::new(exe);
            cmd.arg(SHIM_FLAG).arg(&report.0).arg("--").arg(program);
            (cmd, Some(report))
        }
        None => (std::process::Command::new(program), None),
    }
}

/// The shim's report file in the system temporary directory, removed when dropped.
pub struct ReportFile(PathBuf);

impl ReportFile {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "prod-code-exec-{}-{}.report",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // A file left by an earlier gateway that had the same pid must not pass for this
        // command's report when the shim is killed before it writes one.
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    /// The command's raw wait status and resource use, if the shim wrote them.
    pub fn read(&self) -> Option<(i32, ExecUsage)> {
        parse_report(&std::fs::read_to_string(&self.0).ok()?)
    }
}

impl Drop for ReportFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_reads_back_as_written() {
        let usage = ExecUsage {
            cpu_user_ms: 12,
            cpu_sys_ms: 3,
            max_rss_kb: 4096,
        };
        let (status, read) = parse_report(&format_report(768, &usage)).expect("parses");
        assert_eq!(status, 768);
        assert_eq!(read.cpu_user_ms, 12);
        assert_eq!(read.cpu_sys_ms, 3);
        assert_eq!(read.max_rss_kb, 4096);
    }

    #[test]
    fn a_truncated_report_is_rejected() {
        assert!(parse_report("status=0 user_ms=1 sys_ms=0 max_rss_kb=1").is_none());
        assert!(parse_report("status=0 user_ms=1\n").is_none());
        assert!(parse_report("").is_none());
    }

    #[test]
    fn a_test_binary_starts_commands_directly() {
        let (cmd, report) = command("true");
        assert!(report.is_none());
        assert_eq!(cmd.get_program(), "true");
    }
}
