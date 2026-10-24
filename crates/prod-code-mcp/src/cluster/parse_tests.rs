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

#[test]
fn runtime_paths_2_discovery_seed_inputs_deduplicate_and_ignore_modes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cluster.json");
    let a: SocketAddr = "127.0.0.1:9001".parse().unwrap();
    let b: SocketAddr = "127.0.0.1:9002".parse().unwrap();
    let c: SocketAddr = "[::1]:9003".parse().unwrap();
    for contents in [
        None,
        Some("invalid"),
        Some("{}"),
        Some("{\"nodes\":[\"bad\",\"127.0.0.1:9001\",\"127.0.0.1:9001\"]}"),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&path, contents).unwrap();
        }
        let specs = [
            " auto ",
            "10G",
            "TRUE",
            "1",
            "LAN",
            " ,127.0.0.1:9001,127.0.0.1:9002,127.0.0.1:9001,",
            "[::1]:9003",
            "invalid address",
        ]
        .map(String::from);
        assert_eq!(discovery_seeds(Some(&path), &specs), [a, b, c]);
    }
    assert_eq!(discovery_seeds(None, &[]), []);
}

#[test]
fn runtime_paths_2_parse_numeric_remotes_and_errors() {
    assert_eq!(
        parse_remotes(" , [::1]:9001, 127.0.0.1:9002, [::1]:9001, ").unwrap(),
        [
            "[::1]:9001".parse::<SocketAddr>().unwrap(),
            "127.0.0.1:9002".parse().unwrap()
        ]
    );
    assert_eq!(
        parse_remotes("127.0.0.1").unwrap(),
        ["127.0.0.1:9400".parse::<SocketAddr>().unwrap()]
    );
    assert_eq!(
        parse_remotes(" , ").unwrap_err().to_string(),
        "no gateway addresses given"
    );
    let err = parse_remotes("invalid address").unwrap_err().to_string();
    assert!(
        err.starts_with("cannot resolve gateway address invalid address:"),
        "{err}"
    );
}
