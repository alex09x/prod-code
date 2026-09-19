//! Wire framing, codecs, and transport types for prod-code Remote Code Intelligence.

pub mod codec;
pub mod messages;
pub mod path;

pub use codec::ProdCodeCodec;
pub use messages::{
    HandshakeRequest, HandshakeResponse, PROTOCOL_VERSION, StatusResponse, WireMessage,
};
pub use path::PathTranslator;

pub const DEFAULT_PORT: u16 = 9400;
