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

#[test]
fn test_c_cpp_proves_import_via_header() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let compute_cpp = root.join("compute.cpp");
    std::fs::write(
        &compute_cpp,
        "#include \"api.h\"\nint calculate(int x) { return x * 2; }\n",
    )
    .unwrap();

    let api_h = root.join("api.h");
    let api_content = "int calculate(int x);\n";
    assert!(proves_cross_file_import(
        api_content,
        &api_h,
        &compute_cpp,
        "calculate",
        Language::Cpp,
    ));

    let client_cpp = root.join("client.cpp");
    let client_content = "#include \"api.h\"\nvoid run() { calculate(5); }\n";
    assert!(proves_cross_file_import(
        client_content,
        &client_cpp,
        &compute_cpp,
        "calculate",
        Language::Cpp,
    ));

    let other_cpp = root.join("other.cpp");
    let other_content = "#include \"other.h\"\nvoid run() { calculate(5); }\n";
    assert!(!proves_cross_file_import(
        other_content,
        &other_cpp,
        &compute_cpp,
        "calculate",
        Language::Cpp,
    ));
}

#[test]
fn test_python_proves_import_from_module() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let db_py = root.join("db.py");
    std::fs::write(&db_py, "def find_user(id):\n    return {}\n").unwrap();

    let client1_py = root.join("client1.py");
    let content1 = "from . import db\ndef run():\n    return db.find_user(1)\n";
    assert!(proves_cross_file_import(
        content1,
        &client1_py,
        &db_py,
        "find_user",
        Language::Python,
    ));

    let client2_py = root.join("client2.py");
    let content2 = "from package import db as my_db\ndef run():\n    return my_db.find_user(1)\n";
    assert!(proves_cross_file_import(
        content2,
        &client2_py,
        &db_py,
        "find_user",
        Language::Python,
    ));

    let client3_py = root.join("client3.py");
    let content3 = "from package import other\ndef run():\n    return find_user(1)\n";
    assert!(!proves_cross_file_import(
        content3,
        &client3_py,
        &db_py,
        "find_user",
        Language::Python,
    ));
}

#[test]
fn test_ts_js_imported_symbols_tracks_alias() {
    let content = r#"import { retry as again } from "./selected";"#;
    let syms = imported_caller_symbols(
        content,
        Path::new("repro/caller.ts"),
        Path::new("repro/selected.ts"),
        "retry",
        Language::TypeScript,
    );
    assert_eq!(syms, vec!["again"]);

    let content_require = r#"const { retry: again } = require("./selected");"#;
    let syms_require = imported_caller_symbols(
        content_require,
        Path::new("repro/caller.ts"),
        Path::new("repro/selected.ts"),
        "retry",
        Language::TypeScript,
    );
    assert_eq!(syms_require, vec!["again"]);

    let content_unrelated = r#"import { unrelated as retry } from "./selected";"#;
    let syms_unrelated = imported_caller_symbols(
        content_unrelated,
        Path::new("repro/caller.ts"),
        Path::new("repro/selected.ts"),
        "retry",
        Language::TypeScript,
    );
    assert!(syms_unrelated.is_empty());
}

#[test]
fn test_python_imported_symbols_tracks_alias() {
    let content = "from selected import retry as again\n";
    let syms = imported_caller_symbols(
        content,
        Path::new("repro/caller.py"),
        Path::new("repro/selected.py"),
        "retry",
        Language::Python,
    );
    assert_eq!(syms, vec!["again"]);

    let content_unrelated = "from selected import other as retry\n";
    let syms_unrelated = imported_caller_symbols(
        content_unrelated,
        Path::new("repro/caller.py"),
        Path::new("repro/selected.py"),
        "retry",
        Language::Python,
    );
    assert!(syms_unrelated.is_empty());
}

#[test]
fn test_ts_js_imported_symbols_handles_identifier_containing_import() {
    let content = r#"import { important } from "./selected";"#;
    let syms = imported_caller_symbols(
        content,
        Path::new("src/caller.ts"),
        Path::new("src/selected.ts"),
        "important",
        Language::TypeScript,
    );
    assert_eq!(syms, vec!["important"]);
}

#[test]
fn test_python_recognizes_init_package_imports() {
    let content = "from pkg import find_user\n";
    let syms = imported_caller_symbols(
        content,
        Path::new("caller.py"),
        Path::new("pkg/__init__.py"),
        "find_user",
        Language::Python,
    );
    assert_eq!(syms, vec!["find_user"]);

    let content_rel = "from . import find_user\n";
    let syms_rel = imported_caller_symbols(
        content_rel,
        Path::new("pkg/service.py"),
        Path::new("pkg/__init__.py"),
        "find_user",
        Language::Python,
    );
    assert_eq!(syms_rel, vec!["find_user"]);
}

#[test]
fn test_ts_recognizes_index_module_imports() {
    let content = r#"import { find_user } from "./pkg";"#;
    let syms = imported_caller_symbols(
        content,
        Path::new("src/caller.ts"),
        Path::new("src/pkg/index.ts"),
        "find_user",
        Language::TypeScript,
    );
    assert_eq!(syms, vec!["find_user"]);
}

#[test]
fn test_ts_js_recognizes_require_with_whitespace() {
    let content = r#"const { retry } = require ("./selected");"#;
    let syms = imported_caller_symbols(
        content,
        Path::new("src/caller.js"),
        Path::new("src/selected.js"),
        "retry",
        Language::JavaScript,
    );
    assert_eq!(syms, vec!["retry"]);

    let content_tab = "const { retry: myRetry } = require\t(\"./selected\");";
    let syms_tab = imported_caller_symbols(
        content_tab,
        Path::new("src/caller.js"),
        Path::new("src/selected.js"),
        "retry",
        Language::JavaScript,
    );
    assert_eq!(syms_tab, vec!["myRetry"]);
}

#[test]
fn test_has_require_call_whitespace_and_boundaries() {
    use crate::wrap_return::utils::has_require_call;
    assert!(has_require_call(
        r#"const { retry } = require ("./selected");"#
    ));
    assert!(has_require_call(r#"const res = require("./selected");"#));
    assert!(!has_require_call(r#"const required = true;"#));
    assert!(!has_require_call(r#"const is_required = require_func();"#));
}

#[test]
fn test_python_recognizes_semicolon_separated_imports() {
    let content = "from db import find_user; find_user()\n";
    let syms = imported_caller_symbols(
        content,
        Path::new("caller.py"),
        Path::new("db.py"),
        "find_user",
        Language::Python,
    );
    assert_eq!(syms, vec!["find_user"]);

    let content_mod = "import db; db.find_user()\n";
    let syms_mod = imported_caller_symbols(
        content_mod,
        Path::new("caller.py"),
        Path::new("db.py"),
        "find_user",
        Language::Python,
    );
    assert_eq!(syms_mod, vec!["find_user"]);
}
