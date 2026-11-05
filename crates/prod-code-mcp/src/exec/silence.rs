/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A command that ran to its deadline and printed nothing (#1892). The node starts commands with
//! standard input closed and no terminal, so a command that waits for a login, a browser, a
//! prompt or an approval waits for ever and shows no sign of it; the cause is the wait, and
//! the report says so instead of only that the deadline passed.

/// What to tell the caller of a command that timed out with no output, if that is what happened.
pub fn silent_timeout_hint(timed_out: bool, output_bytes: usize) -> Option<&'static str> {
    (timed_out && output_bytes == 0).then_some(
        "The command printed nothing before the deadline. On the node it runs with standard \
         input closed and no terminal, so a command waiting for a login, a browser, a prompt or \
         an approval waits until the deadline. Run it with its non-interactive or token option, \
         or where it can authenticate; and raise the deadline only if it is known to be \
         working.",
    )
}

#[cfg(test)]
mod tests {
    use super::silent_timeout_hint;

    #[test]
    fn only_a_timeout_with_no_output_is_explained() {
        let hint = silent_timeout_hint(true, 0).expect("a silent timeout is explained");
        assert!(hint.contains("standard input closed"), "{hint}");
        assert!(hint.contains("non-interactive"), "{hint}");
        assert_eq!(silent_timeout_hint(true, 1), None, "it printed something");
        assert_eq!(silent_timeout_hint(false, 0), None, "it did not time out");
        assert_eq!(silent_timeout_hint(false, 5), None);
    }
}
