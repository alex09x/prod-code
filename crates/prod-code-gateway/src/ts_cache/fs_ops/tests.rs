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
fn failed_publication_keeps_destination_and_removes_staging() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.d.ts");
    fs::write(&source, "declare const value: number;").unwrap();
    let destination = temp.path().join("destination");
    fs::create_dir(&destination).unwrap();
    fs::File::open(&destination)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH)
        .unwrap();
    assert_eq!(
        copy_and_publish_type_file(&source, &destination)
            .unwrap_err()
            .kind(),
        io::ErrorKind::IsADirectory
    );
    assert!(destination.is_dir());
    assert_eq!(
        copy_and_publish_type_file(&source, Path::new(""))
            .unwrap_err()
            .to_string(),
        "destination file has no parent directory"
    );
    assert_eq!(
        copy_and_publish_type_file(
            &temp.path().join("missing"),
            &temp.path().join("missing.d.ts")
        )
        .unwrap_err()
        .kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(
        copy_and_publish_type_file(temp.path(), &temp.path().join("directory.d.ts"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::IsADirectory
    );
    assert!(!fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-ts")
    }));
}
