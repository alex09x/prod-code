/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{extract_gradle_project_block, extract_maven_artifact_id, kebab_to_camel};

#[test]
fn maven_artifact_id_table() {
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("empty input", "", None),
        (
            "plain",
            "<project><artifactId>core</artifactId></project>",
            Some("core"),
        ),
        (
            "whitespace is trimmed",
            "<artifactId>\n  core-api \n</artifactId>",
            Some("core-api"),
        ),
        (
            "the parent's artifact id is skipped",
            "<parent><artifactId>root-pom</artifactId></parent>\n<artifactId>child</artifactId>",
            Some("child"),
        ),
        (
            "only the parent declares an id",
            "<parent><artifactId>root-pom</artifactId></parent><name>x</name>",
            None,
        ),
        ("unterminated element", "<artifactId>core", None),
        (
            "no artifact id",
            "<project><groupId>g</groupId></project>",
            None,
        ),
        (
            "first of several wins",
            "<artifactId>a</artifactId><artifactId>b</artifactId>",
            Some("a"),
        ),
        ("empty element", "<artifactId></artifactId>", Some("")),
    ];
    for (name, pom, expected) in cases {
        assert_eq!(
            extract_maven_artifact_id(pom).as_deref(),
            *expected,
            "case: {name}"
        );
    }
}

#[test]
fn gradle_project_block_table() {
    let cases: &[(&str, &str, &str, Option<&str>)] = &[
        (
            "single quotes",
            "project(':app') {\n  a { b }\n}\nafter {}",
            "app",
            Some("{\n  a { b }\n}"),
        ),
        (
            "double quotes",
            "project(\":app\") { x }",
            "app",
            Some("{ x }"),
        ),
        (
            "name without leading colon",
            "project('app') { x }",
            "app",
            Some("{ x }"),
        ),
        (
            "name without leading colon, double quotes",
            "project(\"app\") { x }",
            "app",
            Some("{ x }"),
        ),
        (
            "nested path with colons",
            "project(':lib:core') { c }",
            "lib:core",
            Some("{ c }"),
        ),
        (
            "colons replaced by dashes",
            "project(':lib-core') { dash }",
            "lib:core",
            Some("{ dash }"),
        ),
        (
            "double-quoted dashed name",
            "project(\":lib-core\") { dash }",
            "lib:core",
            Some("{ dash }"),
        ),
        (
            "last path segment",
            "project(':core') { last }",
            "lib:core",
            Some("{ last }"),
        ),
        (
            "last path segment, double quotes",
            "project(\":core\") { last }",
            "lib:core",
            Some("{ last }"),
        ),
        (
            "the full name wins over a block that comes earlier",
            "project(':core') { late }\nproject(':lib:core') { early }",
            "lib:core",
            Some("{ early }"),
        ),
        ("unknown project", "project(':other') { x }", "app", None),
        ("empty content", "", "app", None),
        (
            "no brace after the reference",
            "project(':app')\n",
            "app",
            None,
        ),
        (
            "unbalanced braces",
            "project(':app') { a { b }",
            "app",
            None,
        ),
        (
            "CRLF line ends stay in the block",
            "project(':app') {\r\n  x\r\n}\r\n",
            "app",
            Some("{\r\n  x\r\n}"),
        ),
        (
            "non-ASCII text inside the block",
            "project(':app') { \"héllo ü\" }",
            "app",
            Some("{ \"héllo ü\" }"),
        ),
        (
            "non-ASCII text before the block",
            "// ünï\nproject(':app') { x }",
            "app",
            Some("{ x }"),
        ),
    ];
    for (name, content, project, expected) in cases {
        assert_eq!(
            extract_gradle_project_block(content, project).as_deref(),
            *expected,
            "case: {name}"
        );
    }
}

#[test]
fn kebab_to_camel_table() {
    let cases: &[(&str, &str)] = &[
        ("", ""),
        ("already", "already"),
        ("my-lib", "myLib"),
        ("a_b_c", "aBC"),
        ("a-b_c-d", "aBCD"),
        ("lead-", "lead"),
        ("-lead", "Lead"),
        ("--x", "X"),
        ("a:b-c", "a:bC"),
        ("my-éclair", "myÉclair"),
        ("a-ß", "aSS"),
        ("UPPER-case", "UPPERCase"),
    ];
    for (input, expected) in cases {
        assert_eq!(kebab_to_camel(input), *expected, "input: {input:?}");
    }
}
