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

#[test]
fn explicit_configuration_preserves_values_and_reports_the_failing_stage() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("gateway.toml");
    assert_eq!(
        load_config_file(Some(&path)).unwrap_err().to_string(),
        format!("Configuration file not found: {}", path.display())
    );
    fs::create_dir(&path).unwrap();
    assert_eq!(
        load_config_file(Some(&path)).unwrap_err().to_string(),
        format!("Failed to read configuration file: {}", path.display())
    );
    fs::remove_dir(&path).unwrap();
    fs::write(&path, "[server\n").unwrap();
    assert_eq!(
        load_config_file(Some(&path)).unwrap_err().to_string(),
        format!("Failed to parse configuration file: {}", path.display())
    );
    fs::write(&path, "[server]\nbind='127.0.0.1:0'\nengines=['python','go']\nidle_evict_secs=0\n[metrics]\nenabled=false\n[metrics.retention]\nretention_days=7\nmax_storage_bytes=42\n[metrics.prometheus]\nmode='disabled'\n").unwrap();
    let config = load_config_file(Some(&path)).unwrap().unwrap();
    assert_eq!(config.server.bind, Some("127.0.0.1:0".parse().unwrap()));
    assert_eq!(
        config.server.engines,
        Some(vec!["python".into(), "go".into()])
    );
    assert_eq!(config.server.idle_evict_secs, Some(0));
    assert_eq!(config.metrics.enabled, Some(false));
    assert_eq!(
        config.metrics.retention,
        Some(MetricsRetentionConfig {
            retention_days: Some(7),
            max_storage_bytes: Some(42)
        })
    );
    assert_eq!(
        config.metrics.prometheus.as_ref().unwrap().mode.as_deref(),
        Some("disabled")
    );
    assert_eq!(
        toml::from_str::<GatewayConfigFile>(&toml::to_string(&config).unwrap()).unwrap(),
        config
    );
}

use std::fs;

#[test]
fn candidate_search_skips_unreadable_and_malformed_configs_and_stops_at_the_first_valid_one() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    let binary = temp.path().join("binary");
    let malformed = temp.path().join("malformed");
    let valid = temp.path().join("valid");
    let later = temp.path().join("later");
    fs::write(&binary, [0xff]).unwrap();
    fs::write(&malformed, "[").unwrap();
    fs::write(&valid, "[server]\nidle_evict_secs=7\n").unwrap();
    fs::write(&later, "[server]\nidle_evict_secs=8\n").unwrap();
    assert_eq!(
        load_first_config([
            missing.clone(),
            temp.path().to_path_buf(),
            binary.clone(),
            malformed.clone()
        ]),
        None
    );
    let mut expected = GatewayConfigFile::default();
    expected.server.idle_evict_secs = Some(7);
    assert_eq!(
        load_first_config([missing, binary, malformed, valid, later]),
        Some(expected)
    );
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert_eq!(
        default_config_paths(),
        vec![
            PathBuf::from("/etc/prod-code/gateway.toml"),
            home.join(".config/prod-code/gateway.toml"),
            home.join(".prod-code/gateway.toml"),
            PathBuf::from("./gateway.toml")
        ]
    );
    unsafe {
        std::env::remove_var("HOME");
    }
    assert_eq!(
        default_config_paths(),
        vec![
            PathBuf::from("/etc/prod-code/gateway.toml"),
            PathBuf::from("./gateway.toml")
        ]
    );
}
