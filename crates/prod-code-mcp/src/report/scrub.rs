/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

use super::types::Draft;

/// `text` without the reporter's private details: IPv4 addresses become `<node>` (loopback,
/// the unspecified address and the documentation ranges stay), the home directory and any
/// `/Users/<name>` or `/home/<name>` become `~`, and `host` (with its first label alone) becomes
/// `<host>`.
pub fn scrub(text: &str, home: Option<&str>, host: Option<&str>) -> String {
    let mut out = text.to_string();
    if let Some(home) = home.filter(|h| h.len() > 1) {
        out = out.replace(home.trim_end_matches('/'), "~");
    }
    for prefix in ["/Users/", "/home/"] {
        out = replace_user_dirs(&out, prefix);
    }
    out = replace_ipv4(&out);
    if let Some(host) = host.filter(|h| !h.is_empty() && *h != "unknown") {
        let short = host.split('.').next().unwrap_or(host);
        out = replace_word(&out, host, "<host>");
        if short.len() >= 3 {
            out = replace_word(&out, short, "<host>");
        }
    }
    out
}

/// `word` replaced wherever it is not part of a longer name: a host called `dev` must not turn
/// `device` into `<host>ice`.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let is_name = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(word) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + word.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_some_and(is_name) || after.is_some_and(is_name) {
            out.push_str(word);
        } else {
            out.push_str(with);
        }
        rest = &rest[at + word.len()..];
    }
    out.push_str(rest);
    out
}

/// `/Users/<name>/rest` and `/Users/<name>` at the end of a word become `~/rest` and `~`.
fn replace_user_dirs(text: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(prefix) {
        out.push_str(&rest[..at]);
        let after = &rest[at + prefix.len()..];
        let name_len = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'))
            .unwrap_or(after.len());
        if name_len == 0 {
            out.push_str(prefix);
            rest = after;
            continue;
        }
        out.push('~');
        rest = &after[name_len..];
    }
    out.push_str(rest);
    out
}

/// Every dotted IPv4 address in `text` that is not loopback, unspecified or a documentation
/// address becomes `<node>`; a port after it stays.
fn replace_ipv4(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let starts_word = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'.');
        if starts_word
            && bytes[i].is_ascii_digit()
            && let Some(len) = ipv4_at(&text[i..])
        {
            let address = &text[i..i + len];
            if keeps(address) {
                out.push_str(address);
            } else {
                out.push_str("<node>");
            }
            i += len;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The length of the IPv4 address `text` starts with, if it starts with one: four numbers of
/// 0-255 joined by dots, not followed by another digit, dot-digit or letter.
fn ipv4_at(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    for part in 0..4 {
        let start = at;
        while at < bytes.len() && bytes[at].is_ascii_digit() && at - start < 3 {
            at += 1;
        }
        if at == start || text[start..at].parse::<u16>().ok()? > 255 {
            return None;
        }
        if part < 3 {
            if bytes.get(at) != Some(&b'.') {
                return None;
            }
            at += 1;
        }
    }
    match bytes.get(at) {
        Some(b) if b.is_ascii_alphanumeric() => None,
        Some(b'.') if bytes.get(at + 1).is_some_and(|b| b.is_ascii_digit()) => None,
        _ => Some(at),
    }
}

/// Addresses that identify nothing: loopback, unspecified, and the documentation ranges.
fn keeps(address: &str) -> bool {
    address.starts_with("127.")
        || address == "0.0.0.0"
        || address.starts_with("192.0.2.")
        || address.starts_with("198.51.100.")
        || address.starts_with("203.0.113.")
}

/// The Environment section: the client's version, commit, and platform, and the platform,
/// version, commit, and engines of the node the checkout is placed on, when it answered.
pub fn environment(node: Option<&prod_code_protocol::StatusResponse>) -> String {
    let client_commit = prod_code_protocol::git_commit();
    let client_meta = if client_commit != "unknown" {
        format!(" (commit {client_commit})")
    } else {
        String::new()
    };
    let mut out = format!(
        "## Environment\n\n- client: prod-code {}{} on {}\n",
        env!("CARGO_PKG_VERSION"),
        client_meta,
        prod_code_protocol::platform()
    );
    if let Some(status) = node {
        let mut details = Vec::new();
        if let Some(ver) = &status.version {
            details.push(format!("prod-code {ver}"));
        }
        if let Some(commit) = &status.git_commit {
            details.push(format!("commit {commit}"));
        }
        let meta = if details.is_empty() {
            String::new()
        } else {
            format!(" ({})", details.join(", "))
        };
        out.push_str(&format!(
            "- node: {}{}, engines: {}\n",
            status.platform.as_deref().unwrap_or("unknown platform"),
            meta,
            status.detected_engines.join(", ")
        ));
    }
    out
}

/// The scrubbed title and the scrubbed body with the Environment section, or an error when the
/// report is too thin to act on.
pub fn draft(
    title: &str,
    body: &str,
    node: Option<&prod_code_protocol::StatusResponse>,
) -> Result<Draft> {
    let title = title.trim();
    let body = body.trim();
    anyhow::ensure!(
        title.chars().count() >= 10,
        "the title is too short to search for: say what went wrong and where, e.g. \
         `code_references returns nothing for a field in a Go struct`"
    );
    anyhow::ensure!(
        body.chars().count() >= 40,
        "the body must say what was run, what came back and what was expected"
    );
    let home = std::env::var("HOME").ok();
    let host = prod_code_protocol::client_host();
    let clean = |text: &str| scrub(text, home.as_deref(), Some(&host));
    Ok(Draft {
        title: clean(title),
        body: format!(
            "{}\n\n{}\n_Filed with `prod-code report-issue`._\n",
            clean(body),
            clean(&environment(node))
        ),
        labels: Vec::new(),
    })
}
