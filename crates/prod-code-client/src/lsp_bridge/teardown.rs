/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The editor's own teardown (#1521). Once the editor has sent `exit`, the language server
//! leaves and the gateway closes the session: that is the end the editor asked for, not a
//! broken connection to report or reconnect.

#[derive(Default)]
pub struct Teardown {
    shutdown: bool,
    exit: bool,
}

impl Teardown {
    /// Notes a message the editor sent, by its method.
    pub fn observe(&mut self, method: Option<&str>) {
        match method {
            Some("shutdown") => self.shutdown = true,
            Some("exit") => self.exit = true,
            _ => {}
        }
    }

    /// The exit code when the gateway closing the session is the expected end: after `exit`,
    /// 0 when `shutdown` came first and 1 when it did not, as the protocol has a server exit.
    /// `None` while the editor has not asked to end, so the closure is a failure.
    pub fn expected_exit(&self) -> Option<i32> {
        self.exit.then_some(if self.shutdown { 0 } else { 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::Teardown;

    #[test]
    fn a_closure_after_exit_is_the_end_the_editor_asked_for() {
        let mut t = Teardown::default();
        t.observe(Some("textDocument/didOpen"));
        assert_eq!(
            t.expected_exit(),
            None,
            "an unexpected closure is a failure"
        );
        t.observe(Some("shutdown"));
        assert_eq!(
            t.expected_exit(),
            None,
            "shutdown alone does not end the session"
        );
        t.observe(Some("exit"));
        assert_eq!(t.expected_exit(), Some(0));

        let mut abrupt = Teardown::default();
        abrupt.observe(Some("exit"));
        assert_eq!(abrupt.expected_exit(), Some(1), "exit without shutdown");
    }
}
