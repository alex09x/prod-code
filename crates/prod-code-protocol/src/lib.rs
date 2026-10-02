//! Wire framing, codecs, and transport types for prod-code Remote Code Intelligence.

pub mod codec;
pub mod discovery;
pub mod messages;
pub mod negotiation;
pub mod path;
pub mod readiness;
pub mod tls;
pub mod transport;

pub use codec::ProdCodeCodec;
pub use messages::{
    ANALYZER_PANIC_CODE, AuthToken, ClientCapabilities, ClusterResponse, DenseStatus, ExecChanges,
    ExecChunk, ExecExit, ExecMetric, ExecRequest, ExecUsage, FileDelta, FileStamp, HandshakeRequest,
    HandshakeResponse, HostResources, LoadedWorkspaceInfo, MEMORY_PRESSURE_USED, MetricsRequest,
    MetricsResponse, NodeGossip, PROTOCOL_VERSION, PURPOSE_EDITOR, PURPOSE_VALIDATION, PeerInfo,
    PlaceRequest, PlaceResponse, QueryMetric, ReadFileRequest, ReadFileResponse, RunningCommand,
    STORAGE_PRESSURE_FREE, SearchHit, SearchRequest, SearchResponse, ServerCapabilities,
    ShadowHypothesis, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, StatusResponse,
    SyncProbeRequest, SyncProbeResponse, SyncRequest, SyncResponse, WireMessage, client_host,
    content_hash, detect_client_agent, platform,
};
pub use negotiation::{
    ProtocolNegotiationError, SUPPORTED_PROTOCOL_VERSIONS, default_server_capabilities,
    negotiate_capabilities, negotiate_protocol_version, supported_protocol_versions,
    validate_selected_protocol_version,
};
pub use path::PathTranslator;
pub use tls::{
    ClientTlsConfig, ServerTlsConfig, TlsMode, DEFAULT_TLS_SERVER_NAME, TLS_CA_ENV, TLS_CERT_ENV,
    TLS_ENV_VARS, TLS_KEY_ENV, TLS_MODE_ENV, TLS_PIN_ENV, TLS_SERVER_NAME_ENV,
    cert_sha256_fingerprint, parse_pins,
};
pub use transport::{
    AnyStream, clear_client_tls_cache, connect, connect_stream, connect_stream_with,
    connect_stream_with_client_config, connect_stream_with_tls, connect_with,
    default_client_tls_built, init_client_tls_from_env, set_default_client_tls,
    set_default_client_tls_built,
};
#[cfg(unix)]
pub use transport::{connect_unix, connect_unix_with};

pub const DEFAULT_PORT: u16 = 9400;
