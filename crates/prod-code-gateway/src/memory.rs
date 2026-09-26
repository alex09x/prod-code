//! Memory watchdog and RSS tracking module for prod-code gateway daemon.

/// Retrieve current resident set size (RSS) in bytes for this process.
pub fn get_process_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            let parts: Vec<&str> = statm.split_whitespace().collect();
            if parts.len() >= 2
                && let Ok(pages) = parts[1].parse::<u64>()
            {
                let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) as u64 };
                return Some(pages * page_size);
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        use std::mem::MaybeUninit;
        let mut info = MaybeUninit::<libc::mach_task_basic_info>::uninit();
        let mut count = (std::mem::size_of::<libc::mach_task_basic_info>()
            / std::mem::size_of::<libc::natural_t>())
            as libc::mach_msg_type_number_t;
        #[allow(deprecated)]
        let kret = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                info.as_mut_ptr() as libc::task_info_t,
                &mut count,
            )
        };
        if kret == libc::KERN_SUCCESS {
            let info = unsafe { info.assume_init() };
            Some(info.resident_size)
        } else {
            None
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Retrieve current RSS in megabytes.
pub fn get_process_rss_mb() -> Option<f64> {
    get_process_rss_bytes().map(|b| (b as f64) / (1024.0 * 1024.0))
}

/// 1-minute load average from `/proc/loadavg` (Linux) or `sysctl vm.loadavg` (macOS).
pub fn load_average_1m() -> Option<f64> {
    if let Ok(text) = std::fs::read_to_string("/proc/loadavg") {
        return text.split_whitespace().next()?.parse().ok();
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        return text
            .split_whitespace()
            .find(|t| t.parse::<f64>().is_ok())?
            .parse()
            .ok();
    }
    #[allow(unreachable_code)]
    None
}

/// Memory the host can still give out and its physical memory, in bytes: `MemAvailable` and
/// `MemTotal` on Linux; on macOS the kernel's free percentage (`kern.memorystatus_level`, what
/// `memory_pressure` prints) of `hw.memsize`.
pub fn system_memory() -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        return parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?);
    }
    #[cfg(target_os = "macos")]
    {
        let total: u64 = sysctl_number(c"hw.memsize")?;
        let free_percent: u32 = sysctl_number(c"kern.memorystatus_level")?;
        return Some((total / 100 * u64::from(free_percent.min(100)), total));
    }
    #[allow(unreachable_code)]
    None
}

/// `MemAvailable` and `MemTotal` of a `/proc/meminfo` text, in bytes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
            .map(|kib| kib * 1024)
    };
    Some((field("MemAvailable")?, field("MemTotal")?))
}

/// A number the macOS kernel publishes under `name`.
#[cfg(target_os = "macos")]
fn sysctl_number<T: Default + Copy>(name: &std::ffi::CStr) -> Option<T> {
    let mut value = T::default();
    let mut len = std::mem::size_of::<T>();
    // SAFETY: the kernel writes at most `len` bytes into `value` and says how many it wrote.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut T).cast::<libc::c_void>(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && len == std::mem::size_of::<T>()).then_some(value)
}

/// What this host has left (#396): its memory, and the free share of the filesystem holding
/// `storage`, the directory the workspace copies live in.
pub fn host_resources(storage: &std::path::Path) -> prod_code_protocol::HostResources {
    let memory = system_memory();
    prod_code_protocol::HostResources {
        memory_available_bytes: memory.map(|(available, _)| available),
        memory_total_bytes: memory.map(|(_, total)| total),
        storage_free_millis: crate::workspace::free_share(storage)
            .map(|share| (share * 1000.0).round() as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_gives_available_and_total_bytes() {
        let text = "MemTotal:       263842716 kB\nMemFree:         1523316 kB\nMemAvailable:   198765432 kB\nBuffers:            1024 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some((198_765_432 * 1024, 263_842_716 * 1024))
        );
        assert_eq!(parse_meminfo("MemTotal: 1024 kB\n"), None);
    }

    /// The host this runs on reports its memory and the disk under a directory.
    #[test]
    fn this_host_reports_memory_and_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let host = host_resources(dir.path());
        let (Some(available), Some(total)) = (host.memory_available_bytes, host.memory_total_bytes)
        else {
            panic!("no memory figures: {host:?}");
        };
        assert!(0 < available && available <= total, "{host:?}");
        assert!(
            host.storage_free_millis.is_some_and(|m| m <= 1000),
            "{host:?}"
        );
    }

    #[test]
    fn test_rss_reading() {
        let bytes = get_process_rss_bytes();
        assert!(
            bytes.is_some(),
            "Process RSS must be readable on supported OS"
        );
        assert!(bytes.unwrap() > 0, "Process RSS must be positive");

        let mb = get_process_rss_mb();
        assert!(mb.is_some());
        assert!(mb.unwrap() > 0.0);
    }
}
