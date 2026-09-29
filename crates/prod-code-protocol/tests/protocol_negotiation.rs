use prod_code_protocol::{
    ClientCapabilities, HandshakeRequest, HandshakeResponse, PROTOCOL_VERSION,
    ProtocolNegotiationError, default_server_capabilities, negotiate_capabilities,
    negotiate_protocol_version, supported_protocol_versions, validate_selected_protocol_version,
};
use serde::Deserialize;

fn request(protocol_version: u32, supported_versions: Option<Vec<u32>>) -> HandshakeRequest {
    HandshakeRequest {
        protocol_version,
        supported_versions,
        capabilities: None,
        client_name: "compatibility-test".to_string(),
        client_pid: 1,
        auth_token: None,
        client_workspace_root: "/workspace".to_string(),
        preferred_engine: None,
        base_workspace_name: None,
        engine_subpath: None,
        client_agent: None,
        client_host: None,
        purpose: None,
        redirect_count: 0,
    }
}

#[test]
fn current_peers_select_the_current_version() {
    let offer = supported_protocol_versions();
    let request = request(PROTOCOL_VERSION, Some(offer.clone()));
    let selected = negotiate_protocol_version(&request).unwrap();
    assert_eq!(selected, PROTOCOL_VERSION);
    validate_selected_protocol_version(selected, &offer).unwrap();
}

#[test]
fn the_highest_common_version_wins_and_duplicates_are_harmless() {
    let request = request(PROTOCOL_VERSION, Some(vec![2, 1, 1]));
    assert_eq!(negotiate_protocol_version(&request), Ok(1));
}

#[test]
fn empty_disjoint_and_unsupported_legacy_offers_are_refused() {
    assert_eq!(
        negotiate_protocol_version(&request(PROTOCOL_VERSION, Some(Vec::new()))),
        Err(ProtocolNegotiationError::EmptyOffer)
    );
    assert!(matches!(
        negotiate_protocol_version(&request(PROTOCOL_VERSION, Some(vec![2, 3]))),
        Err(ProtocolNegotiationError::NoCommonVersion { .. })
    ));
    assert!(matches!(
        negotiate_protocol_version(&request(999, None)),
        Err(ProtocolNegotiationError::NoCommonVersion { .. })
    ));
}

#[test]
fn a_client_refuses_unoffered_and_unimplemented_server_selections_contextually() {
    let unoffered = validate_selected_protocol_version(1, &[2]).unwrap_err();
    assert!(unoffered.to_string().contains("did not offer [2]"));

    let unsupported = validate_selected_protocol_version(2, &[2]).unwrap_err();
    assert!(unsupported.to_string().contains("does not implement"));
}

#[derive(Deserialize)]
struct LegacyHandshakeRequest {
    protocol_version: u32,
}

#[test]
fn new_requests_are_readable_by_legacy_peers_and_legacy_requests_by_new_peers() {
    let new_value = serde_json::to_value(request(1, Some(vec![2, 1]))).unwrap();
    let legacy: LegacyHandshakeRequest = serde_json::from_value(new_value).unwrap();
    assert_eq!(legacy.protocol_version, 1);

    let legacy_value = serde_json::json!({
        "protocol_version": 1,
        "client_name": "legacy",
        "client_pid": 1,
        "auth_token": null,
        "client_workspace_root": "/workspace"
    });
    let decoded: HandshakeRequest = serde_json::from_value(legacy_value).unwrap();
    assert_eq!(decoded.supported_versions, None);
    assert_eq!(negotiate_protocol_version(&decoded), Ok(1));
}

#[test]
fn the_unchanged_response_shape_round_trips_for_both_peer_generations() {
    let response = HandshakeResponse {
        protocol_version: 1,
        server_pid: 2,
        session_id: 3,
        server_workspace_root: "/workspace".to_string(),
        detected_engine: "rust".to_string(),
        stale_paths: Vec::new(),
        engine_age_ms: None,
        index_gated: false,
        capabilities: None,
    };
    let value = serde_json::to_value(&response).unwrap();
    let decoded: HandshakeResponse = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, response);
}

#[test]
fn capabilities_negotiation_computes_common_subset_or_defaults() {
    let server_caps = default_server_capabilities();

    // Legacy client offering None gets server supported defaults
    let negotiated_default = negotiate_capabilities(None, &server_caps);
    assert_eq!(negotiated_default, server_caps);

    // Client offering partial capabilities
    let client_caps = ClientCapabilities {
        direct_edit: true,
        watch_files: false,
        indexing_status: true,
        shadow_runs: false,
        multi_root: true,
        sync_chunking: false,
        unix_socket_local: true,
    };
    let negotiated = negotiate_capabilities(Some(&client_caps), &server_caps);
    assert!(negotiated.direct_edit);
    assert!(!negotiated.watch_files);
    assert!(negotiated.indexing_status);
    assert!(!negotiated.shadow_runs);
    assert!(negotiated.multi_root);
    assert!(!negotiated.sync_chunking);
    assert_eq!(negotiated.unix_socket_local, cfg!(unix));
}
