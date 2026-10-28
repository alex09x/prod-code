/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use super::types::IndexBuild;

pub(super) async fn swift_index_build(remote: SocketAddr, root: &Path) -> Option<IndexBuild> {
    let command = vec![
        "swift".to_string(),
        "build".to_string(),
        "--build-tests".to_string(),
    ];
    let outcome = crate::exec::run_remote(
        remote,
        root,
        None,
        command.clone(),
        Vec::new(),
        900,
        false,
        |_, _| {},
    )
    .await;
    Some(IndexBuild {
        command: command.join(" "),
        ok: outcome
            .as_ref()
            .is_ok_and(|o| o.exit.exit_code == Some(0) && !o.exit.timed_out),
        duration_ms: outcome.as_ref().map_or(0, |o| o.exit.duration_ms),
    })
}
