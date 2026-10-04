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
    /// The first frame of a connection when the cluster shares a token (#402). A gateway that
    /// requires one closes a connection that does not open with it; one that does not ignores
    /// it. Nothing answers it.
    Auth(AuthToken),
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
    /// Transparent redirect to another gateway node in the cluster (Roadmap 5.1).
    Redirect {
        target_addr: String,
        #[serde(default)]
        reason: Option<String>,
    },
    /// High-level typed remote execution request across supported languages (Roadmap 6.1).
    RemoteExecRequest(RemoteExecRequest),
    /// Real-time streaming chunks or structured diagnostic/test events (Roadmap 6.1).
    RemoteExecStream(RemoteExecStream),
    /// Final execution verdict, diagnostics, and test summary (Roadmap 6.1).
    RemoteExecResult(RemoteExecResult),
}

/// The token a cluster's connections open with (#402). Its `Debug` never shows it, so a
/// message that is logged cannot leak it.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct AuthToken(pub String);

impl std::fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthToken(<redacted>)")
    }
}

impl AuthToken {
    /// Whether `expected` is this token, compared in a time that does not depend on where the
    /// two differ.
    pub fn matches(&self, expected: &str) -> bool {
        let (given, expected) = (self.0.as_bytes(), expected.as_bytes());
        given.len() == expected.len()
            && given
                .iter()
                .zip(expected)
                .fold(0u8, |differ, (a, b)| differ | (a ^ b))
                == 0
    }
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
    Java,
    Kotlin,
    Csharp,
    Php,
    Ruby,
    Dart,
    Zig,
    Elixir,
    Scala,
    Lua,
    Haskell,
    Ocaml,
    Clojure,
    Julia,
    Shell,
    R,
    Erlang,
    Fsharp,
    Perl,
    Solidity,
    Nim,
    D,
    Fortran,
    Sql,
    Graphql,
    Protobuf,
    Crystal,
    Groovy,
    Ada,
    V,
    Racket,
    Terraform,
    Nix,
    Markdown,
    Yaml,
    Toml,
    Json,
    Html,
    Css,
    Dockerfile,
    Svelte,
    Vue,
    Assembly,
    Powershell,
    Starlark,
    Hcl,
    Typst,
    Wat,
    SystemVerilog,
    Vhdl,
    Ballerina,
    Jsonnet,
    Cue,
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
            EngineKind::Java => "java",
            EngineKind::Kotlin => "kotlin",
            EngineKind::Csharp => "csharp",
            EngineKind::Php => "php",
            EngineKind::Ruby => "ruby",
            EngineKind::Dart => "dart",
            EngineKind::Zig => "zig",
            EngineKind::Elixir => "elixir",
            EngineKind::Scala => "scala",
            EngineKind::Lua => "lua",
            EngineKind::Haskell => "haskell",
            EngineKind::Ocaml => "ocaml",
            EngineKind::Clojure => "clojure",
            EngineKind::Julia => "julia",
            EngineKind::Shell => "shell",
            EngineKind::R => "r",
            EngineKind::Erlang => "erlang",
            EngineKind::Fsharp => "fsharp",
            EngineKind::Perl => "perl",
            EngineKind::Solidity => "solidity",
            EngineKind::Nim => "nim",
            EngineKind::D => "d",
            EngineKind::Fortran => "fortran",
            EngineKind::Sql => "sql",
            EngineKind::Graphql => "graphql",
            EngineKind::Protobuf => "protobuf",
            EngineKind::Crystal => "crystal",
            EngineKind::Groovy => "groovy",
            EngineKind::Ada => "ada",
            EngineKind::V => "v",
            EngineKind::Racket => "racket",
            EngineKind::Terraform => "terraform",
            EngineKind::Nix => "nix",
            EngineKind::Markdown => "markdown",
            EngineKind::Yaml => "yaml",
            EngineKind::Toml => "toml",
            EngineKind::Json => "json",
            EngineKind::Html => "html",
            EngineKind::Css => "css",
            EngineKind::Dockerfile => "dockerfile",
            EngineKind::Svelte => "svelte",
            EngineKind::Vue => "vue",
            EngineKind::Assembly => "assembly",
            EngineKind::Powershell => "powershell",
            EngineKind::Starlark => "starlark",
            EngineKind::Hcl => "hcl",
            EngineKind::Typst => "typst",
            EngineKind::Wat => "wat",
            EngineKind::SystemVerilog => "systemverilog",
            EngineKind::Vhdl => "vhdl",
            EngineKind::Ballerina => "ballerina",
            EngineKind::Jsonnet => "jsonnet",
            EngineKind::Cue => "cue",
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
            "java" => EngineKind::Java,
            "kotlin" | "kt" => EngineKind::Kotlin,
            "csharp" | "cs" | "c#" | "dotnet" => EngineKind::Csharp,
            "php" => EngineKind::Php,
            "ruby" | "rb" => EngineKind::Ruby,
            "dart" => EngineKind::Dart,
            "zig" => EngineKind::Zig,
            "elixir" | "ex" | "exs" => EngineKind::Elixir,
            "scala" | "sbt" => EngineKind::Scala,
            "lua" => EngineKind::Lua,
            "haskell" | "hs" => EngineKind::Haskell,
            "ocaml" | "ml" => EngineKind::Ocaml,
            "clojure" | "clj" | "cljs" | "edn" => EngineKind::Clojure,
            "julia" | "jl" => EngineKind::Julia,
            "shell" | "sh" | "bash" | "zsh" => EngineKind::Shell,
            "r" | "rstats" => EngineKind::R,
            "erlang" | "erl" => EngineKind::Erlang,
            "fsharp" | "fs" | "f#" => EngineKind::Fsharp,
            "perl" | "pl" | "pm" => EngineKind::Perl,
            "solidity" | "sol" => EngineKind::Solidity,
            "nim" => EngineKind::Nim,
            "d" | "dlang" => EngineKind::D,
            "fortran" | "f90" | "f95" => EngineKind::Fortran,
            "sql" => EngineKind::Sql,
            "graphql" | "gql" => EngineKind::Graphql,
            "protobuf" | "proto" => EngineKind::Protobuf,
            "crystal" | "cr" => EngineKind::Crystal,
            "groovy" | "gvy" => EngineKind::Groovy,
            "ada" | "adb" | "ads" => EngineKind::Ada,
            "v" | "vsh" => EngineKind::V,
            "racket" | "rkt" => EngineKind::Racket,
            "terraform" | "tf" | "tofu" => EngineKind::Terraform,
            "nix" => EngineKind::Nix,
            "markdown" | "md" => EngineKind::Markdown,
            "yaml" | "yml" => EngineKind::Yaml,
            "toml" => EngineKind::Toml,
            "json" | "jsonc" => EngineKind::Json,
            "html" | "htm" => EngineKind::Html,
            "css" | "scss" | "less" => EngineKind::Css,
            "dockerfile" | "docker" | "containerfile" => EngineKind::Dockerfile,
            "svelte" => EngineKind::Svelte,
            "vue" => EngineKind::Vue,
            "assembly" | "asm" | "s" => EngineKind::Assembly,
            "powershell" | "pwsh" | "ps1" => EngineKind::Powershell,
            "starlark" | "bazel" | "bzl" => EngineKind::Starlark,
            "hcl" | "terragrunt" => EngineKind::Hcl,
            "typst" | "typ" => EngineKind::Typst,
            "wat" | "wast" | "wasm" => EngineKind::Wat,
            "systemverilog" | "verilog" | "sv" => EngineKind::SystemVerilog,
            "vhdl" | "vhd" => EngineKind::Vhdl,
            "ballerina" | "bal" => EngineKind::Ballerina,
            "jsonnet" | "libsonnet" => EngineKind::Jsonnet,
            "cue" => EngineKind::Cue,
            _ => EngineKind::Generic,
        })
    }
}

/// Client capabilities advertised during handshake.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub direct_edit: bool,
    #[serde(default)]
    pub watch_files: bool,
    #[serde(default)]
    pub indexing_status: bool,
    #[serde(default)]
    pub shadow_runs: bool,
    #[serde(default)]
    pub multi_root: bool,
    #[serde(default)]
    pub sync_chunking: bool,
    #[serde(default)]
    pub unix_socket_local: bool,
}

/// Server capabilities granted during handshake.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub direct_edit: bool,
    #[serde(default)]
    pub watch_files: bool,
    #[serde(default)]
    pub indexing_status: bool,
    #[serde(default)]
    pub shadow_runs: bool,
    #[serde(default)]
    pub multi_root: bool,
    #[serde(default)]
    pub sync_chunking: bool,
    #[serde(default)]
    pub unix_socket_local: bool,
}

/// Initial handshake request sent by client upon connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeRequest {
    pub protocol_version: u32,
    /// Versions this client can actually speak. Absent means the legacy singleton offer in
    /// `protocol_version`; present-but-empty is invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_versions: Option<Vec<u32>>,
    /// Negotiated client capabilities offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ClientCapabilities>,
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
    /// Number of times this handshake has been transparently redirected across nodes (Roadmap 5.1).
    #[serde(default)]
    pub redirect_count: u32,
}

/// Code of the diagnostic a gateway reports for a file the analyzer panicked on (#94): the file
/// was not checked at all. It is never set aside as a diagnostic the file already had.
pub const ANALYZER_PANIC_CODE: &str = "prod-code::analyzer-panic";

/// [`HandshakeRequest::purpose`] of a session that only validates proposed texts.
pub const PURPOSE_VALIDATION: &str = "validation";

/// [`HandshakeRequest::purpose`] of an editor's session (`prod-code lsp`): the gateway pushes
/// the diagnostics of the documents it opens and changes, as a language server does (#310).
pub const PURPOSE_EDITOR: &str = "editor";

/// Handshake acknowledgement sent by remote gateway.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandshakeResponse {
    pub protocol_version: u32,
    pub server_pid: u32,
    pub session_id: u64,
    pub server_workspace_root: String,
    pub detected_engine: String,
    /// Files the gateway removed from its copy of the workspace because a command changed them
    /// after its client left and their old contents were not kept (#262). The client drops them
    /// from its sync watermark for this node, so that its next sync sends them again.
    #[serde(default)]
    pub stale_paths: Vec<String>,
    /// How long ago, in milliseconds, the gateway loaded the engine this session attaches to.
    /// `None` from a gateway too old to say, and for an editor's own server. An empty
    /// `workspace/symbol` from an engine loaded moments ago may be early and is asked again; one
    /// from a warm engine is the answer (#381).
    #[serde(default)]
    pub engine_age_ms: Option<u64>,
    /// Whether the gateway holds this engine's index questions (`workspace/symbol`,
    /// `references`, ...) until its server has loaded and indexed, and otherwise sends a
    /// `prod-code/indexing` note with the answer: then an empty answer is final. `false` from a
    /// gateway too old to do so, and for a server whose readiness is not known (#391).
    #[serde(default)]
    pub index_gated: bool,
    /// Negotiated server capabilities granted to this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ServerCapabilities>,
}

/// Real-time health and session status of the remote gateway.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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
    /// The gateway's OS and architecture, as [`crate::platform`] gives them (`macos aarch64`).
    /// Absent from older gateways, which a checkout that needs macOS must not be placed on.
    #[serde(default)]
    pub platform: Option<String>,
    /// Remote commands running on the gateway (`exec`, `check`, `test`, `lint`). A node with
    /// one running is not idle, whatever the session count says (#273).
    #[serde(default)]
    pub running_commands: Vec<RunningCommand>,
    /// What the host has left: memory and space for workspaces (#396). Empty from older
    /// gateways.
    #[serde(default)]
    pub host: HostResources,
    /// The gateway's version, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The gateway's git commit hash, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
}

/// Past this share of physical memory in use, a node takes no new workspace while another can
/// (ROADMAP 5.3: 85%).
pub const MEMORY_PRESSURE_USED: f64 = 0.85;

/// Under this share of its workspaces filesystem free, a node takes no new workspace while
/// another can: a full disk truncates synced files (#385).
pub const STORAGE_PRESSURE_FREE: f64 = 0.10;

/// Memory and disk space a gateway's host has left, those it could read.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostResources {
    /// Memory the host can still give out without swapping (`MemAvailable` on Linux, the
    /// kernel's free percentage on macOS), in bytes.
    #[serde(default)]
    pub memory_available_bytes: Option<u64>,
    /// The host's physical memory, in bytes.
    #[serde(default)]
    pub memory_total_bytes: Option<u64>,
    /// The free share of the filesystem holding the workspaces, in thousandths (150 = 15%).
    #[serde(default)]
    pub storage_free_millis: Option<u32>,
}

impl HostResources {
    /// The share of physical memory in use (0.0 to 1.0), when both numbers are known.
    pub fn memory_used_share(&self) -> Option<f64> {
        let total = self.memory_total_bytes.filter(|t| *t > 0)?;
        let available = self.memory_available_bytes?.min(total);
        Some(1.0 - available as f64 / total as f64)
    }

    /// The free share of the workspaces filesystem (0.0 to 1.0), when known.
    pub fn storage_free_share(&self) -> Option<f64> {
        self.storage_free_millis.map(|m| m as f64 / 1000.0)
    }

    /// Why the host should take no new workspace (`memory 91% used`, `disk 4% free`), or `None`
    /// when it is not known to be short of either.
    pub fn pressure(&self) -> Option<String> {
        let mut why = Vec::new();
        if let Some(used) = self.memory_used_share()
            && used > MEMORY_PRESSURE_USED
        {
            why.push(memory_text(used));
        }
        if let Some(free) = self.storage_free_share()
            && free < STORAGE_PRESSURE_FREE
        {
            why.push(disk_text(free));
        }
        (!why.is_empty()).then(|| why.join(", "))
    }

    /// `memory 41% used, disk 62% free`, the parts that are known; empty when none is.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(used) = self.memory_used_share() {
            parts.push(memory_text(used));
        }
        if let Some(free) = self.storage_free_share() {
            parts.push(disk_text(free));
        }
        parts.join(", ")
    }
}

/// Whole percent, rounded down from the nearest tenth: a disk 9.9% free reads `9%`, under the
/// 10% it is short of, not `10%`.
fn whole_percent(share: f64) -> u32 {
    ((share * 1000.0).round() as u32) / 10
}

fn memory_text(used: f64) -> String {
    format!("memory {}% used", whole_percent(used))
}

fn disk_text(free: f64) -> String {
    format!("disk {}% free", whole_percent(free))
}

/// A remote command the gateway is running.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunningCommand {
    /// The workspace copy's directory name, as `prod-code--wt-<hash>`.
    pub workspace: String,
    pub command: String,
    pub running_seconds: u64,
}

impl StatusResponse {
    /// One line per running command, longest-running first: `workspace  12m 5s  command`,
    /// with the command cut at 100 characters.
    pub fn running_lines(&self) -> Vec<String> {
        let mut commands = self.running_commands.clone();
        commands.sort_by_key(|c| std::cmp::Reverse(c.running_seconds));
        commands
            .iter()
            .map(|c| {
                let mut command: String = c.command.chars().take(100).collect();
                if c.command.chars().count() > 100 {
                    command.push('…');
                }
                format!(
                    "{}  {}m {}s  {command}",
                    c.workspace,
                    c.running_seconds / 60,
                    c.running_seconds % 60
                )
            })
            .collect()
    }

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

    /// Multi-dimensional cluster congestion score. Lower is quieter and roomier; higher is more congested.
    ///
    /// Combines:
    /// - Hard resource pressure: disqualified (>= 1000.0) if memory > 85% or disk < 10%.
    /// - Base CPU load: load per CPU (conservative 0.50 uncertainty penalty if unmeasured).
    /// - Memory pressure curve: steep penalty if memory > 70% (0.40 uncertainty penalty if unmeasured).
    /// - Storage pressure curve: penalty if disk < 25% (0.40 uncertainty penalty if unmeasured).
    /// - Workspace & session density: penalty per loaded workspace, active session, and active query.
    /// - Running command penalty: high load penalty for currently executing commands (builds/tests).
    pub fn congestion_score(&self) -> f64 {
        if self.host.pressure().is_some() {
            return 1000.0 + self.load_per_cpu().unwrap_or(1.0);
        }

        // Unknown load is penalized conservatively (0.50) so an unmonitored node is not mistaken for completely idle.
        let mut score = self.load_per_cpu().unwrap_or(0.50);

        // Memory usage penalty (above 70% used) or uncertainty penalty when telemetry is absent
        match self.host.memory_used_share() {
            Some(mem_used) => {
                if mem_used > 0.70 {
                    score += (mem_used - 0.70) * 8.0;
                }
                if mem_used > MEMORY_PRESSURE_USED {
                    score += 50.0;
                }
            }
            None => {
                score += 0.40;
            }
        }

        // Disk space penalty (below 25% free) or uncertainty penalty when telemetry is absent
        match self.host.storage_free_share() {
            Some(disk_free) => {
                if disk_free < 0.25 {
                    score += (0.25 - disk_free) * 5.0;
                }
                if disk_free < STORAGE_PRESSURE_FREE {
                    score += 50.0;
                }
            }
            None => {
                score += 0.40;
            }
        }

        // Density penalties: loaded workspaces, sessions, active queries
        score += self.loaded_workspaces as f64 * 0.08;
        score += self.active_sessions as f64 * 0.05;
        score += self.active_queries as f64 * 0.10;

        // Running commands (builds, tests, checks) add significant instantaneous load
        score += self.running_commands.len() as f64 * 0.35;

        score
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
    /// [`HandshakeResponse::stale_paths`] still recorded once this sync has been applied. A
    /// remote command syncs without a handshake, so the list comes back here as well.
    #[serde(default)]
    pub stale_paths: Vec<String>,
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
    /// What the command and every descendant it waited for used, when the server could tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ExecUsage>,
    /// The OS and architecture the command ran on (`linux x86_64`), from [`platform`]. A fix or a
    /// lint computed there is for that platform, whatever the checkout targets (#140).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// Resource use of a finished command, from `wait4`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecUsage {
    pub cpu_user_ms: u64,
    pub cpu_sys_ms: u64,
    /// Peak resident set size of the largest process in the tree, in KiB.
    pub max_rss_kb: u64,
}

impl ExecUsage {
    /// `cpu 12.3s user 1.2s sys, peak 512 MB`.
    pub fn render(&self) -> String {
        format!(
            "cpu {:.1}s user {:.1}s sys, peak {} MB",
            self.cpu_user_ms as f64 / 1000.0,
            self.cpu_sys_ms as f64 / 1000.0,
            self.max_rss_kb.div_ceil(1024)
        )
    }
}

/// Target language for polyglot remote build and test execution (Roadmap 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteExecLanguage {
    Rust,
    Go,
    Cpp,
    TypeScript,
    Python,
    Swift,
    Generic,
}

impl RemoteExecLanguage {
    pub fn as_str(&self) -> &'static str {
        match self {
            RemoteExecLanguage::Rust => "rust",
            RemoteExecLanguage::Go => "go",
            RemoteExecLanguage::Cpp => "cpp",
            RemoteExecLanguage::TypeScript => "typescript",
            RemoteExecLanguage::Python => "python",
            RemoteExecLanguage::Swift => "swift",
            RemoteExecLanguage::Generic => "generic",
        }
    }
}

impl std::fmt::Display for RemoteExecLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Target verification action or custom command (Roadmap 6.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteExecCommand {
    Check,
    Test,
    Lint,
    Bench,
    #[serde(untagged)]
    Custom(String),
}

impl RemoteExecCommand {
    pub fn as_str(&self) -> &str {
        match self {
            RemoteExecCommand::Check => "check",
            RemoteExecCommand::Test => "test",
            RemoteExecCommand::Lint => "lint",
            RemoteExecCommand::Bench => "bench",
            RemoteExecCommand::Custom(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for RemoteExecCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Output format for remote execution streams: raw streaming or structured json (Roadmap 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RemoteExecFormat {
    #[default]
    Raw,
    Json,
}

impl RemoteExecFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            RemoteExecFormat::Raw => "raw",
            RemoteExecFormat::Json => "json",
        }
    }
}

/// High-level typed remote execution request across supported languages (Roadmap 6.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    pub language: RemoteExecLanguage,
    pub command: RemoteExecCommand,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub format: RemoteExecFormat,
    #[serde(default)]
    pub timeout_secs: u64,
    #[serde(default)]
    pub pull_changes: bool,
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub client_agent: Option<String>,
    #[serde(default)]
    pub client_host: Option<String>,
}

impl RemoteExecRequest {
    /// Constructs the toolchain command line (argv) for this execution request.
    pub fn to_argv(&self) -> Vec<String> {
        let mut cmd = match (self.language, &self.command) {
            (RemoteExecLanguage::Rust, RemoteExecCommand::Check) => {
                let mut v = vec![
                    "cargo".into(),
                    "check".into(),
                    "--workspace".into(),
                    "--all-targets".into(),
                ];
                if self.format == RemoteExecFormat::Json {
                    v.push("--message-format=json".into());
                }
                v
            }
            (RemoteExecLanguage::Rust, RemoteExecCommand::Test) => {
                let mut v = vec!["cargo".into(), "test".into(), "--workspace".into()];
                if self.format == RemoteExecFormat::Json {
                    v.push("--message-format=json".into());
                }
                v
            }
            (RemoteExecLanguage::Rust, RemoteExecCommand::Lint) => {
                let mut v = vec![
                    "cargo".into(),
                    "clippy".into(),
                    "--workspace".into(),
                    "--all-targets".into(),
                ];
                if self.format == RemoteExecFormat::Json {
                    v.push("--message-format=json".into());
                }
                v
            }
            (RemoteExecLanguage::Rust, RemoteExecCommand::Bench) => {
                let mut v = vec!["cargo".into(), "bench".into(), "--workspace".into()];
                if self.format == RemoteExecFormat::Json {
                    v.push("--message-format=json".into());
                }
                v
            }
            (RemoteExecLanguage::Rust, RemoteExecCommand::Custom(c)) => {
                c.split_whitespace().map(String::from).collect()
            }

            (RemoteExecLanguage::Go, RemoteExecCommand::Check) => {
                vec!["go".into(), "vet".into(), "./...".into()]
            }
            (RemoteExecLanguage::Go, RemoteExecCommand::Test) => {
                let mut v = vec!["go".into(), "test".into()];
                if self.format == RemoteExecFormat::Json {
                    v.push("-json".into());
                } else {
                    v.push("-v".into());
                }
                v.push("./...".into());
                v
            }
            (RemoteExecLanguage::Go, RemoteExecCommand::Lint) => {
                let mut v = vec!["golangci-lint".into(), "run".into()];
                if self.format == RemoteExecFormat::Json {
                    v.push("--out-format=json".into());
                }
                v
            }
            (RemoteExecLanguage::Go, RemoteExecCommand::Bench) => {
                vec![
                    "go".into(),
                    "test".into(),
                    "-run=^$".into(),
                    "-bench=.".into(),
                    "./...".into(),
                ]
            }
            (RemoteExecLanguage::Go, RemoteExecCommand::Custom(c)) => {
                c.split_whitespace().map(String::from).collect()
            }

            (RemoteExecLanguage::TypeScript, RemoteExecCommand::Check) => {
                vec![
                    "npx".into(),
                    "--no-install".into(),
                    "tsc".into(),
                    "--noEmit".into(),
                ]
            }
            (RemoteExecLanguage::TypeScript, RemoteExecCommand::Test) => {
                let mut v = vec![
                    "npx".into(),
                    "--no-install".into(),
                    "vitest".into(),
                    "run".into(),
                ];
                if self.format == RemoteExecFormat::Json {
                    v.push("--reporter=json".into());
                }
                v
            }
            (RemoteExecLanguage::TypeScript, RemoteExecCommand::Lint) => {
                let mut v = vec![
                    "npx".into(),
                    "--no-install".into(),
                    "eslint".into(),
                    ".".into(),
                ];
                if self.format == RemoteExecFormat::Json {
                    v.push("--format=json".into());
                }
                v
            }
            (RemoteExecLanguage::TypeScript, RemoteExecCommand::Bench) => {
                vec![
                    "npx".into(),
                    "--no-install".into(),
                    "vitest".into(),
                    "bench".into(),
                    "run".into(),
                ]
            }
            (RemoteExecLanguage::TypeScript, RemoteExecCommand::Custom(c)) => {
                c.split_whitespace().map(String::from).collect()
            }

            (RemoteExecLanguage::Python, RemoteExecCommand::Check) => {
                let mut v = vec!["python3".into(), "-m".into(), "compileall".into(), "-q".into()];
                if self.args.is_empty() {
                    v.push(".".into());
                }
                v
            }
            (RemoteExecLanguage::Python, RemoteExecCommand::Test) => {
                let mut v = vec!["pytest".into()];
                if self.format == RemoteExecFormat::Json {
                    v.push("--json-report".into());
                }
                v
            }
            (RemoteExecLanguage::Python, RemoteExecCommand::Lint) => {
                let mut v = vec!["ruff".into(), "check".into(), ".".into()];
                if self.format == RemoteExecFormat::Json {
                    v.push("--output-format=json".into());
                }
                v
            }
            (RemoteExecLanguage::Python, RemoteExecCommand::Bench) => {
                vec!["pytest".into(), "--benchmark-only".into()]
            }
            (RemoteExecLanguage::Python, RemoteExecCommand::Custom(c)) => {
                c.split_whitespace().map(String::from).collect()
            }

            (RemoteExecLanguage::Cpp, RemoteExecCommand::Check) => {
                vec!["ninja".into(), "-k".into(), "0".into()]
            }
            (RemoteExecLanguage::Cpp, RemoteExecCommand::Test) => {
                vec!["ctest".into(), "--output-on-failure".into()]
            }
            (RemoteExecLanguage::Cpp, RemoteExecCommand::Lint) => {
                vec!["clang-tidy".into(), "-p".into(), "build".into()]
            }
            (RemoteExecLanguage::Cpp, RemoteExecCommand::Bench) => {
                vec!["ninja".into(), "bench".into()]
            }
            (RemoteExecLanguage::Cpp, RemoteExecCommand::Custom(c)) => {
                c.split_whitespace().map(String::from).collect()
            }

            (RemoteExecLanguage::Swift, RemoteExecCommand::Check) => {
                vec!["swift".into(), "build".into()]
            }
            (RemoteExecLanguage::Swift, RemoteExecCommand::Test) => {
                vec!["swift".into(), "test".into()]
            }
            (RemoteExecLanguage::Swift, RemoteExecCommand::Lint) => {
                let mut v = vec!["swiftlint".into()];
                if self.format == RemoteExecFormat::Json {
                    v.push("--reporter".into());
                    v.push("json".into());
                }
                v
            }
            (RemoteExecLanguage::Swift, RemoteExecCommand::Bench) => {
                vec![
                    "swift".into(),
                    "run".into(),
                    "-c".into(),
                    "release".into(),
                    "bench".into(),
                ]
            }
            (RemoteExecLanguage::Swift, RemoteExecCommand::Custom(c)) => {
                c.split_whitespace().map(String::from).collect()
            }

            (RemoteExecLanguage::Generic, RemoteExecCommand::Custom(c)) => {
                c.split_whitespace().map(String::from).collect()
            }
            (RemoteExecLanguage::Generic, cmd) => {
                vec![cmd.as_str().to_string()]
            }
        };

        cmd.extend(self.args.clone());
        cmd
    }

    /// Converts this high-level request into the low-level `ExecRequest` executed by the gateway.
    pub fn into_exec_request(self) -> ExecRequest {
        let command = self.to_argv();
        ExecRequest {
            client_workspace_root: self.client_workspace_root,
            base_workspace_name: self.base_workspace_name,
            command,
            env: self.env,
            timeout_secs: self.timeout_secs,
            pull_changes: self.pull_changes,
            subdir: self.subdir,
            client_agent: self.client_agent,
            client_host: self.client_host,
        }
    }
}

/// One source span within a diagnostic event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecSpan {
    pub file: String,
    pub line_start: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    pub col_start: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col_end: Option<u32>,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A structured compiler or linter diagnostic finding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecDiagnostic {
    pub level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<RemoteExecSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// A structured test or benchmark execution event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum RemoteExecTestEvent {
    Started {
        name: String,
    },
    Passed {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Failed {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assertion_diff: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backtrace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    Skipped {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Bench {
        name: String,
        estimate: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<String>,
    },
}

impl RemoteExecTestEvent {
    pub fn name(&self) -> &str {
        match self {
            Self::Started { name }
            | Self::Passed { name, .. }
            | Self::Failed { name, .. }
            | Self::Skipped { name, .. }
            | Self::Bench { name, .. } => name,
        }
    }
}

/// Real-time streaming message emitted during remote execution (Roadmap 6.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteExecStream {
    Chunk(ExecChunk),
    Diagnostic(RemoteExecDiagnostic),
    TestEvent(RemoteExecTestEvent),
}

/// Final execution verdict and summary across compiler and test runs (Roadmap 6.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecResult {
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub server_workspace_root: String,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ExecUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<RemoteExecDiagnostic>,
    #[serde(default)]
    pub tests_passed: u64,
    #[serde(default)]
    pub tests_failed: u64,
    #[serde(default)]
    pub tests_skipped: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_failures: Vec<RemoteExecTestEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub benches: Vec<RemoteExecTestEvent>,
}

impl RemoteExecResult {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && self.error.is_none()
    }
}

/// Parse a line from `cargo --message-format=json` (or standard Cargo/libtest output) into a diagnostic or test event if applicable.
pub fn parse_cargo_json_event(line: &str) -> Option<RemoteExecStream> {
    let trimmed = line.trim();

    // 1. Try parsing JSON format (compiler messages or unstable/custom json test records)
    if trimmed.starts_with('{')
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(reason) = value.get("reason").and_then(|r| r.as_str())
                && reason == "compiler-message"
                    && let Some(msg) = value.get("message") {
                        let level = msg.get("level").and_then(|l| l.as_str()).unwrap_or("error").to_string();
                        let message_text = msg.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
                        let code = msg
                            .get("code")
                            .and_then(|c| c.get("code"))
                            .and_then(|c| c.as_str())
                            .map(str::to_string);
                        let rendered = msg.get("rendered").and_then(|r| r.as_str()).map(str::to_string);

                        let mut spans = Vec::new();
                        if let Some(spans_arr) = msg.get("spans").and_then(|s| s.as_array()) {
                            for s in spans_arr {
                                if let Some(file) = s.get("file_name").and_then(|f| f.as_str()) {
                                    let line_start = s.get("line_start").and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                                    let line_end = s.get("line_end").and_then(|l| l.as_u64()).map(|l| l as u32);
                                    let col_start = s.get("column_start").and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                                    let col_end = s.get("column_end").and_then(|c| c.as_u64()).map(|c| c as u32);
                                    let is_primary = s.get("is_primary").and_then(|p| p.as_bool()).unwrap_or(false);
                                    let label = s.get("label").and_then(|lbl| lbl.as_str()).map(str::to_string);
                                    spans.push(RemoteExecSpan {
                                        file: file.to_string(),
                                        line_start,
                                        line_end,
                                        col_start,
                                        col_end,
                                        is_primary,
                                        label,
                                    });
                                }
                            }
                        }

                        return Some(RemoteExecStream::Diagnostic(RemoteExecDiagnostic {
                            level,
                            code,
                            message: message_text,
                            spans,
                            rendered,
                            suggestion: None,
                        }));
                    }

            if let Some(t) = value.get("type").and_then(|t| t.as_str())
                && t == "test" {
                    let event = value.get("event").and_then(|e| e.as_str()).unwrap_or("");
                    let name = value.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let duration_ms = value.get("exec_time").and_then(|t| t.as_f64()).map(|s| (s * 1000.0) as u64);
                    match event {
                        "started" => return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Started { name })),
                        "ok" => return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms })),
                        "failed" => {
                            let output = value.get("stdout").and_then(|o| o.as_str()).map(str::to_string);
                            return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
                                name,
                                duration_ms,
                                message: None,
                                assertion_diff: None,
                                backtrace: None,
                                output,
                            }));
                        }
                        "ignored" => return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped { name, reason: None })),
                        "bench" => {
                            let median = value.get("median").and_then(|m| m.as_f64()).unwrap_or(0.0);
                            return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Bench {
                                name,
                                estimate: format!("{median:.2} ns/iter"),
                                range: None,
                            }));
                        }
                        _ => {}
                    }
                }
        }

    // 2. Parse standard Cargo/libtest text output lines (emitted by the test runner during `cargo test`)
    // Format: `test <name> ... ok` / `test <name> ... FAILED` / `test <name> ... ignored` / `test <name> ... bench: <est>`
    if let Some(rest) = trimmed.strip_prefix("test ")
        && !rest.starts_with("result:")
            && let Some((name, outcome_part)) = rest.rsplit_once(" ... ") {
                let test_name = name.trim().to_string();
                let outcome = outcome_part.trim();
                if outcome == "ok" || outcome.starts_with("ok ") {
                    let duration_ms = outcome
                        .find('(')
                        .and_then(|open| outcome[open..].find('s').map(|close| &outcome[open + 1..open + close]))
                        .and_then(|s_str| s_str.trim().parse::<f64>().ok())
                        .map(|s| (s * 1000.0) as u64);
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed {
                        name: test_name,
                        duration_ms,
                    }));
                } else if outcome == "FAILED" || outcome.starts_with("FAILED ") {
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
                        name: test_name,
                        duration_ms: None,
                        message: None,
                        assertion_diff: None,
                        backtrace: None,
                        output: None,
                    }));
                } else if outcome == "ignored" || outcome.starts_with("ignored ") {
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped {
                        name: test_name,
                        reason: None,
                    }));
                } else if let Some(bench_str) = outcome.strip_prefix("bench:") {
                    return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Bench {
                        name: test_name,
                        estimate: bench_str.trim().to_string(),
                        range: None,
                    }));
                }
            }

    None
}

/// Parse a line from `go test` (either `-json` or standard human-readable format) into a test event if applicable.
pub fn parse_go_test_json_event(line: &str) -> Option<RemoteExecStream> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with('{')
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let action = value.get("Action").and_then(|a| a.as_str())?;
            let test = value.get("Test").and_then(|t| t.as_str())?;
            let pkg = value.get("Package").and_then(|p| p.as_str()).unwrap_or("");
            let name = if pkg.is_empty() {
                test.to_string()
            } else {
                format!("{pkg}.{test}")
            };
            let duration_ms = value.get("Elapsed").and_then(|e| e.as_f64()).map(|s| (s * 1000.0) as u64);

            return match action {
                "run" => Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Started { name })),
                "pass" => Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms })),
                "fail" => Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
                    name,
                    duration_ms,
                    message: None,
                    assertion_diff: None,
                    backtrace: None,
                    output: None,
                })),
                "skip" => Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped { name, reason: None })),
                _ => None,
            };
        }

    // Standard human-readable `go test` text lines:
    // "=== RUN   TestFoo"
    // "--- PASS: TestFoo (0.01s)"
    // "--- FAIL: TestBar (0.05s)"
    // "--- SKIP: TestBaz (0.00s)"
    // "--- BENCH: BenchmarkFoo (0.00s)"
    // Summary lines such as "PASS", "FAIL", "ok  \tpkg\t0.012s", "FAIL\tpkg\t0.015s" are ignored.
    if let Some(rest) = trimmed.strip_prefix("=== RUN") {
        let name = rest.trim().to_string();
        if !name.is_empty() {
            return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Started { name }));
        }
    } else if let Some(rest) = trimmed.strip_prefix("--- PASS:") {
        let (name, duration_ms) = parse_go_raw_test_suffix(rest);
        return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed {
            name,
            duration_ms,
        }));
    } else if let Some(rest) = trimmed.strip_prefix("--- FAIL:") {
        let (name, duration_ms) = parse_go_raw_test_suffix(rest);
        return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed {
            name,
            duration_ms,
            message: None,
            assertion_diff: None,
            backtrace: None,
            output: None,
        }));
    } else if let Some(rest) = trimmed.strip_prefix("--- SKIP:") {
        let (name, _) = parse_go_raw_test_suffix(rest);
        return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped {
            name,
            reason: None,
        }));
    } else if let Some(rest) = trimmed.strip_prefix("--- BENCH:") {
        let (name, _) = parse_go_raw_test_suffix(rest);
        return Some(RemoteExecStream::TestEvent(RemoteExecTestEvent::Bench {
            name,
            estimate: "bench".to_string(),
            range: None,
        }));
    }

    None
}

fn parse_go_raw_test_suffix(rest: &str) -> (String, Option<u64>) {
    let rest = rest.trim();
    if let Some((name, dur_part)) = rest.rsplit_once(" (") {
        let dur_ms = dur_part
            .strip_suffix("s)")
            .or_else(|| dur_part.strip_suffix(')'))
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(|s| (s * 1000.0) as u64);
        (name.trim().to_string(), dur_ms)
    } else {
        (rest.to_string(), None)
    }
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
    /// The OS the node must run (`macos`), matched against the start of its status platform.
    /// A Go module whose cgo includes macOS headers compiles nowhere else.
    #[serde(default)]
    pub os: Option<String>,
    /// Whether to rebalance workload even if the workspace has active sessions (Phase 5.3).
    #[serde(default)]
    pub rebalance_active: bool,
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

/// This machine's OS and architecture, as `linux x86_64` or `macos aarch64`.
pub fn platform() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
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
    /// Run hypotheses in a lightweight RAM-backed (/dev/shm) in-memory overlay shadow root (Phase 7.4).
    #[serde(default)]
    pub in_memory: bool,
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
    /// Attributable ranking score formatted as a string (e.g. "0.0345").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<String>,
    /// Attributable ranking breakdown reasons (e.g. lexical BM25, typed graph centrality, dense similarity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_reasons: Option<Vec<String>>,
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
    /// The dense half of the ranking; `None` when the gateway has no embedding model and the
    /// ranking was lexical only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dense: Option<DenseStatus>,
    /// Whether typed graph fusion was applied during search ranking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_fused: Option<bool>,
}

/// How far the dense half of a search had got.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct DenseStatus {
    /// The question was ranked by meaning too: the model is there and some declarations have
    /// vectors.
    pub used: bool,
    /// Declarations with a vector so far; the rest are being embedded in the background.
    pub embedded: usize,
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

    /// An older gateway sends no running commands, and a status lists the ones it has longest
    /// first, with a long command line cut (#273).
    #[test]
    fn a_status_lists_its_running_commands_longest_first() {
        let old = r#"{"server_pid":1,"uptime_seconds":2,"active_sessions":0,"loaded_workspaces":0,"detected_engines":[]}"#;
        let status: StatusResponse = serde_json::from_str(old).expect("an older gateway's status");
        assert!(status.running_commands.is_empty());
        assert!(status.running_lines().is_empty());

        let status = StatusResponse {
            running_commands: vec![
                RunningCommand {
                    workspace: "shop".to_string(),
                    command: "cargo check".to_string(),
                    running_seconds: 5,
                },
                RunningCommand {
                    workspace: "shop--wt-1a2b".to_string(),
                    command: format!("cargo test {}", "x".repeat(120)),
                    running_seconds: 725,
                },
            ],
            ..status
        };
        let lines = status.running_lines();
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].starts_with("shop--wt-1a2b  12m 5s  cargo test xx"),
            "{}",
            lines[0]
        );
        assert!(lines[0].ends_with('…'), "{}", lines[0]);
        assert_eq!(lines[1], "shop  0m 5s  cargo check");
    }

    /// A host past 85% of its memory or under 10% of its disk is under pressure, and says which;
    /// one that reports neither, as an older gateway does, is not (#396).
    #[test]
    fn a_host_short_of_memory_or_disk_says_so() {
        let old = r#"{"server_pid":1,"uptime_seconds":2,"active_sessions":0,"loaded_workspaces":0,"detected_engines":[]}"#;
        let status: StatusResponse = serde_json::from_str(old).expect("an older gateway's status");
        assert_eq!(status.host, HostResources::default());
        assert_eq!(status.host.pressure(), None);
        assert_eq!(status.host.describe(), "");

        let gib = 1 << 30;
        let roomy = HostResources {
            memory_available_bytes: Some(20 * gib),
            memory_total_bytes: Some(100 * gib),
            storage_free_millis: Some(620),
        };
        assert_eq!(roomy.pressure(), None);
        assert_eq!(roomy.describe(), "memory 80% used, disk 62% free");

        let short_of_memory = HostResources {
            memory_available_bytes: Some(9 * gib),
            ..roomy.clone()
        };
        assert_eq!(
            short_of_memory.pressure().as_deref(),
            Some("memory 91% used")
        );

        let short_of_both = HostResources {
            storage_free_millis: Some(40),
            ..short_of_memory
        };
        assert_eq!(
            short_of_both.pressure().as_deref(),
            Some("memory 91% used, disk 4% free")
        );

        let disk_only = HostResources {
            storage_free_millis: Some(99),
            ..HostResources::default()
        };
        assert_eq!(disk_only.pressure().as_deref(), Some("disk 9% free"));
        assert_eq!(disk_only.describe(), "disk 9% free");
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
            stale_paths: vec!["src/big.rs".to_string()],
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

    #[test]
    fn redirect_message_round_trips() {
        let msg = WireMessage::Redirect {
            target_addr: "192.168.2.191:9400".to_string(),
            reason: Some("engine loaded warm on peer".to_string()),
        };
        let encoded = serde_json::to_string(&msg).expect("encode redirect");
        let decoded: WireMessage = serde_json::from_str(&encoded).expect("decode redirect");
        assert_eq!(msg, decoded);
    }

    #[test]
    fn polyglot_engine_kinds_round_trip() {
        let languages = [
            ("haskell", EngineKind::Haskell),
            ("ocaml", EngineKind::Ocaml),
            ("clojure", EngineKind::Clojure),
            ("julia", EngineKind::Julia),
            ("shell", EngineKind::Shell),
            ("r", EngineKind::R),
            ("erlang", EngineKind::Erlang),
            ("fsharp", EngineKind::Fsharp),
            ("perl", EngineKind::Perl),
            ("solidity", EngineKind::Solidity),
            ("nim", EngineKind::Nim),
            ("d", EngineKind::D),
            ("fortran", EngineKind::Fortran),
            ("sql", EngineKind::Sql),
            ("graphql", EngineKind::Graphql),
            ("protobuf", EngineKind::Protobuf),
            ("crystal", EngineKind::Crystal),
            ("groovy", EngineKind::Groovy),
            ("ada", EngineKind::Ada),
            ("v", EngineKind::V),
            ("racket", EngineKind::Racket),
            ("terraform", EngineKind::Terraform),
            ("nix", EngineKind::Nix),
            ("markdown", EngineKind::Markdown),
            ("yaml", EngineKind::Yaml),
            ("toml", EngineKind::Toml),
            ("json", EngineKind::Json),
            ("html", EngineKind::Html),
            ("css", EngineKind::Css),
            ("dockerfile", EngineKind::Dockerfile),
            ("svelte", EngineKind::Svelte),
            ("vue", EngineKind::Vue),
            ("assembly", EngineKind::Assembly),
            ("powershell", EngineKind::Powershell),
            ("starlark", EngineKind::Starlark),
            ("hcl", EngineKind::Hcl),
            ("typst", EngineKind::Typst),
            ("wat", EngineKind::Wat),
            ("systemverilog", EngineKind::SystemVerilog),
            ("vhdl", EngineKind::Vhdl),
            ("ballerina", EngineKind::Ballerina),
            ("jsonnet", EngineKind::Jsonnet),
            ("cue", EngineKind::Cue),
        ];
        for (name, kind) in languages {
            assert_eq!(kind.as_str(), name);
            assert_eq!(name.parse::<EngineKind>().unwrap(), kind);
        }
    }

    #[test]
    fn search_response_survives_wire_trip_and_older_payloads() {
        let hit = SearchHit {
            file: "src/search.rs".into(),
            line: 42,
            kind: "struct".into(),
            name: "WorkspaceIndex".into(),
            container: None,
            signature: "pub struct WorkspaceIndex".into(),
            doc: "Primary workspace declaration index.".into(),
            score: Some("0.0385".into()),
            rank_reasons: Some(vec![
                "lexical: matched 'workspace'".into(),
                "graph: struct in-degree 12 (centrality 1.76)".into(),
            ]),
        };
        let response = SearchResponse {
            server_workspace_root: "/srv/w".into(),
            hits: vec![hit.clone()],
            indexed_files: 10,
            indexed_declarations: 200,
            took_ms: 3,
            error: None,
            dense: Some(DenseStatus {
                used: true,
                embedded: 200,
            }),
            graph_fused: Some(true),
        };
        let encoded = serde_json::to_string(&response).expect("serialize response");
        let decoded: SearchResponse = serde_json::from_str(&encoded).expect("deserialize response");
        assert_eq!(response, decoded);

        // Older payload without score, rank_reasons, or graph_fused still decodes cleanly
        let old_json = r#"{
            "server_workspace_root": "/srv/w",
            "hits": [{
                "file": "src/search.rs",
                "line": 42,
                "kind": "struct",
                "name": "WorkspaceIndex",
                "signature": "pub struct WorkspaceIndex",
                "doc": "Primary index."
            }],
            "indexed_files": 10,
            "indexed_declarations": 200,
            "took_ms": 3
        }"#;
        let old_decoded: SearchResponse = serde_json::from_str(old_json).expect("older payload");
        assert_eq!(old_decoded.hits[0].score, None);
        assert_eq!(old_decoded.hits[0].rank_reasons, None);
        assert_eq!(old_decoded.graph_fused, None);
    }

    #[test]
    fn congestion_score_reflects_load_memory_disk_and_density() {
        let gib = 1 << 30;

        // 1. Idle node: plenty of resources, 0 load, 0 workspaces
        let idle = StatusResponse {
            server_pid: 1,
            uptime_seconds: 100,
            active_sessions: 0,
            loaded_workspaces: 0,
            detected_engines: vec!["rust".into()],
            memory_rss_bytes: None,
            total_queries: 0,
            active_queries: 0,
            load_average_millis: Some(20), // 0.02
            cpu_count: Some(1),
            platform: Some("linux x86_64".into()),
            running_commands: Vec::new(),
            host: HostResources {
                memory_available_bytes: Some(16 * gib),
                memory_total_bytes: Some(32 * gib), // 50% used
                storage_free_millis: Some(650),      // 65% free
            },
            version: None,
            git_commit: None,
        };
        assert!((idle.congestion_score() - 0.02).abs() < 1e-6);

        // 2. Memory penalty above 70%
        let mem_heavy = StatusResponse {
            host: HostResources {
                memory_available_bytes: Some(6 * gib),
                memory_total_bytes: Some(30 * gib), // 80% used (0.10 above 0.70 => +0.80)
                storage_free_millis: Some(500),
            },
            ..idle.clone()
        };
        // 0.02 + 0.10 * 8.0 = 0.82
        assert!((mem_heavy.congestion_score() - 0.82).abs() < 1e-4);

        // 3. Disk penalty below 25%
        let disk_heavy = StatusResponse {
            host: HostResources {
                memory_available_bytes: Some(16 * gib),
                memory_total_bytes: Some(32 * gib),
                storage_free_millis: Some(150), // 15% free (0.10 below 0.25 => +0.50)
            },
            ..idle.clone()
        };
        // 0.02 + 0.10 * 5.0 = 0.52
        assert!((disk_heavy.congestion_score() - 0.52).abs() < 1e-4);

        // 4. Density penalties (workspaces, sessions, running commands)
        let loaded = StatusResponse {
            loaded_workspaces: 5, // 5 * 0.08 = 0.40
            active_sessions: 4,   // 4 * 0.05 = 0.20
            running_commands: vec![
                RunningCommand {
                    workspace: "w1".into(),
                    command: "cargo check".into(),
                    running_seconds: 10,
                },
                RunningCommand {
                    workspace: "w2".into(),
                    command: "cargo test".into(),
                    running_seconds: 20,
                },
            ], // 2 * 0.35 = 0.70
            ..idle.clone()
        };
        // 0.02 + 0.40 + 0.20 + 0.70 = 1.32
        assert!((loaded.congestion_score() - 1.32).abs() < 1e-4);

        // 5. Hard pressure (e.g. storage < 10%)
        let pressured = StatusResponse {
            host: HostResources {
                storage_free_millis: Some(50), // 5% free => hard pressure
                ..idle.host.clone()
            },
            ..idle.clone()
        };
        assert!(pressured.congestion_score() >= 1000.0);

        // 6. Unknown telemetry penalized conservatively (never scored as 0.0 idle)
        let unmeasured = StatusResponse {
            load_average_millis: None,
            cpu_count: None,
            host: HostResources::default(),
            ..idle.clone()
        };
        // 0.50 (load) + 0.40 (mem) + 0.40 (disk) = 1.30
        assert!((unmeasured.congestion_score() - 1.30).abs() < 1e-4);
        assert!(unmeasured.congestion_score() > idle.congestion_score());
    }

    #[test]
    fn remote_exec_request_to_argv_and_round_trip() {
        let req = RemoteExecRequest {
            client_workspace_root: "/path/to/project".into(),
            base_workspace_name: Some("project".into()),
            language: RemoteExecLanguage::Rust,
            command: RemoteExecCommand::Check,
            args: vec!["--lib".into()],
            env: vec![("RUST_BACKTRACE".into(), "1".into())],
            format: RemoteExecFormat::Json,
            timeout_secs: 60,
            pull_changes: true,
            subdir: Some("subcrate".into()),
            client_agent: Some("agent-cli".into()),
            client_host: Some("host.lan".into()),
        };

        // 1. Verify toolchain argv generation
        let argv = req.to_argv();
        assert_eq!(
            argv,
            vec!["cargo", "check", "--workspace", "--all-targets", "--message-format=json", "--lib"]
        );

        // 2. Verify conversion into ExecRequest
        let exec_req = req.clone().into_exec_request();
        assert_eq!(exec_req.command, argv);
        assert_eq!(exec_req.timeout_secs, 60);
        assert!(exec_req.pull_changes);

        // 3. Verify wire round-trip as WireMessage::RemoteExecRequest
        let wire = WireMessage::RemoteExecRequest(req.clone());
        let json = serde_json::to_string(&wire).unwrap();
        let decoded: WireMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, decoded);

        // 4. Test other languages argv generation
        let go_test = RemoteExecRequest {
            language: RemoteExecLanguage::Go,
            command: RemoteExecCommand::Test,
            format: RemoteExecFormat::Json,
            args: vec!["-run".into(), "TestOrder".into()],
            ..req.clone()
        };
        assert_eq!(go_test.to_argv(), vec!["go", "test", "-json", "./...", "-run", "TestOrder"]);

        let go_test_raw = RemoteExecRequest {
            language: RemoteExecLanguage::Go,
            command: RemoteExecCommand::Test,
            format: RemoteExecFormat::Raw,
            args: vec![],
            ..req.clone()
        };
        assert_eq!(go_test_raw.to_argv(), vec!["go", "test", "-v", "./..."]);

        let ts_lint = RemoteExecRequest {
            language: RemoteExecLanguage::TypeScript,
            command: RemoteExecCommand::Lint,
            format: RemoteExecFormat::Json,
            args: vec![],
            ..req.clone()
        };
        assert_eq!(ts_lint.to_argv(), vec!["npx", "--no-install", "eslint", ".", "--format=json"]);

        let py_test = RemoteExecRequest {
            language: RemoteExecLanguage::Python,
            command: RemoteExecCommand::Test,
            format: RemoteExecFormat::Json,
            args: vec!["-k".into(), "test_auth".into()],
            ..req.clone()
        };
        assert_eq!(py_test.to_argv(), vec!["pytest", "--json-report", "-k", "test_auth"]);
    }

    #[test]
    fn remote_exec_stream_and_result_round_trip() {
        let diag = RemoteExecDiagnostic {
            level: "error".into(),
            code: Some("E0308".into()),
            message: "mismatched types".into(),
            spans: vec![RemoteExecSpan {
                file: "src/lib.rs".into(),
                line_start: 12,
                line_end: Some(12),
                col_start: 5,
                col_end: Some(15),
                is_primary: true,
                label: Some("expected u32, found &str".into()),
            }],
            rendered: Some("error[E0308]: mismatched types".into()),
            suggestion: None,
        };

        let stream_msg = WireMessage::RemoteExecStream(RemoteExecStream::Diagnostic(diag.clone()));
        let json = serde_json::to_string(&stream_msg).unwrap();
        let decoded: WireMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(stream_msg, decoded);

        let test_ev = RemoteExecTestEvent::Failed {
            name: "test_login".into(),
            duration_ms: Some(145),
            message: Some("assertion failed: `left == right`".into()),
            assertion_diff: Some("- expected 200\n+ got 403".into()),
            backtrace: Some("at test_login (tests/auth.rs:42)".into()),
            output: Some("panicked at tests/auth.rs:42".into()),
        };

        let stream_ev = WireMessage::RemoteExecStream(RemoteExecStream::TestEvent(test_ev.clone()));
        let json_ev = serde_json::to_string(&stream_ev).unwrap();
        let decoded_ev: WireMessage = serde_json::from_str(&json_ev).unwrap();
        assert_eq!(stream_ev, decoded_ev);

        let result = RemoteExecResult {
            exit_code: Some(1),
            duration_ms: 1250,
            server_workspace_root: "/home/alex/storage/ws".into(),
            timed_out: false,
            error: None,
            usage: Some(ExecUsage {
                cpu_user_ms: 850,
                cpu_sys_ms: 120,
                max_rss_kb: 45000,
            }),
            platform: Some("linux x86_64".into()),
            diagnostics: vec![diag],
            tests_passed: 10,
            tests_failed: 1,
            tests_skipped: 2,
            test_failures: vec![test_ev],
            benches: vec![RemoteExecTestEvent::Bench {
                name: "bench_throughput".into(),
                estimate: "254.5 ns/iter".into(),
                range: Some("+/- 12".into()),
            }],
        };

        let result_msg = WireMessage::RemoteExecResult(result.clone());
        let json_res = serde_json::to_string(&result_msg).unwrap();
        let decoded_res: WireMessage = serde_json::from_str(&json_res).unwrap();
        assert_eq!(result_msg, decoded_res);
        assert!(!result.ok());
    }

    #[test]
    fn parse_cargo_and_go_test_json_events_correctly() {
        // Cargo compiler message
        let cargo_diag_json = r#"{"reason":"compiler-message","package_id":"foo","message":{"level":"error","code":{"code":"E0425"},"message":"cannot find value `x` in this scope","spans":[{"file_name":"src/lib.rs","line_start":3,"column_start":9,"is_primary":true,"label":"not found in this scope"}],"rendered":"error[E0425]: cannot find value `x` in this scope\n"}}"#;
        let event = parse_cargo_json_event(cargo_diag_json).unwrap();
        match event {
            RemoteExecStream::Diagnostic(d) => {
                assert_eq!(d.level, "error");
                assert_eq!(d.code.as_deref(), Some("E0425"));
                assert_eq!(d.spans.len(), 1);
                assert_eq!(d.spans[0].file, "src/lib.rs");
                assert_eq!(d.spans[0].line_start, 3);
            }
            other => panic!("expected Diagnostic, got {other:?}"),
        }

        // Cargo test passed
        let cargo_test_ok = r#"{"type":"test","event":"ok","name":"tests::it_works","exec_time":0.005}"#;
        let event = parse_cargo_json_event(cargo_test_ok).unwrap();
        match event {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
                assert_eq!(name, "tests::it_works");
                assert_eq!(duration_ms, Some(5));
            }
            other => panic!("expected TestEvent::Passed, got {other:?}"),
        }

        // Go test fail event
        let go_fail = r#"{"Time":"2026-10-02T12:00:00Z","Action":"fail","Package":"pkg/orders","Test":"TestCalculateTotal","Elapsed":0.042}"#;
        let event = parse_go_test_json_event(go_fail).unwrap();
        match event {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed { name, duration_ms, .. }) => {
                assert_eq!(name, "pkg/orders.TestCalculateTotal");
                assert_eq!(duration_ms, Some(42));
            }
            other => panic!("expected TestEvent::Failed, got {other:?}"),
        }

        // Standard Cargo/libtest text output parsing
        let pass_line = "test tests::my_test_pass ... ok";
        let pass_ev = parse_cargo_json_event(pass_line).unwrap();
        match pass_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
                assert_eq!(name, "tests::my_test_pass");
                assert_eq!(duration_ms, None);
            }
            other => panic!("expected TestEvent::Passed, got {other:?}"),
        }

        let pass_with_dur = "test tests::my_test_fast ... ok (0.012s)";
        let pass_dur_ev = parse_cargo_json_event(pass_with_dur).unwrap();
        match pass_dur_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
                assert_eq!(name, "tests::my_test_fast");
                assert_eq!(duration_ms, Some(12));
            }
            other => panic!("expected TestEvent::Passed with duration, got {other:?}"),
        }

        let fail_line = "test tests::my_test_fail ... FAILED";
        let fail_ev = parse_cargo_json_event(fail_line).unwrap();
        match fail_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed { name, .. }) => {
                assert_eq!(name, "tests::my_test_fail");
            }
            other => panic!("expected TestEvent::Failed, got {other:?}"),
        }

        let skip_line = "test tests::my_test_skip ... ignored";
        let skip_ev = parse_cargo_json_event(skip_line).unwrap();
        match skip_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped { name, .. }) => {
                assert_eq!(name, "tests::my_test_skip");
            }
            other => panic!("expected TestEvent::Skipped, got {other:?}"),
        }

        let bench_line = "test tests::bench_compute ... bench: 45.20 ns/iter (+/- 2)";
        let bench_ev = parse_cargo_json_event(bench_line).unwrap();
        match bench_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Bench { name, estimate, .. }) => {
                assert_eq!(name, "tests::bench_compute");
                assert_eq!(estimate, "45.20 ns/iter (+/- 2)");
            }
            other => panic!("expected TestEvent::Bench, got {other:?}"),
        }

        // Summary line should be ignored
        let summary_line = "test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.05s";
        assert!(parse_cargo_json_event(summary_line).is_none());

        // Standard Go human-readable text output parsing
        let go_run_line = "=== RUN   TestLoginHandler";
        let go_run_ev = parse_go_test_json_event(go_run_line).unwrap();
        match go_run_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Started { name }) => {
                assert_eq!(name, "TestLoginHandler");
            }
            other => panic!("expected TestEvent::Started, got {other:?}"),
        }

        let go_pass_line = "--- PASS: TestLoginHandler (0.015s)";
        let go_pass_ev = parse_go_test_json_event(go_pass_line).unwrap();
        match go_pass_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
                assert_eq!(name, "TestLoginHandler");
                assert_eq!(duration_ms, Some(15));
            }
            other => panic!("expected TestEvent::Passed, got {other:?}"),
        }

        let go_subtest_pass = "    --- PASS: TestLoginHandler/Valid_Credentials (0.002s)";
        let go_sub_ev = parse_go_test_json_event(go_subtest_pass).unwrap();
        match go_sub_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Passed { name, duration_ms }) => {
                assert_eq!(name, "TestLoginHandler/Valid_Credentials");
                assert_eq!(duration_ms, Some(2));
            }
            other => panic!("expected TestEvent::Passed, got {other:?}"),
        }

        let go_fail_line = "--- FAIL: TestRefreshToken (0.034s)";
        let go_fail_ev = parse_go_test_json_event(go_fail_line).unwrap();
        match go_fail_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Failed { name, duration_ms, .. }) => {
                assert_eq!(name, "TestRefreshToken");
                assert_eq!(duration_ms, Some(34));
            }
            other => panic!("expected TestEvent::Failed, got {other:?}"),
        }

        let go_skip_line = "--- SKIP: TestIntegrationDisabled (0.00s)";
        let go_skip_ev = parse_go_test_json_event(go_skip_line).unwrap();
        match go_skip_ev {
            RemoteExecStream::TestEvent(RemoteExecTestEvent::Skipped { name, .. }) => {
                assert_eq!(name, "TestIntegrationDisabled");
            }
            other => panic!("expected TestEvent::Skipped, got {other:?}"),
        }

        // Go summaries should be ignored
        assert!(parse_go_test_json_event("PASS").is_none());
        assert!(parse_go_test_json_event("FAIL").is_none());
        assert!(parse_go_test_json_event("ok  \tpkg/auth\t0.021s").is_none());
    }

    #[test]
    fn python_check_default_and_custom_targets() {
        let default_check = RemoteExecRequest {
            client_workspace_root: "/test".into(),
            base_workspace_name: None,
            language: RemoteExecLanguage::Python,
            command: RemoteExecCommand::Check,
            args: vec![],
            env: vec![],
            format: RemoteExecFormat::Raw,
            timeout_secs: 10,
            pull_changes: false,
            subdir: None,
            client_agent: None,
            client_host: None,
        };
        assert_eq!(
            default_check.to_argv(),
            vec!["python3", "-m", "compileall", "-q", "."]
        );

        let custom_check = RemoteExecRequest {
            args: vec!["src/mypackage".into(), "tests/".into()],
            ..default_check
        };
        assert_eq!(
            custom_check.to_argv(),
            vec!["python3", "-m", "compileall", "-q", "src/mypackage", "tests/"]
        );
    }
}

