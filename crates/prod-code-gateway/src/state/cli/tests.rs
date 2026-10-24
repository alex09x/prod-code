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
use crate::state::config::{GatewayConfigFile, PrometheusConfig};

#[test]
fn server_configuration_populates_defaults_and_preserves_explicit_values() {
    crate::test_cache_support::isolated!();
    let config: GatewayConfigFile = toml::from_str(
        r#"
[server]
bind='127.0.0.1:1234'
socket_path='sample.sock'
storage='sample-storage'
advertise='127.0.0.1:1234'
peers='127.0.0.1:1235'
engines=['python']
idle_evict_secs=17
engine_reserve_mib=18
max_concurrent_engine_loads=19
prune_worktree_secs=20
prune_workspace_secs=21
prune_below_free_percent=22
build_cache_ram=true
build_cache_dir='sample-cache'
[metrics.prometheus]
listen='127.0.0.1:1236'
"#,
    )
    .unwrap();
    let mut cli = ServerCli::try_parse_from(["gateway"]).unwrap();
    cli.merge_config(&config, None);
    assert_eq!(cli.bind, "127.0.0.1:1234".parse().unwrap());
    assert_eq!(cli.socket_path, Some(PathBuf::from("sample.sock")));
    assert_eq!(cli.storage, PathBuf::from("sample-storage"));
    assert_eq!(cli.advertise.as_deref(), Some("127.0.0.1:1234"));
    assert_eq!(cli.peers, "127.0.0.1:1235");
    assert_eq!(cli.engines, vec!["python"]);
    assert_eq!(
        (
            cli.idle_evict_secs,
            cli.engine_reserve_mib,
            cli.max_concurrent_engine_loads
        ),
        (17, 18, 19)
    );
    assert_eq!(
        (
            cli.prune_worktree_secs,
            cli.prune_workspace_secs,
            cli.prune_below_free_percent
        ),
        (20, 21, 22)
    );
    assert!(cli.build_cache_ram);
    assert_eq!(cli.build_cache_dir, Some(PathBuf::from("sample-cache")));
    assert_eq!(
        cli.prometheus_listen,
        Some("127.0.0.1:1236".parse().unwrap())
    );
    use clap::{CommandFactory, FromArgMatches};
    let matches = ServerCli::command()
        .try_get_matches_from([
            "gateway",
            "--bind",
            "127.0.0.1:2345",
            "--idle-evict-secs",
            "0",
            "--engines",
            "go",
            "--peers",
            "127.0.0.1:2346",
        ])
        .unwrap();
    let mut explicit = ServerCli::from_arg_matches(&matches).unwrap();
    explicit.merge_config(&config, Some(&matches));
    assert_eq!(explicit.bind, "127.0.0.1:2345".parse().unwrap());
    assert_eq!(explicit.idle_evict_secs, 0);
    assert_eq!(explicit.engines, vec!["go"]);
    assert_eq!(explicit.peers, "127.0.0.1:2346");
}

#[test]
fn prometheus_modes_and_explicit_arguments_merge_without_losing_cli_values() {
    crate::test_cache_support::isolated!();
    for (mode, enabled, expected) in [
        ("scrape", None, Some("0.0.0.0:9401")),
        ("both", None, Some("0.0.0.0:9401")),
        ("push", None, None),
        ("disabled", None, None),
        ("both", Some(false), None),
    ] {
        let mut cli = ServerCli::try_parse_from(["gateway"]).unwrap();
        let mut config = GatewayConfigFile::default();
        config.metrics.prometheus = Some(PrometheusConfig {
            mode: Some(mode.into()),
            enabled,
            push_url: Some("http://127.0.0.1:1".into()),
            push_interval_secs: Some(9),
            job: Some("sample".into()),
            instance: Some("instance".into()),
            ..Default::default()
        });
        cli.merge_config(&config, None);
        assert_eq!(cli.prometheus_listen, expected.map(|s| s.parse().unwrap()));
        if mode == "disabled" || enabled == Some(false) {
            assert_eq!(cli.prometheus_push_url, None);
        } else {
            assert_eq!(
                cli.prometheus_push_url.as_deref(),
                Some("http://127.0.0.1:1")
            );
            assert_eq!(cli.prometheus_push_interval_secs, 9);
            assert_eq!(cli.prometheus_job, "sample");
            assert_eq!(cli.prometheus_instance.as_deref(), Some("instance"));
        }
    }
    use clap::{CommandFactory, FromArgMatches};
    let matches = ServerCli::command()
        .try_get_matches_from([
            "gateway",
            "--prometheus-listen",
            "127.0.0.1:1234",
            "--prometheus-push-url",
            "http://127.0.0.1:2",
            "--prune-worktree-days",
            "0",
            "--prune-workspace-days",
            "18446744073709551615",
        ])
        .unwrap();
    let mut cli = ServerCli::from_arg_matches(&matches).unwrap();
    let mut config = GatewayConfigFile::default();
    config.metrics.prometheus = Some(PrometheusConfig {
        enabled: Some(false),
        ..Default::default()
    });
    cli.merge_config(&config, Some(&matches));
    assert_eq!(
        cli.prometheus_listen,
        Some("127.0.0.1:1234".parse().unwrap())
    );
    assert_eq!(
        cli.prometheus_push_url.as_deref(),
        Some("http://127.0.0.1:2")
    );
    assert_eq!(cli.effective_prune_worktree_secs(), 0);
    assert_eq!(cli.effective_prune_workspace_secs(), u64::MAX);
}
