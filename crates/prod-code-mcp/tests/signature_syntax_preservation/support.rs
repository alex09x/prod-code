/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::signature::SignatureChange;
use std::process::Command;

pub fn position(text: &str, needle: &str) -> (u32, u32) {
    position_at(text, text.find(needle).expect(needle))
}

pub fn position_at(text: &str, at: usize) -> (u32, u32) {
    let prefix = &text[..at];
    (
        prefix.matches('\n').count() as u32 + 1,
        prefix.rsplit('\n').next().unwrap().encode_utf16().count() as u32 + 1,
    )
}

pub fn assert_complete(plan: &SignatureChange) {
    assert!(plan.unmatched.is_empty(), "{:?}", plan.unmatched);
    assert!(plan.unexpected.is_empty(), "{:?}", plan.unexpected);
    assert!(plan.not_async.is_empty(), "{:?}", plan.not_async);
    assert!(plan.diagnostics.is_empty(), "{:?}", plan.diagnostics);
    plan.ensure_writable(false).unwrap();
}

pub fn run(program: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("main.rs");
    let bin = dir.path().join("fixture");
    std::fs::write(&src, program).unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let built = Command::new(rustc)
        .args(["--edition", "2021", "-A", "warnings", "-o"])
        .arg(&bin)
        .arg(&src)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}\n{program}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(bin).output().unwrap();
    assert!(ran.status.success(), "{ran:?}");
    String::from_utf8(ran.stdout).unwrap()
}

pub const EXECUTOR_MAIN: &str = r#"
fn main() {
    struct Wake;
    impl std::task::Wake for Wake {
        fn wake(self: std::sync::Arc<Self>) {}
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(Wake));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(caller());
    let std::task::Poll::Ready(value) = std::future::Future::poll(future.as_mut(), &mut context) else {
        panic!("fixture future must be ready");
    };
    println!("{value}");
}
"#;
