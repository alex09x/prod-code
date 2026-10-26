/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_testkit::Workspace;

#[tokio::test]
async fn semicolon_python_call_is_not_import_context() {
    let client = "from db import find_user; print(find_user() + 1)\n";
    let files = [
        ("db.py", "def find_user() -> int:\n    return 1\n"),
        ("client.py", client),
    ];
    for precise in [false, true] {
        let ws = Workspace::new(&files);
        assert!(run(&ws, "python3", &["client.py"]).status.success());
        let refs = if precise {
            vec![("client.py", client.rfind("find_user").unwrap())]
        } else {
            vec![]
        };
        let preview = wrap(&ws, "db.py", "find_user", &refs, false, false)
            .await
            .unwrap();
        eprintln!(
            "python precise={precise}: propagated={} blocked={:?} unmatched={:?}",
            preview.propagated, preview.blocked, preview.unmatched
        );
        assert_eq!(preview.blocked.len(), 1);
        assert!(
            wrap(&ws, "db.py", "find_user", &refs, true, false)
                .await
                .is_err()
        );
        unchanged(&ws, &files);
    }
}

#[tokio::test]
async fn python_import_shapes_keep_real_calls_visible() {
    let shapes = [
        (
            "db.py",
            "import db as data; print(data.find_user() + 1)\n",
            "client.py",
        ),
        (
            "pkg/db.py",
            "from pkg import db as data; print(data.find_user() + 1)\n",
            "client.py",
        ),
        (
            "pkg/db.py",
            "from . import db as data; print(data.find_user() + 1)\n",
            "pkg/client.py",
        ),
        (
            "pkg/__init__.py",
            "from pkg import find_user as lookup; print(lookup() + 1)\n",
            "client.py",
        ),
        (
            "db.py",
            "from db import (\n    find_user as lookup,\n); print(lookup() + 1)\n",
            "client.py",
        ),
        (
            "db.py",
            "from db import \\\n    find_user as lookup\nprint(lookup() + 1)\n",
            "client.py",
        ),
    ];
    for (selected, client, path) in shapes {
        let files = [
            (selected, "def find_user() -> int:\n    return 1\n"),
            (path, client),
        ];
        let ws = Workspace::new(&files);
        let args = if path == "pkg/client.py" {
            vec!["-m", "pkg.client"]
        } else {
            vec![path]
        };
        assert!(run(&ws, "python3", &args).status.success());
        let preview = wrap(&ws, selected, "find_user", &[], false, false)
            .await
            .unwrap();
        assert_eq!(preview.blocked.len(), 1, "{client}: {preview:?}");
        assert!(preview.unmatched.is_empty(), "{preview:?}");
        assert!(
            wrap(&ws, selected, "find_user", &[], true, false)
                .await
                .is_err()
        );
        unchanged(&ws, &files);
    }
}

#[tokio::test]
async fn compatible_python_optional_return_still_applies_and_executes() {
    let client = "from typing import Optional\nfrom db import (\n    find_user,\n)\ndef run() -> Optional[int]:\n    return find_user()\nassert run() == 1\n";
    let files = [
        (
            "db.py",
            "from typing import Optional\ndef find_user() -> int:\n    return 1\n",
        ),
        ("client.py", client),
    ];
    for precise in [false, true] {
        let ws = Workspace::new(&files);
        assert!(run(&ws, "python3", &["client.py"]).status.success());
        let refs = if precise {
            vec![("client.py", client.rfind("find_user").unwrap())]
        } else {
            vec![]
        };
        let result = wrap(&ws, "db.py", "find_user", &refs, true, false)
            .await
            .unwrap();
        assert_eq!(result.propagated, 1, "{result:?}");
        assert!(result.applied);
        unchanged(&ws, &files[1..]);
        assert!(run(&ws, "python3", &["client.py"]).status.success());
    }
}
