#![cfg(unix)]

use prod_code_gateway::compiler_cache_env;
use prod_code_gateway::cpp_index::{
    clangd_path_digest, relocate_shard, seed_cpp_worktree, shard_filename_for_path,
};
use serde_json::json;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[test]
fn test_clangd_digest_invariants_and_path_sensitivity() {
    let base = "/srv/workspaces/engine-cpp/src/core.cpp";
    let digest = clangd_path_digest(base);

    // LLVM clangd digest specification
    assert_eq!(digest.len(), 16);
    assert!(
        digest.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()),
        "clangd digest must be 16 uppercase ASCII hexadecimal characters"
    );

    // Deterministic
    assert_eq!(digest, clangd_path_digest(base));

    // Path sensitivity: slight variation in path produces different digest
    let var1 = "/srv/workspaces/engine-cpp/src/core.h";
    let var2 = "/srv/workspaces/engine-cpp--wt-1234/src/core.cpp";
    assert_ne!(digest, clangd_path_digest(var1));
    assert_ne!(digest, clangd_path_digest(var2));

    // Shard filename format
    let shard_name = shard_filename_for_path(Path::new(base)).unwrap();
    assert_eq!(shard_name, format!("core.cpp.{digest}.idx"));
}

#[test]
fn test_riff_shard_relocation_preserves_null_order_and_replaces_paths() {
    let from = Path::new("/srv/workspaces/trading-engine");
    let to = Path::new("/srv/workspaces/trading-engine--wt-feed");

    // Construct synthetic uncompressed string table
    let original_strings = vec![
        "".to_string(),
        "/srv/workspaces/trading-engine".to_string(),
        "/srv/workspaces/trading-engine/src/order.cpp".to_string(),
        "file:///srv/workspaces/trading-engine/src/order.cpp".to_string(),
        "OrderBook::submit_limit".to_string(),
        "int".to_string(),
    ];
    let mut uncompressed = Vec::new();
    for s in &original_strings {
        uncompressed.extend_from_slice(s.as_bytes());
        uncompressed.push(0);
    }

    // Compress string table with zlib
    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&uncompressed).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut stri_data = Vec::new();
    stri_data.extend_from_slice(&(uncompressed.len() as u32).to_le_bytes());
    stri_data.extend_from_slice(&compressed);

    // Build RIFF container with meta and stri chunks
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

    let (relocated, source_path) = relocate_shard(&riff, from, to, None).unwrap();
    assert_eq!(
        source_path,
        Some(PathBuf::from(
            "/srv/workspaces/trading-engine--wt-feed/src/order.cpp"
        ))
    );

    // Parse the relocated RIFF container
    assert_eq!(&relocated[0..4], b"RIFF");
    assert_eq!(&relocated[8..12], b"CdIx");

    // Extract stri chunk from relocated bytes
    let stri_offset = 24;
    assert_eq!(&relocated[stri_offset..stri_offset + 4], b"stri");
    let uncomp_size = u32::from_le_bytes(
        relocated[stri_offset + 8..stri_offset + 12]
            .try_into()
            .unwrap(),
    ) as usize;
    let mut decoder = flate2::read::ZlibDecoder::new(&relocated[stri_offset + 12..]);
    let mut decomp = Vec::with_capacity(uncomp_size);
    decoder.read_to_end(&mut decomp).unwrap();

    let bytes = decomp.strip_suffix(&[0]).unwrap_or(&decomp);
    let decomp_strings: Vec<String> = bytes
        .split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect();

    // Positional order and count of interned strings preserved, including index 0 sentinel
    assert_eq!(decomp_strings.len(), original_strings.len());
    assert_eq!(decomp_strings[0], "");
    assert_eq!(decomp_strings[1], "/srv/workspaces/trading-engine--wt-feed");
    assert_eq!(decomp_strings[2], "/srv/workspaces/trading-engine--wt-feed/src/order.cpp");
    assert_eq!(decomp_strings[3], "file:///srv/workspaces/trading-engine--wt-feed/src/order.cpp");
    assert_eq!(decomp_strings[4], "OrderBook::submit_limit");
    assert_eq!(decomp_strings[5], "int");
    assert!(!decomp_strings.iter().any(|s| s.contains("trading-engine/src")));
}

#[test]
fn test_clangd_relocation_derives_tu_from_shard_identity_and_ignores_included_headers() {
    let from = Path::new("/srv/workspaces/complex-engine");
    let to = Path::new("/srv/workspaces/complex-engine--wt-worker");

    let header_path = "/srv/workspaces/complex-engine/include/common.h";
    let source_path = "/srv/workspaces/complex-engine/src/z_dispatch.cpp";

    // Alphabetically, include/common.h comes before src/z_dispatch.cpp
    let original_strings = vec![
        "".to_string(),
        header_path.to_string(),
        format!("file://{header_path}"),
        source_path.to_string(),
        format!("file://{source_path}"),
        "dispatch_event".to_string(),
    ];
    let mut uncompressed = Vec::new();
    for s in &original_strings {
        uncompressed.extend_from_slice(s.as_bytes());
        uncompressed.push(0);
    }

    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::<u8>::new(), flate2::Compression::default());
    encoder.write_all(&uncompressed).unwrap();
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

    let origin_digest = clangd_path_digest(source_path);
    let shard_name = format!("z_dispatch.cpp.{origin_digest}.idx");

    let (relocated, source) =
        relocate_shard(&riff, from, to, Some(&shard_name)).unwrap();

    // Must resolve to z_dispatch.cpp, not common.h
    assert_eq!(
        source,
        Some(PathBuf::from(
            "/srv/workspaces/complex-engine--wt-worker/src/z_dispatch.cpp"
        ))
    );

    let target_shard = shard_filename_for_path(&source.unwrap()).unwrap();
    assert!(target_shard.starts_with("z_dispatch.cpp."));
    assert!(!target_shard.starts_with("common.h."));

    // Check index 0 sentinel preservation
    let stri_offset = 24;
    let mut decoder = flate2::read::ZlibDecoder::new(&relocated[stri_offset + 12..]);
    let mut decomp = Vec::new();
    decoder.read_to_end(&mut decomp).unwrap();
    let bytes = decomp.strip_suffix(&[0]).unwrap();
    let decomp_strings: Vec<String> = bytes
        .split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect();

    assert_eq!(decomp_strings.len(), original_strings.len());
    assert_eq!(decomp_strings[0], "");
    assert_eq!(decomp_strings[1], "/srv/workspaces/complex-engine--wt-worker/include/common.h");
    assert_eq!(decomp_strings[3], "/srv/workspaces/complex-engine--wt-worker/src/z_dispatch.cpp");
}

#[test]
fn test_seed_cpp_worktree_relocates_compile_commands_and_index() {
    let temp_from = tempfile::tempdir().unwrap();
    let temp_to = tempfile::tempdir().unwrap();

    let from_root = temp_from.path();
    let to_root = temp_to.path();

    // 1. Setup mock compile_commands.json
    let from_build = from_root.join("build");
    std::fs::create_dir_all(&from_build).unwrap();
    let cdb_content = format!(
        r#"[{{ "directory": "{}/build", "command": "clang++ -I{}/include -c {}/src/feed.cpp", "file": "{}/src/feed.cpp" }}]"#,
        from_root.display(),
        from_root.display(),
        from_root.display(),
        from_root.display()
    );
    std::fs::write(from_build.join("compile_commands.json"), cdb_content).unwrap();

    // 2. Setup mock .cache/clangd/index
    let from_index = from_root.join(".cache").join("clangd").join("index");
    std::fs::create_dir_all(&from_index).unwrap();

    let source_file = from_root.join("src").join("feed.cpp");
    let shard_name = shard_filename_for_path(&source_file).unwrap();

    let uncompressed = format!(
        "\0{}\0file://{}\0",
        source_file.display(),
        source_file.display()
    );
    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
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

    // Verify index shard in to
    let to_source_file = to_root.join("src").join("feed.cpp");
    let expected_to_shard = shard_filename_for_path(&to_source_file).unwrap();
    let to_shard_path = to_root
        .join(".cache")
        .join("clangd")
        .join("index")
        .join(&expected_to_shard);
    assert!(
        to_shard_path.is_file(),
        "expected shard {expected_to_shard} was not created in worktree"
    );

    let to_gitignore = to_root
        .join(".cache")
        .join("clangd")
        .join("index")
        .join(".gitignore");
    assert!(to_gitignore.is_file());
}

#[test]
fn test_compiler_cache_env_contains_pch_sharing_flags() {
    let ws = Path::new("/srv/workspaces/cpp-project--wt-test");
    let env = compiler_cache_env(ws, true);

    let get = |k: &str| {
        env.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    };

    assert_eq!(get("CCACHE_BASEDIR"), Some("/srv/workspaces/cpp-project--wt-test"));
    assert_eq!(get("CCACHE_NOHASHDIR"), Some("1"));
    assert_eq!(get("CCACHE_SLOPPINESS"), Some("pch_defines,time_macros"));
    assert_eq!(get("CCACHE_PCH_EXTERNAL_CHECKS"), Some("1"));
    assert_eq!(get("CMAKE_C_COMPILER_LAUNCHER"), Some("ccache"));
    assert_eq!(get("CMAKE_CXX_COMPILER_LAUNCHER"), Some("ccache"));
}

#[tokio::test]
async fn test_real_clangd_indexes_origin_and_worktree_loads_seeded_index_instantly() {
    // Only run if clangd is runnable on the host
    let clangd_available = tokio::process::Command::new("clangd")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !clangd_available {
        tracing::warn!("clangd not found on PATH; skipping live clangd subprocess verification");
        return;
    }

    let origin_dir = tempfile::tempdir().unwrap();
    let worktree_dir = tempfile::tempdir().unwrap();

    let origin = origin_dir.path();
    let worktree = worktree_dir.path();

    // 1. Create source file in origin
    let src_dir = origin.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let source_path = src_dir.join("calc.cpp");
    let source_code = "int super_fast_cpp_calculation(int a, int b) { return a * b + 42; }\n";
    std::fs::write(&source_path, source_code).unwrap();

    // 2. Create compile_commands.json in origin
    let build_dir = origin.join("build");
    std::fs::create_dir_all(&build_dir).unwrap();
    let cdb_entry = json!([{
        "directory": origin.display().to_string(),
        "command": format!("clang++ -c {}", source_path.display()),
        "file": source_path.display().to_string(),
    }]);
    std::fs::write(
        build_dir.join("compile_commands.json"),
        serde_json::to_string_pretty(&cdb_entry).unwrap(),
    )
    .unwrap();

    // 3. Start clangd in origin to build background index
    let mut origin_child = tokio::process::Command::new("clangd")
        .args([
            "--background-index",
            "--compile-commands-dir=build",
            "--log=error",
        ])
        .current_dir(origin)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("clangd must spawn");

    let mut stdin = origin_child.stdin.take().unwrap();

    async fn send_rpc(stdin: &mut tokio::process::ChildStdin, val: serde_json::Value) {
        use tokio::io::AsyncWriteExt;
        let msg = val.to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{}", msg.len(), msg);
        stdin.write_all(frame.as_bytes()).await.unwrap();
        stdin.flush().await.unwrap();
    }

    send_rpc(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": format!("file://{}", origin.display()),
                "capabilities": {}
            }
        }),
    )
    .await;
    send_rpc(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }),
    )
    .await;
    send_rpc(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": format!("file://{}", source_path.display()),
                    "languageId": "cpp",
                    "version": 1,
                    "text": source_code,
                }
            }
        }),
    )
    .await;

    // Wait for origin clangd to write the index shard to .cache/clangd/index
    let origin_shard_dir = origin.join(".cache").join("clangd").join("index");
    let mut shard_created = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if origin_shard_dir.is_dir()
            && std::fs::read_dir(&origin_shard_dir)
                .map(|mut d| d.any(|e| e.map(|e| e.path().extension() == Some("idx".as_ref())).unwrap_or(false)))
                .unwrap_or(false)
        {
            shard_created = true;
            break;
        }
    }
    assert!(shard_created, "origin clangd must produce index shard in .cache/clangd/index");

    // Terminate origin clangd
    drop(stdin);
    let _ = origin_child.kill().await;

    // 4. Mirror source to worktree and seed the C/C++ cache
    let wt_src_dir = worktree.join("src");
    std::fs::create_dir_all(&wt_src_dir).unwrap();
    std::fs::write(wt_src_dir.join("calc.cpp"), source_code).unwrap();

    let seeded_bytes = seed_cpp_worktree(origin, worktree).unwrap();
    assert!(seeded_bytes.is_some(), "seed_cpp_worktree must seed compile_commands and index");

    // Verify worktree has the relocated shard
    let wt_source_path = wt_src_dir.join("calc.cpp");
    let wt_expected_shard = shard_filename_for_path(&wt_source_path).unwrap();
    let wt_shard_path = worktree
        .join(".cache")
        .join("clangd")
        .join("index")
        .join(&wt_expected_shard);
    assert!(wt_shard_path.is_file(), "worktree must possess relocated shard file");

    // 5. Start a fresh clangd in the worktree
    let mut wt_child = tokio::process::Command::new("clangd")
        .args([
            "--background-index",
            "--compile-commands-dir=build",
            "--log=error",
        ])
        .current_dir(worktree)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("worktree clangd must spawn");

    let mut wt_stdin = wt_child.stdin.take().unwrap();
    let wt_stdout = wt_child.stdout.take().unwrap();

    send_rpc(
        &mut wt_stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "rootUri": format!("file://{}", worktree.display()),
                "capabilities": {}
            }
        }),
    )
    .await;
    send_rpc(
        &mut wt_stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }),
    )
    .await;

    // Ask for workspace symbol WITHOUT opening any file!
    // If the index was successfully seeded and loaded, clangd knows the symbol immediately!
    send_rpc(
        &mut wt_stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "workspace/symbol",
            "params": {
                "query": "super_fast_cpp_calculation"
            }
        }),
    )
    .await;

    // Read responses from clangd until we get id == 2
    let mut reader = tokio::io::BufReader::new(wt_stdout);
    let mut found_symbol = false;

    for _ in 0..10 {
        use tokio::io::AsyncBufReadExt;
        let mut header_line = String::new();
        if reader.read_line(&mut header_line).await.unwrap() == 0 {
            break;
        }
        if header_line.starts_with("Content-Length: ") {
            let len: usize = header_line
                .trim_start_matches("Content-Length: ")
                .trim()
                .parse()
                .unwrap();
            let mut empty = String::new();
            reader.read_line(&mut empty).await.unwrap();

            let mut body_bytes = vec![0u8; len];
            use tokio::io::AsyncReadExt;
            reader.read_exact(&mut body_bytes).await.unwrap();

            let resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
            if resp.get("id") == Some(&json!(2)) {
                let results = resp.get("result").and_then(|r| r.as_array());
                if let Some(symbols) = results {
                    for sym in symbols {
                        if sym.get("name") == Some(&json!("super_fast_cpp_calculation")) {
                            found_symbol = true;
                            // Check that URI points to the worktree, not origin!
                            let uri = sym["location"]["uri"].as_str().unwrap();
                            assert!(
                                uri.contains(&worktree.display().to_string()),
                                "symbol location URI must point to worktree, got: {uri}"
                            );
                            assert!(
                                !uri.contains(&origin.display().to_string()),
                                "symbol location URI must not point to origin"
                            );
                        }
                    }
                }
                break;
            }
        }
    }

    assert!(
        found_symbol,
        "clangd in worktree must instantly answer workspace/symbol from seeded index without opening file"
    );

    drop(wt_stdin);
    let _ = wt_child.kill().await;
}

#[test]
fn test_seed_cpp_worktree_preserves_sibling_dependency_paths() {
    let temp_from = tempfile::tempdir().unwrap();
    let temp_to = tempfile::tempdir().unwrap();

    let from_root = temp_from.path();
    let to_root = temp_to.path();

    let sibling_dep = format!("{}-deps", from_root.display());
    let sibling_other = format!("{}other", from_root.display());

    // 1. Setup mock compile_commands.json with sibling paths
    let from_build = from_root.join("build");
    std::fs::create_dir_all(&from_build).unwrap();
    let cdb_content = format!(
        r#"[
  {{
    "directory": "{}/build",
    "command": "clang++ -I{}/include -I{}/include -I\"{}\" -c {}/src/feed.cpp",
    "file": "{}/src/feed.cpp"
  }}
]"#,
        from_root.display(),
        from_root.display(),
        sibling_dep,
        sibling_other,
        from_root.display(),
        from_root.display()
    );
    std::fs::write(from_build.join("compile_commands.json"), cdb_content).unwrap();

    // 2. Setup mock .cache/clangd/index with sibling paths
    let from_index = from_root.join(".cache").join("clangd").join("index");
    std::fs::create_dir_all(&from_index).unwrap();

    let source_file = from_root.join("src").join("feed.cpp");
    let shard_name = shard_filename_for_path(&source_file).unwrap();

    let uncompressed = format!(
        "\0{}\0file://{}\0{}\0file://{}\0-I{}\0",
        source_file.display(),
        source_file.display(),
        sibling_dep,
        sibling_dep,
        sibling_dep
    );
    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
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

    // Verify compile_commands.json in worktree
    let to_cdb_path = to_root.join("build").join("compile_commands.json");
    let to_cdb = std::fs::read_to_string(to_cdb_path).unwrap();

    // Worktree paths are relocated
    assert!(to_cdb.contains(&to_root.display().to_string()));
    assert!(!to_cdb.contains(&format!("{}/include", from_root.display())));

    // Sibling paths are completely untouched!
    assert!(to_cdb.contains(&sibling_dep));
    assert!(to_cdb.contains(&sibling_other));

    // Verify index shard in worktree
    let to_source_file = to_root.join("src").join("feed.cpp");
    let expected_to_shard = shard_filename_for_path(&to_source_file).unwrap();
    let to_shard_path = to_root
        .join(".cache")
        .join("clangd")
        .join("index")
        .join(&expected_to_shard);
    let relocated_bytes = std::fs::read(to_shard_path).unwrap();

    let stri_offset = 24;
    let uncomp_size = u32::from_le_bytes(
        relocated_bytes[stri_offset + 8..stri_offset + 12]
            .try_into()
            .unwrap(),
    ) as usize;
    let mut decoder = flate2::read::ZlibDecoder::new(&relocated_bytes[stri_offset + 12..]);
    let mut decomp = Vec::with_capacity(uncomp_size);
    decoder.read_to_end(&mut decomp).unwrap();

    let bytes = decomp.strip_suffix(&[0]).unwrap();
    let decomp_strings: Vec<String> = bytes
        .split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).to_string())
        .collect();

    assert_eq!(decomp_strings[0], "");
    assert_eq!(decomp_strings[1], to_source_file.display().to_string());
    assert_eq!(decomp_strings[2], format!("file://{}", to_source_file.display()));
    assert_eq!(decomp_strings[3], sibling_dep);
    assert_eq!(decomp_strings[4], format!("file://{sibling_dep}"));
    assert_eq!(decomp_strings[5], format!("-I{sibling_dep}"));
}
