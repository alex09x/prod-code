/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io::Write;

use crate::cpp_index::relocate::{
    relocate_arg_token, relocate_command_string, relocate_compile_commands_content,
    relocate_path_or_uri,
};
use crate::cpp_index::seed::seed_cpp_worktree;
use crate::cpp_index::shard::shard_filename_for_path;

#[test]
fn test_seed_cpp_worktree_e2e() {
    let temp_from = tempfile::tempdir().unwrap();
    let temp_to = tempfile::tempdir().unwrap();

    let from_root = temp_from.path();
    let to_root = temp_to.path();

    // 1. Setup mock compile_commands.json
    let from_build = from_root.join("build");
    std::fs::create_dir_all(&from_build).unwrap();
    let cdb_content = format!(
        r#"[{{ "directory": "{}/build", "command": "clang++ -c {}/src/lib.cpp", "file": "{}/src/lib.cpp" }}]"#,
        from_root.display(),
        from_root.display(),
        from_root.display()
    );
    std::fs::write(from_build.join("compile_commands.json"), cdb_content).unwrap();

    // 2. Setup a build-local clangd index plus an empty conventional index directory.
    let conventional_index = from_root.join(".cache").join("clangd").join("index");
    std::fs::create_dir_all(&conventional_index).unwrap();
    let from_index = from_root
        .join("build")
        .join(".cache")
        .join("clangd")
        .join("index");
    std::fs::create_dir_all(&from_index).unwrap();

    let source_file = from_root.join("src").join("lib.cpp");
    let shard_name = shard_filename_for_path(&source_file).unwrap();

    // Create a minimal synthetic shard for lib.cpp with index 0 sentinel
    let uncompressed = format!(
        "\0{}\0file://{}\0",
        source_file.display(),
        source_file.display()
    );
    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
    encoder.write_all(uncompressed.as_bytes()).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut stri_data = Vec::new();
    stri_data.extend_from_slice(&(uncompressed.len() as u32).to_le_bytes());
    stri_data.extend_from_slice(&compressed);

    let mut riff = Vec::new();
    riff.extend_from_slice(b"RIFF");
    let total_len = 4 + 8 + 4 + (8 + stri_data.len());
    riff.extend_from_slice(&(total_len as u32).to_le_bytes());
    riff.extend_from_slice(b"CdIx");
    riff.extend_from_slice(b"meta");
    riff.extend_from_slice(&4u32.to_le_bytes());
    riff.extend_from_slice(&21u32.to_le_bytes());
    riff.extend_from_slice(b"stri");
    let stri_len = stri_data.len() as u32;
    riff.extend_from_slice(&stri_len.to_le_bytes());
    riff.extend_from_slice(&stri_data);
    if stri_len % 2 != 0 {
        riff.push(0);
    }

    std::fs::write(from_index.join(&shard_name), &riff).unwrap();

    // 3. Seed to new worktree
    let seeded = seed_cpp_worktree(from_root, to_root).unwrap();
    assert!(seeded.is_some());

    // Verify compile_commands.json in to
    let to_cdb_path = to_root.join("build").join("compile_commands.json");
    assert!(to_cdb_path.is_file());
    let to_cdb = std::fs::read_to_string(to_cdb_path).unwrap();
    assert!(to_cdb.contains(&to_root.display().to_string()));
    assert!(!to_cdb.contains(&from_root.display().to_string()));

    // A build-local index beside compile_commands.json is seeded into the matching target.
    let to_source_file = to_root.join("src").join("lib.cpp");
    let expected_to_shard = shard_filename_for_path(&to_source_file).unwrap();
    let to_index = to_root
        .join("build")
        .join(".cache")
        .join("clangd")
        .join("index");
    let to_shard_path = to_index.join(&expected_to_shard);
    assert!(
        to_shard_path.is_file(),
        "expected shard {expected_to_shard} was not created"
    );

    let to_gitignore = to_index.join(".gitignore");
    assert!(to_gitignore.is_file());
    let conventional_shard = to_root
        .join(".cache")
        .join("clangd")
        .join("index")
        .join(&expected_to_shard);
    assert!(!conventional_shard.exists());
}

#[test]
fn test_path_and_uri_component_boundary_relocation() {
    let from = "/srv/workspaces/repo";
    let to = "/srv/workspaces/repo--wt-1234";

    // Exact match
    assert_eq!(
        relocate_path_or_uri("/srv/workspaces/repo", from, to),
        "/srv/workspaces/repo--wt-1234"
    );
    assert_eq!(
        relocate_path_or_uri("/srv/workspaces/repo/", from, to),
        "/srv/workspaces/repo--wt-1234/"
    );

    // Child path
    assert_eq!(
        relocate_path_or_uri("/srv/workspaces/repo/src/main.cpp", from, to),
        "/srv/workspaces/repo--wt-1234/src/main.cpp"
    );

    // Sibling directories sharing prefix: MUST NOT BE TOUCHED
    assert_eq!(
        relocate_path_or_uri("/srv/workspaces/repository-deps/include", from, to),
        "/srv/workspaces/repository-deps/include"
    );
    assert_eq!(
        relocate_path_or_uri("/srv/workspaces/repo-deps/include", from, to),
        "/srv/workspaces/repo-deps/include"
    );
    assert_eq!(
        relocate_path_or_uri("/srv/workspaces/repo.bak/src", from, to),
        "/srv/workspaces/repo.bak/src"
    );
    assert_eq!(
        relocate_path_or_uri("/srv/workspaces/repo_old", from, to),
        "/srv/workspaces/repo_old"
    );

    // URIs
    assert_eq!(
        relocate_path_or_uri("file:///srv/workspaces/repo", from, to),
        "file:///srv/workspaces/repo--wt-1234"
    );
    assert_eq!(
        relocate_path_or_uri("file:///srv/workspaces/repo/src/main.cpp", from, to),
        "file:///srv/workspaces/repo--wt-1234/src/main.cpp"
    );
    assert_eq!(
        relocate_path_or_uri("file:///srv/workspaces/repository-deps/include", from, to),
        "file:///srv/workspaces/repository-deps/include"
    );
    assert_eq!(
        relocate_path_or_uri("file:///srv/workspaces/repo-deps/include", from, to),
        "file:///srv/workspaces/repo-deps/include"
    );

    // Compiler flags in relocate_arg_token
    assert_eq!(
        relocate_arg_token("-I/srv/workspaces/repo/include", from, to),
        "-I/srv/workspaces/repo--wt-1234/include"
    );
    assert_eq!(
        relocate_arg_token("-I/srv/workspaces/repo-deps/include", from, to),
        "-I/srv/workspaces/repo-deps/include"
    );
    assert_eq!(
        relocate_arg_token("-I\"/srv/workspaces/repo/include\"", from, to),
        "-I\"/srv/workspaces/repo--wt-1234/include\""
    );
    assert_eq!(
        relocate_arg_token("-I\"/srv/workspaces/repository-deps/include\"", from, to),
        "-I\"/srv/workspaces/repository-deps/include\""
    );
    assert_eq!(
        relocate_arg_token(
            "-fdebug-prefix-map=/srv/workspaces/repo=/work/build",
            from,
            to
        ),
        "-fdebug-prefix-map=/srv/workspaces/repo--wt-1234=/work/build"
    );

    // Sentinels and non-matching tokens
    assert_eq!(relocate_arg_token("", from, to), "");
    assert_eq!(relocate_arg_token("repo", from, to), "repo");
    assert_eq!(
        relocate_arg_token("compute_magic", from, to),
        "compute_magic"
    );
}

#[test]
fn test_command_string_relocation_respects_component_boundaries() {
    let from = "/srv/workspaces/repo";
    let to = "/srv/workspaces/repo--wt-1234";

    let cmd = "clang++ -c /srv/workspaces/repo/src/main.cpp -I/srv/workspaces/repo/include -I/srv/workspaces/repo-deps/include -I\"/srv/workspaces/repository-deps/include\" -o /srv/workspaces/repo/build/main.o";
    let relocated = relocate_command_string(cmd, from, to);

    assert_eq!(
        relocated,
        "clang++ -c /srv/workspaces/repo--wt-1234/src/main.cpp -I/srv/workspaces/repo--wt-1234/include -I/srv/workspaces/repo-deps/include -I\"/srv/workspaces/repository-deps/include\" -o /srv/workspaces/repo--wt-1234/build/main.o"
    );
}

#[test]
fn test_compile_commands_json_relocation_respects_component_boundaries() {
    let from = "/srv/workspaces/repo";
    let to = "/srv/workspaces/repo--wt-1234";

    let json_input = r#"[
  {
    "directory": "/srv/workspaces/repo/build",
    "file": "/srv/workspaces/repo/src/main.cpp",
    "command": "clang++ -I/srv/workspaces/repo/include -I/srv/workspaces/repo-deps/include -c /srv/workspaces/repo/src/main.cpp -o /srv/workspaces/repo/build/main.o",
    "output": "/srv/workspaces/repo/build/main.o"
  },
  {
    "directory": "/srv/workspaces/repository-deps/build",
    "file": "/srv/workspaces/repository-deps/src/dep.cpp",
    "arguments": [
      "clang++",
      "-I/srv/workspaces/repository-deps/include",
      "-c",
      "/srv/workspaces/repository-deps/src/dep.cpp"
    ]
  }
]"#;

    let relocated = relocate_compile_commands_content(json_input, from, to).unwrap();
    let val: serde_json::Value = serde_json::from_str(&relocated).unwrap();
    let arr = val.as_array().unwrap();

    // Entry 1 (workspace unit)
    let e1 = &arr[0];
    assert_eq!(e1["directory"], "/srv/workspaces/repo--wt-1234/build");
    assert_eq!(e1["file"], "/srv/workspaces/repo--wt-1234/src/main.cpp");
    assert_eq!(e1["output"], "/srv/workspaces/repo--wt-1234/build/main.o");
    assert!(
        e1["command"]
            .as_str()
            .unwrap()
            .contains("-I/srv/workspaces/repo--wt-1234/include")
    );
    assert!(
        e1["command"]
            .as_str()
            .unwrap()
            .contains("-I/srv/workspaces/repo-deps/include")
    );

    // Entry 2 (sibling dependency unit): MUST BE COMPLETELY UNTOUCHED
    let e2 = &arr[1];
    assert_eq!(e2["directory"], "/srv/workspaces/repository-deps/build");
    assert_eq!(e2["file"], "/srv/workspaces/repository-deps/src/dep.cpp");
    assert_eq!(
        e2["arguments"][1],
        "-I/srv/workspaces/repository-deps/include"
    );
    assert_eq!(
        e2["arguments"][3],
        "/srv/workspaces/repository-deps/src/dep.cpp"
    );
}
