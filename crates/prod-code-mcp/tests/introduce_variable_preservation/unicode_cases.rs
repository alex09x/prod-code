/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::cases::{Case, case};

pub fn collisions() -> Vec<Case> {
    [
        ("nfc-collision-kelvin", "K", "K"),
        ("nfc-collision-kelvin-reverse", "K", "K"),
        ("nfc-collision-raw-kelvin", "r#K", "K"),
        ("nfc-collision-composed", "e\u{301}", "é"),
    ]
    .into_iter()
    .map(|(id, existing, requested)| {
        let mut c = case(
            id,
            &format!(
                "fn run(w: u32, {existing}: u32) -> (u32, u32) {{\n    let a = w + 1;\n    let b = w + 1;\n    (b, {existing})\n}}\nfn main() {{ println!(\"{{:?}}\", run(2, 100)); }}\n"
            ),
            "w + 1",
        );
        c.name = requested;
        c
    })
    .collect()
}

pub fn positive() -> Vec<Case> {
    [
        ("nfc-fresh-unicode-name", "K", "100"),
        // NFC preserves case and does not perform compatibility normalization.
        ("nfc-case-sensitive-name", "k", "K"),
        ("nfc-compatibility-distinct-name", "Ｋ", "K"),
    ]
    .into_iter()
    .map(|(id, requested, retained)| {
        let mut c = case(
            id,
            &format!(
                "fn run(w: u32, K: u32) -> (u32, u32) {{\n    let a = w + 1;\n    let b = w + 1;\n    (b, {retained})\n}}\nfn main() {{ println!(\"{{:?}}\", run(2, 100)); }}\n"
            ),
            "w + 1",
        );
        if id == "nfc-fresh-unicode-name" {
            c.source = c.source.replace("K: u32", "other: u32");
        }
        c.name = requested;
        c.refuse = false;
        c
    })
    .collect()
}
