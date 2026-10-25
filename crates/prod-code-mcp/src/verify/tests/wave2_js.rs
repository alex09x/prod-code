/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::verify::plan::*;
use crate::verify::types::*;

#[test]
fn wave2_package_manager_commands() {
    for (pm, exec, run) in [
        (
            PackageManager::Npm,
            vec!["npx", "--no-install"],
            vec!["npm", "run", "--silent"],
        ),
        (
            PackageManager::Pnpm,
            vec!["pnpm", "exec"],
            vec!["pnpm", "run", "--silent"],
        ),
        (PackageManager::Yarn, vec!["yarn"], vec!["yarn", "run"]),
        (PackageManager::Bun, vec!["bunx"], vec!["bun", "run"]),
    ] {
        assert_eq!(pm.exec(), exec);
        assert_eq!(pm.run(), run);
    }
}

#[test]
fn wave2_javascript_plans() {
    for (runner, suffix) in [
        (
            JsTestRunner::Vitest,
            vec!["vitest", "run", "--reporter=verbose"],
        ),
        (
            JsTestRunner::Jest,
            vec!["jest", "--ci", "--colors=false", "--verbose"],
        ),
        (JsTestRunner::BunTest, vec!["bun", "test"]),
        (JsTestRunner::Mocha, vec!["mocha"]),
        (JsTestRunner::Script, vec!["yarn", "run", "test"]),
    ] {
        let tools = ProjectTools {
            package_manager: PackageManager::Yarn,
            js_tests: runner,
            ..Default::default()
        };
        for filter in [None, Some(""), Some("adds")] {
            let mut expected = if matches!(runner, JsTestRunner::BunTest | JsTestRunner::Script) {
                vec![]
            } else {
                vec!["yarn"]
            };
            expected.extend(&suffix);
            if filter == Some("adds") && runner != JsTestRunner::Script {
                expected.extend([
                    if runner == JsTestRunner::Mocha {
                        "-g"
                    } else {
                        "-t"
                    },
                    "adds",
                ]);
            }
            assert_eq!(
                plan_command_with(&tools, "typescript", VerifyKind::Test, filter).unwrap(),
                expected
            );
        }
    }
    for (linter, suffix) in [
        ("eslint", vec!["eslint", ".", "-f", "unix"]),
        ("biome", vec!["biome", "lint", "."]),
    ] {
        let tools = ProjectTools {
            js_linter: Some(linter),
            ..Default::default()
        };
        let mut expected = vec!["npx", "--no-install"];
        expected.extend(suffix);
        assert_eq!(
            plan_command_with(&tools, "typescript", VerifyKind::Lint, None).unwrap(),
            expected
        );
    }
    assert_eq!(
        plan_command("typescript", VerifyKind::Lint, None)
            .unwrap_err()
            .to_string(),
        "no linter configured (eslint or biome) in this project"
    );
}
