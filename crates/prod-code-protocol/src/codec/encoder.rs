/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::ProdCodeCodec;
use crate::messages::WireMessage;
use bytes::BytesMut;
use std::io;
use tokio_util::codec::Encoder;

impl Encoder<WireMessage> for ProdCodeCodec {
    type Error = io::Error;

    fn encode(&mut self, item: WireMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        self.encode_ref(&item, dst)
    }
}
