/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::gateway::CargoCompileGateway;
use prod_code_mcp::compile_check::check;
use prod_code_testkit::Workspace;

const MANIFEST: &str = "[package]\nname = \"conditional\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[features]\noptional = []\n[[test]]\nname = \"gated\"\npath = \"outside/gated.rs\"\nrequired-features = [\"optional\"]\n";
const FILES: &[(&str, &str)] = &[
    ("Cargo.toml", MANIFEST),
    (
        "src/lib.rs",
        "#[cfg(any())]\n#[path = \"../outside/module.rs\"]\npub mod unused;\n",
    ),
    ("outside/module.rs", "pub fn value() -> u32 { 1 }\n"),
    ("outside/gated.rs", "pub fn value() -> u32 { 1 }\n"),
];

#[tokio::test]
async fn skipped_targets_and_modules_cannot_certify_source_verification() {
    let workspace = Workspace::new(FILES);
    let gateway = CargoCompileGateway::with_files(FILES).await;
    for path in ["outside/module.rs", "outside/gated.rs"] {
        let edits = [
            ("Cargo.toml".into(), MANIFEST.into()),
            (path.into(), "pub fn value() -> u32 { \"bad\" }\n".into()),
        ];
        let verdict = check(gateway.addr, &workspace.root(), &edits)
            .await
            .unwrap();
        eprintln!("conditional {path}: {verdict:?}");
        let runs = gateway.take_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].exit_code,
            Some(0),
            "Cargo really skips the gated source"
        );
        assert!(
            !verdict.passed,
            "uncompiled source cannot be certified: {verdict:?}"
        );
        assert!(
            verdict
                .errors
                .iter()
                .any(|e| e.contains("incomplete compiler verification") && e.contains(path))
        );
    }
}
