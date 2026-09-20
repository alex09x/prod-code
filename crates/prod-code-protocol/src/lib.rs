//! Wire framing, codecs, and transport types for prod-code Remote Code Intelligence.

pub mod codec;
pub mod messages;
pub mod path;

pub use codec::ProdCodeCodec;
pub use messages::{
    ClusterResponse, ExecChanges, ExecChunk, ExecExit, ExecMetric, ExecRequest, FileDelta,
    FileStamp, HandshakeRequest, HandshakeResponse, LoadedWorkspaceInfo, MetricsRequest,
    MetricsResponse, NodeGossip, PROTOCOL_VERSION, PeerInfo, PlaceRequest, PlaceResponse,
    QueryMetric, ReadFileRequest, ReadFileResponse, StatusResponse, SyncProbeRequest,
    SyncProbeResponse, SyncRequest, SyncResponse, WireMessage, client_host, content_hash,
    detect_client_agent,
};
pub use path::PathTranslator;

pub const DEFAULT_PORT: u16 = 9400;
