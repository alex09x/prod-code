/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::time::Duration;

use super::super::overlay::run_overlay;
use super::super::root::overlay_unavailable;
use super::super::sccache::{ensure_sccache_server, find_sccache};
use super::fixtures::{SCHEDULER_TOML, delta, job, output};

#[tokio::test]
async fn ensure_sccache_server_runs_without_panic() {
    ensure_sccache_server();
}

/// The regression of #426 on a real node: a proposed source file and a proposed build
/// script compile through sccache inside the hypothesis's mount namespace, and the build
/// script's output, the rlib and the test binary exist only in the shadow. It fails, rather
/// than skips, without sccache or overlay support:
/// `cargo test -p prod-code-gateway shadow -- --ignored --nocapture` on a Linux build node.
#[tokio::test]
#[ignore = "needs sccache and unprivileged overlay mounts; run with --ignored on a Linux node"]
async fn overlay_hypotheses_compile_proposed_source_through_sccache() {
    if let Some(reason) = overlay_unavailable() {
        panic!("this test needs overlay shadows: {reason}");
    }
    let sccache = find_sccache().expect("this test needs sccache on PATH or in ~/.cargo/bin");
    // A daemon started from inside the namespace would outlive the hypothesis; start it
    // here so the wrapper only connects to it, as it does on a node with warm builds.
    let _ = std::process::Command::new(&sccache)
        .arg("--start-server")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let ws = tempfile::tempdir().unwrap();
    let shadow = tempfile::tempdir().unwrap();
    // Outside the workspace, so what the wrapper records survives the shadow.
    let tools = tempfile::tempdir().unwrap();
    let wrapper = tools.path().join("record-sccache");
    let log = tools.path().join("wrapper.log");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\n\"$SHADOW_TEST_SCCACHE\" \"$@\"\nrc=$?\n\
         echo \"rc=$rc client_side=${SCCACHE_CLIENT_SIDE-unset} mntns=$(readlink /proc/self/ns/mnt) $*\" \
         >> \"$SHADOW_TEST_WRAPPER_LOG\"\nexit $rc\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(
        ws.path().join("Cargo.toml"),
        "[package]\nname = \"shadow-regress\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        ws.path().join("build.rs"),
        "fn main() {\n    println!(\"cargo:rustc-env=SHADOW_BUILD_RS_VAL=base\");\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(ws.path().join("src")).unwrap();
    let base_lib = "pub fn message() -> String {\n    format!(\"base:{}\", env!(\"SHADOW_BUILD_RS_VAL\"))\n}\n";
    std::fs::write(ws.path().join("src/lib.rs"), base_lib).unwrap();

    let proposed_build = r#"fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    std::fs::write(format!("{out}/marker.txt"), "marker:proposed-build-script\n").unwrap();
    println!("cargo:rustc-env=SHADOW_BUILD_RS_VAL=proposed");
}
"#;
    let proposed_lib = r#"pub fn message() -> String {
    format!("hypothesis:{}", env!("SHADOW_BUILD_RS_VAL"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn sees_the_proposed_source_and_build_script() {
        assert_eq!(super::message(), "hypothesis:proposed");
    }
}
"#;
    let script = "set -e\n\
         cargo build --quiet\n\
         cargo test --quiet\n\
         ls target/debug/deps/libshadow_regress-*.rlib\n\
         cat target/debug/build/shadow-regress-*/out/marker.txt\n";
    let mut hypothesis = job(
        ws.path(),
        shadow.path(),
        "sccache",
        vec![
            delta("src/lib.rs", Some(proposed_lib)),
            delta("build.rs", Some(proposed_build)),
        ],
        &["sh", "-c", script],
    );
    hypothesis.timeout = Duration::from_secs(600);
    let target = ws.path().join("target");
    for (k, v) in [
        ("RUSTC_WRAPPER", wrapper.to_str().unwrap()),
        ("SHADOW_TEST_SCCACHE", sccache.to_str().unwrap()),
        ("SHADOW_TEST_WRAPPER_LOG", log.to_str().unwrap()),
        // sccache exits on CARGO_INCREMENTAL=1 and cannot cache incremental builds.
        ("CARGO_INCREMENTAL", "0"),
        ("CARGO_TARGET_DIR", target.to_str().unwrap()),
    ] {
        hypothesis.env.push((k.to_string(), v.to_string()));
    }
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let result = run_overlay(hypothesis, rx).await;
    let out = output(&result);
    assert_eq!(result.exit_code, Some(0), "{:?} {out}", result.error);
    assert!(out.contains("test result: ok. 1 passed"), "{out}");
    assert!(out.contains("libshadow_regress-"), "{out}");
    assert!(out.contains("marker:proposed-build-script"), "{out}");

    // Nothing reached the workspace copy: no target directory, the base files unchanged.
    assert!(!target.exists(), "the build escaped the shadow");
    assert_eq!(
        std::fs::read_to_string(ws.path().join("src/lib.rs")).unwrap(),
        base_lib
    );
    assert!(
        std::fs::read_to_string(ws.path().join("build.rs"))
            .unwrap()
            .contains("=base")
    );

    // sccache itself ran for the build script and the library, inside another mount
    // namespace, with client-side mode on, and succeeded.
    let recorded = std::fs::read_to_string(&log).expect("the wrapper was never invoked");
    let own_ns = std::fs::read_link("/proc/self/ns/mnt").unwrap();
    let own_ns = format!("mntns={}", own_ns.display());
    for krate in ["build_script_build", "shadow_regress"] {
        let calls: Vec<&str> = recorded
            .lines()
            .filter(|l| l.contains(&format!("--crate-name {krate} ")))
            .collect();
        assert!(!calls.is_empty(), "no sccache call for {krate}: {recorded}");
        for call in calls {
            assert!(call.starts_with("rc=0 client_side=1 mntns="), "{call}");
            assert!(
                !call.contains(&own_ns),
                "compiled outside the shadow: {call}"
            );
        }
    }
    for line in recorded.lines() {
        let crate_name = line
            .split("--crate-name ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or("-");
        eprintln!(
            "wrapper: {} crate={crate_name}",
            line.split(' ').take(3).collect::<Vec<_>>().join(" ")
        );
    }
}

#[tokio::test]
async fn overlay_hypotheses_refuse_sccache_settings_before_running() {
    let ws = tempfile::tempdir().unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let conf = tempfile::tempdir().unwrap();
    let safe = conf.path().join("safe.toml");
    std::fs::write(&safe, "").unwrap();
    let safe = safe.to_str().unwrap();
    let in_ws = ws.path().join("sccache.toml");
    let in_ws = in_ws.to_str().unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    // Would leave a file in the shadow root if the command ran.
    let argv = ["sh", "-c", "echo ran"];
    let cases = [
        (
            "log",
            vec![("SCCACHE_CONF", safe), ("SCCACHE_LOG", "info")],
            vec![],
            "SCCACHE_LOG",
        ),
        (
            "empty",
            vec![("SCCACHE_CONF", safe), ("SCCACHE_CLIENT_SIDE", "")],
            vec![],
            "SCCACHE_CLIENT_SIDE is empty",
        ),
        (
            "no",
            vec![("SCCACHE_CONF", safe), ("SCCACHE_CLIENT_SIDE", "no")],
            vec![],
            "SCCACHE_CLIENT_SIDE is not one of",
        ),
        (
            "proposed-scheduler",
            vec![("SCCACHE_CONF", in_ws)],
            vec![delta("sccache.toml", Some(SCHEDULER_TOML))],
            "dist.scheduler_url",
        ),
    ];
    for (name, env, files, needle) in cases {
        let mut j = job(ws.path(), shadow.path(), name, files, &argv);
        j.env = env
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let r = run_overlay(j, rx.clone()).await;
        assert!(
            r.exit_code.is_none() && r.output_tail.is_none(),
            "{name} ran"
        );
        let why = r.error.unwrap_or_default();
        assert!(
            why.starts_with("cannot run an overlay shadow: "),
            "{name}: {why}"
        );
        assert!(why.contains(needle), "{name}: {why}");
        assert!(!why.contains("hunter2"), "{name}: {why}");
    }
    // Refused before staging: nothing was created under the shadow root.
    assert_eq!(std::fs::read_dir(shadow.path()).unwrap().count(), 0);
}
