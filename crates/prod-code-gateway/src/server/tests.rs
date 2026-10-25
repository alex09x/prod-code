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

#[tokio::test]
async fn startup_reports_tls_configuration_errors_and_releases_shadow_ownership() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    for var in prod_code_protocol::tls::TLS_ENV_VARS {
        unsafe {
            std::env::remove_var(var);
        }
    }
    for (mode, expected) in [
        (
            "strict",
            "TLS mode 'Strict' is required on server, but 'PROD_CODE_TLS_CERT' and/or 'PROD_CODE_TLS_KEY' are missing",
        ),
        (
            "mutual",
            "TLS mode 'Mutual' is required on server, but 'PROD_CODE_TLS_CERT' and/or 'PROD_CODE_TLS_KEY' are missing",
        ),
        (
            "unknown",
            "invalid PROD_CODE_TLS_MODE: 'unknown'. Expected disabled, auto, strict, or mutual",
        ),
    ] {
        unsafe {
            std::env::set_var("PROD_CODE_TLS_MODE", mode);
        }
        let mut cli =
            ServerCli::try_parse_from(["gateway", "--bind", "127.0.0.1:0", "--no-admission"])
                .unwrap();
        cli.storage = temp.path().join(mode).join("storage");
        cli.shadow_dir = Some(temp.path().join(mode).join("shadow"));
        cli.build_cache_ram = true;
        cli.build_cache_dir = Some(temp.path().join(mode).join("build-cache"));
        cli.engines = vec![" UNAVAILABLE-TEST-ENGINE ".into(), "".into()];
        let shadow = cli.shadow_dir.clone().unwrap();
        assert_eq!(run(cli).await.unwrap_err().to_string(), expected);
        drop(crate::shadow::ShadowRootOwner::acquire(&shadow).unwrap());
    }
    unsafe {
        std::env::set_var("PROD_CODE_TLS_MODE", "disabled");
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut cli = ServerCli::try_parse_from(["gateway"]).unwrap();
    cli.bind = listener.local_addr().unwrap();
    cli.storage = temp.path().join("occupied/storage");
    cli.shadow_dir = Some(temp.path().join("occupied/shadow"));
    let error = run(cli).await.unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::AddrInUse
    );
}
