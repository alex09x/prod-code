//! Wire framing, codecs, and transport types for prod-code Remote Code Intelligence.

pub mod codec;
pub mod messages;
pub mod path;

pub use codec::ProdCodeCodec;
pub use messages::{
    ANALYZER_PANIC_CODE, ClusterResponse, DenseStatus, ExecChanges, ExecChunk, ExecExit,
    ExecMetric, ExecRequest, ExecUsage, FileDelta, FileStamp, HandshakeRequest, HandshakeResponse,
    LoadedWorkspaceInfo, MetricsRequest, MetricsResponse, NodeGossip, PROTOCOL_VERSION,
    PURPOSE_VALIDATION, PeerInfo, PlaceRequest, PlaceResponse, QueryMetric, ReadFileRequest,
    ReadFileResponse, SearchHit, SearchRequest, SearchResponse, ShadowHypothesis,
    ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, StatusResponse, SyncProbeRequest,
    SyncProbeResponse, SyncRequest, SyncResponse, WireMessage, client_host, content_hash,
    detect_client_agent,
};
pub use path::PathTranslator;

pub const DEFAULT_PORT: u16 = 9400;
