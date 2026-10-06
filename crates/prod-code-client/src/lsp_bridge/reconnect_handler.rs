/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::reconnect::reconnect_editor_session;
use super::state::{LspStateTracker, fail_pending_requests};
use super::sync::keep_checkout_synced;
use super::transport::{EditorFrameReceiver, LspTrace, PendingRequests, spawn_editor_stdout_task};
use futures_util::stream::SplitSink;
use prod_code_protocol::{ProdCodeCodec, WireMessage};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU8;
use tokio::io::Stdout;
use tokio::sync::Mutex;
use tokio_util::codec::Framed;

pub struct ReconnectContext<'a> {
    pub engine: Option<&'static str>,
    pub cwd: &'a Path,
    pub cwd_str: &'a str,
    pub identity: &'a prod_code_mcp::sync::WorkspaceIdentity,
    pub tracker: &'a LspStateTracker,
    pub files: &'a Arc<prod_code_client::editor_files::RemoteFiles>,
    pub pending_requests: &'a PendingRequests,
    pub editor_out: &'a Arc<Mutex<Stdout>>,
    pub position_encoding: &'a Arc<AtomicU8>,
    pub editor_frames: &'a EditorFrameReceiver,
    pub deferred_editor_frames: &'a mut VecDeque<String>,
    pub trace: &'a LspTrace,
    pub pushing: &'a Arc<Mutex<()>>,
}

pub async fn handle_reconnect(
    ctx: &mut ReconnectContext<'_>,
    target_remote: SocketAddr,
    is_redirect: bool,
    why: &str,
    remote: &mut SocketAddr,
    keeper: &mut tokio::task::JoinHandle<()>,
    socket_tx: &mut SplitSink<Framed<prod_code_protocol::AnyStream, ProdCodeCodec>, WireMessage>,
    stdout_task: &mut tokio::task::JoinHandle<()>,
    closed_rx: &mut tokio::sync::oneshot::Receiver<(String, Option<SocketAddr>)>,
    replay_state_after_initialize: &mut bool,
    outstanding_ping: &mut Arc<AtomicBool>,
) {
    if is_redirect {
        eprintln!("prod-code lsp: following gateway redirect to {target_remote}; reconnecting...");
    } else {
        eprintln!("prod-code lsp: the gateway at {remote} {why}; reconnecting...");
    }

    match reconnect_editor_session(
        target_remote,
        is_redirect,
        ctx.engine,
        ctx.cwd,
        ctx.cwd_str,
        ctx.identity,
        ctx.tracker,
        ctx.files,
        ctx.pending_requests,
        ctx.editor_out,
        ctx.position_encoding,
        ctx.editor_frames,
        ctx.deferred_editor_frames,
        ctx.trace,
    )
    .await
    {
        Ok((new_tx, new_rx, replay_after_init, new_remote)) => {
            if *remote != new_remote {
                *remote = new_remote;
                keeper.abort();
                *keeper = tokio::spawn(keep_checkout_synced(
                    *remote,
                    ctx.cwd.to_path_buf(),
                    Arc::clone(ctx.pushing),
                ));
            }
            *socket_tx = new_tx;
            *replay_state_after_initialize = replay_after_init;
            *outstanding_ping = Arc::new(AtomicBool::new(false));
            let (new_task, new_closed_rx) = spawn_editor_stdout_task(
                new_rx,
                Arc::clone(ctx.files),
                ctx.trace.clone(),
                ctx.identity.clone(),
                Arc::clone(ctx.editor_out),
                Arc::clone(ctx.pending_requests),
                Arc::clone(outstanding_ping),
                Arc::clone(ctx.position_encoding),
            );
            *stdout_task = new_task;
            *closed_rx = new_closed_rx;
        }
        Err(rec_err) => {
            fail_pending_requests(
                ctx.pending_requests,
                ctx.editor_out,
                &format!("{why} (reconnect failed: {rec_err})"),
            )
            .await;
            keeper.abort();
            eprintln!("prod-code lsp: the gateway at {remote} {why} (reconnect failed: {rec_err})");
            std::process::exit(1);
        }
    }
}
