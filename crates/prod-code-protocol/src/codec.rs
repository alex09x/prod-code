//! Binary framing codec for prod-code WireMessage streams.

use crate::messages::WireMessage;
use bytes::{Buf, BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

/// Maximum allowed wire frame size. A sync message carries at most 24 MiB of content, but a
/// vendored library a build links (up to 128 MiB, base64 on the wire) travels in a message of
/// its own (#313).
pub const MAX_FRAME_SIZE: usize = 256 * 1024 * 1024;

/// Length-delimited codec for `WireMessage`, optionally with trailing NUL completion marker.
///
/// Format on wire:
/// `[4-byte big-endian length N] [N bytes UTF-8 JSON encoded WireMessage] [optional 0x00 NUL marker]`
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProdCodeCodec {
    /// When true, outgoing frames will append a trailing NUL byte (0x00) after the frame body,
    /// and incoming frames will require the NUL completion marker.
    nul_marker: bool,
    /// When decoding without `nul_marker` explicitly configured, tracks whether the peer
    /// stream has been confirmed to emit trailing NUL completion markers.
    detected_nul_marker: bool,
    /// When a frame was decoded without consuming a trailing NUL marker (e.g. before NUL
    /// detection was confirmed), records that a trailing NUL marker may follow in the stream.
    pending_trailing_nul: bool,
}

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
        Self {
            nul_marker: false,
            detected_nul_marker: false,
            pending_trailing_nul: false,
        }
    }

    /// Creates a codec configured with or without trailing NUL completion markers.
    pub fn with_nul_marker(mut self, enabled: bool) -> Self {
        self.nul_marker = enabled;
        self.detected_nul_marker = enabled;
        self.pending_trailing_nul = false;
        self
    }

    /// Whether this codec is configured to emit and enforce trailing NUL completion markers.
    pub fn has_nul_marker(&self) -> bool {
        self.nul_marker || self.detected_nul_marker
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
        if self.nul_marker {
            dst.reserve(1);
            dst.put_u8(0);
        }
        Ok(())
    }
}

impl Decoder for ProdCodeCodec {
    type Item = WireMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if self.pending_trailing_nul {
            if src.is_empty() {
                return Ok(None);
            }
            if src[0] != 0 {
                // The byte immediately following the previous frame is non-zero, so the stream
                // is confirmed to be markerless.
                self.pending_trailing_nul = false;
            } else {
                // src[0] == 0: could be the trailing NUL marker from the previous frame,
                // or the first byte of a markerless frame with length < 16 MiB.
                // We need at least 5 bytes to inspect the next frame's 4-byte header under
                // the trailing-NUL hypothesis (1 byte NUL + 4 bytes header).
                if src.len() < 5 {
                    return Ok(None);
                }

                let len_a = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;
                let len_b = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;

                if len_b < 2 {
                    // Hypothesis B is impossible because no valid WireMessage payload has length < 2.
                    // Therefore, src[0] is definitely a trailing NUL marker.
                    src.advance(1);
                    self.detected_nul_marker = true;
                    self.pending_trailing_nul = false;
                } else if len_a > MAX_FRAME_SIZE {
                    self.pending_trailing_nul = false;
                } else {
                    let hyp_a_valid = if src.len() >= 5 + len_a {
                        Some(serde_json::from_slice::<WireMessage>(&src[5..5 + len_a]).is_ok())
                    } else {
                        None
                    };

                    let hyp_b_valid = if src.len() >= 4 + len_b {
                        Some(serde_json::from_slice::<WireMessage>(&src[4..4 + len_b]).is_ok())
                    } else {
                        None
                    };

                    match (hyp_a_valid, hyp_b_valid) {
                        (Some(true), _) => {
                            src.advance(1);
                            self.detected_nul_marker = true;
                            self.pending_trailing_nul = false;
                        }
                        (_, Some(true)) => {
                            self.pending_trailing_nul = false;
                        }
                        (Some(false), Some(false)) => {
                            self.pending_trailing_nul = false;
                        }
                        (Some(false), None) => {
                            self.pending_trailing_nul = false;
                        }
                        (None, Some(false)) => {
                            src.advance(1);
                            self.detected_nul_marker = true;
                            self.pending_trailing_nul = false;
                        }
                        (None, None) => {
                            // Partial ambiguous bytes; wait for more data to confirm boundary
                            return Ok(None);
                        }
                    }
                }
            }
        }

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

        let uses_nul = self.nul_marker || self.detected_nul_marker;

        if uses_nul {
            if src.len() < 4 + frame_len + 1 {
                return Ok(None);
            }

            src.advance(4);
            let mut frame_data = src.split_to(frame_len);

            if src.first() == Some(&0) {
                src.advance(1);
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Missing expected NUL completion marker after frame",
                ));
            }

            if frame_data.ends_with(b"\0") {
                frame_data.truncate(frame_data.len() - 1);
            }

            let message = serde_json::from_slice::<WireMessage>(&frame_data).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Failed to parse WireMessage: {e}"),
                )
            })?;

            return Ok(Some(message));
        }

        if src.len() < 4 + frame_len {
            return Ok(None);
        }

        src.advance(4);
        let mut frame_data = src.split_to(frame_len);

        if src.is_empty() {
            self.pending_trailing_nul = true;
        } else if src[0] != 0 {
            self.pending_trailing_nul = false;
        } else if src.len() >= 5 {
            let hyp_a_valid = {
                let len_a = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;
                if !(2..=MAX_FRAME_SIZE).contains(&len_a) {
                    Some(false)
                } else if src.len() >= 4 + len_a {
                    Some(serde_json::from_slice::<WireMessage>(&src[4..4 + len_a]).is_ok())
                } else {
                    None
                }
            };

            let hyp_b_valid = {
                let len_b = u32::from_be_bytes([src[1], src[2], src[3], src[4]]) as usize;
                if !(2..=MAX_FRAME_SIZE).contains(&len_b) {
                    Some(false)
                } else if src.len() >= 5 + len_b {
                    Some(serde_json::from_slice::<WireMessage>(&src[5..5 + len_b]).is_ok())
                } else {
                    None
                }
            };

            match (hyp_a_valid, hyp_b_valid) {
                (_, Some(true)) | (Some(false), None) => {
                    src.advance(1);
                    self.detected_nul_marker = true;
                    self.pending_trailing_nul = false;
                }
                (Some(true), _) | (None, Some(false)) => {
                    self.pending_trailing_nul = false;
                    self.detected_nul_marker = false;
                }
                _ => {
                    self.pending_trailing_nul = true;
                }
            }
        } else {
            // Ambiguous 1-4 zero-prefixed bytes; leave in buffer and evaluate when next frame arrives or at EOF
            self.pending_trailing_nul = true;
        }

        if frame_data.ends_with(b"\0") {
            frame_data.truncate(frame_data.len() - 1);
        }

        let message = serde_json::from_slice::<WireMessage>(&frame_data).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse WireMessage: {e}"),
            )
        })?;

        Ok(Some(message))
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(src)? {
            Some(frame) => Ok(Some(frame)),
            None => {
                if self.pending_trailing_nul && src.len() == 1 && src[0] == 0 {
                    src.advance(1);
                    self.pending_trailing_nul = false;
                } else if !self.nul_marker && !self.detected_nul_marker && src.len() >= 4 {
                    let mut length_bytes = [0u8; 4];
                    length_bytes.copy_from_slice(&src[..4]);
                    let frame_len = u32::from_be_bytes(length_bytes) as usize;
                    if frame_len <= MAX_FRAME_SIZE
                        && src.len() == 4 + frame_len + 1
                        && src[4 + frame_len] == 0
                    {
                        src.advance(4);
                        let mut frame_data = src.split_to(frame_len);
                        src.advance(1); // consume trailing NUL at EOF
                        if frame_data.ends_with(b"\0") {
                            frame_data.truncate(frame_data.len() - 1);
                        }
                        let message =
                            serde_json::from_slice::<WireMessage>(&frame_data).map_err(|e| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    format!("Failed to parse WireMessage: {e}"),
                                )
                            })?;
                        return Ok(Some(message));
                    }
                }
                if src.is_empty() {
                    Ok(None)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("Unexpected EOF with {} unparsed bytes", src.len()),
                    ))
                }
            }
        }
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
    fn test_codec_does_not_consume_next_frame_header_as_a_marker() {
        let mut codec = ProdCodeCodec::new();
        let mut buf = BytesMut::new();
        codec.encode(WireMessage::Ping, &mut buf).unwrap();
        codec.encode(WireMessage::Pong, &mut buf).unwrap();

        assert_eq!(codec.decode(&mut buf).unwrap(), Some(WireMessage::Ping));
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(WireMessage::Pong));
        assert!(buf.is_empty());
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
        // Start with 16 bytes of capacity and an 8-byte prefix.
        // Adding the 4-byte length header leaves only 4 bytes of capacity,
        // so serializing up to the 50-byte bound forces BytesMut to reallocate and grow.
        let mut buf = BytesMut::with_capacity(16);
        buf.put_slice(b"prefix8B");
        let initial_len = buf.len();
        let initial_cap = buf.capacity();
        assert_eq!(initial_len, 8);
        assert_eq!(initial_cap, 16);

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

        // Limit to 50 bytes: the message is ~200 bytes, so BoundedWriter aborts serialization
        // after growing the buffer past initial_cap (16 bytes).
        let err = codec
            .encode_ref_bounded(&original, &mut buf, 50)
            .expect_err("must fail early when frame exceeds 50 bytes");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("Message size exceeds maximum allowed frame size"));

        // Preexisting bytes are untouched and logical length is restored
        assert_eq!(buf.len(), initial_len);
        assert_eq!(&buf[..initial_len], b"prefix8B");
        // Capacity restoration branch was exercised and restored the original capacity
        assert_eq!(buf.capacity(), initial_cap);
    }
    #[test]
    fn test_codec_nul_marker_roundtrip() {
        let mut codec = ProdCodeCodec::new().with_nul_marker(true);
        assert!(codec.has_nul_marker());
        let mut buf = BytesMut::new();

        let original = WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: 1,
            supported_versions: Some(vec![1]),
            capabilities: None,
            client_name: "test-client-nul".to_string(),
            client_pid: 5678,
            auth_token: None,
            client_workspace_root: "/home/user/project-nul".to_string(),
            preferred_engine: None,
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: None,
            redirect_count: 0,
        });

        codec.encode(original.clone(), &mut buf).unwrap();
        // The last byte on the wire must be the NUL completion marker (0x00)
        assert_eq!(buf.last(), Some(&0));

        let decoded = codec
            .decode(&mut buf)
            .unwrap()
            .expect("should decode message with NUL marker");
        assert_eq!(decoded, original);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_codec_decodes_trailing_nul_without_flag() {
        let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
        let mut dec_codec = ProdCodeCodec::new(); // default: with_nul_marker(false)
        assert!(!dec_codec.has_nul_marker());

        let mut buf = BytesMut::new();
        let original = WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: 1,
            supported_versions: Some(vec![1]),
            capabilities: None,
            client_name: "compat-client".to_string(),
            client_pid: 9999,
            auth_token: None,
            client_workspace_root: "/home/user/compat".to_string(),
            preferred_engine: None,
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: None,
            redirect_count: 0,
        });

        enc_codec.encode(original.clone(), &mut buf).unwrap();
        assert_eq!(buf.last(), Some(&0));

        // Default codec must transparently decode the frame without waiting for EOF
        let decoded = dec_codec.decode(&mut buf).unwrap().expect("decodes transparently");
        assert_eq!(decoded, original);
        // Trailing NUL marker is resolved and consumed at EOF
        assert_eq!(dec_codec.decode_eof(&mut buf).unwrap(), None);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_codec_streaming_single_nul_terminated_request_not_withheld() {
        let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
        let mut dec_codec = ProdCodeCodec::new(); // default markerless

        let mut buf = BytesMut::new();
        enc_codec.encode(WireMessage::Ping, &mut buf).unwrap();
        assert_eq!(buf.last(), Some(&0));

        // Stream remains open with exactly one NUL-terminated frame buffered.
        // decode must emit the frame without requiring EOF or a subsequent frame.
        let msg = dec_codec
            .decode(&mut buf)
            .unwrap()
            .expect("must emit completed frame while stream is open");
        assert_eq!(msg, WireMessage::Ping);

        // While no new data arrives, subsequent decode calls return None
        assert_eq!(dec_codec.decode(&mut buf).unwrap(), None);

        // When stream terminates, decode_eof cleanly consumes the trailing NUL marker
        assert_eq!(dec_codec.decode_eof(&mut buf).unwrap(), None);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_codec_markerless_two_frames_split_after_first_byte_of_second_header() {
        let mut enc_codec = ProdCodeCodec::new(); // markerless
        let mut dec_codec = ProdCodeCodec::new(); // default markerless

        let mut full_wire = BytesMut::new();
        enc_codec.encode(WireMessage::Ping, &mut full_wire).unwrap();
        enc_codec.encode(WireMessage::Pong, &mut full_wire).unwrap();

        // In markerless mode, WireMessage::Ping length is serialized with a 4-byte BE header.
        let ping_len = u32::from_be_bytes([full_wire[0], full_wire[1], full_wire[2], full_wire[3]]) as usize;
        let split_idx = 4 + ping_len + 1;
        assert_eq!(full_wire[split_idx - 1], 0, "first byte of second header must be 0x00");

        let mut incoming = BytesMut::new();
        incoming.put_slice(&full_wire[..split_idx]);

        // First frame is emitted immediately without being withheld
        let msg1 = dec_codec.decode(&mut incoming).unwrap().expect("first frame decodes immediately");
        assert_eq!(msg1, WireMessage::Ping);
        assert!(!dec_codec.has_nul_marker(), "must not latch NUL marker on ambiguous single byte");

        // Subsequent decode call returns None before remaining bytes of second frame arrive
        assert_eq!(dec_codec.decode(&mut incoming).unwrap(), None);

        // Now deliver the rest of the stream
        incoming.put_slice(&full_wire[split_idx..]);

        let msg2 = dec_codec.decode(&mut incoming).unwrap().expect("second frame decodes");
        assert_eq!(msg2, WireMessage::Pong);
        assert!(!dec_codec.has_nul_marker(), "must remain markerless");
        assert!(incoming.is_empty());
    }

    #[test]
    fn test_codec_decodes_multiple_trailing_nul_without_flag() {
        let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
        let mut dec_codec = ProdCodeCodec::new();

        let mut buf = BytesMut::new();
        enc_codec.encode(WireMessage::Ping, &mut buf).unwrap();
        enc_codec.encode(WireMessage::Pong, &mut buf).unwrap();

        assert_eq!(dec_codec.decode(&mut buf).unwrap(), Some(WireMessage::Ping));
        assert_eq!(dec_codec.decode(&mut buf).unwrap(), Some(WireMessage::Pong));
        assert!(buf.is_empty());
    }

    #[test]
    fn test_codec_fragmented_byte_by_byte_with_nul_marker() {
        let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
        let mut dec_codec = ProdCodeCodec::new();

        let mut full_wire = BytesMut::new();
        enc_codec.encode(WireMessage::Ping, &mut full_wire).unwrap();
        enc_codec.encode(WireMessage::Pong, &mut full_wire).unwrap();

        let mut incoming = BytesMut::new();
        let mut decoded_messages = Vec::new();

        for byte in full_wire {
            incoming.put_u8(byte);
            while let Some(msg) = dec_codec.decode(&mut incoming).unwrap() {
                decoded_messages.push(msg);
            }
        }

        assert_eq!(decoded_messages, vec![WireMessage::Ping, WireMessage::Pong]);
        assert!(incoming.is_empty());
    }

    #[test]
    fn test_codec_fragmented_splits_at_every_byte_boundary() {
        let mut enc_codec = ProdCodeCodec::new().with_nul_marker(true);
        let mut full_wire = BytesMut::new();
        enc_codec.encode(WireMessage::Ping, &mut full_wire).unwrap();
        enc_codec.encode(WireMessage::Pong, &mut full_wire).unwrap();

        // Feed chunks split at every possible byte boundary, including lengths 4 and 6
        for split in 1..full_wire.len() {
            let mut dec_codec = ProdCodeCodec::new();
            let mut incoming = BytesMut::new();
            let mut decoded = Vec::new();

            incoming.put_slice(&full_wire[..split]);
            while let Some(msg) = dec_codec.decode(&mut incoming).unwrap() {
                decoded.push(msg);
            }

            incoming.put_slice(&full_wire[split..]);
            while let Some(msg) = dec_codec.decode(&mut incoming).unwrap() {
                decoded.push(msg);
            }

            assert_eq!(
                decoded,
                vec![WireMessage::Ping, WireMessage::Pong],
                "failed decoding with split at index {split}"
            );
            assert!(
                incoming.is_empty(),
                "buffer not empty with split at index {split}"
            );
        }
    }

    #[test]
    fn test_codec_handles_whitespace_prefixed_json() {
        let mut dec_codec = ProdCodeCodec::new();
        let mut buf = BytesMut::new();

        // Encode Ping with whitespace prefix in its body
        let json_ping = b"   {\"type\":\"Ping\"}";
        buf.put_u32(json_ping.len() as u32);
        buf.put_slice(json_ping);

        // Encode Pong with whitespace prefix in its body
        let json_pong = b"\n\t{\"type\":\"Pong\"}";
        buf.put_u32(json_pong.len() as u32);
        buf.put_slice(json_pong);

        assert_eq!(dec_codec.decode(&mut buf).unwrap(), Some(WireMessage::Ping));
        assert_eq!(dec_codec.decode(&mut buf).unwrap(), Some(WireMessage::Pong));
        assert!(buf.is_empty());
    }

    #[test]
    fn test_codec_decodes_embedded_nul_in_frame_len() {
        let mut dec_codec = ProdCodeCodec::new();
        let mut buf = BytesMut::new();

        let original = WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: 1,
            supported_versions: Some(vec![1]),
            capabilities: None,
            client_name: "embedded-nul".to_string(),
            client_pid: 1,
            auth_token: None,
            client_workspace_root: "/p".to_string(),
            preferred_engine: None,
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: None,
            redirect_count: 0,
        });

        let mut json_with_nul = serde_json::to_vec(&original).unwrap();
        json_with_nul.push(0);
        buf.put_u32(json_with_nul.len() as u32);
        buf.put_slice(&json_with_nul);

        let decoded = dec_codec.decode(&mut buf).unwrap().expect("decodes frame with internal NUL");
        assert_eq!(decoded, original);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_codec_nul_marker_fails_when_nul_missing() {
        let mut enc_codec = ProdCodeCodec::new(); // no NUL marker
        let mut dec_codec = ProdCodeCodec::new().with_nul_marker(true); // requires NUL marker

        let mut buf = BytesMut::new();
        let original = WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: 1,
            supported_versions: Some(vec![1]),
            capabilities: None,
            client_name: "missing-nul-client".to_string(),
            client_pid: 4321,
            auth_token: None,
            client_workspace_root: "/home/user/missing".to_string(),
            preferred_engine: None,
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: None,
            redirect_count: 0,
        });

        enc_codec.encode(original, &mut buf).unwrap();
        // Since dec_codec expects length + NUL marker (+1 byte), it will wait for the missing NUL byte
        assert_eq!(dec_codec.decode(&mut buf).unwrap(), None);

        // If we append a non-zero byte instead of NUL, it should report an error
        buf.put_u8(b'X');
        let err = dec_codec.decode(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("Missing expected NUL completion marker"));
    }
}
