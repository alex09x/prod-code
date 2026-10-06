/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

/// Past this share of physical memory in use, a node takes no new workspace while another can
/// (ROADMAP 5.3: 85%).
pub const MEMORY_PRESSURE_USED: f64 = 0.85;

/// Under this share of its workspaces filesystem free, a node takes no new workspace while
/// another can: a full disk truncates synced files (#385).
pub const STORAGE_PRESSURE_FREE: f64 = 0.10;

/// Memory and disk space a gateway's host has left, those it could read.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostResources {
    /// Memory the host can still give out without swapping (`MemAvailable` on Linux, the
    /// kernel's free percentage on macOS), in bytes.
    #[serde(default)]
    pub memory_available_bytes: Option<u64>,
    /// The host's physical memory, in bytes.
    #[serde(default)]
    pub memory_total_bytes: Option<u64>,
    /// The free share of the filesystem holding the workspaces, in thousandths (150 = 15%).
    #[serde(default)]
    pub storage_free_millis: Option<u32>,
}

impl HostResources {
    /// The share of physical memory in use (0.0 to 1.0), when both numbers are known.
    pub fn memory_used_share(&self) -> Option<f64> {
        let total = self.memory_total_bytes.filter(|t| *t > 0)?;
        let available = self.memory_available_bytes?.min(total);
        Some(1.0 - available as f64 / total as f64)
    }

    /// The free share of the workspaces filesystem (0.0 to 1.0), when known.
    pub fn storage_free_share(&self) -> Option<f64> {
        self.storage_free_millis.map(|m| m as f64 / 1000.0)
    }

    /// Why the host should take no new workspace (`memory 91% used`, `disk 4% free`), or `None`
    /// when it is not known to be short of either.
    pub fn pressure(&self) -> Option<String> {
        let mut why = Vec::new();
        if let Some(used) = self.memory_used_share()
            && used > MEMORY_PRESSURE_USED
        {
            why.push(memory_text(used));
        }
        if let Some(free) = self.storage_free_share()
            && free < STORAGE_PRESSURE_FREE
        {
            why.push(disk_text(free));
        }
        (!why.is_empty()).then(|| why.join(", "))
    }

    /// `memory 41% used, disk 62% free`, the parts that are known; empty when none is.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(used) = self.memory_used_share() {
            parts.push(memory_text(used));
        }
        if let Some(free) = self.storage_free_share() {
            parts.push(disk_text(free));
        }
        parts.join(", ")
    }
}

/// Whole percent, rounded down from the nearest tenth: a disk 9.9% free reads `9%`, under the
/// 10% it is short of, not `10%`.
fn whole_percent(share: f64) -> u32 {
    ((share * 1000.0).round() as u32) / 10
}

fn memory_text(used: f64) -> String {
    format!("memory {}% used", whole_percent(used))
}

fn disk_text(free: f64) -> String {
    format!("disk {}% free", whole_percent(free))
}

/// A remote command the gateway is running.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunningCommand {
    /// The workspace copy's directory name, as `prod-code--wt-<hash>`.
    pub workspace: String,
    pub command: String,
    pub running_seconds: u64,
}

/// Real-time health and session status of the remote gateway.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusResponse {
    pub server_pid: u32,
    pub uptime_seconds: u64,
    pub active_sessions: usize,
    pub loaded_workspaces: usize,
    pub detected_engines: Vec<String>,
    #[serde(default)]
    pub memory_rss_bytes: Option<u64>,
    #[serde(default)]
    pub total_queries: u64,
    #[serde(default)]
    pub active_queries: usize,
    /// 1-minute load average of the host in thousandths (1500 = 1.5), when known.
    #[serde(default)]
    pub load_average_millis: Option<u32>,
    /// Logical CPUs of the host, when known.
    #[serde(default)]
    pub cpu_count: Option<usize>,
    /// The gateway's OS and architecture, as [`crate::platform`] gives them (`macos aarch64`).
    /// Absent from older gateways, which a checkout that needs macOS must not be placed on.
    #[serde(default)]
    pub platform: Option<String>,
    /// Remote commands running on the gateway (`exec`, `check`, `test`, `lint`). A node with
    /// one running is not idle, whatever the session count says (#273).
    #[serde(default)]
    pub running_commands: Vec<RunningCommand>,
    /// What the host has left: memory and space for workspaces (#396). Empty from older
    /// gateways.
    #[serde(default)]
    pub host: HostResources,
    /// The gateway's version, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The gateway's git commit hash, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
}

impl StatusResponse {
    /// One line per running command, longest-running first: `workspace  12m 5s  command`,
    /// with the command cut at 100 characters.
    pub fn running_lines(&self) -> Vec<String> {
        let mut commands = self.running_commands.clone();
        commands.sort_by_key(|c| std::cmp::Reverse(c.running_seconds));
        commands
            .iter()
            .map(|c| {
                let mut command: String = c.command.chars().take(100).collect();
                if c.command.chars().count() > 100 {
                    command.push('…');
                }
                format!(
                    "{}  {}m {}s  {command}",
                    c.workspace,
                    c.running_seconds / 60,
                    c.running_seconds % 60
                )
            })
            .collect()
    }

    /// Load per CPU (1-minute load average divided by CPU count); lower is quieter.
    pub fn load_per_cpu(&self) -> Option<f64> {
        match (self.load_average_millis, self.cpu_count) {
            (Some(load), Some(cpus)) if cpus > 0 => Some(load as f64 / 1000.0 / cpus as f64),
            _ => None,
        }
    }

    pub fn memory_rss_mb(&self) -> Option<f64> {
        self.memory_rss_bytes
            .map(|b| (b as f64) / (1024.0 * 1024.0))
    }

    /// Multi-dimensional cluster congestion score. Lower is quieter and roomier; higher is more congested.
    ///
    /// Combines:
    /// - Hard resource pressure: disqualified (>= 1000.0) if memory > 85% or disk < 10%.
    /// - Base CPU load: load per CPU (conservative 0.50 uncertainty penalty if unmeasured).
    /// - Memory pressure curve: steep penalty if memory > 70% (0.40 uncertainty penalty if unmeasured).
    /// - Storage pressure curve: penalty if disk < 25% (0.40 uncertainty penalty if unmeasured).
    /// - Workspace & session density: penalty per loaded workspace, active session, and active query.
    /// - Running command penalty: high load penalty for currently executing commands (builds/tests).
    pub fn congestion_score(&self) -> f64 {
        if self.host.pressure().is_some() {
            return 1000.0 + self.load_per_cpu().unwrap_or(1.0);
        }

        // Unknown load is penalized conservatively (0.50) so an unmonitored node is not mistaken for completely idle.
        let mut score = self.load_per_cpu().unwrap_or(0.50);

        // Memory usage penalty (above 70% used) or uncertainty penalty when telemetry is absent
        match self.host.memory_used_share() {
            Some(mem_used) => {
                if mem_used > 0.70 {
                    score += (mem_used - 0.70) * 8.0;
                }
                if mem_used > MEMORY_PRESSURE_USED {
                    score += 50.0;
                }
            }
            None => {
                score += 0.40;
            }
        }

        // Disk space penalty (below 25% free) or uncertainty penalty when telemetry is absent
        match self.host.storage_free_share() {
            Some(disk_free) => {
                if disk_free < 0.25 {
                    score += (0.25 - disk_free) * 5.0;
                }
                if disk_free < STORAGE_PRESSURE_FREE {
                    score += 50.0;
                }
            }
            None => {
                score += 0.40;
            }
        }

        // Density penalties: loaded workspaces, sessions, active queries
        score += self.loaded_workspaces as f64 * 0.08;
        score += self.active_sessions as f64 * 0.05;
        score += self.active_queries as f64 * 0.10;

        // Running commands (builds, tests, checks) add significant instantaneous load
        score += self.running_commands.len() as f64 * 0.35;

        score
    }
}
