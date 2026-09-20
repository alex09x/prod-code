//! Wire framing, codecs, and transport types for prod-code Remote Code Intelligence.

pub mod codec;
pub mod messages;
pub mod path;

pub use codec::ProdCodeCodec;
pub use messages::{
    ClusterResponse, ExecChanges, ExecChunk, ExecExit, ExecRequest, FileDelta, FileStamp,
    HandshakeRequest, HandshakeResponse, LoadedWorkspaceInfo, NodeGossip, PROTOCOL_VERSION,
    PeerInfo, PlaceRequest, PlaceResponse, ReadFileRequest, ReadFileResponse, StatusResponse,
    SyncProbeRequest, SyncProbeResponse, SyncRequest, SyncResponse, WireMessage, content_hash,
};
pub use path::PathTranslator;

pub const DEFAULT_PORT: u16 = 9400;
