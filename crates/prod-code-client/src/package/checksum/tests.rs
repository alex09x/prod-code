/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn compute_sha256_matches_known_digests() {
    let dir = tempfile::tempdir().unwrap();
    let abc = dir.path().join("abc.bin");
    std::fs::write(&abc, "abc").unwrap();
    assert_eq!(compute_sha256(&abc).unwrap(), ABC_SHA256);

    let empty = dir.path().join("empty.bin");
    std::fs::write(&empty, "").unwrap();
    assert_eq!(
        compute_sha256(&empty).unwrap(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn compute_sha256_rejects_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    let err = compute_sha256(&missing).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("sha256 command failed on {}", missing.display())
    );
}

#[test]
fn parse_checksums_file_table() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("", &[]),
        ("\n  \n# only a comment\n", &[]),
        // Hash is lowercased, a binary-mode `*` marker is stripped, CRLF is tolerated.
        (
            "ABCDEF  file-a\r\n0123 *file-b\r\n",
            &[("file-a", "abcdef"), ("file-b", "0123")],
        ),
        // Lines with a single field are ignored, extra fields after the name too.
        ("lonelyhash\nfeed  name extra words\n", &[("name", "feed")]),
        // A later duplicate wins; indented comment is skipped.
        ("  # hidden  x\n11  dup\n22  dup\n", &[("dup", "22")]),
    ];
    for (text, expected) in cases {
        let got = parse_checksums_file(text);
        let want: HashMap<String, String> = expected
            .iter()
            .map(|(n, h)| (n.to_string(), h.to_string()))
            .collect();
        assert_eq!(got, want, "input: {text:?}");
    }
}

/// Serves `status` and `body` to the first connection on a loopback port.
fn serve_once(status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(reply.as_bytes()).unwrap();
    });
    format!("http://127.0.0.1:{port}/SHA256SUMS")
}

#[test]
fn fetch_checksums_from_url_parses_the_served_manifest() {
    let url = serve_once(
        "200 OK",
        "# manifest\nAA11  prod-code-x86_64-unknown-linux-gnu\nbb22 *prod-code-aarch64-apple-darwin\n",
    );
    let map = fetch_checksums_from_url(&url, "v9.9.9").unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map["prod-code-x86_64-unknown-linux-gnu"], "aa11");
    assert_eq!(map["prod-code-aarch64-apple-darwin"], "bb22");
}

#[test]
fn fetch_checksums_from_url_names_the_tag_on_http_failure() {
    let url = serve_once("404 Not Found", "missing");
    let err = fetch_checksums_from_url(&url, "v0.0.1").unwrap_err();
    assert_eq!(
        err.to_string(),
        "failed to fetch official checksums for v0.0.1"
    );
}

#[test]
fn checksums_url_and_default_tag() {
    assert_eq!(
        checksums_url("v1.2.3"),
        "https://github.com/alex09x/prod-code/releases/download/v1.2.3/SHA256SUMS"
    );
    assert_eq!(
        release_tag(&serde_json::json!({"tag_name": "v0.4.0"})),
        "v0.4.0"
    );
    // Without a usable tag_name the manifest of the pinned fallback release is used.
    assert_eq!(release_tag(&serde_json::json!({})), "v0.3.19");
    assert_eq!(release_tag(&serde_json::json!({"tag_name": 7})), "v0.3.19");
}
