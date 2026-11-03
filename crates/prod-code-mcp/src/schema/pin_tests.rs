/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::cluster::pin_fake::{FakeGateway, Kind, checkout, write};

#[tokio::test]
async fn issue_1793_schema_repositories_hold_their_pins_without_partial_writes() {
    for force in [false, true] {
        let (_a, a) = checkout(&[]);
        let (_b, b) = checkout(&[]);
        write(&a, "config.json", "{\"alpha\": 1}\n");
        write(&b, "config.json", "{\"alpha\": 2}\n");
        write(&b, "src/lib.rs", "pub struct Packet { pub alpha: u32 }\n");
        let home = FakeGateway::start(Kind::Current).await;
        let pinned = FakeGateway::start(Kind::Old).await;
        write(
            &b,
            "prod-code.toml",
            &format!("[cluster]\npin = \"{}\"\n", pinned.addr),
        );
        let files = [
            a.join("config.json"),
            b.join("config.json"),
            b.join("src/lib.rs"),
        ];
        let before: Vec<_> = files.iter().map(|p| std::fs::read(p).unwrap()).collect();
        let result = super::rename_across(home.addr, &[a, b], "alpha", "beta", true, force).await;
        assert!(
            result.is_err(),
            "a pinned semantic rename failure must abort: {result:?}"
        );
        assert_eq!(
            pinned.frames(),
            ["NodePinRequest"],
            "the target checkout must reach its own pinned node before any sync"
        );
        for (file, bytes) in files.iter().zip(before) {
            assert_eq!(
                std::fs::read(file).unwrap(),
                bytes,
                "neither code nor schema/config text is partially written"
            );
        }
    }
}
