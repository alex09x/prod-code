/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Lightweight DNS & Service Discovery for prod-code cluster (*.code.internal).
//!
//! Provides embedded DNS/mDNS resolution, project-to-node mapping, SRV record
//! encoding/decoding (RFC 1035 / RFC 2782), and zero-configuration cluster discovery (Phase 5.2).

mod smart;
mod types;
mod wire;

#[cfg(test)]
mod tests;

pub use smart::{
    extract_project_name, generate_srv_records, handle_dns_packet, is_code_internal_domain,
    is_srv_service_domain, resolve_project_node, resolve_smart_domain,
};
pub use types::{
    CODE_INTERNAL_ROOT, CODE_INTERNAL_SUFFIX, DnsAnswer, DnsQueryType, DnsQuestion, DnsRecordData,
    DnsSrvRecord, SRV_SERVICE_DOMAIN, SRV_SERVICE_NAME,
};
pub use wire::{decode_dns_name, encode_dns_name, format_dns_response, parse_dns_query, query_dns};
