//! Path and URI translation between local client environments and remote server storage.
//!
//! An LSP message is translated as JSON, not as text: only a value that names a file under the
//! workspace root, at a whole path component, is mapped. Source text (`didOpen`, `didChange`, an
//! edit's `newText`) and documentation pass through as they were written (#438).

use serde_json::Value;
use std::path::{Path, PathBuf};
use url::Url;

/// The `file://` URI of an absolute `path`, percent-encoded so that spaces, `#`, `%` and
/// non-ASCII names survive the trip back. A relative path, which no file URI can name, keeps
/// the plain form.
pub fn file_uri(path: &Path) -> String {
    Url::from_file_path(path)
        .map(String::from)
        .unwrap_or_else(|_| format!("file://{}", path.display()))
}

/// The local path a `file:` URI names, decoded exactly once; `None` for another scheme or a
/// URI naming another host.
pub fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let url = Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

/// The path a `file:` URI or a plain path names. A URI is decoded once; a plain path is taken
/// literally, so a file named `100%41.rs` stays that file.
pub fn uri_or_path(text: &str) -> PathBuf {
    if text.starts_with("file:") {
        file_uri_path(text).unwrap_or_else(|| PathBuf::from(text.trim_start_matches("file://")))
    } else {
        PathBuf::from(text)
    }
}

/// Fields whose strings are text — source, edits, documentation, messages — never a location.
const TEXT_FIELDS: &[&str] = &[
    "text",
    "newText",
    "insertText",
    "filterText",
    "sortText",
    "documentation",
    "contents",
    "value",
    "message",
    "label",
    "detail",
    "title",
    "tooltip",
];

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

/// Maps LSP location strings and file-URI object keys, leaving source and documentation text
/// alone. The mapper returns `None` for locations it does not own. This also lets the editor
/// mirror use the same payload boundaries as workspace translation. Returns whether anything
/// changed; a mapper that only observes locations can always return `None`.
pub fn map_lsp_locations(
    value: &mut Value,
    mapper: &mut impl FnMut(&str) -> Option<String>,
) -> bool {
    match value {
        Value::String(text) => match mapper(text) {
            Some(mapped) => {
                *text = mapped;
                true
            }
            None => false,
        },
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            map_lsp_locations(item, mapper) | changed
        }),
        Value::Object(map) => {
            let mut changed = false;
            let uri_keys: Vec<(String, String)> = map
                .keys()
                .filter(|key| key.starts_with("file:"))
                .filter_map(|key| mapper(key).map(|mapped| (key.clone(), mapped)))
                .collect();
            for (key, mapped) in uri_keys {
                if let Some(item) = map.remove(&key) {
                    map.insert(mapped, item);
                    changed = true;
                }
            }
            for (key, item) in map.iter_mut() {
                if !TEXT_FIELDS.contains(&key.as_str()) || (key == "label" && item.is_array()) {
                    changed |= map_lsp_locations(item, mapper);
                }
            }
            changed
        }
        _ => false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_and_uri_translation() {
        let translator = PathTranslator::new(
            "/Users/dev/Documents/workspace/my-app",
            "/srv/prod-code/workspaces/my-app",
        );

        let server_path =
            translator.to_server_path("/Users/dev/Documents/workspace/my-app/src/main.rs");
        assert_eq!(server_path, "/srv/prod-code/workspaces/my-app/src/main.rs");

        let client_path = translator.to_client_path("/srv/prod-code/workspaces/my-app/src/main.rs");
        assert_eq!(
            client_path,
            "/Users/dev/Documents/workspace/my-app/src/main.rs"
        );

        let client_uri = "file:///Users/dev/Documents/workspace/my-app/src/lib.rs";
        let server_uri = translator.to_server_uri(client_uri);
        assert_eq!(
            server_uri,
            "file:///srv/prod-code/workspaces/my-app/src/lib.rs"
        );

        let roundtrip_uri = translator.to_client_uri(&server_uri);
        assert_eq!(roundtrip_uri, client_uri);
    }

    #[test]
    fn test_lsp_json_translation() {
        let translator = PathTranslator::new(
            "/Users/dev/Documents/workspace/my-app",
            "/srv/prod-code/workspaces/my-app",
        );

        let client_lsp = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///Users/dev/Documents/workspace/my-app/src/main.rs","text":"fn main() {}"}}}"#;
        let server_lsp = translator.translate_lsp_to_server(client_lsp);
        assert!(server_lsp.contains("file:///srv/prod-code/workspaces/my-app/src/main.rs"));
        assert!(!server_lsp.contains("/Users/dev"));

        let restored_lsp = translator.translate_lsp_to_client(&server_lsp);
        assert_eq!(json(&restored_lsp), json(client_lsp));
    }

    #[test]
    fn a_payload_with_nothing_to_map_passes_through_byte_for_byte() {
        let raw = r#"{"jsonrpc":"2.0","id":7,"result":{"z":1,"a":"é"}}"#;
        assert_eq!(app().translate_lsp_to_client(raw), raw);
    }

    #[test]
    fn a_payload_that_is_not_json_is_not_rewritten() {
        let raw = r#"{"uri":"file:///Users/dev/app/src/lib.rs","text":"/Users/dev/app""#;
        let t = app();
        assert_eq!(t.translate_lsp_to_server(raw), raw);
        assert!(t.try_translate_lsp_to_server(raw).is_err());
        assert!(t.try_translate_lsp_to_client("not json").is_err());
    }

    #[test]
    fn workspace_edit_keys_resource_operations_and_command_arguments_are_mapped() {
        let edit = serde_json::json!({ "jsonrpc": "2.0", "id": 8, "result": {
            "changes": { "file:///srv/ws/app/src/a.rs": [] },
            "documentChanges": [
                { "kind": "rename", "oldUri": "file:///srv/ws/app/src/old.rs", "newUri": "file:///srv/ws/app/src/new.rs" },
                { "kind": "create", "uri": "file:///srv/ws/app/src/fresh.rs" },
                { "textDocument": { "uri": "file:///srv/ws/app/src/b.rs", "version": null },
                  "edits": [ { "range": {}, "newText": "file:///srv/ws/app/src/b.rs" } ] }
            ],
            "command": { "title": "Run /srv/ws/app", "command": "rust-analyzer.runSingle", "arguments": [
                "file:///srv/ws/app/src/main.rs",
                { "cwd": "/srv/ws/app", "workspaceRoot": "/srv/ws/app/", "cargoArgs": ["--manifest-path", "/srv/ws/app/Cargo.toml"] }
            ] }
        } });
        let back = json(&app().translate_lsp_to_client(&edit.to_string()));
        let r = &back["result"];
        assert!(r["changes"].get("file:///Users/dev/app/src/a.rs").is_some());
        assert_eq!(r["changes"].as_object().unwrap().len(), 1);
        let dc = &r["documentChanges"];
        assert_eq!(dc[0]["oldUri"], "file:///Users/dev/app/src/old.rs");
        assert_eq!(dc[0]["newUri"], "file:///Users/dev/app/src/new.rs");
        assert_eq!(dc[1]["uri"], "file:///Users/dev/app/src/fresh.rs");
        assert_eq!(
            dc[2]["textDocument"]["uri"],
            "file:///Users/dev/app/src/b.rs"
        );
        assert_eq!(dc[2]["edits"][0]["newText"], "file:///srv/ws/app/src/b.rs");
        let args = &r["command"]["arguments"];
        assert_eq!(args[0], "file:///Users/dev/app/src/main.rs");
        assert_eq!(args[1]["cwd"], "/Users/dev/app");
        assert_eq!(args[1]["workspaceRoot"], "/Users/dev/app/");
        assert_eq!(args[1]["cargoArgs"][1], "/Users/dev/app/Cargo.toml");
        assert_eq!(r["command"]["title"], "Run /srv/ws/app");
    }

    #[test]
    fn external_sources_and_other_schemes_are_left_alone() {
        let t = app();
        for uri in [
            "file:///home/dev/.cargo/registry/src/index/serde-1.0.0/src/lib.rs",
            "file:///srv/ws/app2/src/lib.rs",
            "untitled:Untitled-1",
            "https://docs.rs/serde",
            "jar:file:///srv/ws/app/lib.jar!/A.class",
            "file://build-host/srv/ws/app/src/lib.rs",
        ] {
            assert_eq!(t.to_client_uri(uri), uri);
        }
        assert_eq!(
            t.to_client_path("relative/srv/ws/app"),
            "relative/srv/ws/app"
        );
        let empty = PathTranslator::new("", "/srv/ws/app");
        assert_eq!(empty.to_server_path("/etc/hosts"), "/etc/hosts");
        assert_eq!(
            empty.to_server_uri("file:///etc/hosts"),
            "file:///etc/hosts"
        );
        assert_eq!(empty.to_client_path("/srv/ws/app/a.rs"), "/srv/ws/app/a.rs");
        assert_eq!(
            empty.to_client_uri("file:///srv/ws/app/a.rs"),
            "file:///srv/ws/app/a.rs"
        );
    }

    #[test]
    fn roots_with_trailing_separators_map_like_those_without() {
        let t = PathTranslator::new("/Users/dev/app/", "/srv/ws/app//");
        assert_eq!(
            t.to_server_path("/Users/dev/app/src/x.rs"),
            "/srv/ws/app/src/x.rs"
        );
        assert_eq!(t.to_server_path("/Users/dev/app"), "/srv/ws/app");
        assert_eq!(t.to_server_path("/Users/dev/app2"), "/Users/dev/app2");
        assert_eq!(
            t.to_server_uri("file:///Users/dev/app/"),
            "file:///srv/ws/app/"
        );
        let root = PathTranslator::new("/", "/srv/ws/app");
        assert_eq!(root.to_server_path("/a/b.rs"), "/srv/ws/app/a/b.rs");
        assert_eq!(root.to_client_path("/srv/ws/app/a/b.rs"), "/a/b.rs");
        assert_eq!(root.to_client_path("/srv/ws/app"), "/");
        assert_eq!(
            root.to_server_uri("file:///a/b.rs"),
            "file:///srv/ws/app/a/b.rs"
        );
        assert_eq!(root.to_server_uri("file:///"), "file:///srv/ws/app/");
        assert_eq!(
            root.to_client_uri("file:///srv/ws/app/a/b.rs"),
            "file:///a/b.rs"
        );
        assert_eq!(root.to_client_uri("file:///srv/ws/app"), "file:///");
        assert_eq!(
            root.to_client_uri("file:///srv/ws/app?query#fragment"),
            "file:///?query#fragment"
        );
    }

    #[test]
    fn special_characters_in_the_root_round_trip_as_uris() {
        let client = "/Users/dev/my app #1/100%41 ü";
        let server = "/srv/ws/my app #1";
        let t = PathTranslator::new(client, server);
        let file = Path::new(client).join("src/ä b#%.rs");
        let client_uri = file_uri(&file);
        assert_eq!(
            client_uri,
            "file:///Users/dev/my%20app%20%231/100%2541%20%C3%BC/src/%C3%A4%20b%23%25.rs"
        );
        let server_uri = t.to_server_uri(&client_uri);
        assert_eq!(
            file_uri_path(&server_uri).unwrap(),
            Path::new(server).join("src/ä b#%.rs")
        );
        assert_eq!(t.to_client_uri(&server_uri), client_uri);
        assert_eq!(file_uri_path(&client_uri).unwrap(), file);

        // A peer that encodes the root another way still names the same directory.
        let odd = "file:///Users/dev/my%20app%20%231/100%2541%20u%CC%88/x.rs";
        assert_eq!(t.to_server_uri(odd), odd, "a different name, not the root");
        let lower = "file:///Users/dev/my%20app%20%231/100%2541%20%c3%bc/x.rs";
        assert_eq!(
            t.to_server_uri(lower),
            "file:///srv/ws/my%20app%20%231/x.rs"
        );
        let host = "file://localhost/Users/dev/my%20app%20%231/100%2541%20%C3%BC/x.rs";
        assert_eq!(t.to_server_uri(host), "file:///srv/ws/my%20app%20%231/x.rs");
    }

    #[test]
    fn uris_decode_once_and_plain_paths_not_at_all() {
        assert_eq!(
            file_uri_path("file:///tmp/100%2541.rs").unwrap(),
            Path::new("/tmp/100%41.rs")
        );
        assert_eq!(uri_or_path("/tmp/100%41.rs"), Path::new("/tmp/100%41.rs"));
        assert_eq!(
            uri_or_path("file:///tmp/a%20b%23c.rs"),
            Path::new("/tmp/a b#c.rs")
        );
        assert_eq!(uri_or_path("untitled:x"), Path::new("untitled:x"));
        assert!(file_uri_path("https://example.com/a.rs").is_none());
        assert_eq!(file_uri(Path::new("rel/a.rs")), "file://rel/a.rs");
    }

    fn app() -> PathTranslator {
        PathTranslator::new("/Users/dev/app", "/srv/ws/app")
    }

    fn json(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn source_text_of_did_open_and_did_change_is_not_rewritten() {
        let source =
            "const ROOT: &str = \"/Users/dev/app\";\n// see file:///Users/dev/app/src/lib.rs\n";
        let open = serde_json::json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
            "textDocument": { "uri": "file:///Users/dev/app/src/lib.rs", "languageId": "rust", "version": 1, "text": source }
        } });
        let sent = json(&app().translate_lsp_to_server(&open.to_string()));
        assert_eq!(sent["params"]["textDocument"]["text"], source);
        assert_eq!(
            sent["params"]["textDocument"]["uri"],
            "file:///srv/ws/app/src/lib.rs"
        );

        let change = serde_json::json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
            "textDocument": { "uri": "file:///Users/dev/app/src/lib.rs", "version": 2 },
            "contentChanges": [ { "text": source } ]
        } });
        let sent = json(&app().translate_lsp_to_server(&change.to_string()));
        assert_eq!(sent["params"]["contentChanges"][0]["text"], source);
    }

    #[test]
    fn edit_text_and_documentation_from_the_server_are_not_rewritten() {
        let edit = serde_json::json!({ "jsonrpc": "2.0", "id": 3, "result": { "changes": {
            "file:///srv/ws/app/src/lib.rs": [ { "range": { "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 3 } }, "newText": "\"/srv/ws/app/data\"" } ]
        } } });
        let back = json(&app().translate_lsp_to_client(&edit.to_string()));
        let changes = back["result"]["changes"].as_object().unwrap();
        let (uri, edits) = changes.iter().next().unwrap();
        assert_eq!(uri, "file:///Users/dev/app/src/lib.rs");
        assert_eq!(edits[0]["newText"], "\"/srv/ws/app/data\"");

        let hover = serde_json::json!({ "jsonrpc": "2.0", "id": 4, "result": {
            "contents": { "kind": "markdown", "value": "Reads /srv/ws/app/config.toml" }
        } });
        let back = json(&app().translate_lsp_to_client(&hover.to_string()));
        assert_eq!(
            back["result"]["contents"]["value"],
            "Reads /srv/ws/app/config.toml"
        );
    }

    #[test]
    fn a_sibling_root_sharing_the_prefix_is_not_mapped() {
        let t = app();
        assert_eq!(
            t.to_server_path("/Users/dev/app2/src/main.rs"),
            "/Users/dev/app2/src/main.rs"
        );
        assert_eq!(
            t.to_server_uri("file:///Users/dev/app-old/src/main.rs"),
            "file:///Users/dev/app-old/src/main.rs"
        );
        assert_eq!(t.to_server_path("/Users/dev/app"), "/srv/ws/app");
        let refs = serde_json::json!({ "jsonrpc": "2.0", "id": 5, "method": "textDocument/references", "params": {
            "textDocument": { "uri": "file:///Users/dev/app2/src/lib.rs" },
            "position": { "line": 0, "character": 0 }
        } });
        let sent = json(&t.translate_lsp_to_server(&refs.to_string()));
        assert_eq!(
            sent["params"]["textDocument"]["uri"],
            "file:///Users/dev/app2/src/lib.rs"
        );
    }
    #[test]
    fn inlay_label_locations_are_mapped_while_label_text_stays_verbatim() {
        let input = serde_json::json!({"result": [{"label": [{
            "value": "/srv/ws/app/type",
            "tooltip": {"kind": "markdown", "value": "file:///srv/ws/app/type"},
            "location": {"uri": "file:///srv/ws/app/src/lib.rs", "range": {}},
            "command": {"title": "/srv/ws/app/title", "command": "open", "arguments": ["file:///srv/ws/app/src/lib.rs"]}
        }]}]});
        let output = json(&app().translate_lsp_to_client(&input.to_string()));
        let label = &output["result"][0]["label"][0];
        assert_eq!(label["value"], "/srv/ws/app/type");
        assert_eq!(label["tooltip"]["value"], "file:///srv/ws/app/type");
        assert_eq!(label["command"]["title"], "/srv/ws/app/title");
        assert_eq!(label["location"]["uri"], "file:///Users/dev/app/src/lib.rs");
        assert_eq!(
            label["command"]["arguments"][0],
            "file:///Users/dev/app/src/lib.rs"
        );
    }
}
