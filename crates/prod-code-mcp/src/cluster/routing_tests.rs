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
async fn runtime_paths_2_routing_unknown_projects_and_directory_engine_selection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let default = "127.0.0.1:1".parse().unwrap();
    assert_eq!(engine_for_path(&root, "missing"), None);
    assert_eq!(
        route_in(&[], "sample", default, &root, "missing", None)
            .await
            .unwrap(),
        default
    );
    assert_eq!(route_for_path(default, &root, None).await.unwrap(), default);
    assert_eq!(route_for_checkout(default, &root).await.unwrap(), default);
    assert_eq!(known_cluster_nodes(), None);
    std::fs::create_dir(root.join("code")).unwrap();
    std::fs::create_dir(root.join("code/subdir")).unwrap();
    std::fs::write(root.join("code/00.md"), "# Notes").unwrap();
    std::fs::write(root.join("code/01.json"), "{}").unwrap();
    assert_eq!(engine_for_path(&root, "code"), None);
    std::fs::write(root.join("code/02.py"), "x = 1").unwrap();
    std::fs::write(root.join("code/03.rs"), "fn a() {}").unwrap();
    assert_eq!(engine_for_path(&root, "code"), Some("python"));
    assert_eq!(
        engine_for_path(&root, root.join("code/03.rs").to_str().unwrap()),
        Some("rust")
    );
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"sample\"\n").unwrap();
    assert_eq!(engine_for_path(&root, "code"), Some("rust"));
    assert_eq!(engine_for_path(&root, "missing"), Some("rust"));
}
