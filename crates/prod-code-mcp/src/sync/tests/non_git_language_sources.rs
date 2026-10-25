/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::entry::sync_file_entry;
use crate::sync::*;
use prod_code_protocol::{FileDelta, content_hash};
use std::path::Path;

type Source = (&'static str, &'static [u8]);

const FSHARP: &[Source] = &[
    (
        "App.fsproj",
        br#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework></PropertyGroup>
  <ItemGroup>
    <Compile Include="Model.fsi" /><Compile Include="Model.fs" />
    <Compile Include="Upper.FSI" /><Compile Include="Upper.FS" />
    <Compile Include="Program.fs" />
  </ItemGroup>
</Project>
"#,
    ),
    ("Model.fsi", b"module Model\nval greeting: string\n"),
    ("Model.fs", "module Model\nlet greeting = \"Hello, λ\"\n".as_bytes()),
    ("Upper.FSI", b"module Upper\nval answer: int\n"),
    ("Upper.FS", b"module Upper\nlet answer = 42\n"),
    (
        "Program.fs",
        b"module Program\n[<EntryPoint>]\nlet main _ =\n    printfn \"%s\" Model.greeting\n    0\n",
    ),
    ("scripts/check.fsx", b"#load \"../Model.fs\"\nprintfn \"%s\" Model.greeting\n"),
    ("scripts/upper.FSX", b"#load \"../Upper.FS\"\nprintfn \"%d\" Upper.answer\n"),
];

const ASSEMBLY: &[Source] = &[
    (
        "CMakeLists.txt",
        b"cmake_minimum_required(VERSION 3.20)\nproject(native LANGUAGES C ASM ASM_NASM)\nadd_library(native STATIC src/entry.nasm src/upper.NASM src/legacy.asm src/helper.s src/preprocess.S src/helper.c)\n",
    ),
    ("src/entry.nasm", b"bits 64\nsection .text\nglobal entry\nentry:\n    xor eax, eax\n    ret\n"),
    ("src/upper.NASM", b"bits 64\nsection .text\nglobal answer\nanswer:\n    mov eax, 42\n    ret\n"),
    ("src/legacy.asm", b"bits 64\nsection .text\nlegacy:\n    ret\n"),
    ("src/helper.s", b".text\nhelper:\n    ret\n"),
    ("src/preprocess.S", b"#define VALUE 1\n.text\npreprocess:\n    ret\n"),
    ("src/helper.c", b"int helper_c(void) { return 42; }\n"),
];

const ORDINARY: &[Source] = &[
    ("src/ordinary.rs", b"pub fn ordinary() -> u8 { 42 }\n"),
    ("src/ordinary.py", b"def ordinary():\n    return 42\n"),
];

const EXCLUDED: &[Source] = &[
    ("target/generated.fs", b"module Generated\nlet value = 0\n"),
    ("build/generated.nasm", b"bits 64\nret\n"),
    ("artifacts/output.fsx", b"printfn \"artifact\"\n"),
    (
        "vendor/dependency.fsi",
        b"module Dependency\nval value: int\n",
    ),
    ("obj/App.dll", b"MZ\0compiled"),
    ("src/helper.o", b"\x7fELF\0object"),
    ("src/data.csv", b"value\n42\n"),
    ("data/rows.jsonl", b"{\"value\":42}\n"),
    ("notes.txt", b"not a source file\n"),
];

struct Fixture {
    temp: tempfile::TempDir,
    expected: Vec<Source>,
}

impl Fixture {
    fn new(sources: &[Source]) -> Self {
        // The public APIs must take their filesystem fallback, not Git's tracked-file path.
        let temp = tempfile::tempdir().unwrap();
        let git = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        assert!(!git.status.success(), "fixture must be outside Git");
        let mut expected = sources.to_vec();
        expected.extend_from_slice(ORDINARY);
        expected.sort_by_key(|(path, _)| *path);
        for (relative, bytes) in expected.iter().chain(EXCLUDED) {
            let path = temp.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        Self { temp, expected }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn assert_files(&self, files: &[FileDelta]) {
        let mut actual: Vec<_> = files
            .iter()
            .map(|file| (file.relative_path.as_str(), file.content.as_deref()))
            .collect();
        actual.sort_by_key(|(path, _)| *path);
        let expected: Vec<_> = self
            .expected
            .iter()
            .map(|(path, bytes)| (*path, Some(*bytes)))
            .collect();
        assert_eq!(actual, expected, "relative paths and exact source bytes");
    }

    fn assert_manifest(&self, plan: &SyncPlan) {
        let actual: Vec<_> = plan
            .manifest()
            .into_iter()
            .map(|stamp| (stamp.relative_path, stamp.size, stamp.hash))
            .collect();
        let expected: Vec<_> = self
            .expected
            .iter()
            .map(|(path, bytes)| (path.to_string(), bytes.len() as u64, content_hash(bytes)))
            .collect();
        assert_eq!(actual, expected);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        clear_sync_cache(self.root());
    }
}

#[test]
fn non_git_language_sources_scan_fsharp_project() {
    let fixture = Fixture::new(FSHARP);
    fixture.assert_files(&scan_workspace_files(fixture.root(), None).unwrap());
}

#[test]
fn non_git_language_sources_scan_assembly_project() {
    let fixture = Fixture::new(ASSEMBLY);
    fixture.assert_files(&scan_workspace_files(fixture.root(), None).unwrap());
}

fn assert_initial_and_warm_plan(sources: &[Source]) {
    let fixture = Fixture::new(sources);
    let initial = prepare_workspace_sync(fixture.root(), None).unwrap();
    fixture.assert_files(&initial.files);
    fixture.assert_manifest(&initial);
    assert!(initial.initial);
    assert!(initial.state.base_commit_sha.is_none());
    assert_eq!(initial.state.filter_version, RELEVANCE_VERSION);
    commit_workspace_sync(fixture.root(), &initial);

    let warm = prepare_workspace_sync(fixture.root(), None).unwrap();
    assert!(
        warm.files.is_empty(),
        "unchanged sources must not be uploaded again"
    );
    fixture.assert_manifest(&warm);
}

#[test]
fn non_git_language_sources_plan_fsharp_project() {
    assert_initial_and_warm_plan(FSHARP);
}

#[test]
fn non_git_language_sources_plan_assembly_project() {
    assert_initial_and_warm_plan(ASSEMBLY);
}

#[test]
fn non_git_language_sources_version_12_watermark_is_invalidated() {
    let mut sources = FSHARP.to_vec();
    sources.extend_from_slice(ASSEMBLY);
    let fixture = Fixture::new(&sources);
    // Even matching stamps in an old watermark must not suppress the refreshed upload.
    // Sources may already be on the mirror from when the directory was Git-backed.
    let mut old = SyncCache {
        filter_version: 12,
        ..SyncCache::default()
    };
    for (relative, bytes) in &fixture.expected {
        let metadata = std::fs::metadata(fixture.root().join(relative)).unwrap();
        old.files
            .insert(relative.to_string(), sync_file_entry(&metadata, bytes));
    }
    save_sync_cache(fixture.root(), &old);

    let refreshed = prepare_workspace_sync(fixture.root(), None).unwrap();
    fixture.assert_files(&refreshed.files);
    fixture.assert_manifest(&refreshed);
    assert!(refreshed.state.filter_version > 12);
    commit_workspace_sync(fixture.root(), &refreshed);

    let warm = prepare_workspace_sync(fixture.root(), None).unwrap();
    assert!(warm.files.is_empty());
    fixture.assert_manifest(&warm);
}
