/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Usage-metrics event logging level.

use serde::{Deserialize, Serialize};

/// Granularity of usage-metrics events recorded to JSONL files and in-memory ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MetricsEventLevel {
    /// Record real operations (sync, lsp, exec, place, read_file, search, shadow, cluster)
    /// and failed operations of any kind. Successful status polls write no line and take no
    /// slot in the in-memory ring; aggregate counters still count them for totals and Prometheus.
    #[default]
    Operations,
    /// Record all events including successful status polls.
    All,
}

impl MetricsEventLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operations => "operations",
            Self::All => "all",
        }
    }
}

impl std::fmt::Display for MetricsEventLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MetricsEventLevel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "operations" => Ok(MetricsEventLevel::Operations),
            "all" => Ok(MetricsEventLevel::All),
            other => anyhow::bail!(
                "invalid metrics event_level: '{other}'; accepted values are 'operations' or 'all'"
            ),
        }
    }
}

impl<'de> Deserialize<'de> for MetricsEventLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
