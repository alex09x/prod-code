/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::tests::{graph, write};
use super::*;

#[test]
fn resolves_artifact_and_directory_names_from_declared_modules() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "pom.xml",
        "<project>\r\n<module>outside-before</module>\r\n <modules>\r\n <module> client </module>\r\n<module>core</module>\r\n<module>legacy</module>\r\n<module>独立</module>\r\n<module>missing</module>\r\n<module>invalid</module>\r\n<module>incomplete\r\n </modules>\r\n<module>outside-after</module>\r\n</project>",
    );
    write(
        root,
        "client/pom.xml",
        "<project><parent><artifactId>parent</artifactId></parent><artifactId>client-art</artifactId><dependencies><dependency><artifactId>core-art</artifactId></dependency><dependency><artifactId>legacy</artifactId></dependency><dependency><artifactId>external</artifactId></dependency><dependency><artifactId>core-art</artifactId></dependency></dependencies></project>",
    );
    write(
        root,
        "core/pom.xml",
        "<project><artifactId>core-art</artifactId><artifactId>client-art</artifactId></project>",
    );
    write(root, "legacy/pom.xml", "<project />");
    write(
        root,
        "独立/pom.xml",
        "<project><artifactId>独立</artifactId></project>",
    );
    write(root, "invalid/pom.xml", "placeholder");
    std::fs::write(root.join("invalid/pom.xml"), [0xff]).unwrap();
    for name in ["outside-before", "outside-after", "unlisted"] {
        write(root, &format!("{name}/pom.xml"), "<project />");
    }
    let mut adj = graph(root, &[("existing", "existing", &[])]);
    collect_maven_dependencies(root, &mut adj);
    assert_eq!(
        adj,
        graph(
            root,
            &[
                ("existing", "existing", &[]),
                ("client", "client", &["core", "legacy"]),
                ("core", "core", &["client"]),
                ("legacy", "legacy", &[]),
                ("独立", "独立", &[]),
            ]
        )
    );
}

#[test]
fn missing_unreadable_empty_and_non_module_poms_leave_graph_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let expected = graph(root, &[("existing", "existing", &["other"])]);
    let mut adj = expected.clone();
    collect_maven_dependencies(root, &mut adj);
    assert_eq!(adj, expected);
    write(root, "pom.xml", "placeholder");
    std::fs::write(root.join("pom.xml"), [0xff]).unwrap();
    collect_maven_dependencies(root, &mut adj);
    assert_eq!(adj, expected);
    for content in [
        "",
        "<project />",
        "<modules>\n</modules>",
        "<module>unlisted</module>",
    ] {
        write(root, "pom.xml", content);
        collect_maven_dependencies(root, &mut adj);
        assert_eq!(adj, expected);
    }
}
