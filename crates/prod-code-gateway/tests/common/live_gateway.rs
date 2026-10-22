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
use std::process::Command;
use std::sync::Arc;

const CHILD_STORAGE: &str = "PROD_CODE_LIVE_GATEWAY_CHILD_STORAGE";

/// Run this revision's gateway handlers in an owned child with deterministic admission.
/// No discovery or janitor runs: placement has exactly one node and cannot depend on the
/// machine's memory, disk, load, or gateways belonging to other parallel tests.
pub fn command(storage: &Path, test: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("live test executable"));
    command
        .args(["--exact", test, "--nocapture"])
        .env(CHILD_STORAGE, storage.join("workspaces"));
    command
}

pub async fn serve_if_child() -> bool {
    let Some(storage) = std::env::var_os(CHILD_STORAGE) else {
        return false;
    };
    let storage = std::path::PathBuf::from(storage);
    std::fs::create_dir_all(&storage).expect("child workspace storage");
    prod_code_gateway::refresh_available_engines();
    let mut state = prod_code_gateway::ServerState::new(storage.clone());
    state.workspace_manager = Arc::new(
        prod_code_gateway::workspace::WorkspaceManager::with_admission(Arc::new(
            prod_code_gateway::admission::Admission::unbounded(),
        )),
    );
    state.shadow_root = storage.join("shadow");
    state._shadow_root_owner = Some(
        prod_code_gateway::shadow::ShadowRootOwner::acquire(&state.shadow_root)
            .expect("child owns its shadow storage"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    *state.advertise.write().await = address.to_string();
    let state = Arc::new(state);
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("child termination signal");
    println!("prod-code gateway listening on {address}");
    let mut sessions = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = terminate.recv() => break,
            accepted = listener.accept() => {
                let (socket, peer) = accepted.expect("accept live client");
                let state = Arc::clone(&state);
                sessions.spawn(async move {
                    prod_code_gateway::handle_client(socket, peer, state).await
                });
            }
            _ = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
    sessions.abort_all();
    while sessions.join_next().await.is_some() {}
    true
}
