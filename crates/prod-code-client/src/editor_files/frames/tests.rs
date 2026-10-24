/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[tokio::test]
async fn refusal_ignores_invalid_json_notifications_and_responses() {
    let mut input = Vec::new();
    for body in [
        "{",
        r#"{"jsonrpc":"2.0","method":"initialized"}"#,
        r#"{"jsonrpc":"2.0","id":9,"result":null}"#,
        r#"{"jsonrpc":"2.0","id":"λ","method":"initialize"}"#,
        r#"{"method":"exit"}"#,
        r#"{"id":2,"method":"shutdown"}"#,
    ] {
        write_frame(&mut input, body).await.unwrap();
    }
    let mut output = Vec::new();
    refuse_session(&mut input.as_slice(), &mut output, "unavailable")
        .await
        .unwrap();
    let mut reader = output.as_slice();
    let response = read_frame(&mut reader).await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response).unwrap(),
        serde_json::json!({"jsonrpc":"2.0","id":"λ","error":{"code":-32603,"message":"unavailable"}})
    );
    assert_eq!(read_frame(&mut reader).await.unwrap(), None);
}
