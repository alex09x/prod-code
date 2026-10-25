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

#[tokio::test]
async fn coverage_b2_static_rejects_unsupported_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("source.txt");
    let remote = "127.0.0.1:1".parse().unwrap();
    assert_eq!(
        make_static_polyglot(remote, dir.path(), &file, None, "f", true, false)
            .await
            .unwrap_err()
            .to_string(),
        format!("Cannot read {}", file.display())
    );
    std::fs::write(&file, "unchanged").unwrap();
    assert_eq!(
        make_static_polyglot(remote, dir.path(), &file, None, "f", true, false)
            .await
            .unwrap_err()
            .to_string(),
        format!("Unsupported language for file: {}", file.display())
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "unchanged");
}
