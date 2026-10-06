/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;

use crate::session::LspSession;

pub(crate) struct SessionPool<'a> {
    remote: SocketAddr,
    root: &'a Path,
    sessions: HashMap<String, LspSession>,
}

impl<'a> SessionPool<'a> {
    pub(crate) fn new(remote: SocketAddr, root: &'a Path) -> Self {
        Self {
            remote,
            root,
            sessions: HashMap::new(),
        }
    }

    pub(crate) async fn session_for_file(&mut self, file: &Path) -> Result<&mut LspSession> {
        let (subpath, mut engine) = crate::sync::engine_project(self.root, file);
        if let Some(own) = crate::sync::engine_for_file(file)
            && engine == crate::sync::expected_engine(self.root)
            && Some(own) != engine
        {
            engine = Some(own);
        }
        let key = format!(
            "{}|{}",
            subpath.as_deref().unwrap_or(""),
            engine.unwrap_or("generic")
        );
        if !self.sessions.contains_key(&key) {
            let session = LspSession::open(self.remote, self.root, Some(file)).await?;
            self.sessions.insert(key.clone(), session);
        }
        Ok(self.sessions.get_mut(&key).expect("session exists"))
    }

    pub(crate) async fn close_all(&mut self) {
        for (_, session) in self.sessions.drain() {
            session.close().await;
        }
    }
}
