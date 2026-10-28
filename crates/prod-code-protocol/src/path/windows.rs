/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Windows workspace roots (`C:\repo`): a client on Windows names its files with a drive letter
//! and either separator, and its URIs as `file:///C:/repo/…`, the colon sometimes encoded
//! (`c%3A`). The translator maps them to and from the node's POSIX copy (#1523).

use url::Url;

/// Whether `path` is an absolute Windows path with a drive letter: `C:\…`, `C:/…` or `C:` alone.
pub(super) fn is_drive_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || matches!(bytes[2], b'\\' | b'/'))
}

/// `root` without trailing separators, with `\` between its components.
pub(super) fn normalize_root(root: &str) -> String {
    let trimmed = root.trim_end_matches(['\\', '/']);
    trimmed.replace('/', "\\")
}

/// The part of `value` after the Windows `root`, with `/` separators, empty or starting with
/// `/`, when `value` is under it at a whole component. Windows names are compared without
/// regard to case.
pub(super) fn rest_under(root: &str, value: &str) -> Option<String> {
    if !is_drive_absolute(value) || value.len() < root.len() {
        return None;
    }
    let (head, rest) = value.split_at(root.len());
    let same = head.chars().zip(root.chars()).all(|(a, b)| {
        a.eq_ignore_ascii_case(&b) || (matches!(a, '\\' | '/') && matches!(b, '\\' | '/'))
    });
    (same && (rest.is_empty() || rest.starts_with(['\\', '/']))).then(|| rest.replace('\\', "/"))
}

/// The Windows `root` followed by `rest` (empty or starting with `/`).
pub(super) fn join(root: &str, rest: &str) -> String {
    format!("{root}{}", rest.replace('/', "\\"))
}

/// The `file:` URI of the Windows path `path`: `file:///C:/repo/a%20b.rs`.
pub(super) fn uri_of(path: &str) -> String {
    let forward = format!("/{}", path.replace('\\', "/"));
    let mut url = Url::parse("file:///").expect("a file URL");
    url.set_path(&forward);
    url.into()
}

/// The Windows path a `file:` URI names, decoded once: `file:///c%3A/repo/x` is `c:\repo\x`.
/// With the URI's query and fragment, which a mapped URI keeps.
pub(super) fn path_of(uri: &str) -> Option<(String, Option<String>, Option<String>)> {
    let url = Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let decoded = percent_decode(url.path());
    let path = decoded.strip_prefix('/').unwrap_or(&decoded);
    is_drive_absolute(path).then(|| {
        (
            path.replace('/', "\\"),
            url.query().map(str::to_string),
            url.fragment().map(str::to_string),
        )
    })
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(byte) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
