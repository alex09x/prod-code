/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::loop_run::run_session_loop;
use super::redirect::{check_cluster_redirects, try_validation_redirect};
use crate::*;
use futures_util::SinkExt;
use prod_code_protocol::HandshakeRequest;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub async fn handle_handshake(
    req: HandshakeRequest,
    state: &Arc<ServerState>,
    mut framed: Framed<AnyStream, ProdCodeCodec>,
    addr: &str,
) -> Result<()> {
    let protocol_version = match negotiate_protocol_version(&req) {
        Ok(version) => version,
        Err(err) => {
            let reason = format!("gateway refused protocol negotiation: {err}");
            tracing::warn!(reason, "refusing incompatible handshake");
            framed.send(WireMessage::Disconnect { reason }).await?;
            return Ok(());
        }
    };
    let session_id = state.next_session_id.fetch_add(1, Ordering::Relaxed);
    let _active_session = ActiveSession::start(&state.active_sessions);
    let session_capabilities = prod_code_protocol::negotiate_capabilities(
        req.capabilities.as_ref(),
        &prod_code_protocol::default_server_capabilities(),
    );

    let client_root_path = PathBuf::from(&req.client_workspace_root);
    let server_workspace = workspace::resolve_server_workspace(
        &state.storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let server_workspace_str = server_workspace.to_string_lossy().to_string();
    workspace::touch_last_used(&server_workspace);

    // A nested project of another language (engine_subpath) gets its own engine
    // rooted there; sync and path translation stay on the checkout root.
    let engine_root = match req.engine_subpath.as_deref() {
        Some(sub)
            if !sub.is_empty()
                && !sub.starts_with('/')
                && !sub.split('/').any(|c| c == "..")
                && server_workspace.join(sub).is_dir() =>
        {
            server_workspace.join(sub)
        }
        Some(sub) if !sub.is_empty() => {
            tracing::warn!(subpath = sub, "engine_subpath ignored (missing or unsafe)");
            server_workspace.clone()
        }
        _ => server_workspace.clone(),
    };

    let engine_kind = detect::resolve_engine(&engine_root, req.preferred_engine.as_deref());
    let engine = engine_kind.as_str();

    if let Some(()) =
        check_cluster_redirects(&req, state, engine, &server_workspace, &mut framed).await?
    {
        return Ok(());
    }

    let translator = PathTranslator::new(&req.client_workspace_root, &server_workspace_str);

    // An editor gets the language server it would run locally, a process of its own
    // on this node (#332); without one here, the shared engines answer it.
    if req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_EDITOR)
        && editor_proxy::enabled()
        && let Some(command) =
            editor_proxy::server_command_for_workspace(engine, &server_workspace)
    {
        framed
            .send(WireMessage::HandshakeResponse(HandshakeResponse {
                protocol_version,
                server_pid: state.server_pid,
                session_id,
                server_workspace_root: server_workspace_str.clone(),
                detected_engine: engine.to_string(),
                stale_paths: workspace::stale_paths(&server_workspace),
                engine_age_ms: None,
                index_gated: false,
                capabilities: Some(session_capabilities.clone()),
            }))
            .await?;
        let outcome = editor_proxy::run(
            framed,
            translator,
            command,
            &engine_root,
            &state.workspace_manager.editor_servers,
            session_id,
        )
        .await;
        return outcome;
    }

    // Attach to shared workspace using leader-follower coalescing.
    let shared_ws = match state
        .workspace_manager
        .get_or_load(&engine_root, engine)
        .await
    {
        Ok(shared_ws) => shared_ws,
        Err(err) => {
            let reason = format!("{err:#}");
            tracing::warn!(
                client_root = %req.client_workspace_root,
                engine,
                reason,
                "refusing handshake: the engine could not be loaded"
            );
            framed.send(WireMessage::Disconnect { reason }).await?;
            return Ok(());
        }
    };
    let engine_age_ms = shared_ws.loaded_at.elapsed().as_millis() as u64;
    let index_gated = shared_ws.rust_engine.is_some()
        || shared_ws.go_engine.is_some()
        || shared_ws
            .generic_engine
            .as_ref()
            .is_some_and(|engine| engine.readiness_known());

    let validation = req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_VALIDATION);
    let generic_validation_session = if validation && shared_ws.generic_engine.is_some() {
        Some(Arc::clone(&shared_ws.generic_validation_session))
    } else {
        None
    };
    let mut session_view = state
        .workspace_manager
        .register_session_view(session_id, client_root_path.clone(), shared_ws)
        .await;
    let _generic_validation_session = if let Some(serial) = generic_validation_session {
        Some(serial.lock_owned().await)
    } else {
        None
    };
    if validation {
        let validation_view = session_view
            .accounted
            .validation_view(state.workspace_manager.admission())
            .await;
        match validation_view {
            Ok(view) => session_view.workspace = view,
            Err(err) => {
                if let Some(()) = try_validation_redirect(
                    &req,
                    state,
                    engine,
                    session_view,
                    session_id,
                    &mut framed,
                    &err,
                )
                .await?
                {
                    return Ok(());
                }

                let reason = format!("private validation engine unavailable: {err:#}");
                tracing::warn!(session_id, engine, reason, "refusing validation handshake");
                framed.send(WireMessage::Disconnect { reason }).await?;
                return Ok(());
            }
        }
    }

    tracing::info!(
        session_id,
        client_pid = req.client_pid,
        client_root = %req.client_workspace_root,
        server_root = %server_workspace_str,
        engine_root = %engine_root.display(),
        engine,
        is_single_owner = session_view.is_single_owner(),
        "Client session established (Direct-Edit fast path active: {})",
        session_view.is_single_owner()
    );

    framed
        .send(WireMessage::HandshakeResponse(HandshakeResponse {
            protocol_version,
            server_pid: state.server_pid,
            session_id,
            server_workspace_root: server_workspace_str,
            detected_engine: engine.to_string(),
            stale_paths: workspace::stale_paths(&server_workspace),
            engine_age_ms: Some(engine_age_ms),
            index_gated,
            capabilities: Some(session_capabilities),
        }))
        .await?;

    let meta = Arc::new(SessionMeta {
        session_id,
        client_name: req.client_name.clone(),
        agent: req
            .client_agent
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        host: req
            .client_host
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        client_addr: addr.to_string(),
        workspace: engine_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        engine: engine.to_string(),
        engine_root: engine_root.clone(),
        storage_root: state.storage_root.clone(),
        metrics: Arc::clone(&state.metrics),
        editor: req.purpose.as_deref() == Some(prod_code_protocol::PURPOSE_EDITOR),
        edits: Arc::default(),
    });

    let session_res = run_session_loop(framed, &translator, &session_view, meta).await;

    state
        .workspace_manager
        .unregister_session_view(session_view)
        .await;

    tracing::debug!(session_id, "Client session retired: {:?}", session_res);
    session_res
}
