/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::scan_order;
use std::path::PathBuf;

/// The declaring file is scanned first and its neighbours next, whatever the walk order: a
/// common name in many earlier files must not spend the budget before the file where a private
/// member's only uses are (#2008).
#[test]
fn the_declaring_file_and_its_neighbours_are_scanned_first() {
    let root = tempfile::tempdir().unwrap();
    let file = |rel: &str| {
        let path = root.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "present()\n").unwrap();
        path
    };
    let far_a = file("A/Alpha.swift");
    let far_b = file("B/Beta.swift");
    let neighbour = file("Features/Sessions/Other.swift");
    let declaring = file("Features/Sessions/Presentation.swift");
    let far_c = file("Z/Zeta.swift");

    let walk = vec![
        far_a.clone(),
        far_b.clone(),
        declaring.clone(),
        neighbour.clone(),
        far_c.clone(),
    ];
    let ordered = scan_order(&declaring, walk.into_iter());
    assert_eq!(ordered, vec![declaring, neighbour, far_a, far_b, far_c]);
}

/// The declaring file is listed once even when the walk spells its path differently.
#[test]
fn the_declaring_file_is_not_scanned_twice() {
    let root = tempfile::tempdir().unwrap();
    let declaring = root.path().join("Presentation.swift");
    std::fs::write(&declaring, "present()\n").unwrap();
    let spelled_otherwise: PathBuf = root.path().join(".").join("Presentation.swift");
    let ordered = scan_order(&declaring, vec![spelled_otherwise].into_iter());
    assert_eq!(ordered, vec![declaring]);
}
