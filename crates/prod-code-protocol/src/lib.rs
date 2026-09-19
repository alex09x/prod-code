//! Wire framing and transport types for prod-code Remote Code Intelligence over 10 GbE LAN.

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 9400;
pub const PROTOCOL_VERSION: u32 = 1;

/// Initial handshake sent by the client upon connecting to the remote gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeRequest {
    pub protocol_version: u32,
    pub client_name: String,
    pub client_pid: u32,
    pub auth_token: Option<String>,
    pub client_workspace_root: String,
}

/// Response returned by the remote gateway to acknowledge the handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    pub protocol_version: u32,
    pub server_pid: u32,
    pub session_id: u64,
    pub server_workspace_root: String,
    pub detected_engine: String,
}

/// Path translator mapping paths between client workstation and remote server storage.
#[derive(Debug, Clone)]
pub struct PathTranslator {
    pub client_prefix: String,
    pub server_prefix: String,
}

impl PathTranslator {
    pub fn new(client_prefix: impl Into<String>, server_prefix: impl Into<String>) -> Self {
        Self {
            client_prefix: client_prefix.into(),
            server_prefix: server_prefix.into(),
        }
    }

    pub fn to_server(&self, client_path: &str) -> String {
        if let Some(rel) = client_path.strip_prefix(&self.client_prefix) {
            format!("{}{}", self.server_prefix.trim_end_matches('/'), rel)
        } else {
            client_path.to_string()
        }
    }

    pub fn to_client(&self, server_path: &str) -> String {
        if let Some(rel) = server_path.strip_prefix(&self.server_prefix) {
            format!("{}{}", self.client_prefix.trim_end_matches('/'), rel)
        } else {
            server_path.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_translation() {
        let translator = PathTranslator::new(
            "/Users/alex09x/Documents/workspace/my-project",
            "/srv/prod-code/workspaces/my-project",
        );

        let server_path = translator.to_server("/Users/alex09x/Documents/workspace/my-project/src/main.rs");
        assert_eq!(server_path, "/srv/prod-code/workspaces/my-project/src/main.rs");

        let client_path = translator.to_client("/srv/prod-code/workspaces/my-project/src/main.rs");
        assert_eq!(client_path, "/Users/alex09x/Documents/workspace/my-project/src/main.rs");
    }
}
