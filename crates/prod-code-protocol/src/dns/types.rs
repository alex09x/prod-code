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
use std::net::Ipv4Addr;

/// Default synthetic top-level domain for internal cluster resolution.
pub const CODE_INTERNAL_SUFFIX: &str = ".code.internal";

/// Root internal domain.
pub const CODE_INTERNAL_ROOT: &str = "code.internal";

/// Standard DNS SRV service prefix for prod-code.
pub const SRV_SERVICE_NAME: &str = "_prod-code._tcp";

/// Canonical DNS SRV domain for prod-code cluster discovery.
pub const SRV_SERVICE_DOMAIN: &str = "_prod-code._tcp.code.internal";

/// Standard DNS query types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DnsQueryType {
    A,
    TXT,
    SRV,
    ANY,
    Other(u16),
}

impl DnsQueryType {
    pub fn from_u16(val: u16) -> Self {
        match val {
            1 => Self::A,
            16 => Self::TXT,
            33 => Self::SRV,
            255 => Self::ANY,
            other => Self::Other(other),
        }
    }

    pub fn to_u16(self) -> u16 {
        match self {
            Self::A => 1,
            Self::TXT => 16,
            Self::SRV => 33,
            Self::ANY => 255,
            Self::Other(v) => v,
        }
    }
}

/// A parsed DNS Question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsQuestion {
    pub name: String,
    pub qtype: DnsQueryType,
    pub qclass: u16,
}

/// A DNS SRV record (RFC 2782).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsSrvRecord {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

/// A DNS Answer record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DnsRecordData {
    A(Ipv4Addr),
    SRV(DnsSrvRecord),
    TXT(String),
}

/// A full DNS Answer entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsAnswer {
    pub name: String,
    pub ttl: u32,
    pub data: DnsRecordData,
}
