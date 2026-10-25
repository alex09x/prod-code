/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::Ipv6Addr;

/// Replace parsed IPv6 literals, including their optional zone identifiers. Brackets,
/// ports and punctuation stay outside the replacement. Parse before the IPv4 pass so
/// embedded dotted addresses are handled as part of the IPv6 literal.
pub(super) fn replace_ipv6(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        // A single colon can separate a compact log label from an address.
        // Keep `::` path separators blocked, including all-hex Rust identifiers.
        let after_label = at > 0 && bytes[at - 1] == b':' && (at == 1 || bytes[at - 2] != b':');
        let boundary = after_label
            || at == 0
            || !(bytes[at - 1].is_ascii_alphanumeric()
                || matches!(bytes[at - 1], b'_' | b':' | b'.'));
        if boundary && let Some((len, address)) = address_at(&text[at..]) {
            if keeps(address) {
                out.push_str(&text[at..at + len]);
            } else {
                out.push_str("<node>");
            }
            at += len;
        } else {
            let ch = text[at..].chars().next().unwrap();
            out.push(ch);
            at += ch.len_utf8();
        }
    }
    out
}

fn address_at(text: &str) -> Option<(usize, Ipv6Addr)> {
    let bytes = text.as_bytes();
    let core_len = bytes
        .iter()
        .take_while(|&&b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
        .count();
    // A compact error label may follow one punctuation colon. Its first bytes
    // can themselves be hex digits (`error`, `closed`), so retry before the last
    // colon rather than merely trimming the hexadecimal prefix. Rust `::`
    // separators leave an invalid IPv6 prefix and are not accepted here.
    if bytes
        .get(core_len)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        let separator = text[..core_len].rfind(':')?;
        let address = text[..separator].parse::<Ipv6Addr>().ok()?;
        return Some((separator, address));
    }
    // A sentence-ending dot is punctuation, including after a dotted IPv4 suffix.
    let mut core = text[..core_len].trim_end_matches('.');
    let address = loop {
        if let Ok(address) = core.parse::<Ipv6Addr>() {
            break address;
        }
        // Prefer the full literal so valid trailing `::` stays part of the address.
        // On parse failure, a colon after the literal may instead be punctuation.
        core = core.strip_suffix(':')?;
    };
    let mut len = core.len();
    if bytes.get(len) == Some(&b'%') {
        let zone_len = bytes[len + 1..]
            .iter()
            .take_while(|&&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            .count();
        // RFC 6874's URI-encoded %25 delimiter is consumed along with the zone.
        len += 1 + text[len + 1..len + 1 + zone_len]
            .trim_end_matches('.')
            .len();
    }
    if bytes
        .get(len)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        return None;
    }
    Some((len, address))
}

fn keeps(address: Ipv6Addr) -> bool {
    address.is_loopback()
        || address.is_unspecified()
        || address.segments()[..2] == [0x2001, 0x0db8]
        || address
            .to_ipv4_mapped()
            .is_some_and(|v4| super::scrub::keeps(&v4.to_string()))
}
