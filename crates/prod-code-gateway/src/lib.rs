/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! prod-code gateway daemon: multi-tenant server for remote code intelligence over 10 GbE LAN.
//!
//! The daemon is a library with a thin binary on top, so that the pieces a socket normally
//! stands in front of — the workspace manager, the dispatch, the language server backends —
//! can be exercised directly by tests.

pub mod admission;
pub mod backend;
pub mod cpp_index;
pub mod detect;
pub mod editor_proxy;
pub mod embed;
pub mod exec_shim;
pub mod memory;
mod metrics;
pub mod priming;
pub mod python_cache;
pub mod search;
pub mod shadow;
pub mod swift_cache;
pub mod ts_cache;
pub mod workspace;

pub mod engines;
pub(crate) mod gossip;
pub(crate) mod janitor_task;
pub(crate) mod lsp;
pub(crate) mod lsp_handlers;
pub mod runner;
pub mod seed_cache;
pub mod server;
pub mod session;
pub mod state;
pub mod sync;

pub use detect::detect_engine;
pub use engines::*;
pub(crate) use gossip::*;
pub(crate) use janitor_task::*;
pub(crate) use lsp::*;
pub(crate) use lsp_handlers::*;
pub use runner::*;
pub use seed_cache::*;
pub use server::*;
pub use session::*;
pub use state::*;
pub use sync::*;

pub(crate) use anyhow::{Context, Result};
pub(crate) use clap::Parser;
pub(crate) use futures_util::{SinkExt, StreamExt};
pub(crate) use prod_code_protocol::{
    content_hash, negotiate_protocol_version, parse_cargo_json_event, parse_go_test_json_event,
    path::{file_uri, uri_or_path},
    AnyStream, ClusterResponse, ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta,
    FileStamp, HandshakeResponse, LoadedWorkspaceInfo, NodeGossip, PathTranslator, PeerInfo,
    PlaceRequest, PlaceResponse, ProdCodeCodec, RemoteExecCommand, RemoteExecFormat,
    RemoteExecLanguage, RemoteExecRequest, RemoteExecResult, RemoteExecStream, RemoteExecTestEvent,
    ScrubSecrets, StatusResponse, SyncProbeRequest, SyncProbeResponse, SyncRequest, SyncResponse,
    WireMessage,
};
pub(crate) use std::net::SocketAddr;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
pub(crate) use std::sync::Arc;
pub(crate) use std::time::{Duration, Instant};
pub(crate) use tokio::net::TcpListener;
pub(crate) use tokio_util::codec::Framed;
pub(crate) use workspace::{SessionView, WorkspaceManager};

#[cfg(test)]
mod tests;
