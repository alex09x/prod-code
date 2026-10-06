/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::config::find_gopls_binary;

#[test]
fn test_gopls_binary_discovery() {
    let found = find_gopls_binary(None);
    // On systems with go/bin/gopls installed, verify it resolves
    if let Some(path) = &found {
        assert!(path.exists());
    }
}
