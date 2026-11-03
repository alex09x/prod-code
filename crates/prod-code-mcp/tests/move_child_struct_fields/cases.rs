/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{MANIFEST, all_coords_of, compile_and_run_tests, coord_of, locations_across};
use prod_code_mcp::move_item::move_item;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};

#[tokio::test]
async fn child_move_preserves_impl_self_fru_and_matches_rest_pattern() {
    let fru_src = "\
struct Builder {
    width: u32,
    height: u32,
}

impl Builder {
    pub fn new(width: u32) -> Self {
        Self { width, height: 10 }
    }

    pub fn resize(self, width: u32) -> Self {
        Builder { width, ..self }
    }

    pub fn area(&self) -> u32 {
        self.width * self.height
    }
}

pub fn area_after_resize() -> u32 {
    Builder::new(2).resize(5).area()
}
";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "pub mod a;\n#[test]\nfn check() { assert_eq!(a::area_after_resize(), 50); }\n",
        ),
        ("src/a.rs", fru_src),
    ]);
    let file = ws.path("src/a.rs");
    let mut refs = all_coords_of(&file, fru_src, "Builder");
    refs.retain(|(_, l, _)| *l != 1);
    let gw = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("Builder", 23, 1, 4, 8)]),
        "textDocument/references" => locations_across(&refs),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    move_item(
        gw.addr(),
        &ws.root(),
        &file,
        1,
        8,
        &ws.path("src/a/types.rs"),
        true,
        false,
    )
    .await
    .unwrap();
    let types_rs = ws.read("src/a/types.rs");
    assert!(
        types_rs.contains("pub(super) struct Builder {"),
        "{types_rs}"
    );
    assert!(
        types_rs.contains("    pub(super) width: u32,\n"),
        "{types_rs}"
    );
    assert!(
        types_rs.contains("    pub(super) height: u32,\n"),
        "{types_rs}"
    );
    compile_and_run_tests(&ws.root(), &ws.path("src/lib.rs"));

    let pat_src = "\
#[derive(Default)]
struct Packet {
    header: u16,
    payload: u32,
}

pub fn verify() {
    let pkt = Packet::default();
    assert!(matches!(pkt, Packet { header: 0, .. }));
}
";
    let ws2 = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "pub mod a;\n#[test]\nfn check() { a::verify(); }\n",
        ),
        ("src/a.rs", pat_src),
    ]);
    let file2 = ws2.path("src/a.rs");
    let mut refs2 = all_coords_of(&file2, pat_src, "Packet");
    refs2.retain(|(_, l, _)| *l != 2);
    let gw2 = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("Packet", 23, 2, 5, 8)]),
        "textDocument/references" => locations_across(&refs2),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    move_item(
        gw2.addr(),
        &ws2.root(),
        &file2,
        2,
        8,
        &ws2.path("src/a/wire.rs"),
        true,
        false,
    )
    .await
    .unwrap();
    let wire_rs = ws2.read("src/a/wire.rs");
    assert!(
        wire_rs.contains("    pub(super) header: u16,\n"),
        "{wire_rs}"
    );
    assert!(wire_rs.contains("    payload: u32,\n"), "{wire_rs}");
    compile_and_run_tests(&ws2.root(), &ws2.path("src/lib.rs"));
}

#[tokio::test]
async fn child_move_handles_tuple_struct_positional_read_and_refuses_ambiguity() {
    let tup_src = "\
#[derive(Default)]
struct Pair(u32, u64, pub bool);

pub fn first_plus_anon() -> u32 {
    let p = Pair::default();
    let anon = (1u8, 2u8);
    p.0 + anon.1 as u32
}
";
    let ws = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        (
            "src/lib.rs",
            "pub mod a;\n#[test]\nfn check() { assert_eq!(a::first_plus_anon(), 2); }\n",
        ),
        ("src/a.rs", tup_src),
    ]);
    let file = ws.path("src/a.rs");
    let field0_def = coord_of(tup_src, "u32, u64");
    let field0_use = coord_of(tup_src, "0 + anon");
    let mut refs = all_coords_of(&file, tup_src, "Pair");
    refs.retain(|(_, l, _)| *l != 2);
    let file_for_gw = file.clone();
    let gw = ScriptedGateway::start(move |method, params| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("Pair", 23, 2, 2, 8)]),
        "textDocument/references" => locations_across(&refs),
        "textDocument/definition" => {
            let l = params["position"]["line"].as_u64().unwrap() as u32 + 1;
            let c = params["position"]["character"].as_u64().unwrap() as u32 + 1;
            if (l, c) == field0_use {
                answers::locations(&file_for_gw, &[field0_def])
            } else {
                json!([])
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    move_item(
        gw.addr(),
        &ws.root(),
        &file,
        2,
        8,
        &ws.path("src/a/pair.rs"),
        true,
        false,
    )
    .await
    .unwrap();
    let pair_rs = ws.read("src/a/pair.rs");
    assert_eq!(
        pair_rs.trim(),
        "#[derive(Default)]\npub(super) struct Pair(pub(super) u32, u64, pub bool);"
    );
    compile_and_run_tests(&ws.root(), &ws.path("src/lib.rs"));

    let macro_src = "macro_rules! build { ($t:ident) => { $t::default() }; }\n#[derive(Default)]\nstruct Secret { token: u8 }\npub fn make() -> u8 { let _ = build!(Secret); 0 }\n";
    let ws_mac = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub mod a;\n"),
        ("src/a.rs", macro_src),
    ]);
    let file_mac = ws_mac.path("src/a.rs");
    let mut refs_mac = all_coords_of(&file_mac, macro_src, "Secret");
    refs_mac.retain(|(_, l, _)| *l != 3);
    let gw_mac = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("Secret", 23, 3, 3, 8)]),
        "textDocument/references" => locations_across(&refs_mac),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let err = move_item(
        gw_mac.addr(),
        &ws_mac.root(),
        &file_mac,
        3,
        8,
        &ws_mac.path("src/a/sub.rs"),
        true,
        false,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("build!") && err.contains("nothing was written"),
        "{err}"
    );
    assert!(!ws_mac.path("src/a/sub.rs").exists());
    assert_eq!(ws_mac.read("src/a.rs"), macro_src);

    let unres_src = "#[derive(Default)]\nstruct Secret { token: u8 }\n#[cfg(windows)]\nfn win_only(s: &Secret) -> u8 { s.token }\n";
    let ws_unres = Workspace::new(&[
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", "pub mod a;\n"),
        ("src/a.rs", unres_src),
    ]);
    let file_unres = ws_unres.path("src/a.rs");
    let mut refs_unres = all_coords_of(&file_unres, unres_src, "Secret");
    refs_unres.retain(|(_, l, _)| *l != 2);
    let gw_unres = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => json!([answers::document_symbol("Secret", 23, 2, 2, 8)]),
        "textDocument/references" => locations_across(&refs_unres),
        "textDocument/definition" => Value::Null,
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await;
    let err2 = move_item(
        gw_unres.addr(),
        &ws_unres.root(),
        &file_unres,
        2,
        8,
        &ws_unres.path("src/a/sub.rs"),
        true,
        false,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        err2.contains("Secret::token") && err2.contains("nothing was written"),
        "{err2}"
    );
    assert!(!ws_unres.path("src/a/sub.rs").exists());
    assert_eq!(ws_unres.read("src/a.rs"), unres_src);
}
