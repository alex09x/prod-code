/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::probe::lock_unpoisoned;
use std::collections::VecDeque;
use std::sync::{Mutex as StdMutex, Weak};
use std::time::Instant;

pub(crate) const MAX_RETAINED_DISPATCH_IDENTITIES: usize = 256;

/// The ordinary requests sent to the server and not answered yet, oldest first (#1428).
#[derive(Default)]
pub(crate) struct IssuedRequestHistory {
    pub(crate) next_token: u64,
    pub(crate) requests: VecDeque<(u64, serde_json::Value, Instant)>,
}

impl IssuedRequestHistory {
    pub(crate) fn insert(&mut self, id: serde_json::Value) -> u64 {
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.requests.push_back((token, id, Instant::now()));
        while self.requests.len() > MAX_RETAINED_DISPATCH_IDENTITIES {
            self.requests.pop_front();
        }
        token
    }

    pub(crate) fn remove(&mut self, token: u64) {
        if let Some(index) = self
            .requests
            .iter()
            .position(|(candidate, _, _)| *candidate == token)
        {
            self.requests.remove(index);
        }
    }

    /// Whether `id` answers an outstanding request, which it then no longer is: a response
    /// replayed for a request already answered is not new evidence that the server works.
    pub(crate) fn take(&mut self, id: &serde_json::Value) -> bool {
        let Some(index) = self
            .requests
            .iter()
            .position(|(_, candidate, _)| candidate == id)
        else {
            return false;
        };
        self.requests.remove(index);
        true
    }

    /// When the oldest unanswered request was sent.
    pub(crate) fn oldest_outstanding(&self) -> Option<Instant> {
        self.requests.iter().map(|(_, _, at)| *at).min()
    }
}

pub(crate) struct IssuedRequestRegistration {
    pub(crate) history: Weak<StdMutex<IssuedRequestHistory>>,
    pub(crate) token: u64,
    pub(crate) committed: bool,
}

impl Drop for IssuedRequestRegistration {
    fn drop(&mut self) {
        if !self.committed
            && let Some(history) = self.history.upgrade()
        {
            lock_unpoisoned(&history).remove(self.token);
        }
    }
}

pub(crate) struct OwnedTask(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
