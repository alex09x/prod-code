/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::Duration;

#[tokio::test]
async fn invalidation_precedes_capability_cleanup_while_a_reader_is_active() {
    let alive = Arc::new(AtomicBool::new(true));
    let capabilities = Arc::new(tokio::sync::RwLock::new(Some(serde_json::json!({}))));
    let reader = capabilities.read().await;
    let child = Weak::new();
    let pending = Weak::new();
    let health_pending = Weak::new();
    let weak_alive = Arc::downgrade(&alive);
    let weak_capabilities = Arc::downgrade(&capabilities);
    let mut retirement = Box::pin(crate::types::retire_health_generation(
        &child,
        &pending,
        &health_pending,
        &weak_alive,
        &weak_capabilities,
    ));

    let first_poll =
        std::future::poll_fn(|cx| Poll::Ready(std::future::Future::poll(retirement.as_mut(), cx)))
            .await;
    assert!(first_poll.is_pending());
    assert!(!alive.load(Ordering::Acquire));
    assert!(reader.is_some());
    drop(reader);
    tokio::time::timeout(Duration::from_secs(1), retirement)
        .await
        .expect("cleanup resumes after the capability reader releases");
    assert!(capabilities.read().await.is_none());
}
