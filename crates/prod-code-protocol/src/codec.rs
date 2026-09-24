//! Binary framing codec for prod-code WireMessage streams.

use crate::messages::WireMessage;
use bytes::{Buf, BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

/// Maximum allowed wire frame size (64 MiB) to accommodate large ASTs or symbol queries.
pub const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

/// Length-delimited codec for `WireMessage`.
///
/// Format on wire:
/// `[4-byte big-endian length N] [N bytes UTF-8 JSON encoded WireMessage]`
#[derive(Debug, Default, Clone)]
pub struct ProdCodeCodec;

impl ProdCodeCodec {
    pub fn new() -> Self {
        Self
    }
}

impl Decoder for ProdCodeCodec {
    type Item = WireMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }

        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(&src[..4]);
        let frame_len = u32::from_be_bytes(length_bytes) as usize;

        if frame_len > MAX_FRAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Frame size {frame_len} exceeds maximum {MAX_FRAME_SIZE}"),
            ));
        }

        if src.len() < 4 + frame_len {
            // Need more data
            src.reserve(4 + frame_len - src.len());
            return Ok(None);
        }

        // Consume the length header
        src.advance(4);
        let frame_data = src.split_to(frame_len);

        let message = serde_json::from_slice::<WireMessage>(&frame_data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse WireMessage: {e}"),
            )
        })?;

        Ok(Some(message))
    }
}

impl Encoder<WireMessage> for ProdCodeCodec {
    type Error = io::Error;

    fn encode(&mut self, item: WireMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let json_bytes = serde_json::to_vec(&item).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to serialize WireMessage: {e}"),
            )
        })?;

        let frame_len = json_bytes.len();
        if frame_len > MAX_FRAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Message size {frame_len} exceeds maximum {MAX_FRAME_SIZE}"),
            ));
        }

        dst.reserve(4 + frame_len);
        dst.put_u32(frame_len as u32);
        dst.put_slice(&json_bytes);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::HandshakeRequest;

    #[test]
    fn test_codec_roundtrip() {
        let mut codec = ProdCodeCodec::new();
        let mut buf = BytesMut::new();

        let original = WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: 1,
            client_name: "test-client".to_string(),
            client_pid: 1234,
            auth_token: None,
            client_workspace_root: "/home/user/project".to_string(),
            preferred_engine: None,
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: None,
        });

        codec.encode(original.clone(), &mut buf).unwrap();
        assert!(buf.len() > 4);

        let decoded = codec
            .decode(&mut buf)
            .unwrap()
            .expect("should decode message");
        assert_eq!(decoded, original);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_shadow_codec_roundtrip() {
        use crate::messages::{
            FileDelta, ShadowHypothesis, ShadowHypothesisResult, ShadowRunRequest,
            ShadowRunResponse,
        };

        let mut codec = ProdCodeCodec::new();
        let mut buf = BytesMut::new();
        let req = WireMessage::ShadowRunRequest(ShadowRunRequest {
            client_workspace_root: "/Users/dev/repo".to_string(),
            base_workspace_name: Some("repo".to_string()),
            hypotheses: vec![ShadowHypothesis {
                name: "h1".to_string(),
                files: vec![
                    FileDelta {
                        relative_path: "src/lib.rs".to_string(),
                        content: Some(b"pub fn x() {}".to_vec()),
                        is_executable: false,
                    },
                    FileDelta {
                        relative_path: "old.rs".to_string(),
                        content: None,
                        is_executable: false,
                    },
                ],
            }],
            command: vec!["cargo".to_string(), "test".to_string()],
            env: Vec::new(),
            timeout_secs: 0,
            subdir: None,
            parallel: 0,
            tail_bytes: 0,
            client_agent: None,
            client_host: None,
        });
        codec.encode(req.clone(), &mut buf).unwrap();
        assert_eq!(codec.decode(&mut buf).unwrap().expect("decodes"), req);

        let resp = WireMessage::ShadowRunResponse(ShadowRunResponse {
            server_workspace_root: "/srv/ws/repo".to_string(),
            mode: "overlay".to_string(),
            results: vec![ShadowHypothesisResult {
                name: "h1".to_string(),
                exit_code: Some(0),
                duration_ms: 950,
                timed_out: false,
                error: None,
                output_tail: Some(b"test result: ok".to_vec()),
                output_len: 15,
            }],
            error: None,
        });
        codec.encode(resp.clone(), &mut buf).unwrap();
        assert_eq!(codec.decode(&mut buf).unwrap().expect("decodes"), resp);
    }

    #[test]
    fn test_sync_codec_roundtrip() {
        use crate::messages::{FileDelta, SyncRequest, SyncResponse};

        let mut codec = ProdCodeCodec::new();
        let mut buf = BytesMut::new();

        let req = WireMessage::SyncRequest(SyncRequest {
            client_workspace_root: "/Users/dev/repo".to_string(),
            files: vec![
                FileDelta {
                    relative_path: "src/main.rs".to_string(),
                    content: Some(b"fn main() {}".to_vec()),
                    is_executable: false,
                },
                FileDelta {
                    relative_path: "old_file.rs".to_string(),
                    content: None,
                    is_executable: false,
                },
            ],
            clean_others: false,
            base_workspace_name: None,
        });

        codec.encode(req.clone(), &mut buf).unwrap();
        let decoded = codec
            .decode(&mut buf)
            .unwrap()
            .expect("should decode SyncRequest");
        assert_eq!(decoded, req);

        let resp = WireMessage::SyncResponse(SyncResponse {
            files_updated: 1,
            files_deleted: 1,
            bytes_transferred: 12,
            duration_ms: 15,
            server_workspace_root: "/srv/prod-code/workspaces/repo".to_string(),
            workspace_was_fresh: false,
            stale_paths: Vec::new(),
        });

        codec.encode(resp.clone(), &mut buf).unwrap();
        let decoded_resp = codec
            .decode(&mut buf)
            .unwrap()
            .expect("should decode SyncResponse");
        assert_eq!(decoded_resp, resp);
    }
}
