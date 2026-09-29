use crate::{ClientCapabilities, HandshakeRequest, PROTOCOL_VERSION, ServerCapabilities};
use std::fmt;

/// Wire protocol versions implemented by this build, in no particular order.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u32] = &[PROTOCOL_VERSION];

/// The version offer sent by a current client.
pub fn supported_protocol_versions() -> Vec<u32> {
    SUPPORTED_PROTOCOL_VERSIONS.to_vec()
}

/// Returns the server capabilities supported by this build.
pub fn default_server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        direct_edit: true,
        watch_files: true,
        indexing_status: true,
        shadow_runs: true,
        multi_root: true,
        sync_chunking: true,
        unix_socket_local: cfg!(unix),
    }
}

/// Negotiates session capabilities between client offer and server supported capabilities.
pub fn negotiate_capabilities(
    client_offer: Option<&ClientCapabilities>,
    server_supported: &ServerCapabilities,
) -> ServerCapabilities {
    match client_offer {
        Some(client) => ServerCapabilities {
            direct_edit: client.direct_edit && server_supported.direct_edit,
            watch_files: client.watch_files && server_supported.watch_files,
            indexing_status: client.indexing_status && server_supported.indexing_status,
            shadow_runs: client.shadow_runs && server_supported.shadow_runs,
            multi_root: client.multi_root && server_supported.multi_root,
            sync_chunking: client.sync_chunking && server_supported.sync_chunking,
            unix_socket_local: client.unix_socket_local && server_supported.unix_socket_local,
        },
        None => server_supported.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolNegotiationError {
    EmptyOffer,
    NoCommonVersion {
        offered: Vec<u32>,
        supported: Vec<u32>,
    },
    SelectedVersionNotOffered {
        selected: u32,
        offered: Vec<u32>,
    },
    SelectedVersionNotSupported {
        selected: u32,
        supported: Vec<u32>,
    },
}

impl fmt::Display for ProtocolNegotiationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyOffer => write!(f, "protocol version offer is empty"),
            Self::NoCommonVersion { offered, supported } => write!(
                f,
                "no compatible protocol version (client offered {offered:?}, server supports {supported:?})"
            ),
            Self::SelectedVersionNotOffered { selected, offered } => write!(
                f,
                "gateway selected protocol version {selected}, which the client did not offer {offered:?}"
            ),
            Self::SelectedVersionNotSupported {
                selected,
                supported,
            } => write!(
                f,
                "gateway selected protocol version {selected}, which this client does not implement (supported {supported:?})"
            ),
        }
    }
}

impl std::error::Error for ProtocolNegotiationError {}

/// Chooses the highest protocol version implemented by both peers.
///
/// A request without `supported_versions` is a legacy singleton offer of its existing
/// `protocol_version` field. An explicitly empty offer is invalid.
pub fn negotiate_protocol_version(
    request: &HandshakeRequest,
) -> Result<u32, ProtocolNegotiationError> {
    let offered = match request.supported_versions.as_deref() {
        Some([]) => return Err(ProtocolNegotiationError::EmptyOffer),
        Some(offered) => offered,
        None => std::slice::from_ref(&request.protocol_version),
    };

    offered
        .iter()
        .copied()
        .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
        .max()
        .ok_or_else(|| ProtocolNegotiationError::NoCommonVersion {
            offered: offered.to_vec(),
            supported: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        })
}

/// Checks a gateway's selected version before a client sends any LSP initialization.
pub fn validate_selected_protocol_version(
    selected: u32,
    offered: &[u32],
) -> Result<(), ProtocolNegotiationError> {
    if offered.is_empty() {
        return Err(ProtocolNegotiationError::EmptyOffer);
    }
    if !offered.contains(&selected) {
        return Err(ProtocolNegotiationError::SelectedVersionNotOffered {
            selected,
            offered: offered.to_vec(),
        });
    }
    if !SUPPORTED_PROTOCOL_VERSIONS.contains(&selected) {
        return Err(ProtocolNegotiationError::SelectedVersionNotSupported {
            selected,
            supported: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
        });
    }
    Ok(())
}
