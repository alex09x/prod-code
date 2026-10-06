/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use crate::detect::markers::{GROOVY_MARKERS, JAVA_MARKERS};

fn directory_has_kotlin_source(dir: &Path, depth: usize) -> bool {
    if depth > 6 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.')
            || matches!(
                name.as_str(),
                "build" | "target" | "node_modules" | ".gradle"
            )
        {
            return false;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            directory_has_kotlin_source(&path, depth + 1)
        } else {
            path.extension().and_then(|extension| extension.to_str()) == Some("kt")
        }
    })
}

/// A Gradle Kotlin DSL script is not evidence that the compiled source language is Kotlin.
/// Prefer actual Kotlin sources or an applied Kotlin plugin before selecting kotlin-language-server.
pub fn has_kotlin_project(root: &Path) -> bool {
    let source_roots = [
        "src",
        "app/src",
        "common/src",
        "shared/src",
        "src/main/kotlin",
        "src/test/kotlin",
    ];
    if source_roots
        .iter()
        .any(|relative| directory_has_kotlin_source(&root.join(relative), 0))
    {
        return true;
    }
    const KOTLIN_PLUGIN_MARKERS: &[&str] = &[
        "kotlin(\"jvm\")",
        "kotlin(\"android\")",
        "kotlin(\"multiplatform\")",
        "id(\"org.jetbrains.kotlin.jvm\")",
        "id(\"org.jetbrains.kotlin.android\")",
        "id(\"org.jetbrains.kotlin.multiplatform\")",
        "id 'org.jetbrains.kotlin.jvm'",
        "id 'org.jetbrains.kotlin.android'",
        "id 'org.jetbrains.kotlin.multiplatform'",
        "apply plugin: 'org.jetbrains.kotlin.jvm'",
        "apply plugin: 'org.jetbrains.kotlin.android'",
        "apply plugin: 'org.jetbrains.kotlin.multiplatform'",
        "apply plugin: 'kotlin'",
    ];
    [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ]
    .iter()
    .filter_map(|name| std::fs::read_to_string(root.join(name)).ok())
    .any(|text| {
        text.lines()
            .filter(|line| {
                let line = line.trim_start();
                !line.starts_with("//") && !line.starts_with('*')
            })
            .any(|line| {
                KOTLIN_PLUGIN_MARKERS
                    .iter()
                    .any(|marker| line.contains(marker))
            })
    })
}

pub fn has_java_project(root: &Path) -> bool {
    JAVA_MARKERS.iter().any(|marker| {
        if !root.join(marker).exists() {
            return false;
        }
        !matches!(*marker, "build.gradle.kts" | "settings.gradle.kts")
            || !has_kotlin_project(root)
            || ["src/main/java", "src/test/java", "app/src/main/java"]
                .iter()
                .any(|relative| root.join(relative).is_dir())
    })
}

/// A Groovy project (Jenkinsfile, *.groovy, *.gvy) at the root.
pub fn has_groovy_project(root: &Path) -> bool {
    if GROOVY_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".groovy") || name.ends_with(".gvy")
            })
        })
        .unwrap_or(false)
}

/// A Groovy Gradle project can have only `build.gradle` and sources under the conventional
/// Groovy source roots, so it must be distinguished from Java Gradle projects before Java wins.
pub fn has_groovy_gradle_sources(root: &Path) -> bool {
    root.join("build.gradle").is_file() && root.join("src/main/groovy").is_dir()
}
