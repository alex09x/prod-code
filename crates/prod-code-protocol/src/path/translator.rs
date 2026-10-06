/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::uri::{file_uri, map_lsp_locations};
use serde_json::Value;
use std::path::{Path, PathBuf};
use url::Url;

/// One side's workspace root, as a path and as the URI prefix its files have.
#[derive(Debug, Clone)]
struct Root {
    /// Absolute, without a trailing separator (`/` itself stays `/`).
    path: String,
    /// The encoded `file://` URI of `path`, without a trailing `/`.
    uri: String,
}

impl Root {
    fn new(root: &str) -> Self {
        let trimmed = root.trim_end_matches('/');
        let path = if trimmed.is_empty() && root.starts_with('/') {
            "/".to_string()
        } else {
            trimmed.to_string()
        };
        let encoded = file_uri(Path::new(&path));
        let uri = if path == "/" {
            encoded
        } else {
            encoded.trim_end_matches('/').to_string()
        };
        Self { path, uri }
    }

    /// A relative or empty root names nothing, and must not match every absolute path.
    fn usable(&self) -> bool {
        self.path.starts_with('/')
    }

    /// The part of `value` after this root, empty or starting with `/`, when the root ends at
    /// a whole path component (`/w/app` is not a prefix of `/w/app2`).
    fn rest_of_path<'a>(&self, value: &'a str) -> Option<&'a str> {
        if !self.usable() {
            return None;
        }
        if self.path == "/" {
            return value.starts_with('/').then_some(value);
        }
        let rest = value.strip_prefix(self.path.as_str())?;
        (rest.is_empty() || rest.starts_with('/')).then_some(rest)
    }

    /// The part of the URI `value` after this root's URI, at a whole path component.
    fn rest_of_uri<'a>(&self, value: &'a str) -> Option<&'a str> {
        if !self.usable() {
            return None;
        }
        if self.path == "/" {
            return value
                .strip_prefix("file://")
                .filter(|rest| rest.starts_with('/'));
        }
        let rest = value.strip_prefix(self.uri.as_str())?;
        (rest.is_empty() || rest.starts_with(['/', '?', '#'])).then_some(rest)
    }

    /// This root followed by `rest` (empty or starting with `/`).
    fn join_path(&self, rest: &str) -> String {
        match (self.path.as_str(), rest) {
            ("/", "") => "/".to_string(),
            ("/", rest) => rest.to_string(),
            (path, rest) => format!("{path}{rest}"),
        }
    }
}

/// The plain path `value` moved from under `from` to under `to`.
fn map_path(value: &str, from: &Root, to: &Root) -> Option<String> {
    if !to.usable() {
        return None;
    }
    Some(to.join_path(from.rest_of_path(value)?))
}

/// The `file:` URI `value` moved from under `from` to under `to`. Another scheme, or a file
/// outside the root (a registry source, the standard library), is not a workspace location.
fn map_uri(value: &str, from: &Root, to: &Root) -> Option<String> {
    if !value.starts_with("file:") || !from.usable() || !to.usable() {
        return None;
    }
    // The usual case keeps the rest exactly as the peer encoded it.
    if let Some(rest) = from.rest_of_uri(value) {
        return Some(if to.path == "/" {
            format!("file:///{}", rest.trim_start_matches('/'))
        } else {
            format!("{}{rest}", to.uri)
        });
    }
    // The same root encoded another way: `%7E` for `~`, lower-case escapes, `localhost`.
    let url = Url::parse(value).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    let rel = path.strip_prefix(&from.path).ok()?;
    let target = if rel.as_os_str().is_empty() {
        PathBuf::from(&to.path)
    } else {
        Path::new(&to.path).join(rel)
    };
    let mut mapped = Url::from_file_path(&target).ok()?;
    if url.path().ends_with('/') && !mapped.path().ends_with('/') {
        let with_slash = format!("{}/", mapped.path());
        mapped.set_path(&with_slash);
    }
    mapped.set_query(url.query());
    mapped.set_fragment(url.fragment());
    Some(mapped.into())
}

/// A string that names a workspace file, moved to the other side.
fn map_location(value: &str, from: &Root, to: &Root) -> Option<String> {
    if value.starts_with("file:") {
        map_uri(value, from, to)
    } else {
        map_path(value, from, to)
    }
}

/// `json` with its locations moved from `from` to `to`; the original text when nothing moved.
fn translate_json(json: &str, from: &Root, to: &Root) -> Result<String, serde_json::Error> {
    let mut value: Value = serde_json::from_str(json)?;
    Ok(
        if map_lsp_locations(&mut value, &mut |text| map_location(text, from, to)) {
            value.to_string()
        } else {
            json.to_string()
        },
    )
}

/// Bi-directional path and URI translator for prod-code sessions.
#[derive(Debug, Clone)]
pub struct PathTranslator {
    client: Root,
    server: Root,
}

impl PathTranslator {
    /// Create a new translator given the client workspace root and server workspace root.
    pub fn new(client_root: &str, server_root: &str) -> Self {
        Self {
            client: Root::new(client_root),
            server: Root::new(server_root),
        }
    }

    /// Translate a local client filesystem path to the remote server filesystem path.
    pub fn to_server_path(&self, client_path: &str) -> String {
        map_path(client_path, &self.client, &self.server).unwrap_or_else(|| client_path.to_string())
    }

    /// Translate a remote server filesystem path back to the local client filesystem path.
    pub fn to_client_path(&self, server_path: &str) -> String {
        map_path(server_path, &self.server, &self.client).unwrap_or_else(|| server_path.to_string())
    }

    /// Translate a local client URI (`file:///Users/...`) to a remote server URI (`file:///srv/...`).
    pub fn to_server_uri(&self, client_uri: &str) -> String {
        map_uri(client_uri, &self.client, &self.server).unwrap_or_else(|| client_uri.to_string())
    }

    /// Translate a remote server URI (`file:///srv/...`) back to a local client URI (`file:///Users/...`).
    pub fn to_client_uri(&self, server_uri: &str) -> String {
        map_uri(server_uri, &self.server, &self.client).unwrap_or_else(|| server_uri.to_string())
    }

    /// The client's LSP JSON payload with its workspace locations moved to the server's root;
    /// an error for a payload that is not JSON.
    pub fn try_translate_lsp_to_server(&self, json_payload: &str) -> serde_json::Result<String> {
        translate_json(json_payload, &self.client, &self.server)
    }

    /// The server's LSP JSON payload with its workspace locations moved to the client's root;
    /// an error for a payload that is not JSON.
    pub fn try_translate_lsp_to_client(&self, json_payload: &str) -> serde_json::Result<String> {
        translate_json(json_payload, &self.server, &self.client)
    }

    /// Like [`Self::try_translate_lsp_to_server`]; a payload that is not JSON passes on
    /// unchanged rather than rewritten as text.
    pub fn translate_lsp_to_server(&self, json_payload: &str) -> String {
        self.try_translate_lsp_to_server(json_payload)
            .unwrap_or_else(|error| untranslated(json_payload, &error))
    }

    /// Like [`Self::try_translate_lsp_to_client`]; a payload that is not JSON passes on
    /// unchanged rather than rewritten as text.
    pub fn translate_lsp_to_client(&self, json_payload: &str) -> String {
        self.try_translate_lsp_to_client(json_payload)
            .unwrap_or_else(|error| untranslated(json_payload, &error))
    }
}

fn untranslated(json_payload: &str, error: &serde_json::Error) -> String {
    tracing::warn!(%error, len = json_payload.len(), "LSP payload is not JSON; passed on untranslated");
    json_payload.to_string()
}
