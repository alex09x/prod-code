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
fn publish_refusals_leave_destination_and_no_temporary_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.pyi");
    fs::write(&source, "value: int").unwrap();
    let destination = temp.path().join("destination");
    fs::create_dir(&destination).unwrap();
    fs::File::open(&destination)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH)
        .unwrap();
    assert_eq!(
        copy_and_publish_stub(&source, &destination)
            .unwrap_err()
            .kind(),
        io::ErrorKind::IsADirectory
    );
    assert!(destination.is_dir());
    let file = temp.path().join("parent-file");
    fs::write(&file, "keep").unwrap();
    assert_eq!(
        copy_and_publish_stub(&source, &file.join("stub.pyi"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        copy_and_publish_stub(&source, Path::new(""))
            .unwrap_err()
            .to_string(),
        "destination file has no parent directory"
    );
    let missing = temp.path().join("missing");
    assert_eq!(
        copy_and_publish_stub(&missing, &temp.path().join("new.pyi"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(!fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-stub")
    }));
    assert_eq!(fs::read_to_string(file).unwrap(), "keep");
}
