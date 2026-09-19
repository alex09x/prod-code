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
    /// Run a command inside the client's server workspace copy (remote build/test execution).
    ExecRequest(ExecRequest),
    /// A chunk of the running command's stdout or stderr.
    ExecChunk(ExecChunk),
    /// The command finished (or could not be started).
    ExecExit(ExecExit),
    /// Files the command changed on the server (only with `pull_changes`).
    ExecChanges(ExecChanges),
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
    /// UTF-8 or binary file content. If None, indicates file deletion. Carried as base64 on
    /// the wire: a JSON byte array inflates content four-fold and dominates sync time.
    #[serde(default, with = "base64_bytes")]
    pub content: Option<Vec<u8>>,
    #[serde(default)]
    pub is_executable: bool,
}

/// Standard base64 (with padding) for `Option<Vec<u8>>` fields.
pub mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[(n >> 18) as usize & 63] as char);
            out.push(TABLE[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                TABLE[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                TABLE[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    fn value(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a') as u32 + 26),
            b'0'..=b'9' => Some((c - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    pub fn decode(text: &str) -> Result<Vec<u8>, String> {
        let bytes = text.as_bytes();
        if !bytes.len().is_multiple_of(4) {
            return Err("base64 length is not a multiple of 4".to_string());
        }
        let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
        for chunk in bytes.chunks(4) {
            let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
            if pad > 2 || (pad > 0 && chunk[..4 - pad].contains(&b'=')) {
                return Err("invalid base64 padding".to_string());
            }
            let mut n = 0u32;
            for (i, c) in chunk.iter().enumerate() {
                let v = if i >= 4 - pad {
                    0
                } else {
                    value(*c).ok_or_else(|| format!("invalid base64 character {c:?}"))?
                };
                n = (n << 6) | v;
            }
            out.push((n >> 16) as u8);
            if pad < 2 {
                out.push((n >> 8) as u8);
            }
            if pad < 1 {
                out.push(n as u8);
            }
        }
        Ok(out)
    }

    pub fn serialize<S: Serializer>(value: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(bytes) => s.serialize_some(&encode(bytes)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        let text: Option<String> = Option::deserialize(d)?;
        match text {
            Some(text) => decode(&text).map(Some).map_err(D::Error::custom),
            None => Ok(None),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn round_trips_every_padding_case() {
            for len in 0..40usize {
                let bytes: Vec<u8> = (0..len as u32).map(|i| (i * 37 + 11) as u8).collect();
                let text = encode(&bytes);
                assert_eq!(text.len() % 4, 0);
                assert_eq!(decode(&text).unwrap(), bytes, "len {len}");
            }
            assert_eq!(encode(b"Man"), "TWFu");
            assert_eq!(encode(b"Ma"), "TWE=");
            assert_eq!(encode(b"M"), "TQ==");
            assert!(decode("TQ=").is_err());
            assert!(decode("T*==").is_err());
        }

        #[test]
        fn file_delta_uses_base64_on_the_wire() {
            let delta = crate::FileDelta {
                relative_path: "src/lib.rs".to_string(),
                content: Some(b"pub fn a() {}\n".to_vec()),
                is_executable: false,
            };
            let json = serde_json::to_string(&delta).unwrap();
            assert!(
                json.contains("\"content\":\"cHViIGZuIGEoKSB7fQo=\""),
                "{json}"
            );
            let back: crate::FileDelta = serde_json::from_str(&json).unwrap();
            assert_eq!(back, delta);
            let deleted: crate::FileDelta =
                serde_json::from_str(r#"{"relative_path":"x","content":null}"#).unwrap();
            assert_eq!(deleted.content, None);
        }
    }
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

/// Run `command` (argv, no shell) in the server workspace that mirrors the client's checkout.
/// Build artifacts (`target/`, `node_modules/`) stay on the server between runs, so every
/// worktree keeps its own warm cache.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Kill the command after this many seconds; 0 means the server default.
    #[serde(default)]
    pub timeout_secs: u64,
    /// After the command, send back files it created, changed or deleted (`ExecChanges`), so
    /// formatters, code generators and lockfile updates land in the client's checkout.
    #[serde(default)]
    pub pull_changes: bool,
}

/// Files the command changed in the server workspace, sent before `ExecExit` when
/// `ExecRequest::pull_changes` was set. Deletions carry `content: None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecChanges {
    pub files: Vec<FileDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecChunk {
    pub stderr: bool,
    /// Raw bytes, base64 (output may be partial UTF-8 or contain terminal escapes).
    #[serde(with = "base64_bytes")]
    pub data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecExit {
    /// Process exit code; None when killed by a signal or by the timeout.
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub server_workspace_root: String,
    #[serde(default)]
    pub timed_out: bool,
    /// Set when the command could not be started at all.
    #[serde(default)]
    pub error: Option<String>,
}
