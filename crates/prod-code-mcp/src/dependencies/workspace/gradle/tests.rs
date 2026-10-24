/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::collect_gradle_dependencies;
use std::collections::BTreeMap;

/// project name -> (directory relative to the workspace root, sorted dependencies)
type Graph = BTreeMap<String, (String, Vec<String>)>;

fn collect(files: &[(&str, &str)]) -> Graph {
    let dir = tempfile::Builder::new()
        .prefix("gradle-ws")
        .tempdir()
        .unwrap();
    for (rel, body) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let mut adj = BTreeMap::new();
    collect_gradle_dependencies(dir.path(), &mut adj);
    adj.into_iter()
        .map(|(name, (path, deps))| {
            let rel = path.strip_prefix(dir.path()).unwrap();
            (
                name,
                (
                    rel.to_string_lossy().into_owned(),
                    deps.into_iter().collect(),
                ),
            )
        })
        .collect()
}

fn graph(entries: &[(&str, &str, &[&str])]) -> Graph {
    entries
        .iter()
        .map(|(name, dir, deps)| {
            (
                name.to_string(),
                (
                    dir.to_string(),
                    deps.iter().map(|d| d.to_string()).collect(),
                ),
            )
        })
        .collect()
}

#[test]
fn no_usable_settings_file_adds_nothing() {
    assert_eq!(collect(&[]), Graph::new());
    assert_eq!(
        collect(&[("app/build.gradle", "dependencies {}")]),
        Graph::new()
    );
    // A settings path that cannot be read as text is ignored.
    assert_eq!(collect(&[("settings.gradle/keep.txt", "x")]), Graph::new());
}

#[test]
fn groovy_settings_win_over_kotlin_settings() {
    let found = collect(&[
        ("settings.gradle", "include 'groovy'\n"),
        ("settings.gradle.kts", "include(\"kotlin\")\n"),
    ]);
    assert_eq!(found, graph(&[("groovy", "groovy", &[])]));
}

#[test]
fn groovy_include_forms_and_root_project_blocks() {
    let found = collect(&[
        (
            "settings.gradle",
            "rootProject.name = 'demo'\n\
             include ':app', '', ':util', ':', ':app:'\n\
             rootProject.name = 'renamed'\n\
             include 'multi',\n    'second'\n",
        ),
        (
            "app/build.gradle",
            "dependencies { implementation project(':util') }",
        ),
        ("util/build.gradle", "plugins { id 'java' }"),
        (
            "build.gradle",
            "project(':multi') {\n    dependencies { implementation project(':app') }\n}\n",
        ),
    ]);
    // `renamed` (a quoted string after a one-line include) is not a project.
    assert_eq!(
        found,
        graph(&[
            ("app", "app", &["util"]),
            ("multi", "multi", &["app"]),
            ("second", "second", &[]),
            ("util", "util", &[]),
        ])
    );
}

#[test]
fn kotlin_settings_dsl_forms() {
    let found = collect(&[
        (
            "settings.gradle.kts",
            "rootProject.name = \"demo\"\n\
             include(\":alpha\", \":beta\") // both\n\
             include(\n    // the rest\n    \":gamma\",\n\n    \"delta\"\n)\n\
             include(\":broken\n\
             module(\"tools/helper\")\n\
             module 'x/y'\n\
             module(\"\")\n\
             module(foo)\n\
             +\"epsilon-one\"\n\
             + ':zeta'\n\
             +foo\n\
             +\n",
        ),
        (
            "tools/helper/build.gradle.kts",
            "dependencies { implementation(projects.epsilonOne) }",
        ),
        (
            "build.gradle.kts",
            "project(\":alpha\") {\n    dependencies { implementation(project(\":beta\")) }\n}\n",
        ),
    ]);
    assert_eq!(
        found,
        graph(&[
            ("alpha", "alpha", &["beta"]),
            ("beta", "beta", &[]),
            ("delta", "delta", &[]),
            ("epsilon-one", "epsilon-one", &[]),
            ("gamma", "gamma", &[]),
            ("helper", "tools/helper", &["epsilon-one"]),
            ("y", "y", &[]),
            ("zeta", "zeta", &[]),
        ])
    );
}

#[test]
fn dependency_reference_spellings() {
    let found = collect(&[
        (
            "settings.gradle",
            "include ':p1', ':p2', ':p3', ':p4', ':p5', ':my-lib', ':grp:inner'\n",
        ),
        (
            "p1/build.gradle",
            "dependencies {\n    implementation project(\":p2\")\n    implementation project(':p3')\n}\n",
        ),
        (
            "p2/build.gradle",
            "api project(\"p3\")\napi project('p4')\n",
        ),
        ("p3/build.gradle", "implementation projects.myLib\n"),
        ("p4/build.gradle", "implementation projects.grp.inner\n"),
        (
            "p5/build.gradle",
            "implementation 'org:lib:1.0'\nimplementation project(\":p5\")\n",
        ),
        ("my-lib/build.gradle.kts", "dependencies {}"),
        ("grp/inner/README.md", "no build file here"),
        (
            "build.gradle",
            "project(':grp:inner') {\n    dependencies { implementation project(\":p1\") }\n}\n",
        ),
    ]);
    assert_eq!(
        found,
        graph(&[
            ("grp:inner", "grp/inner", &["p1"]),
            ("my-lib", "my-lib", &[]),
            ("p1", "p1", &["p2", "p3"]),
            ("p2", "p2", &["p3", "p4"]),
            ("p3", "p3", &["my-lib"]),
            ("p4", "p4", &["grp:inner"]),
            ("p5", "p5", &[]),
        ])
    );
}

#[test]
fn project_directories_are_found_by_folder_name() {
    let found = collect(&[
        (
            "settings.gradle",
            "include ':leaf'\ninclude ':sub:leaf'\ninclude ':ghost:none'\n",
        ),
        ("modules/leaf/build.gradle", "plugins {}"),
    ]);
    assert_eq!(
        found,
        graph(&[
            ("ghost:none", "ghost/none", &[]),
            ("leaf", "modules/leaf", &[]),
            ("sub:leaf", "modules/leaf", &[]),
        ])
    );
}

#[test]
fn build_output_and_hidden_directories_are_not_projects() {
    let found = collect(&[
        ("settings.gradle", "include ':real'\n"),
        ("real/build.gradle", "x"),
        ("buildSrc/build.gradle.kts", "x"),
        ("build-logic/build.gradle.kts", "x"),
        ("build/gen/build.gradle", "x"),
        (".gradle/cache/build.gradle", "x"),
        ("tools/other/build.gradle.kts", "x"),
    ]);
    // A directory with a build file is a project even if settings do not list it.
    assert_eq!(
        found,
        graph(&[("other", "tools/other", &[]), ("real", "real", &[])])
    );
}
