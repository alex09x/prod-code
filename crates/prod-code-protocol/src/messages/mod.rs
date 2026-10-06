/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod cluster;
pub mod exec;
pub mod handshake;
pub mod remote_exec;
pub mod search;
pub mod shadow;
pub mod status;
pub mod sync;
pub mod wire;

#[cfg(test)]
mod tests;

pub use cluster::{
    AuthToken, ClusterResponse, ExecMetric, LoadedWorkspaceInfo, MetricsRequest, MetricsResponse,
    NodeGossip, PeerInfo, PlaceRequest, PlaceResponse, QueryMetric, ReadFileRequest,
    ReadFileResponse, client_host, detect_client_agent, platform,
};
pub use exec::{ExecChanges, ExecChunk, ExecExit, ExecRequest, ExecUsage};
pub use handshake::{
    ANALYZER_PANIC_CODE, ClientCapabilities, HandshakeRequest, HandshakeResponse, PURPOSE_EDITOR,
    PURPOSE_VALIDATION, ServerCapabilities,
};
pub use remote_exec::{
    RemoteExecCommand, RemoteExecDiagnostic, RemoteExecFormat, RemoteExecLanguage,
    RemoteExecRequest, RemoteExecResult, RemoteExecSpan, RemoteExecStream, RemoteExecTestEvent,
    parse_cargo_json_event, parse_go_test_json_event,
};
pub use search::{DenseStatus, SearchHit, SearchRequest, SearchResponse};
pub use shadow::{ShadowHypothesis, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse};
pub use status::{
    HostResources, MEMORY_PRESSURE_USED, RunningCommand, STORAGE_PRESSURE_FREE, StatusResponse,
};
pub use sync::{
    FileDelta, FileStamp, SyncProbeRequest, SyncProbeResponse, SyncRequest, SyncResponse,
    base64_bytes, content_hash,
};
pub use wire::{EngineKind, PROTOCOL_VERSION, WireMessage};
