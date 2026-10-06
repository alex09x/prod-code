/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

use super::cluster::{
    AuthToken, ClusterResponse, MetricsRequest, MetricsResponse, NodeGossip, PlaceRequest,
    PlaceResponse, ReadFileRequest, ReadFileResponse,
};
use super::exec::{ExecChanges, ExecChunk, ExecExit, ExecRequest};
use super::handshake::{HandshakeRequest, HandshakeResponse};
use super::remote_exec::{RemoteExecRequest, RemoteExecResult, RemoteExecStream};
use super::search::{SearchRequest, SearchResponse};
use super::shadow::{ShadowRunRequest, ShadowRunResponse};
use super::status::StatusResponse;
use super::sync::{SyncProbeRequest, SyncProbeResponse, SyncRequest, SyncResponse};

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
    /// Lightweight HTTP probe (e.g. GET /health or GET /status) received on gateway port.
    HttpProbe {
        method: String,
        path: String,
    },
    /// HTTP response to an HTTP probe.
    HttpResponse {
        status: u16,
        content_type: String,
        body: String,
    },
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
