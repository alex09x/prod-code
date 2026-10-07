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
use std::path::Path;

#[test]
fn test_ts_js_proves_import_positive() {
    let content = r#"import { calculate } from "./math";"#;
    assert!(proves_cross_file_import(
        content,
        Path::new("src/client.ts"),
        Path::new("src/math.ts"),
        "calculate",
        Language::TypeScript,
    ));

    let content_from_substring = r#"import { fromJSON } from "./codec";"#;
    assert!(proves_cross_file_import(
        content_from_substring,
        Path::new("src/client.ts"),
        Path::new("src/codec.ts"),
        "fromJSON",
        Language::TypeScript,
    ));

    let content_namespace = r#"import * as math from "./math";"#;
    assert!(proves_cross_file_import(
        content_namespace,
        Path::new("src/client.ts"),
        Path::new("src/math.ts"),
        "calculate",
        Language::TypeScript,
    ));

    let content_require_obj = r#"const math = require("./math");"#;
    assert!(proves_cross_file_import(
        content_require_obj,
        Path::new("src/client.ts"),
        Path::new("src/math.ts"),
        "calculate",
        Language::TypeScript,
    ));

    let content_require_destruct = r#"const { calculate } = require("./math");"#;
    assert!(proves_cross_file_import(
        content_require_destruct,
        Path::new("src/client.ts"),
        Path::new("src/math.ts"),
        "calculate",
        Language::TypeScript,
    ));

    let content_multiline = "import {\n  calculate,\n  other,\n} from \"./math\";";
    assert!(proves_cross_file_import(
        content_multiline,
        Path::new("src/client.ts"),
        Path::new("src/math.ts"),
        "calculate",
        Language::TypeScript,
    ));
}

#[test]
fn test_ts_js_proves_import_rejects_unrelated_alias() {
    let content = r#"import { unrelated as retry } from "./other";"#;
    assert!(!proves_cross_file_import(
        content,
        Path::new("repro/caller.ts"),
        Path::new("repro/selected.ts"),
        "retry",
        Language::TypeScript,
    ));

    let content_different_mod = r#"import { retry } from "./other";"#;
    assert!(!proves_cross_file_import(
        content_different_mod,
        Path::new("repro/caller.ts"),
        Path::new("repro/selected.ts"),
        "retry",
        Language::TypeScript,
    ));
}

#[test]
fn test_python_proves_import() {
    let content = "from db import find_user\n";
    assert!(proves_cross_file_import(
        content,
        Path::new("service.py"),
        Path::new("db.py"),
        "find_user",
        Language::Python,
    ));

    let content_import_as = "import db as database\n";
    assert!(proves_cross_file_import(
        content_import_as,
        Path::new("service.py"),
        Path::new("db.py"),
        "find_user",
        Language::Python,
    ));

    let content_other = "from other import find_user\n";
    assert!(!proves_cross_file_import(
        content_other,
        Path::new("service.py"),
        Path::new("db.py"),
        "find_user",
        Language::Python,
    ));

    let content_multiline_paren = "from db import (\n    # comment\n    find_user,\n)\n";
    assert!(proves_cross_file_import(
        content_multiline_paren,
        Path::new("service.py"),
        Path::new("db.py"),
        "find_user",
        Language::Python,
    ));
}
