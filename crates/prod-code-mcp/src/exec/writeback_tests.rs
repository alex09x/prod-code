/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::Writeback;
use prod_code_protocol::FileDelta;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// `ruff format` on the node rewrote an executable script whose node copy was 0644 (an older
/// client never sent the bit). The write-back used to reset the local 0755 to 0644; it now
/// keeps 0755, an existing 0640 and a plain 0644 exactly, and still applies the new bytes.
#[test]
fn issue_1823_formatter_writeback_keeps_local_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    crate::sync::clear_sync_cache(root);
    let files = [
        ("tool.py", "#!/usr/bin/env python3\nprint( 'a' )\n", 0o755),
        ("private.py", "X=1\n", 0o640),
        ("plain.py", "Y=2\n", 0o644),
    ];
    for (rel, text, bits) in files {
        std::fs::write(root.join(rel), text).unwrap();
        std::fs::set_permissions(root.join(rel), std::fs::Permissions::from_mode(bits)).unwrap();
    }
    let mut writeback = Writeback::capture(root, true, true).unwrap();
    let formatted = |rel: &str, text: &str, is_executable| FileDelta {
        relative_path: rel.to_string(),
        content: Some(text.as_bytes().to_vec()),
        is_executable,
    };
    writeback
        .apply(
            root,
            "",
            vec![
                formatted("tool.py", "#!/usr/bin/env python3\nprint('a')\n", false),
                formatted("private.py", "X = 1\n", false),
                formatted("plain.py", "Y = 2\n", true),
            ],
        )
        .unwrap();
    let mut pulled = writeback.pulled.clone();
    pulled.sort();
    assert_eq!(pulled, ["plain.py", "private.py", "tool.py"]);
    assert!(writeback.kept.is_empty(), "{:?}", writeback.kept);
    assert_eq!(
        std::fs::read_to_string(root.join("tool.py")).unwrap(),
        "#!/usr/bin/env python3\nprint('a')\n"
    );
    assert_eq!(mode(&root.join("tool.py")), 0o755);
    assert_eq!(mode(&root.join("private.py")), 0o640);
    assert_eq!(mode(&root.join("plain.py")), 0o644);
    crate::sync::clear_sync_cache(root);
}
