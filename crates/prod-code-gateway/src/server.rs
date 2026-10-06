/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// Runs the gateway: bind, serve, and return when a signal says to stop.
pub async fn run(cli: ServerCli) -> Result<()> {
    ensure_blocking_stdio();
    prefer_rustup_toolchain();
    let shadow_root = cli
        .shadow_dir
        .clone()
        .unwrap_or_else(|| shadow::default_root(&cli.storage));
    // Ownership precedes cleanup: no startup may sweep another live gateway's hypotheses.
    // Transfer the guard into shared state so accepted sessions retain it after this frame returns.
    let shadow_owner = shadow::ShadowRootOwner::acquire(&shadow_root)?;
    let swept = shadow_owner.sweep();
    if swept > 0 {
        tracing::info!(dir = %shadow_root.display(), swept, "removed leftover shadow directories");
    }
    // Probe once here, while nothing is waiting on us, rather than on the first request.
    let engines = refresh_available_engines();
    tracing::info!(?engines, "engines detected");

    let _ = tokio::task::spawn_blocking(|| {
        let _ = python_cache::prune_stale_stub_cache(
            std::time::Duration::from_secs(7 * 86400),
            5 * 1024 * 1024 * 1024,
        );
        let _ = swift_cache::prune_stale_module_cache(
            std::time::Duration::from_secs(7 * 86400),
            10 * 1024 * 1024 * 1024,
        );
        let _ = ts_cache::prune_stale_types_cache(
            std::time::Duration::from_secs(7 * 86400),
            5 * 1024 * 1024 * 1024,
        );
    })
    .await;

    tracing::info!(
        "prod-code gateway daemon starting on {} (storage: {:?})",
        cli.bind,
        cli.storage
    );

    let prune_worktree_secs = cli.effective_prune_worktree_secs();
    let prune_workspace_secs = cli.effective_prune_workspace_secs();
    let mut state = ServerState::new(cli.storage);
    state.build_cache_ram = cli.build_cache_ram;
    state.build_cache_dir = cli.build_cache_dir;
    if state.build_cache_ram {
        let build_cache_base = state.build_cache_dir.clone().unwrap_or_else(|| {
            if Path::new("/dev/shm").is_dir() {
                PathBuf::from("/dev/shm/prod-code-build")
            } else {
                PathBuf::from("/tmp/prod-code-build")
            }
        });
        sweep_ram_build_caches(&build_cache_base);
    }
    state.workspace_manager = Arc::new(WorkspaceManager::with_admission_and_concurrency(
        Arc::new(admission::Admission::host(cli.engine_reserve_mib)),
        cli.max_concurrent_engine_loads,
    ));
    state.engine_allowlist = cli
        .engines
        .iter()
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    if !state.engine_allowlist.is_empty() {
        tracing::info!(engines = ?state.engine_allowlist, "serving only the listed engines");
    }
    // The same token the clients send, from the same variables; its value is never logged.
    state.auth_token = prod_code_protocol::transport::auth_token();
    tracing::info!(
        required = state.auth_token.is_some(),
        "connection token (PROD_CODE_AUTH_TOKEN or PROD_CODE_AUTH_TOKEN_FILE)"
    );
    state.shadow_root = shadow_root;
    // Accepted sessions retain an Arc<ServerState>, so they must also retain exclusive ownership
    // after the accept loop returns on a shutdown signal.
    state._shadow_root_owner = Some(shadow_owner);
    match shadow::overlay_unavailable() {
        None => {
            tracing::info!(dir = %state.shadow_root.display(), "shadow runs: overlay mode (user namespaces + overlayfs)")
        }
        Some(reason) => tracing::info!(reason, "shadow runs: in-place mode"),
    }

    let tls_mode = prod_code_protocol::tls::TlsMode::from_env()?;
    let server_tls = prod_code_protocol::tls::ServerTlsConfig::from_env()?;
    let tls_acceptor = if let Some(tls_cfg) = server_tls {
        Some(tls_cfg.build_acceptor()?)
    } else {
        None
    };

    // Load and build outbound client TLS configuration before scrubbing TLS_KEY_ENV (#Phase 5.6):
    let client_tls_built = match prod_code_protocol::tls::ClientTlsConfig::from_env()? {
        Some(cfg) => Some(cfg.build()?),
        None => None,
    };
    prod_code_protocol::transport::set_default_client_tls_built(client_tls_built.clone());
    state.client_tls = client_tls_built;

    if tls_mode.is_required() && tls_acceptor.is_none() {
        return Err(anyhow::anyhow!(
            "TLS mode {:?} is strictly required, but server TLS credentials/CA are not configured",
            tls_mode
        ));
    }
    tracing::info!(
        mode = ?tls_mode,
        tls_enabled = tls_acceptor.is_some(),
        "cluster transport security (Phase 5.6)"
    );

    let state = Arc::new(state);
    let listener = TcpListener::bind(cli.bind).await?;
    // The address it actually bound, not the one it was asked for: with a port of 0, or an
    // interface that resolves to something else, those differ and only this one is reachable.
    let bound = listener.local_addr().unwrap_or(cli.bind);
    tracing::info!("prod-code gateway listening on {bound}");

    let peers: Vec<String> = cli
        .peers
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    let advertise = cli
        .advertise
        .clone()
        .unwrap_or_else(|| detect_advertise_addr(cli.bind, peers.first().map(String::as_str)));
    *state.advertise.write().await = advertise.clone();
    {
        let mut set = state.peers.write().await;
        for p in &peers {
            if *p != advertise {
                set.insert(p.clone());
            }
        }
    }
    tracing::info!(advertise, peers = ?peers, "cluster identity");
    tokio::spawn(gossip_loop(Arc::clone(&state)));
    tokio::spawn(discovery_loop(Arc::clone(&state)));
    if let Some(rx) = state.metrics.take_receiver() {
        tokio::spawn(metrics::run_writer(state.metrics.dir().to_path_buf(), rx));
    }

    tokio::spawn(janitor(
        Arc::clone(&state),
        cli.idle_evict_secs,
        prune_worktree_secs,
        prune_workspace_secs,
        cli.prune_below_free_percent,
    ));

    #[cfg(unix)]
    let mut socket_cleaner = None;
    #[cfg(unix)]
    let unix_listener = if let Some(ref path) = cli.socket_path {
        remove_stale_unix_socket(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create socket directory {}", parent.display()))?;
        }
        let u_listener = tokio::net::UnixListener::bind(path)?;
        socket_cleaner = Some(SocketCleaner::new(path)?);
        tracing::info!(
            "prod-code gateway listening on unix socket {}",
            path.display()
        );
        Some(u_listener)
    } else {
        None
    };

    #[cfg(unix)]
    let _socket_cleaner = socket_cleaner;

    // A gateway is stopped by its supervisor (launchd, systemd) and by a deploy script, both
    // of which send SIGTERM and then wait. Without a handler the process dies where it stands:
    // in-flight queries are cut, and nothing that runs at exit runs. Stopping the accept loop
    // and returning normally is all that is needed — every session is a task holding its own
    // socket, and the client treats a closed connection as a session to re-open.
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    async fn accept_unix(
        listener: &Option<tokio::net::UnixListener>,
    ) -> std::io::Result<(tokio::net::UnixStream, tokio::net::unix::SocketAddr)> {
        match listener {
            Some(l) => l.accept().await,
            None => std::future::pending().await,
        }
    }

    let handshake_limiter = Arc::new(tokio::sync::Semaphore::new(128));
    let tls_acceptor = tls_acceptor.map(Arc::new);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (socket, addr) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(%e, "TCP accept error");
                        continue;
                    }
                };
                let permit = match handshake_limiter.clone().try_acquire_owned() {
                    Ok(p) => p,
                    Err(_) => {
                        tracing::warn!(%addr, "Too many concurrent handshakes; dropping connection");
                        continue;
                    }
                };
                prod_code_protocol::transport::tune(&socket);
                let state_clone = Arc::clone(&state);
                let tls_acceptor = tls_acceptor.clone();
                let addr_str = addr.to_string();

                tokio::spawn(async move {
                    let _permit = permit;
                    let stream = if let Some(ref acceptor) = tls_acceptor {
                        let mut peek_buf = [0u8; 1];
                        let peek_result = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            socket.peek(&mut peek_buf),
                        ).await;
                        match peek_result {
                            Ok(Ok(1)) if peek_buf[0] == 0x16 => {
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(15),
                                    acceptor.accept(socket),
                                ).await {
                                    Ok(Ok(tls_stream)) => AnyStream::TlsServer(tls_stream),
                                    Ok(Err(e)) => {
                                        tracing::warn!(%addr, %e, "TLS handshake failed on accepted connection");
                                        return;
                                    }
                                    Err(_) => {
                                        tracing::warn!(%addr, "TLS handshake timed out");
                                        return;
                                    }
                                }
                            }
                            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
                                if tls_mode.is_required() {
                                    tracing::warn!(%addr, "Plaintext connection rejected: TLS mode {:?} is strictly required", tls_mode);
                                    use tokio::io::AsyncWriteExt;
                                    let mut s = socket;
                                    let _ = s.write_all(b"HTTP/1.1 426 Upgrade Required\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\nUpgrade to TLS/SSL required: port 9400 enforces encrypted transport.\r\n").await;
                                    let _ = s.flush().await;
                                    return;
                                }
                                AnyStream::Tcp(socket)
                            }
                        }
                    } else if tls_mode.is_required() {
                        tracing::warn!(%addr, "Connection rejected: TLS mode {:?} is required but no server TLS acceptor configured", tls_mode);
                        return;
                    } else {
                        AnyStream::Tcp(socket)
                    };

                    drop(_permit);
                    if let Err(err) = handle_client(stream, addr_str.clone(), state_clone).await {
                        tracing::error!(addr = %addr_str, %err, "Error in client connection");
                    }
                });
            }
            accepted = accept_unix(&unix_listener) => {
                let (socket, _) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!(%e, "Unix socket accept error");
                        continue;
                    }
                };
                let state_clone = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(err) = handle_client(AnyStream::Unix(socket), "unix-socket".to_string(), state_clone).await {
                        tracing::error!(addr = "unix-socket", %err, "Error in client connection");
                    }
                });
            }
            _ = terminate.recv() => {
                tracing::info!("SIGTERM: no longer accepting connections");
                return Ok(());
            }
            _ = interrupt.recv() => {
                tracing::info!("SIGINT: no longer accepting connections");
                return Ok(());
            }
        }
    }
}
