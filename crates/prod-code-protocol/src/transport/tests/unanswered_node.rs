/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A node that does not answer a connection is reported by name, with what to do, soon (#1938).

use std::net::SocketAddr;
use std::time::Duration;

use super::super::client::connect_within;

#[tokio::test]
async fn a_node_that_never_answers_is_a_timed_out_error_naming_it_and_the_way_on() {
    let addr: SocketAddr = "192.0.2.7:9400".parse().unwrap();
    let started = std::time::Instant::now();
    let error = connect_within(
        addr,
        Duration::from_millis(60),
        std::future::pending::<std::io::Result<()>>(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(5));
    let text = error.to_string();
    assert!(text.contains("no answer from 192.0.2.7:9400"), "{text}");
    assert!(text.contains("prod-code status"), "{text}");
    assert!(text.contains("--remote"), "{text}");
}

#[tokio::test]
async fn a_connection_that_succeeds_or_is_refused_in_time_is_passed_on_as_it_is() {
    let addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let ok = connect_within(addr, Duration::from_secs(1), async {
        Ok::<_, std::io::Error>(7)
    })
    .await
    .unwrap();
    assert_eq!(ok, 7);
    let refused = connect_within(addr, Duration::from_secs(1), async {
        Err::<(), _>(std::io::Error::from(std::io::ErrorKind::ConnectionRefused))
    })
    .await
    .unwrap_err();
    assert_eq!(refused.kind(), std::io::ErrorKind::ConnectionRefused);
}
