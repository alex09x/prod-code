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
    /// Run a command per hypothesis in shadows of the workspace (roadmap 7.4).
    ShadowRunRequest(ShadowRunRequest),
    ShadowRunResponse(ShadowRunResponse),
    /// Rank a workspace's declarations against a question (roadmap 8.4).
    SearchRequest(SearchRequest),
    SearchResponse(SearchResponse),
    /// Read a source file that lives only on the gateway host (standard library, dependency
    /// registries, SDK headers): what a definition outside the checkout points at.
    ReadFileRequest(ReadFileRequest),
    ReadFileResponse(ReadFileResponse),
    /// Node-to-node heartbeat: a gateway's status, loaded workspaces and known peers. The
    /// receiving node answers with its own `Gossip`.
    Gossip(NodeGossip),
    /// A client asks any node for the whole cluster as that node sees it.
    ClusterRequest,
    ClusterResponse(ClusterResponse),
    /// A client asks any node where a workspace should live.
    PlaceRequest(PlaceRequest),
    PlaceResponse(PlaceResponse),
    /// Usage metrics of one node (who asked what, how often, how fast).
    MetricsRequest(MetricsRequest),
    MetricsResponse(MetricsResponse),
}

/// Supported code intelligence engine kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Rust,
    Go,
    Python,
    TypeScript,
    Cpp,
    Swift,
    Generic,
}

impl EngineKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EngineKind::Rust => "rust",
            EngineKind::Go => "go",
            EngineKind::Python => "python",
            EngineKind::TypeScript => "typescript",
            EngineKind::Cpp => "cpp",
            EngineKind::Swift => "swift",
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
            "cpp" | "c++" | "c" | "cxx" | "clangd" => EngineKind::Cpp,
            "swift" => EngineKind::Swift,
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
    /// Directory inside the checkout (relative, `/`-separated) whose project the session is
    /// about, when it is a nested project of another language than the checkout root
    /// (a SwiftPM package inside a Rust repository): the gateway loads the engine there.
    #[serde(default)]
    pub engine_subpath: Option<String>,
    /// What drives this client: `claude-code`, `codex`, `cli`, or a custom `PROD_CODE_AGENT`.
    #[serde(default)]
    pub client_agent: Option<String>,
    /// The client machine's hostname.
    #[serde(default)]
    pub client_host: Option<String>,
    /// What the session is for, when that changes where it should run. [`PURPOSE_VALIDATION`]:
    /// the session opens proposed texts only to ask what the analyzer thinks of them, so the
    /// gateway serves it from a second engine for the same workspace, and the overlay and its
    /// revert never invalidate what the main engine has computed (#73).
    #[serde(default)]
    pub purpose: Option<String>,
}

/// Code of the diagnostic a gateway reports for a file the analyzer panicked on (#94): the file
/// was not checked at all. It is never set aside as a diagnostic the file already had.
pub const ANALYZER_PANIC_CODE: &str = "prod-code::analyzer-panic";

/// [`HandshakeRequest::purpose`] of a session that only validates proposed texts.
pub const PURPOSE_VALIDATION: &str = "validation";

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
    /// 1-minute load average of the host in thousandths (1500 = 1.5), when known.
    #[serde(default)]
    pub load_average_millis: Option<u32>,
    /// Logical CPUs of the host, when known.
    #[serde(default)]
    pub cpu_count: Option<usize>,
}

impl StatusResponse {
    /// Load per CPU (1-minute load average divided by CPU count); lower is quieter.
    pub fn load_per_cpu(&self) -> Option<f64> {
        match (self.load_average_millis, self.cpu_count) {
            (Some(load), Some(cpus)) if cpus > 0 => Some(load as f64 / 1000.0 / cpus as f64),
            _ => None,
        }
    }

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
    /// The workspace directory had never been established (no handshake or manifest probe
    /// touched it): the server side was reset while the client still holds a watermark, so
    /// the client must forget it and resend the full manifest.
    #[serde(default)]
    pub workspace_was_fresh: bool,
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
    /// Directory inside the workspace (relative, `/`-separated) to run the command in; the
    /// workspace root when absent. Lets a nested project be built and tested in place.
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub client_agent: Option<String>,
    #[serde(default)]
    pub client_host: Option<String>,
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

/// A request to read a file on the gateway host, for definitions that resolve outside the
/// checkout (toolchain sources, dependency caches, system headers).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadFileRequest {
    /// Absolute path on the gateway host.
    pub path: String,
    /// Upper bound on the bytes returned; 0 means the server default.
    #[serde(default)]
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadFileResponse {
    pub path: String,
    /// The file's bytes (base64 on the wire), or None when it could not be read.
    #[serde(default, with = "base64_bytes")]
    pub content: Option<Vec<u8>>,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// A workspace a gateway currently holds in memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadedWorkspaceInfo {
    pub name: String,
    pub engine: String,
    pub sessions: usize,
}

/// One gateway's heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodeGossip {
    /// The address other nodes and clients reach this gateway at (`host:port`).
    pub addr: String,
    pub status: StatusResponse,
    #[serde(default)]
    pub workspaces: Vec<LoadedWorkspaceInfo>,
    /// Every peer address this node knows, so membership spreads transitively.
    #[serde(default)]
    pub peers: Vec<String>,
    #[serde(default)]
    pub sent_at_ms: u64,
}

/// A peer as seen by the answering node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerInfo {
    pub addr: String,
    pub status: StatusResponse,
    #[serde(default)]
    pub workspaces: Vec<LoadedWorkspaceInfo>,
    /// Seconds since this node last heard from the peer (0 for the answering node itself).
    pub last_seen_secs: u64,
    pub alive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterResponse {
    /// The answering node's own address.
    pub this_node: String,
    pub nodes: Vec<PeerInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaceRequest {
    pub workspace_name: String,
    #[serde(default)]
    pub engine: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaceResponse {
    /// The node to use, or None when no node in the cluster can serve the engine.
    pub node: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsRequest {
    /// Window in seconds; 0 means everything the node still holds.
    #[serde(default)]
    pub since_secs: u64,
}

/// Queries of one (agent, host, workspace, method) group in the window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryMetric {
    pub agent: String,
    pub host: String,
    pub workspace: String,
    pub method: String,
    pub count: u64,
    pub errors: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub max_ms: u64,
}

/// Commands run through `exec` (including check/lint/test) in the window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecMetric {
    pub agent: String,
    pub host: String,
    pub workspace: String,
    pub command: String,
    pub count: u64,
    pub failures: u64,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsResponse {
    pub node: String,
    pub since_secs: u64,
    /// Events the node holds in memory (the JSONL files on disk hold everything).
    pub events_in_memory: u64,
    pub queries: Vec<QueryMetric>,
    pub execs: Vec<ExecMetric>,
    /// Sync rounds: files and bytes uploaded to this node in the window.
    pub sync_rounds: u64,
    pub sync_files: u64,
    pub sync_bytes: u64,
}

/// What drives this client, for usage metrics: `PROD_CODE_AGENT` when set, otherwise
/// `claude-code` / `codex` when launched by those tools, else `cli`.
pub fn detect_client_agent() -> String {
    if let Ok(agent) = std::env::var("PROD_CODE_AGENT")
        && !agent.trim().is_empty()
    {
        return agent;
    }
    if std::env::var_os("CLAUDECODE").is_some()
        || std::env::var_os("CLAUDE_CODE_ENTRYPOINT").is_some()
    {
        return "claude-code".to_string();
    }
    if std::env::var_os("CODEX_SANDBOX").is_some()
        || std::env::var_os("CODEX_HOME").is_some()
        || std::env::var_os("CODEX_THREAD_ID").is_some()
    {
        return "codex".to_string();
    }
    "cli".to_string()
}

/// The client machine's hostname (cached).
pub fn client_host() -> String {
    static HOST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        std::process::Command::new("hostname")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|h| !h.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .unwrap_or_else(|| "unknown".to_string())
    })
    .clone()
}

/// One hypothesis of a shadow run (roadmap 7.4): a name and the complete contents of the files
/// it changes, relative to the workspace root; `content: None` deletes the file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowHypothesis {
    pub name: String,
    pub files: Vec<FileDelta>,
}

/// Run `command` once per hypothesis, each in its own shadow of the server workspace. On Linux
/// a shadow is an overlay mount of the workspace copy at the workspace's own path: build tools
/// see the same absolute paths, so warm caches (`target/`, `node_modules/`) stay valid, every
/// write lands in the hypothesis's upper directory and the workspace copy is never modified;
/// hypotheses run in parallel. Without user namespaces they run one at a time in place and the
/// touched files are restored afterwards.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowRunRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    pub hypotheses: Vec<ShadowHypothesis>,
    pub command: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Kill a hypothesis after this many seconds; 0 means the server default.
    #[serde(default)]
    pub timeout_secs: u64,
    /// Directory inside the workspace (relative, `/`-separated) to run in; the root when absent.
    #[serde(default)]
    pub subdir: Option<String>,
    /// Hypotheses run at once; 0 means the server default (cores / 8, at least 1).
    #[serde(default)]
    pub parallel: usize,
    /// Bytes of combined output kept per hypothesis (its tail); 0 means the server default.
    #[serde(default)]
    pub tail_bytes: usize,
    #[serde(default)]
    pub client_agent: Option<String>,
    #[serde(default)]
    pub client_host: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowHypothesisResult {
    pub name: String,
    /// Process exit code; None when killed by a signal, the timeout or a client disconnect.
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    #[serde(default)]
    pub timed_out: bool,
    /// Set when the hypothesis could not be staged or started at all.
    #[serde(default)]
    pub error: Option<String>,
    /// Tail of the combined stdout/stderr, base64 on the wire.
    #[serde(default, with = "base64_bytes")]
    pub output_tail: Option<Vec<u8>>,
    /// Bytes the command printed in total.
    #[serde(default)]
    pub output_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShadowRunResponse {
    pub server_workspace_root: String,
    /// `overlay` (user namespace + overlayfs, hypotheses in parallel) or `in-place`
    /// (one at a time, files restored afterwards).
    pub mode: String,
    pub results: Vec<ShadowHypothesisResult>,
    /// Set when the run could not start at all (workspace not synced, empty command, ...).
    #[serde(default)]
    pub error: Option<String>,
}

/// Search a workspace's declarations by intent (roadmap 8.4): the gateway keeps an index of
/// every declaration with the doc comment above it, and ranks them against the query's words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    pub query: String,
    /// Hits to return; 0 means the server default.
    #[serde(default)]
    pub limit: usize,
    /// Restrict to declarations under this relative path.
    #[serde(default)]
    pub subpath: Option<String>,
    #[serde(default)]
    pub client_agent: Option<String>,
    #[serde(default)]
    pub client_host: Option<String>,
}

/// One declaration the query matched.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchHit {
    /// Path relative to the workspace root.
    pub file: String,
    pub line: u32,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub container: Option<String>,
    pub signature: String,
    /// First sentence of the doc comment attached to the declaration.
    #[serde(default)]
    pub doc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchResponse {
    pub server_workspace_root: String,
    pub hits: Vec<SearchHit>,
    pub indexed_files: usize,
    pub indexed_declarations: usize,
    pub took_ms: u64,
    #[serde(default)]
    pub error: Option<String>,
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    /// Every message that crosses the wire has to survive the trip. A field added on one side
    /// and missing on the other is the failure this catches: `#[serde(default)]` makes an old
    /// message readable by a new binary, and that only holds while somebody checks.
    #[test]
    fn a_message_without_its_optional_fields_still_decodes() {
        let minimal = r#"{"client_workspace_root":"/w","files":[],"clean_others":false}"#;
        let request: SyncRequest = serde_json::from_str(minimal).expect("an older client's sync");
        assert_eq!(request.client_workspace_root, "/w");
        assert!(request.base_workspace_name.is_none());

        let delta = r#"{"relative_path":"a.rs"}"#;
        let file: FileDelta = serde_json::from_str(delta).expect("a deletion");
        assert!(
            file.content.is_none(),
            "no content is how a deletion is spelled"
        );
        assert!(!file.is_executable);
    }

    #[test]
    fn a_wire_message_keeps_its_kind_through_a_round_trip() {
        let original = WireMessage::SyncResponse(SyncResponse {
            server_workspace_root: "/srv/w".to_string(),
            files_updated: 3,
            files_deleted: 1,
            bytes_transferred: 4096,
            duration_ms: 12,
            workspace_was_fresh: true,
        });
        let json = serde_json::to_string(&original).expect("encode");
        let back: WireMessage = serde_json::from_str(&json).expect("decode");
        match back {
            WireMessage::SyncResponse(response) => {
                assert_eq!(response.files_updated, 3);
                assert!(response.workspace_was_fresh);
            }
            other => panic!("a sync response came back as {other:?}"),
        }
    }

    #[test]
    fn a_content_hash_depends_on_the_content_and_nothing_else() {
        assert_eq!(content_hash(b"abc"), content_hash(b"abc"));
        assert_ne!(content_hash(b"abc"), content_hash(b"abd"));
        assert_ne!(content_hash(b""), content_hash(b"\0"));
    }
}
