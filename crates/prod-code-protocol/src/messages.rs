use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Top-level wire message transmitted between client and remote gateway.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "payload")]
pub enum WireMessage {
    HandshakeRequest(HandshakeRequest),
    HandshakeResponse(HandshakeResponse),
    LspPayload(String),
    StatusRequest,
    StatusResponse(StatusResponse),
    SyncRequest(SyncRequest),
    SyncResponse(SyncResponse),
    Ping,
    Pong,
    Disconnect {
        reason: String,
    },
    /// Client manifest of its complete relevant file set; answered by `SyncProbeResponse`.
    SyncProbeRequest(SyncProbeRequest),
    SyncProbeResponse(SyncProbeResponse),
}

/// Supported code intelligence engine kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Rust,
    Go,
    Python,
    TypeScript,
    Generic,
}

impl EngineKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EngineKind::Rust => "rust",
            EngineKind::Go => "go",
            EngineKind::Python => "python",
            EngineKind::TypeScript => "typescript",
            EngineKind::Generic => "generic",
        }
    }
}

impl std::fmt::Display for EngineKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for EngineKind {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "rust" => EngineKind::Rust,
            "go" | "golang" => EngineKind::Go,
            "python" | "py" => EngineKind::Python,
            "typescript" | "ts" | "javascript" | "js" => EngineKind::TypeScript,
            _ => EngineKind::Generic,
        })
    }
}

/// Initial handshake request sent by client upon connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeRequest {
    pub protocol_version: u32,
    pub client_name: String,
    pub client_pid: u32,
    pub auth_token: Option<String>,
    pub client_workspace_root: String,
    #[serde(default)]
    pub preferred_engine: Option<String>,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
}

/// Handshake acknowledgement sent by remote gateway.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeResponse {
    pub protocol_version: u32,
    pub server_pid: u32,
    pub session_id: u64,
    pub server_workspace_root: String,
    pub detected_engine: String,
}

/// Real-time health and session status of the remote gateway.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusResponse {
    pub server_pid: u32,
    pub uptime_seconds: u64,
    pub active_sessions: usize,
    pub loaded_workspaces: usize,
    pub detected_engines: Vec<String>,
    #[serde(default)]
    pub memory_rss_bytes: Option<u64>,
    #[serde(default)]
    pub total_queries: u64,
    #[serde(default)]
    pub active_queries: usize,
}

impl StatusResponse {
    pub fn memory_rss_mb(&self) -> Option<f64> {
        self.memory_rss_bytes
            .map(|b| (b as f64) / (1024.0 * 1024.0))
    }
}

/// Individual file delta for fast worktree synchronization over 10G LAN.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileDelta {
    pub relative_path: String,
    /// UTF-8 or binary file content. If None, indicates file deletion.
    pub content: Option<Vec<u8>>,
    #[serde(default)]
    pub is_executable: bool,
}

/// Request to sync local worktree files to remote gateway storage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncRequest {
    pub client_workspace_root: String,
    pub files: Vec<FileDelta>,
    #[serde(default)]
    pub clean_others: bool,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
}

/// Response returned after remote gateway writes files to storage and updates in-memory engines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncResponse {
    pub files_updated: usize,
    pub files_deleted: usize,
    pub bytes_transferred: usize,
    pub duration_ms: u64,
    pub server_workspace_root: String,
}

/// FNV-1a hash of file content, shared by client manifests and gateway probes.
pub fn content_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// Size and content hash of one client file, for a manifest probe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileStamp {
    pub relative_path: String,
    pub size: u64,
    pub hash: u64,
}

/// The client's complete manifest of relevant files, sent on first contact with a workspace
/// before any content. The gateway seeds a missing workspace directory from `seed_from`
/// (the origin repository's workspace, for a worktree), deletes server files that are not in
/// the manifest, and answers with the paths it still needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncProbeRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    #[serde(default)]
    pub seed_from: Option<String>,
    pub files: Vec<FileStamp>,
}

/// Outcome of a manifest probe: what was seeded and deleted, and which files the client must
/// still send with a `SyncRequest`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncProbeResponse {
    pub server_workspace_root: String,
    pub seeded: bool,
    pub files_deleted: usize,
    pub missing: Vec<String>,
}
