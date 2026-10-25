/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

fn touch(root: &Path, rel: &str) -> PathBuf {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "").unwrap();
    p
}

fn no_path() -> Result<PathBuf> {
    anyhow::bail!("gopls not found in PATH")
}

#[test]
fn default_config_probes_once_a_minute_and_sets_nothing_else() {
    let c = GoConfig::default();
    assert_eq!(c.gopls_path, None);
    assert_eq!(c.shared_cache_dir, None);
    assert!(c.extra_env.is_empty());
    assert!(c.build_flags.is_empty());
    assert_eq!(c.health_probe_interval, Some(Duration::from_secs(60)));
    assert_eq!(DEFAULT_HEALTH_PROBE_INTERVAL, Duration::from_secs(60));
    assert_eq!(MAX_IDLE_PROBE_TIMEOUTS, 3);
    assert_eq!(HEALTH_PROBE_METHOD, "prodCode/healthProbe");
    assert_eq!(HEALTH_PROBE_ID_PREFIX, "prod-code-health:");
}

#[test]
fn discovery_prefers_explicit_then_path_then_home_gopath_then_fixed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let explicit = touch(root, "explicit/gopls");
    let on_path = root.join("onpath/gopls");
    let home_go = touch(root, "home/go/bin/gopls");
    let home_local = touch(root, "home/.local/bin/gopls");
    let gopath_bin = touch(root, "gopath/bin/gopls");
    let fixed = touch(root, "fixed/gopls");
    let home = root.join("home").to_string_lossy().into_owned();
    let gopath = root.join("gopath").to_string_lossy().into_owned();
    let env = |k: &str| match k {
        "HOME" => Some(home.clone()),
        "GOPATH" => Some(gopath.clone()),
        _ => None,
    };
    let fixed_list = [fixed.clone()];

    // An existing explicit path wins over everything, even a hit on $PATH.
    let found = find_gopls_with(Some(&explicit), || Ok(on_path.clone()), env, &fixed_list);
    assert_eq!(found, Some(explicit.clone()));
    // A missing explicit path is skipped; $PATH answers next.
    let missing = root.join("missing/gopls");
    let found = find_gopls_with(Some(&missing), || Ok(on_path.clone()), env, &fixed_list);
    assert_eq!(found, Some(on_path.clone()));
    // Without $PATH: $HOME/go/bin, then $HOME/.local/bin, then $GOPATH/bin, then the fixed list.
    let found = find_gopls_with(None, no_path, env, &fixed_list);
    assert_eq!(found, Some(home_go.clone()));
    std::fs::remove_file(&home_go).unwrap();
    assert_eq!(
        find_gopls_with(None, no_path, env, &fixed_list),
        Some(home_local.clone())
    );
    std::fs::remove_file(&home_local).unwrap();
    assert_eq!(
        find_gopls_with(None, no_path, env, &fixed_list),
        Some(gopath_bin.clone())
    );
    std::fs::remove_file(&gopath_bin).unwrap();
    assert_eq!(
        find_gopls_with(None, no_path, env, &fixed_list),
        Some(fixed.clone())
    );
    std::fs::remove_file(&fixed).unwrap();
    assert_eq!(find_gopls_with(None, no_path, env, &fixed_list), None);
    // No HOME and no GOPATH: only the fixed list is consulted.
    assert_eq!(find_gopls_with(None, no_path, |_| None, &fixed_list), None);
}

#[test]
fn public_discovery_returns_an_existing_explicit_path_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let explicit = touch(dir.path(), "bin/gopls");
    assert_eq!(find_gopls_binary(Some(&explicit)), Some(explicit));
    // Whatever the host has, a hit is a real file and never the missing explicit path.
    let missing = dir.path().join("none/gopls");
    let found = find_gopls_binary(Some(&missing));
    assert!(found.as_ref().is_none_or(|p| p.exists() && *p != missing));
}

#[test]
fn which_lookup_fails_or_returns_a_non_empty_path() {
    match which_gopls() {
        Ok(p) => assert!(!p.as_os_str().is_empty()),
        Err(e) => assert!(
            e.to_string() == "gopls not found in PATH" || e.to_string() == "which command failed",
            "unexpected error: {e}"
        ),
    }
}

#[test]
fn which_binary_finds_a_shell_and_names_a_missing_binary() {
    let sh = which_binary("sh").expect("sh is on PATH");
    assert!(sh.is_absolute() && sh.ends_with("sh"), "{sh:?}");
    let err = which_binary("prod-code-no-such-binary").unwrap_err();
    assert_eq!(
        err.to_string(),
        "prod-code-no-such-binary not found in PATH"
    );
}
