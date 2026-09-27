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

async fn document_symbols(
    root: &std::path::Path,
    config: GenericLspConfig,
    path: &std::path::Path,
    language_id: &str,
    text: &str,
) {
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
    let answer = tokio::time::timeout(
        Duration::from_secs(30),
        engine.send_request(
            "textDocument/documentSymbol",
            serde_json::json!({"textDocument":{"uri":uri}}),
        ),
    )
    .await
    .expect("the native server answers a simple query")
    .expect("the native server query succeeds");
    assert!(answer.get("error").is_none(), "{answer}");
    assert!(answer.get("result").is_some(), "{answer}");
    drop(engine);
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
    document_symbols(dir.path(), config, &path, "python", text).await;
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
    document_symbols(dir.path(), config, &path, "cpp", text).await;
}
