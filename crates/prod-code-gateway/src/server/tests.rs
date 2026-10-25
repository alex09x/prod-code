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

mod serve;

#[tokio::test]
async fn startup_optional_metrics_tasks_preserve_an_existing_socket_path() {
    crate::test_cache_support::isolated!();
    let temp = tempfile::tempdir().unwrap();
    for var in prod_code_protocol::tls::TLS_ENV_VARS {
        unsafe {
            std::env::remove_var(var);
        }
    }
    unsafe {
        std::env::set_var("PROD_CODE_TLS_MODE", "disabled");
    }
    let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let push_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = temp.path().join("existing-file");
    std::fs::write(&socket, "keep this file").unwrap();
    let mut cli =
        ServerCli::try_parse_from(["gateway", "--bind", "127.0.0.1:0", "--no-admission"]).unwrap();
    cli.storage = temp.path().join("storage");
    cli.shadow_dir = Some(temp.path().join("shadow"));
    cli.socket_path = Some(socket.clone());
    cli.advertise = Some("test-identity".into());
    cli.peers = "test-identity, ,".into();
    cli.prometheus_listen = Some(occupied.local_addr().unwrap());
    cli.prometheus_push_url = Some(format!("http://{}", push_listener.local_addr().unwrap()));
    cli.prometheus_instance = Some("test-instance".into());
    cli.prometheus_push_interval_secs = 0;
    let error = run(cli).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("refusing to replace non-socket path {}", socket.display())
    );
    assert_eq!(std::fs::read_to_string(&socket).unwrap(), "keep this file");
    // The optional pusher really starts, using the configured instance and a bounded interval.
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), push_listener.accept())
        .await
        .unwrap()
        .unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut request = Vec::new();
    let mut buf = [0; 4096];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        assert_ne!(n, 0);
        request.extend_from_slice(&buf[..n]);
        if request.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let headers = String::from_utf8_lossy(&request);
    assert!(
        headers.starts_with("POST /metrics/job/prod-code/instance/test-instance HTTP/1.1\r\n"),
        "{headers}"
    );
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
}

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
