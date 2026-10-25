/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{SINGLE, assert_routes, fixture};

#[tokio::test]
async fn issue_1330_review_literals_non_utf8_preserves_imports() {
    for source in [
        "package main\nimport _ \"example.org/apple\"\nvar _ = \"\\xff\"\nfunc main() {}\n",
        "package main\nimport (\n _ \"example.org/apple\"\n)\nvar _ = \"\\377\"\nfunc main() {}\n",
        "package main\nimport _ \"example.org/apple\"\nvar _ = \"\\xff\\xfe\"\nfunc main() {}\n",
    ] {
        let temp = fixture(SINGLE, source);
        assert_routes(temp.path(), Some("apple/apple.go")).await;
    }
}

#[tokio::test]
async fn issue_1330_review_literals_comment_import_boundary() {
    for source in [
        "package main\nimport _ \"fmt\" /*\n*/ var _ = \"example.org/apple\"\nfunc main() {}\n",
        "package main\nimport _ \"fmt\" /* comment\n\n*/ var _ = `example.org/apple`\nfunc main() {}\n",
        "package main\nimport (\n _ \"fmt\" /*\n*/\n)\nvar _ = \"example.org/apple\"\nfunc main() {}\n",
    ] {
        let temp = fixture(SINGLE, source);
        assert_routes(temp.path(), None).await;
    }
}

#[tokio::test]
async fn issue_1330_review_literals_comments_with_dependency() {
    for source in [
        "package main\nimport /*\n*/ _ \"example.org/apple\"\nvar _ = \"\\xff\"\nfunc main() {}\n",
        "package main\nimport (\n _ \"fmt\" /*\n*/ _ \"example.org/apple\"\n)\nfunc main() {}\n",
        "package main\nimport _ /* no newline */ \"example.org/apple\"\nfunc main() {}\n",
        "package main\nimport (\n _ \"fmt\"; _ \"example.org/apple\"\n)\nfunc main() {}\n",
    ] {
        let temp = fixture(SINGLE, source);
        assert_routes(temp.path(), Some("apple/apple.go")).await;
    }
}
