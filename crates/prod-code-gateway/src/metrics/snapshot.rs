/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Host and gateway periodic telemetry snapshot collection.
//!
//! Gathers process metrics (RSS, running commands, active sessions and queries),
//! host resource metrics (memory, disk, load average, CPU count and usage), and
//! software versions into standardized [`HostSnapshot`] records.

use super::now_ms;
use crate::memory;
use prod_code_protocol::HostSnapshot;
use std::sync::Mutex;

/// State for computing CPU utilization delta between consecutive snapshot ticks.
#[derive(Debug, Default)]
pub struct CpuTracker {
    last_sample: Option<(u64, u64)>, // (total_ticks, idle_ticks)
}

impl CpuTracker {
    pub fn new() -> Self {
        let mut tracker = Self { last_sample: None };
        tracker.read_ticks(); // Initialize baseline
        tracker
    }

    /// Computes CPU utilization in thousandths (10000 = 10.0%, 100000 = 100.0%).
    pub fn sample_usage_millis(&mut self, cpu_count: usize) -> Option<u32> {
        if let Some((curr_total, curr_idle)) = self.read_ticks() {
            if let Some((prev_total, prev_idle)) = self.last_sample {
                self.last_sample = Some((curr_total, curr_idle));
                let total_diff = curr_total.saturating_sub(prev_total);
                let idle_diff = curr_idle.saturating_sub(prev_idle);
                if total_diff > 0 {
                    let busy_diff = total_diff.saturating_sub(idle_diff);
                    let usage_share = busy_diff as f64 / total_diff as f64;
                    let millis = (usage_share * 100_000.0).round().clamp(0.0, 100_000.0) as u32;
                    return Some(millis);
                }
            } else {
                self.last_sample = Some((curr_total, curr_idle));
            }
        }

        // Fallback: estimate from 1m load average per CPU
        if let Some(load) = memory::load_average_1m() {
            if cpu_count > 0 {
                let share = (load / cpu_count as f64).clamp(0.0, 1.0);
                return Some((share * 100_000.0).round() as u32);
            }
        }
        None
    }

    fn read_ticks(&mut self) -> Option<(u64, u64)> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(text) = std::fs::read_to_string("/proc/stat") {
                if let Some(first_line) = text.lines().next() {
                    let parts: Vec<&str> = first_line.split_whitespace().collect();
                    if parts.len() >= 5 && parts[0] == "cpu" {
                        let user: u64 = parts[1].parse().ok()?;
                        let nice: u64 = parts[2].parse().ok()?;
                        let system: u64 = parts[3].parse().ok()?;
                        let idle: u64 = parts[4].parse().ok()?;
                        let iowait: u64 = parts.get(5).and_then(|p| p.parse().ok()).unwrap_or(0);
                        let irq: u64 = parts.get(6).and_then(|p| p.parse().ok()).unwrap_or(0);
                        let softirq: u64 = parts.get(7).and_then(|p| p.parse().ok()).unwrap_or(0);
                        let steal: u64 = parts.get(8).and_then(|p| p.parse().ok()).unwrap_or(0);

                        let total = user + nice + system + idle + iowait + irq + softirq + steal;
                        let idle_total = idle + iowait;
                        return Some((total, idle_total));
                    }
                }
            }
            None
        }
        #[cfg(target_os = "macos")]
        {
            use std::mem::MaybeUninit;
            let mut cpu_info = MaybeUninit::<libc::host_cpu_load_info_data_t>::uninit();
            let mut count = (std::mem::size_of::<libc::host_cpu_load_info_data_t>()
                / std::mem::size_of::<libc::integer_t>())
                as libc::mach_msg_type_number_t;

            let kr = unsafe {
                libc::host_statistics64(
                    libc::mach_host_self(),
                    libc::HOST_CPU_LOAD_INFO,
                    cpu_info.as_mut_ptr() as libc::host_info64_t,
                    &mut count,
                )
            };

            if kr == libc::KERN_SUCCESS {
                let info = unsafe { cpu_info.assume_init() };
                let user = info.cpu_ticks[libc::CPU_STATE_USER as usize] as u64;
                let system = info.cpu_ticks[libc::CPU_STATE_SYSTEM as usize] as u64;
                let idle = info.cpu_ticks[libc::CPU_STATE_IDLE as usize] as u64;
                let nice = info.cpu_ticks[libc::CPU_STATE_NICE as usize] as u64;
                let total = user + system + idle + nice;
                return Some((total, idle));
            }
            None
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            None
        }
    }
}

/// Global shared CPU usage tracker.
static GLOBAL_CPU_TRACKER: std::sync::OnceLock<Mutex<CpuTracker>> = std::sync::OnceLock::new();

fn global_cpu_tracker() -> &'static Mutex<CpuTracker> {
    GLOBAL_CPU_TRACKER.get_or_init(|| Mutex::new(CpuTracker::new()))
}

/// Gathers a complete host resource and gateway telemetry snapshot.
pub async fn collect_host_snapshot(
    node: &str,
    storage_root: &std::path::Path,
    active_sessions: usize,
    workspace_count: usize,
    engine_count: usize,
    running_commands_count: usize,
) -> HostSnapshot {
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let cpu_usage_millis = global_cpu_tracker()
        .lock()
        .map(|mut t| t.sample_usage_millis(cpu_count))
        .unwrap_or(None);

    let (mem_avail, mem_total) = memory::system_memory()
        .map(|(a, t)| (Some(a), Some(t)))
        .unwrap_or((None, None));

    let host_res = memory::host_resources(storage_root);

    let storage_free_bytes = read_storage_free_bytes(storage_root);

    HostSnapshot {
        ts_ms: now_ms(),
        node: node.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_commit: prod_code_protocol::git_commit().to_string(),
        platform: prod_code_protocol::platform(),
        cpu_count,
        cpu_usage_millis,
        load_average_millis: memory::load_average_1m().map(|l| (l * 1000.0) as u32),
        process_rss_bytes: memory::get_process_rss_bytes(),
        host_memory_available_bytes: mem_avail,
        host_memory_total_bytes: mem_total,
        storage_free_millis: host_res.storage_free_millis,
        storage_free_bytes,
        active_sessions,
        active_queries: crate::ACTIVE_QUERIES.load(std::sync::atomic::Ordering::Relaxed),
        running_commands: running_commands_count,
        workspace_count,
        engine_count,
    }
}

fn read_storage_free_bytes(storage_root: &std::path::Path) -> Option<u64> {
    use std::ffi::CString;
    let path = CString::new(storage_root.to_str()?).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0 {
        let s = unsafe { stat.assume_init() };
        let bavail = s.f_bavail as u64;
        let frsize = if s.f_frsize > 0 {
            s.f_frsize as u64
        } else {
            s.f_bsize as u64
        };
        return Some(bavail * frsize);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_tracker_reports_non_negative() {
        let mut tracker = CpuTracker::new();
        // Allow ticks to accumulate
        std::thread::sleep(std::time::Duration::from_millis(50));
        let usage = tracker.sample_usage_millis(4);
        if let Some(millis) = usage {
            assert!(millis <= 100_000);
        }
    }
}
