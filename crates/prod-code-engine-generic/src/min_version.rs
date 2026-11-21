/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The `minVersion` request option of Rust Intelligence: a request that carries it is answered
//! only once the server has caught up with that version of the document.

/// `capabilities.experimental.ridx.requestOptions.minVersion`, as Rust Intelligence declares it
/// (a version number, 1 so far).
pub fn declares_min_version(capabilities: &serde_json::Value) -> bool {
    capabilities
        .pointer("/experimental/ridx/requestOptions/minVersion")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v >= 1)
}

#[cfg(test)]
mod tests {
    use super::declares_min_version;
    use serde_json::json;

    #[test]
    fn only_a_declared_min_version_option_is_used() {
        let ridx = json!({"experimental": {"ridx": {"methods": {"ridx/status": 1},
            "requestOptions": {"minVersion": 1}}}});
        assert!(declares_min_version(&ridx));
        for caps in [
            json!({}),
            json!({"experimental": {"ridx": {"methods": {"ridx/status": 1}}}}),
            json!({"experimental": {"ridx": {"requestOptions": {"minVersion": 0}}}}),
            json!({"experimental": {"other": {"requestOptions": {"minVersion": 1}}}}),
        ] {
            assert!(!declares_min_version(&caps), "{caps}");
        }
    }
}
