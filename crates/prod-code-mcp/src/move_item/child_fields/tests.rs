/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{NeededFields, apply_pub_super};
use ra_ap_syntax::Edition;

#[test]
fn widens_only_selected_private_fields_after_attributes_and_preserves_existing_visibilities() {
    let item = "pub struct Config {\n    /// Endpoint host.\n    #[allow(dead_code)]\n    r#type: String,\n    secret: String,\n    pub open: u8,\n    pub(crate) shared: u8,\n    #[cfg(unix)]\n    handle: i32,\n    #[cfg(windows)]\n    handle: isize,\n}\n";
    let mut needed = NeededFields::default();
    needed.record_names.insert("type".to_string());
    needed.record_names.insert("handle".to_string());
    let widened = apply_pub_super(item, Edition::CURRENT, &needed).unwrap();
    assert_eq!(
        widened,
        "pub struct Config {\n    /// Endpoint host.\n    #[allow(dead_code)]\n    pub(super) r#type: String,\n    secret: String,\n    pub open: u8,\n    pub(crate) shared: u8,\n    #[cfg(unix)]\n    pub(super) handle: i32,\n    #[cfg(windows)]\n    pub(super) handle: isize,\n}\n"
    );
}

#[test]
fn widens_selected_tuple_fields_after_outer_attributes() {
    let item = "pub(super) struct Pair(#[allow(dead_code)] u32, u8, pub String);";
    let mut needed = NeededFields::default();
    needed.tuple_indices.insert(0);
    let widened = apply_pub_super(item, Edition::CURRENT, &needed).unwrap();
    assert_eq!(
        widened,
        "pub(super) struct Pair(#[allow(dead_code)] pub(super) u32, u8, pub String);"
    );
}
