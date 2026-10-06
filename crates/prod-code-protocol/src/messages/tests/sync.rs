/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::sync::content_hash;

#[test]
fn a_content_hash_depends_on_the_content_and_nothing_else() {
    assert_eq!(content_hash(b"abc"), content_hash(b"abc"));
    assert_ne!(content_hash(b"abc"), content_hash(b"abd"));
    assert_ne!(content_hash(b""), content_hash(b"\0"));
}
