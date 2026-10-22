/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Probe child processes without changing the environment shared by parallel tests.
pub fn go_tools_available() -> bool {
    for program in ["go", "gopls"] {
        match std::process::Command::new(program).arg("version").output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                eprintln!(
                    "SKIPPED live Go test: {program} version failed ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                return false;
            }
            Err(error) => {
                eprintln!("SKIPPED live Go test: {program} is unavailable: {error}");
                return false;
            }
        }
    }
    true
}
