/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::fixtures::{SCHEDULER_TOML, refused, verdict};

#[test]
fn sccache_client_side_values_that_are_not_on_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let h = home.path().to_str().unwrap();
    for on in ["1", "true", "TRUE", "on", "On"] {
        verdict(&[("HOME", h), ("SCCACHE_CLIENT_SIDE", on)], ws.path(), &[])
            .unwrap_or_else(|e| panic!("{on}: {e}"));
    }
    for off in ["0", "false", "FALSE", "off", "Off"] {
        refused(
            verdict(&[("HOME", h), ("SCCACHE_CLIENT_SIDE", off)], ws.path(), &[]),
            "turns sccache's client-side mode off",
        );
    }
    // sccache fails on these, so they cannot silently disable it either.
    for bad in ["no", "yes", "2", " 1", "enabled"] {
        refused(
            verdict(&[("HOME", h), ("SCCACHE_CLIENT_SIDE", bad)], ws.path(), &[]),
            "is not one of true, on, 1",
        );
    }
    // Empty or unset falls back to the config file, which does not enable it.
    refused(
        verdict(&[("HOME", h), ("SCCACHE_CLIENT_SIDE", "")], ws.path(), &[]),
        "client_side_mode = true",
    );
    refused(
        verdict(&[("HOME", h)], ws.path(), &[]),
        "SCCACHE_CLIENT_SIDE is empty",
    );
}

#[test]
fn sccache_log_disables_client_side_mode_but_error_log_does_not() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let h = home.path().to_str().unwrap();
    let base = [("HOME", h), ("SCCACHE_CLIENT_SIDE", "1")];
    refused(
        verdict(
            &[base[0], base[1], ("SCCACHE_LOG", "debug")],
            ws.path(),
            &[],
        ),
        "SCCACHE_LOG",
    );
    refused(
        verdict(&[base[0], base[1], ("SCCACHE_LOG", "")], ws.path(), &[]),
        "SCCACHE_LOG",
    );
    // Only the daemon's stderr goes there; the client-side mode is unaffected.
    verdict(
        &[base[0], base[1], ("SCCACHE_ERROR_LOG", "/tmp/sccache.log")],
        ws.path(),
        &[],
    )
    .unwrap();
}

#[test]
fn sccache_scheduler_in_the_default_config_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let (h, x) = (home.path().to_str().unwrap(), xdg.path().to_str().unwrap());
    let home_conf = home.path().join(".config/sccache/config");
    std::fs::create_dir_all(home_conf.parent().unwrap()).unwrap();
    std::fs::write(&home_conf, SCHEDULER_TOML).unwrap();
    let why = verdict(&[("HOME", h), ("SCCACHE_CLIENT_SIDE", "1")], ws.path(), &[]).unwrap_err();
    assert!(why.contains("dist.scheduler_url"), "{why}");
    assert!(why.contains(&home_conf.display().to_string()), "{why}");
    assert!(!why.contains("hunter2"), "credentials leaked: {why}");
    // A relative XDG_CONFIG_HOME is ignored, as the directories crate does.
    refused(
        verdict(
            &[
                ("HOME", h),
                ("XDG_CONFIG_HOME", "rel"),
                ("SCCACHE_CLIENT_SIDE", "1"),
            ],
            ws.path(),
            &[],
        ),
        "dist.scheduler_url",
    );
    // An absolute XDG_CONFIG_HOME replaces ~/.config.
    verdict(
        &[
            ("HOME", h),
            ("XDG_CONFIG_HOME", x),
            ("SCCACHE_CLIENT_SIDE", "1"),
        ],
        ws.path(),
        &[],
    )
    .unwrap();
    std::fs::create_dir_all(xdg.path().join("sccache")).unwrap();
    std::fs::write(xdg.path().join("sccache/config"), SCHEDULER_TOML).unwrap();
    refused(
        verdict(
            &[("XDG_CONFIG_HOME", x), ("SCCACHE_CLIENT_SIDE", "1")],
            ws.path(),
            &[],
        ),
        "dist.scheduler_url",
    );
    // A dist section without a scheduler is local compilation.
    std::fs::write(
        &home_conf,
        "[dist]\ncache_dir = \"/tmp/x\"\n[cache.disk]\ndir = \"/tmp/c\"\n",
    )
    .unwrap();
    verdict(&[("HOME", h), ("SCCACHE_CLIENT_SIDE", "1")], ws.path(), &[]).unwrap();
}

#[test]
fn sccache_conf_overrides_the_default_config() {
    let home = tempfile::tempdir().unwrap();
    let conf = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let h = home.path().to_str().unwrap();
    let home_conf = home.path().join(".config/sccache/config");
    std::fs::create_dir_all(home_conf.parent().unwrap()).unwrap();
    std::fs::write(&home_conf, SCHEDULER_TOML).unwrap();
    // An explicit safe config wins over the default one with a scheduler.
    let safe = conf.path().join("safe.toml");
    std::fs::write(
        &safe,
        "client_side_mode = true\n[cache.disk]\ndir = \"/tmp/c\"\n",
    )
    .unwrap();
    let s = safe.to_str().unwrap();
    verdict(
        &[
            ("HOME", h),
            ("SCCACHE_CONF", s),
            ("SCCACHE_CLIENT_SIDE", "1"),
        ],
        ws.path(),
        &[],
    )
    .unwrap();
    // With SCCACHE_CLIENT_SIDE empty the file's client_side_mode decides.
    verdict(
        &[
            ("HOME", h),
            ("SCCACHE_CONF", s),
            ("SCCACHE_CLIENT_SIDE", ""),
        ],
        ws.path(),
        &[],
    )
    .unwrap();
    // A non-empty variable wins over the file.
    refused(
        verdict(
            &[("SCCACHE_CONF", s), ("SCCACHE_CLIENT_SIDE", "off")],
            ws.path(),
            &[],
        ),
        "client-side mode off",
    );
    // A missing SCCACHE_CONF file is no config, as for sccache.
    let missing = conf.path().join("missing.toml");
    verdict(
        &[
            ("SCCACHE_CONF", missing.to_str().unwrap()),
            ("SCCACHE_CLIENT_SIDE", "1"),
        ],
        ws.path(),
        &[],
    )
    .unwrap();
    // JSON by extension: a null scheduler is none, a string one is distributed.
    let json = conf.path().join("conf.json");
    let j = json.to_str().unwrap();
    std::fs::write(
        &json,
        r#"{"dist": {"scheduler_url": null}, "client_side_mode": true}"#,
    )
    .unwrap();
    verdict(
        &[("SCCACHE_CONF", j), ("SCCACHE_CLIENT_SIDE", "")],
        ws.path(),
        &[],
    )
    .unwrap();
    std::fs::write(&json, r#"{"dist": {"scheduler_url": "https://s.invalid"}}"#).unwrap();
    refused(
        verdict(
            &[("SCCACHE_CONF", j), ("SCCACHE_CLIENT_SIDE", "1")],
            ws.path(),
            &[],
        ),
        "dist.scheduler_url",
    );
    // Relative or empty paths depend on each compiler's working directory.
    for rel in ["sccache.toml", ""] {
        refused(
            verdict(
                &[("SCCACHE_CONF", rel), ("SCCACHE_CLIENT_SIDE", "1")],
                ws.path(),
                &[],
            ),
            "SCCACHE_CONF must be an absolute path",
        );
    }
    refused(
        verdict(&[("SCCACHE_CLIENT_SIDE", "1")], ws.path(), &[]),
        "cannot locate the sccache config",
    );
}

#[test]
fn invalid_or_unreadable_sccache_configs_are_refused_without_their_content() {
    let conf = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let cases = [
        (
            "bad.toml",
            "token = \"hunter2\"\n[dist\n",
            "is not valid TOML",
        ),
        ("bad.json", "{\"token\": \"hunter2\",", "is not valid JSON"),
        ("list.json", "[\"hunter2\"]", "is not a table"),
        (
            "dist.toml",
            "dist = \"hunter2\"\n",
            "`dist` that is not a table",
        ),
        (
            "mode.toml",
            "client_side_mode = \"hunter2\"\n",
            "not a boolean",
        ),
    ];
    for (name, content, needle) in cases {
        let path = conf.path().join(name);
        std::fs::write(&path, content).unwrap();
        let why = verdict(
            &[
                ("SCCACHE_CONF", path.to_str().unwrap()),
                ("SCCACHE_CLIENT_SIDE", "1"),
            ],
            ws.path(),
            &[],
        )
        .unwrap_err();
        assert!(why.contains(needle), "{name}: {why}");
        assert!(!why.contains("hunter2"), "{name}: content leaked: {why}");
    }
    let binary = conf.path().join("binary");
    std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
    refused(
        verdict(
            &[
                ("SCCACHE_CONF", binary.to_str().unwrap()),
                ("SCCACHE_CLIENT_SIDE", "1"),
            ],
            ws.path(),
            &[],
        ),
        "is not UTF-8 text",
    );
    // A directory where the file should be cannot be read.
    refused(
        verdict(
            &[
                ("SCCACHE_CONF", conf.path().to_str().unwrap()),
                ("SCCACHE_CLIENT_SIDE", "1"),
            ],
            ws.path(),
            &[],
        ),
        "cannot read the sccache config",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = conf.path().join("locked.toml");
        std::fs::write(&locked, SCHEDULER_TOML).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads it anyway; the refusal is only observable for other users.
        if std::fs::read(&locked).is_err() {
            refused(
                verdict(
                    &[
                        ("SCCACHE_CONF", locked.to_str().unwrap()),
                        ("SCCACHE_CLIENT_SIDE", "1"),
                    ],
                    ws.path(),
                    &[],
                ),
                "cannot read the sccache config",
            );
        }
    }
}
