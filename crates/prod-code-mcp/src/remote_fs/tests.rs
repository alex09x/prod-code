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

#[test]
fn snippet_marks_the_target_line() {
    let text = "a\nb\nc\nd\ne\n";
    let s = snippet(text, 3, 1);
    assert_eq!(s, " 2 | b\n>3 | c\n 4 | d\n");
    assert_eq!(snippet(text, 99, 1), " 4 | d\n>5 | e\n");
    assert_eq!(
        uri_to_path("file:///usr/include/c%2B%2B/13/stdlib.h"),
        "/usr/include/c++/13/stdlib.h"
    );
    assert!(is_external(Path::new("/tmp"), "/usr/include/x.h"));
}

#[test]
fn snippet_of_an_empty_file_is_empty() {
    assert_eq!(snippet("", 1, 2), "");
}

#[test]
fn uri_to_path_accepts_a_plain_path_without_the_file_prefix() {
    assert_eq!(uri_to_path("/already/a/path.rs"), "/already/a/path.rs");
}

#[test]
fn uri_to_path_decodes_a_uri_once_and_a_plain_path_never() {
    // A literal `%41` in a file name is `%2541` in its URI, and stays `%41` in the path.
    assert_eq!(
        uri_to_path("file:///w/my%20app%20%231/100%2541%20%C3%BC.rs"),
        "/w/my app #1/100%41 ü.rs"
    );
    assert_eq!(uri_to_path("/w/100%41.rs"), "/w/100%41.rs");
    assert_eq!(uri_to_path("untitled:Untitled-1"), "untitled:Untitled-1");
    assert_eq!(uri_to_path("file://remote/tmp/%€"), "remote/tmp/%€");
}

#[test]
fn percent_decode_leaves_an_invalid_escape_untouched() {
    // Not valid hex after `%`: kept as literal characters rather than decoded.
    assert_eq!(uri_to_path("file:///tmp/100%zz"), "/tmp/100%zz");
    // A `%` too close to the end to have two hex digits after it is also left alone.
    assert_eq!(uri_to_path("file:///tmp/x%2"), "/tmp/x%2");
}

#[test]
fn is_external_is_false_for_a_path_inside_the_root() {
    // Canonicalized first: `is_external` canonicalizes the root itself, and on macOS a
    // temporary directory is reached through a symlink, so an uncanonicalized join here
    // would not share a prefix with it.
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let inside = root.join("src").join("lib.rs");
    assert!(!is_external(&root, &inside.to_string_lossy()));
    assert!(!is_external(&root, "src/lib.rs"));
}

#[tokio::test]
async fn read_source_reads_local_file_and_refuses_escaping_relative_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let src = root.join("crates").join("my-crate");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "pub fn test_local() {}\n").unwrap();

    let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();

    let (bytes, truncated) = read_source(addr, root, "crates/my-crate/lib.rs")
        .await
        .unwrap();
    assert!(!truncated);
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "pub fn test_local() {}\n"
    );

    let (bytes_abs, _) = read_source(addr, root, &src.join("lib.rs").to_string_lossy())
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(bytes_abs).unwrap(),
        "pub fn test_local() {}\n"
    );

    let err = read_source(addr, root, "../../etc/passwd")
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("outside the workspace"));
}

#[tokio::test]
async fn read_source_refuses_absolute_parent_and_symlink_escapes() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let outside = parent.path().join("private.rs");
    std::fs::write(&outside, "private").unwrap();
    let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();

    let traversal = root.join("../private.rs");
    let err = read_source(addr, &root, &traversal.to_string_lossy())
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("outside the workspace"),
        "{err:#}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let external = parent.path().join("external");
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("private.py"), "private").unwrap();
        symlink(&external, root.join("linked")).unwrap();
        let linked = root.join("linked/private.py");
        let err = read_source(addr, &root, &linked.to_string_lossy())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("outside the workspace"),
            "{err:#}"
        );
    }
}

#[test]
fn bounded_source_reader_stops_after_one_truncation_byte() {
    use std::cell::Cell;

    struct CountingReader<'a> {
        bytes: &'a [u8],
        read: &'a Cell<usize>,
    }

    impl std::io::Read for CountingReader<'_> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = output.len().min(self.bytes.len());
            output[..count].copy_from_slice(&self.bytes[..count]);
            self.bytes = &self.bytes[count..];
            self.read.set(self.read.get() + count);
            Ok(count)
        }
    }

    let data = vec![b'x'; MAX_SOURCE_BYTES as usize + 4096];
    let read = Cell::new(0);
    let reader = CountingReader {
        bytes: &data,
        read: &read,
    };
    let (bytes, truncated) = read_limited_source(reader).unwrap();
    assert!(truncated);
    assert_eq!(bytes.len(), MAX_SOURCE_BYTES as usize);
    assert_eq!(read.get() as u64, MAX_SOURCE_BYTES + 1);
}

#[test]
fn external_dependency_source_with_same_suffix_is_not_mapped_to_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    let ws_src = root.join("src");
    std::fs::create_dir_all(&ws_src).unwrap();
    std::fs::write(ws_src.join("lib.rs"), "pub fn workspace_code() {}\n").unwrap();

    let external_dep = temp
        .path()
        .join("cargo/registry/src/tokio-1.0.0/src/lib.rs");
    std::fs::create_dir_all(external_dep.parent().unwrap()).unwrap();
    std::fs::write(&external_dep, "pub fn dependency_code() {}\n").unwrap();

    let ext_str = external_dep.to_str().unwrap();
    assert_eq!(workspace_relative_path(&root, ext_str), None);
    assert!(is_external(&root, ext_str));

    // A valid remote mirror path does map to the workspace relative path
    let mirror_path = "/home/alex09x/prod-code-storage/workspaces/my-ws/src/lib.rs";
    assert_eq!(
        workspace_relative_path(&root, mirror_path),
        Some(PathBuf::from("src/lib.rs"))
    );
    assert!(!is_external(&root, mirror_path));
}
