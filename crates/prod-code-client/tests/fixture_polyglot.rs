use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::json;
use std::net::SocketAddr;
use std::process::Output;

fn tool_text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| {
            let McpContentItem::Text { text } = item;
            text.as_str()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn cli(ws: &Workspace, remote: SocketAddr, args: &[&str]) -> Output {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
    cmd.current_dir(ws.root())
        .arg(format!("--remote={remote}"))
        .args(args);
    cmd.output().await.expect("execute prod-code CLI")
}

#[tokio::test]
async fn polyglot_go_fixture_and_mock_generation() {
    let ws = Workspace::new(&[
        (
            "config.go",
            "package main\n\ntype ServerConfig struct {\n\tHost string\n\tPort int\n\tTLS bool\n}\n",
        ),
        (
            "service.go",
            "package main\n\ntype UserService interface {\n\tGetUser(id string) (string, error)\n\tDeleteUser(id string) error\n}\n",
        ),
    ]);

    let config_path = ws.path("config.go");
    let service_path = ws.path("service.go");
    let c = config_path.clone();
    let s = service_path.clone();

    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
            if query.contains("ServerConfig") {
                json!([answers::symbol("ServerConfig", 23, &c, 3, 5)])
            } else if query.contains("UserService") {
                json!([answers::symbol("UserService", 11, &s, 3, 5)])
            } else {
                json!([])
            }
        }
        "textDocument/documentSymbol" => json!([]),
        _ => json!(null),
    })
    .await;

    // Test Go struct fixture via MCP
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "ServerConfig", "verify": false, "path": "config.go"}),
    )
    .await
    .unwrap();
    let text = tool_text(&res);
    assert!(!res.is_error, "{text}");
    assert!(text.contains("fixture for `ServerConfig`"), "{text}");
    assert!(text.contains("```go"), "{text}");
    assert!(text.contains("var serverConfig = ServerConfig{"), "{text}");
    assert!(text.contains("Host: \"\","), "{text}");
    assert!(text.contains("Port: 0,"), "{text}");

    // Test Go struct fixture with randomized data
    let res_rand = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "ServerConfig", "randomized": true, "verify": false, "path": "config.go"}),
    )
    .await
    .unwrap();
    let rand_text = tool_text(&res_rand);
    assert!(rand_text.contains("Host: \"sample_Host\","), "{rand_text}");
    assert!(rand_text.contains("Port: 8080,"), "{rand_text}");
    assert!(rand_text.contains("TLS: true,"), "{rand_text}");

    // Test Go interface mock generation
    let res_mock = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "UserService", "mock": true, "verify": false, "path": "service.go"}),
    )
    .await
    .unwrap();
    let mock_text = tool_text(&res_mock);
    assert!(mock_text.contains("mock for `UserService`"), "{mock_text}");
    assert!(mock_text.contains("type MockUserService struct"), "{mock_text}");
    assert!(mock_text.contains("GetUserFunc func(id string) (string, error)"), "{mock_text}");
    assert!(mock_text.contains("DeleteUserFunc func(id string) error"), "{mock_text}");
    assert!(mock_text.contains("func (m *MockUserService) GetUser(id string) (string, error)"), "{mock_text}");
    assert!(mock_text.contains("m.Calls = append(m.Calls, \"GetUser\")"), "{mock_text}");

    // Test CLI integration
    let cli_out = cli(
        &ws,
        gw.addr(),
        &["fixture", "ServerConfig", "--no-verify", "--path", "config.go"],
    )
    .await;
    assert!(cli_out.status.success());
    let cli_text = stdout(&cli_out);
    assert!(cli_text.contains("var serverConfig = ServerConfig{"));

    // Test CLI with --randomized
    let cli_rand = cli(
        &ws,
        gw.addr(),
        &["fixture", "ServerConfig", "--randomized", "--no-verify", "--path", "config.go"],
    )
    .await;
    assert!(cli_rand.status.success());
    let cli_rand_text = stdout(&cli_rand);
    assert!(cli_rand_text.contains("Port: 8080,"));

    // Test CLI with --mock
    let cli_mock = cli(
        &ws,
        gw.addr(),
        &["fixture", "UserService", "--mock", "--no-verify", "--path", "service.go"],
    )
    .await;
    assert!(cli_mock.status.success());
    let cli_mock_text = stdout(&cli_mock);
    assert!(cli_mock_text.contains("type MockUserService struct"));
}

#[tokio::test]
async fn polyglot_ts_fixture_and_mock_generation() {
    let ws = Workspace::new(&[
        (
            "user.ts",
            "export interface UserProfile {\n    id: string;\n    email: string;\n    age: number;\n    active: boolean;\n}\n",
        ),
        (
            "service.ts",
            "export interface CacheService {\n    get(key: string): Promise<string>;\n    set(key: string, val: string): Promise<void>;\n}\n",
        ),
    ]);

    let user_path = ws.path("user.ts");
    let service_path = ws.path("service.ts");
    let u = user_path.clone();
    let s = service_path.clone();

    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
            if query.contains("UserProfile") {
                json!([answers::symbol("UserProfile", 11, &u, 1, 17)])
            } else if query.contains("CacheService") {
                json!([answers::symbol("CacheService", 11, &s, 1, 17)])
            } else {
                json!([])
            }
        }
        "textDocument/documentSymbol" => json!([]),
        _ => json!(null),
    })
    .await;

    // TypeScript fixture with randomized dummy values
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "UserProfile", "randomized": true, "verify": false, "path": "user.ts"}),
    )
    .await
    .unwrap();
    let text = tool_text(&res);
    assert!(!res.is_error, "{text}");
    assert!(text.contains("const userProfile: UserProfile = {"), "{text}");
    assert!(text.contains("id: \"id_9823\","), "{text}");
    assert!(text.contains("email: \"user@example.com\","), "{text}");
    assert!(text.contains("age: 30,"), "{text}");
    assert!(text.contains("active: true,"), "{text}");

    // TypeScript mock generation
    let res_mock = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "CacheService", "mock": true, "verify": false, "path": "service.ts"}),
    )
    .await
    .unwrap();
    let mock_text = tool_text(&res_mock);
    assert!(mock_text.contains("export class MockCacheService implements CacheService"), "{mock_text}");
    assert!(mock_text.contains("public getHandler?: (key: string) => Promise<string>;"), "{mock_text}");
    assert!(mock_text.contains("async get(key: string): Promise<string>"), "{mock_text}");
    assert!(mock_text.contains("this.calls.push({ method: \"get\", args: [key] })"), "{mock_text}");
    assert!(mock_text.contains("export const createMockCacheService"), "{mock_text}");
}

#[tokio::test]
async fn polyglot_python_fixture_and_mock_generation() {
    let ws = Workspace::new(&[
        (
            "models.py",
            "from dataclasses import dataclass\n\n@dataclass\nclass Config:\n    host: str\n    port: int\n    debug: bool\n",
        ),
        (
            "service.py",
            "class Storage(Protocol):\n    def save(self, key: str, val: bytes) -> bool:\n        ...\n",
        ),
    ]);

    let models_path = ws.path("models.py");
    let service_path = ws.path("service.py");
    let m = models_path.clone();
    let s = service_path.clone();

    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
            if query.contains("Config") {
                json!([answers::symbol("Config", 5, &m, 4, 7)])
            } else if query.contains("Storage") {
                json!([answers::symbol("Storage", 5, &s, 1, 7)])
            } else {
                json!([])
            }
        }
        "textDocument/documentSymbol" => json!([]),
        _ => json!(null),
    })
    .await;

    // Python dataclass fixture
    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "Config", "verify": false, "path": "models.py"}),
    )
    .await
    .unwrap();
    let text = tool_text(&res);
    assert!(!res.is_error, "{text}");
    assert!(text.contains("config = Config("), "{text}");
    assert!(text.contains("host=\"\","), "{text}");
    assert!(text.contains("port=0,"), "{text}");
    assert!(text.contains("debug=False,"), "{text}");

    // Python mock generation
    let res_mock = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "Storage", "mock": true, "verify": false, "path": "service.py"}),
    )
    .await
    .unwrap();
    let mock_text = tool_text(&res_mock);
    assert!(mock_text.contains("class MockStorage:"), "{mock_text}");
    assert!(mock_text.contains("def save(self, key: str, val: bytes) -> bool:"), "{mock_text}");
    assert!(mock_text.contains("self.calls.append((\"save\", (key, val), {}))"), "{mock_text}");
}

#[tokio::test]
async fn polyglot_swift_fixture_and_mock_generation() {
    let ws = Workspace::new(&[
        (
            "Config.swift",
            "struct AppConfig {\n    var host: String\n    var port: Int\n    var secure: Bool\n}\n",
        ),
        (
            "Service.swift",
            "protocol NetworkService {\n    func fetch(url: String) -> String\n}\n",
        ),
    ]);

    let c = ws.path("Config.swift");
    let s = ws.path("Service.swift");

    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
            if query.contains("AppConfig") {
                json!([answers::symbol("AppConfig", 23, &c, 1, 8)])
            } else if query.contains("NetworkService") {
                json!([answers::symbol("NetworkService", 11, &s, 1, 10)])
            } else {
                json!([])
            }
        }
        "textDocument/documentSymbol" => json!([]),
        _ => json!(null),
    })
    .await;

    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "AppConfig", "verify": false, "path": "Config.swift"}),
    )
    .await
    .unwrap();
    let text = tool_text(&res);
    assert!(text.contains("let appConfig = AppConfig("), "{text}");
    assert!(text.contains("host: \"\","), "{text}");
    assert!(text.contains("port: 0,"), "{text}");
    assert!(text.contains("secure: false"), "{text}");

    let res_mock = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "NetworkService", "mock": true, "verify": false, "path": "Service.swift"}),
    )
    .await
    .unwrap();
    let mock_text = tool_text(&res_mock);
    assert!(mock_text.contains("final class MockNetworkService: NetworkService"), "{mock_text}");
    assert!(mock_text.contains("func fetch(url: String) -> String"), "{mock_text}");
    assert!(mock_text.contains("calls.append(\"fetch\")"), "{mock_text}");
}

#[tokio::test]
async fn polyglot_cpp_fixture_and_mock_generation() {
    let ws = Workspace::new(&[
        (
            "config.hpp",
            "struct DBConfig {\n    std::string host;\n    int port;\n    bool ssl;\n};\n",
        ),
        (
            "service.hpp",
            "class Executor {\npublic:\n    virtual void execute(int code) = 0;\n};\n",
        ),
    ]);

    let c = ws.path("config.hpp");
    let s = ws.path("service.hpp");

    let gw = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            let query = params.get("query").and_then(|q| q.as_str()).unwrap_or("");
            if query.contains("DBConfig") {
                json!([answers::symbol("DBConfig", 23, &c, 1, 8)])
            } else if query.contains("Executor") {
                json!([answers::symbol("Executor", 5, &s, 1, 7)])
            } else {
                json!([])
            }
        }
        "textDocument/documentSymbol" => json!([]),
        _ => json!(null),
    })
    .await;

    let res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "DBConfig", "randomized": true, "verify": false, "path": "config.hpp"}),
    )
    .await
    .unwrap();
    let text = tool_text(&res);
    assert!(text.contains("DBConfig dBConfig = DBConfig{"), "{text}");
    assert!(text.contains(".host = \"sample_host\","), "{text}");
    assert!(text.contains(".port = 8080,"), "{text}");
    assert!(text.contains(".ssl = true,"), "{text}");

    let res_mock = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol": "Executor", "mock": true, "verify": false, "path": "service.hpp"}),
    )
    .await
    .unwrap();
    let mock_text = tool_text(&res_mock);
    assert!(mock_text.contains("class MockExecutor : public Executor"), "{mock_text}");
    assert!(mock_text.contains("void execute(int code) override"), "{mock_text}");
    assert!(mock_text.contains("calls.push_back(\"execute\");"), "{mock_text}");
}

