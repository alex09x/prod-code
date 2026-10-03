//! Binary framing codec for prod-code WireMessage streams.

use crate::messages::WireMessage;
use bytes::{Buf, BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

/// Maximum allowed wire frame size. A sync message carries at most 24 MiB of content, but a
/// vendored library a build links (up to 128 MiB, base64 on the wire) travels in a message of
/// its own (#313).
pub const MAX_FRAME_SIZE: usize = 256 * 1024 * 1024;

/// Length-delimited codec for `WireMessage`.
///
/// Format on wire:
/// `[4-byte big-endian length N] [N bytes UTF-8 JSON encoded WireMessage]`
#[derive(Debug, Default, Clone)]
pub struct ProdCodeCodec;

struct BoundedWriter<W> {
    inner: W,
    written: usize,
    limit: usize,
}

impl<W: io::Write> BoundedWriter<W> {
    fn new(inner: W, limit: usize) -> Self {
        Self {
            inner,
            written: 0,
            limit,
        }
    }
}

impl<W: io::Write> io::Write for BoundedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written.saturating_add(buf.len()) > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Message size exceeds maximum allowed frame size {}",
                    self.limit
                ),
            ));
        }
        let n = self.inner.write(buf)?;
        self.written += n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl ProdCodeCodec {
    pub fn new() -> Self {
        Self
    }

    /// Encode a referenced `WireMessage` directly into `dst` without allocating an intermediate heap buffer.
    pub fn encode_ref(&self, item: &WireMessage, dst: &mut BytesMut) -> Result<(), io::Error> {
        self.encode_ref_bounded(item, dst, MAX_FRAME_SIZE)
    }

    pub(crate) fn encode_ref_bounded(
        &self,
        item: &WireMessage,
        dst: &mut BytesMut,
        max_frame_size: usize,
    ) -> Result<(), io::Error> {
        let initial_capacity = dst.capacity();
        let header_offset = dst.len();
        dst.reserve(4);
        dst.put_u32(0); // Placeholder for 4-byte BE length

        // Stream JSON directly into dst's spare capacity using serde_json::to_writer
        // bounded by BoundedWriter to abort serialization as soon as the payload exceeds max_frame_size.
        let writer = BoundedWriter::new((&mut *dst).writer(), max_frame_size);
        if let Err(e) = serde_json::to_writer(writer, item) {
            dst.truncate(header_offset);
            if dst.capacity() > initial_capacity {
                let mut restored = BytesMut::with_capacity(initial_capacity);
                restored.put_slice(&dst[..header_offset]);
                *dst = restored;
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to serialize WireMessage: {e}"),
            ));
        }

        let frame_len = dst.len() - header_offset - 4;
        if frame_len > max_frame_size {
            dst.truncate(header_offset);
            if dst.capacity() > initial_capacity {
                let mut restored = BytesMut::with_capacity(initial_capacity);
                restored.put_slice(&dst[..header_offset]);
                *dst = restored;
            }
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Message size {frame_len} exceeds maximum {max_frame_size}"),
            ));
        }

        dst[header_offset..header_offset + 4].copy_from_slice(&(frame_len as u32).to_be_bytes());
        Ok(())
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
            // Need more data. Do not reserve the declared frame size here: a peer can advertise
            // a valid maximum-sized frame and then never send its body. The surrounding framed
            // reader will grow its buffer incrementally as bytes actually arrive.
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
        self.encode_ref(&item, dst)
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
            supported_versions: Some(vec![1]),
            capabilities: None,
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
            redirect_count: 0,
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
    fn test_codec_encode_ref_roundtrip() {
        let codec = ProdCodeCodec::new();
        let mut buf = BytesMut::with_capacity(1024);
        let initial_cap = buf.capacity();

        let original = WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: 1,
            supported_versions: Some(vec![1]),
            capabilities: None,
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
            redirect_count: 0,
        });

        // Encode via reference
        codec.encode_ref(&original, &mut buf).unwrap();
        assert!(buf.len() > 4);
        // Did not reallocate or exceed preallocated capacity
        assert_eq!(buf.capacity(), initial_cap);

        let mut dec_codec = ProdCodeCodec::new();
        let decoded = dec_codec
            .decode(&mut buf)
            .unwrap()
            .expect("should decode message");
        assert_eq!(decoded, original);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_incomplete_maximum_sized_frame_does_not_reserve_payload() {
        let mut codec = ProdCodeCodec::new();
        let mut buf = BytesMut::with_capacity(8);
        buf.put_u32(MAX_FRAME_SIZE as u32);
        let capacity_before_decode = buf.capacity();

        assert_eq!(codec.decode(&mut buf).unwrap(), None);
        assert_eq!(buf.capacity(), capacity_before_decode);
        assert_eq!(buf.len(), 4);
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
            in_memory: false,
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

    #[test]
    fn test_codec_preserves_preexisting_bytes_and_appends() {
        let codec = ProdCodeCodec::new();
        let mut buf = BytesMut::new();
        buf.put_slice(b"preexisting-bytes");
        let initial_len = buf.len();

        let original = WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: 1,
            supported_versions: Some(vec![1]),
            capabilities: None,
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
            redirect_count: 0,
        });

        codec.encode_ref(&original, &mut buf).unwrap();
        assert_eq!(&buf[..initial_len], b"preexisting-bytes");
        let mut frame_buf = buf.split_off(initial_len);
        let mut dec_codec = ProdCodeCodec::new();
        let decoded = dec_codec.decode(&mut frame_buf).unwrap().expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_codec_oversized_frame_aborts_early_and_does_not_retain_capacity() {
        let codec = ProdCodeCodec::new();
        let mut buf = BytesMut::with_capacity(64);
        buf.put_slice(b"existing-prefix");
        let initial_len = buf.len();
        let initial_cap = buf.capacity();

        let original = WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: 1,
            supported_versions: Some(vec![1]),
            capabilities: None,
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
            redirect_count: 0,
        });

        // Limit to 20 bytes: the message is ~200 bytes, so BoundedWriter aborts serialization early
        let err = codec
            .encode_ref_bounded(&original, &mut buf, 20)
            .expect_err("must fail early when frame exceeds 20 bytes");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("Message size exceeds maximum allowed frame size"));

        // Preexisting bytes are untouched and logical length is restored
        assert_eq!(buf.len(), initial_len);
        assert_eq!(&buf[..initial_len], b"existing-prefix");
        // Capacity did not stay grown or leak memory
        assert_eq!(buf.capacity(), initial_cap);
    }
}
