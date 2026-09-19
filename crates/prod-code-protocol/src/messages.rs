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
    Ping,
    Pong,
    Disconnect { reason: String },
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
