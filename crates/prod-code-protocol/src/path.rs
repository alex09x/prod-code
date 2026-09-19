//! Path and URI translation between local client environments and remote server storage.

use url::Url;

/// Bi-directional path and URI translator for prod-code sessions.
#[derive(Debug, Clone)]
pub struct PathTranslator {
    client_path_prefix: String,
    server_path_prefix: String,
    client_uri_prefix: String,
    server_uri_prefix: String,
}

impl PathTranslator {
    /// Create a new translator given the client workspace root and server workspace root.
    pub fn new(client_root: &str, server_root: &str) -> Self {
        let client_path = client_root.trim_end_matches('/').to_string();
        let server_path = server_root.trim_end_matches('/').to_string();

        let client_uri = Url::from_file_path(&client_path)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("file://{client_path}"));
        let server_uri = Url::from_file_path(&server_path)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| format!("file://{server_path}"));

        Self {
            client_path_prefix: client_path,
            server_path_prefix: server_path,
            client_uri_prefix: client_uri.trim_end_matches('/').to_string(),
            server_uri_prefix: server_uri.trim_end_matches('/').to_string(),
        }
    }

    /// Translate a local client filesystem path to the remote server filesystem path.
    pub fn to_server_path(&self, client_path: &str) -> String {
        if let Some(rel) = client_path.strip_prefix(&self.client_path_prefix) {
            format!("{}{}", self.server_path_prefix, rel)
        } else {
            client_path.to_string()
        }
    }

    /// Translate a remote server filesystem path back to the local client filesystem path.
    pub fn to_client_path(&self, server_path: &str) -> String {
        if let Some(rel) = server_path.strip_prefix(&self.server_path_prefix) {
            format!("{}{}", self.client_path_prefix, rel)
        } else {
            server_path.to_string()
        }
    }

    /// Translate a local client URI (`file:///Users/...`) to a remote server URI (`file:///srv/...`).
    pub fn to_server_uri(&self, client_uri: &str) -> String {
        if let Some(rel) = client_uri.strip_prefix(&self.client_uri_prefix) {
            format!("{}{}", self.server_uri_prefix, rel)
        } else {
            client_uri.to_string()
        }
    }

    /// Translate a remote server URI (`file:///srv/...`) back to a local client URI (`file:///Users/...`).
    pub fn to_client_uri(&self, server_uri: &str) -> String {
        if let Some(rel) = server_uri.strip_prefix(&self.server_uri_prefix) {
            format!("{}{}", self.client_uri_prefix, rel)
        } else {
            server_uri.to_string()
        }
    }

    /// Rewrite all occurrences of client paths and URIs in an LSP JSON payload to server equivalents.
    pub fn translate_lsp_to_server(&self, json_payload: &str) -> String {
        let with_uris = json_payload.replace(&self.client_uri_prefix, &self.server_uri_prefix);
        with_uris.replace(&self.client_path_prefix, &self.server_path_prefix)
    }

    /// Rewrite all occurrences of server paths and URIs in an LSP JSON payload to client equivalents.
    pub fn translate_lsp_to_client(&self, json_payload: &str) -> String {
        let with_uris = json_payload.replace(&self.server_uri_prefix, &self.client_uri_prefix);
        with_uris.replace(&self.server_path_prefix, &self.client_path_prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_and_uri_translation() {
        let translator = PathTranslator::new(
            "/Users/alex09x/Documents/workspace/my-app",
            "/srv/prod-code/workspaces/my-app",
        );

        let server_path = translator.to_server_path("/Users/alex09x/Documents/workspace/my-app/src/main.rs");
        assert_eq!(server_path, "/srv/prod-code/workspaces/my-app/src/main.rs");

        let client_path = translator.to_client_path("/srv/prod-code/workspaces/my-app/src/main.rs");
        assert_eq!(client_path, "/Users/alex09x/Documents/workspace/my-app/src/main.rs");

        let client_uri = "file:///Users/alex09x/Documents/workspace/my-app/src/lib.rs";
        let server_uri = translator.to_server_uri(client_uri);
        assert_eq!(server_uri, "file:///srv/prod-code/workspaces/my-app/src/lib.rs");

        let roundtrip_uri = translator.to_client_uri(&server_uri);
        assert_eq!(roundtrip_uri, client_uri);
    }

    #[test]
    fn test_lsp_json_translation() {
        let translator = PathTranslator::new(
            "/Users/alex09x/Documents/workspace/my-app",
            "/srv/prod-code/workspaces/my-app",
        );

        let client_lsp = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///Users/alex09x/Documents/workspace/my-app/src/main.rs","text":"fn main() {}"}}}"#;
        let server_lsp = translator.translate_lsp_to_server(client_lsp);
        assert!(server_lsp.contains("file:///srv/prod-code/workspaces/my-app/src/main.rs"));
        assert!(!server_lsp.contains("/Users/alex09x"));

        let restored_lsp = translator.translate_lsp_to_client(&server_lsp);
        assert_eq!(restored_lsp, client_lsp);
    }
}
