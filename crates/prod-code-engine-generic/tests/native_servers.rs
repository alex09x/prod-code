use prod_code_engine_generic::{GenericLspConfig, GenericLspEngine};
use std::time::Duration;

fn engine_is_available(test: &str, engine: &str, available: bool) -> bool {
    if std::env::var_os("CI").is_some() || std::env::var_os("PROD_CODE_REQUIRE_ENGINES").is_some() {
        assert!(
            available,
            "{engine} must be installed where native engine tests are required"
        );
    }
    if !available {
        eprintln!("SKIPPED {test}: {engine} is not installed");
    }
    available
}

async fn hover(
    engine: &GenericLspEngine,
    uri: &str,
    line: u32,
    character: u32,
) -> serde_json::Value {
    tokio::time::timeout(
        Duration::from_secs(30),
        engine.send_request(
            "textDocument/hover",
            serde_json::json!({
                "textDocument":{"uri":uri},
                "position":{"line":line,"character":character}
            }),
        ),
    )
    .await
    .expect("the native server answers hover")
    .expect("the native hover succeeds")
}

async fn hover_before_and_after_probes(
    root: &std::path::Path,
    mut config: GenericLspConfig,
    path: &std::path::Path,
    language_id: &str,
    text: &str,
    line: u32,
    character: u32,
) {
    config.health_probe_interval = Some(Duration::from_millis(100));
    #[cfg(unix)]
    let pid_file = root.join("native-server.pid");
    #[cfg(unix)]
    {
        let command = std::mem::replace(&mut config.command, "/bin/sh".to_string());
        let args = std::mem::take(&mut config.args);
        config.args = vec![
            "-c".to_string(),
            "printf '%s\\n' \"$$\" > \"$PROD_CODE_NATIVE_PID_FILE\"; exec \"$@\"".to_string(),
            "owned-native-server".to_string(),
            command,
        ];
        config.args.extend(args);
        config.env.insert(
            "PROD_CODE_NATIVE_PID_FILE".to_string(),
            pid_file.to_string_lossy().into_owned(),
        );
    }
    let engine = GenericLspEngine::spawn(root, config)
        .await
        .expect("the required native language server initializes");
    let uri = url::Url::from_file_path(path)
        .expect("file URI")
        .to_string();
    engine
        .send_notification(
            "textDocument/didOpen",
            serde_json::json!({"textDocument":{"uri":uri,"languageId":language_id,"version":1,"text":text}}),
        )
        .await
        .expect("the native server accepts an open document");
    let before = hover(&engine, &uri, line, character).await;
    assert!(before.get("error").is_none(), "{before}");
    assert!(
        before.get("result").is_some_and(|result| !result.is_null()),
        "native hover is nonempty before probes: {before}"
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while engine.busy().is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the native server becomes idle before probing");
    tokio::time::timeout(Duration::from_secs(30), async {
        while engine.health_probe_completions() < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the native server answers at least two scheduled probes");
    let after = hover(&engine, &uri, line, character).await;
    assert!(after.get("error").is_none(), "{after}");
    assert!(
        after.get("result").is_some_and(|result| !result.is_null()),
        "native hover is nonempty after at least two scheduled probes: {after}"
    );
    assert_eq!(
        std::fs::read_to_string(path).expect("fixture remains readable"),
        text,
        "health probing leaves the fixture source hash/content unchanged"
    );
    #[cfg(unix)]
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .expect("owned native PID")
        .trim()
        .parse()
        .expect("native PID is numeric");
    drop(engine);
    #[cfg(unix)]
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let status = std::process::Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("inspect the exact owned native PID");
            if !status.success() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the adapter reaps its exact native child");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_basedpyright_initializes_and_answers_a_simple_query() {
    let config = GenericLspConfig::for_python();
    if !engine_is_available(
        "installed_basedpyright_initializes_and_answers_a_simple_query",
        "basedpyright-langserver",
        config.command == "basedpyright-langserver",
    ) {
        return;
    }
    let dir = tempfile::tempdir().expect("isolated workspace");
    let path = dir.path().join("answer.py");
    let text = "def answer() -> int:\n    return 42\n";
    std::fs::write(&path, text).expect("fixture");
    hover_before_and_after_probes(dir.path(), config, &path, "python", text, 0, 5).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_clangd_initializes_and_answers_a_simple_query() {
    let config = GenericLspConfig::for_cpp_validation();
    if !engine_is_available(
        "installed_clangd_initializes_and_answers_a_simple_query",
        "clangd",
        prod_code_engine_generic::which_bin("clangd").is_ok(),
    ) {
        return;
    }
    let dir = tempfile::tempdir().expect("isolated workspace");
    let build = dir.path().join("build");
    std::fs::create_dir(&build).expect("build directory");
    let path = dir.path().join("answer.cpp");
    let text = "int answer() { return 42; }\n";
    std::fs::write(&path, text).expect("fixture");
    let command = format!(
        "[{{\"directory\":\"{}\",\"file\":\"{}\",\"command\":\"clang++ -c answer.cpp\"}}]",
        dir.path().display(),
        path.display()
    );
    std::fs::write(build.join("compile_commands.json"), command).expect("compile commands");
    hover_before_and_after_probes(dir.path(), config, &path, "cpp", text, 0, 5).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_native_typescript_initializes_and_answers_before_and_after_probes() {
    let config = GenericLspConfig::for_typescript();
    let native =
        config.args == ["--lsp", "--stdio"] && std::path::Path::new(&config.command).is_file();
    if !engine_is_available(
        "installed_native_typescript_initializes_and_answers_before_and_after_probes",
        "native TypeScript tsc --lsp",
        native,
    ) {
        return;
    }
    let dir = tempfile::tempdir().expect("isolated workspace");
    let path = dir.path().join("answer.ts");
    let text = "function answer(): number { return 42; }\nanswer();\n";
    std::fs::write(&path, text).expect("fixture");
    hover_before_and_after_probes(dir.path(), config, &path, "typescript", text, 0, 10).await;
}
