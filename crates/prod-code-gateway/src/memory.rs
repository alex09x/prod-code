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

#[cfg(test)]
mod tests {
    use super::*;

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
