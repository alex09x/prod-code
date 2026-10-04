use prod_code_mcp::replace_constructor::{
    ReplaceMode, find_go_instantiations, find_python_instantiations, find_rust_instantiations,
    find_ts_instantiations, generate_builder_code, generate_factory_code, parse_go_struct_fields,
    parse_python_fields, parse_rust_struct_decl, parse_rust_struct_fields,
    parse_struct_declaration, parse_ts_fields, replace_constructor_with_builder,
    replace_constructor_with_factory, rewrite_instantiation,
};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::fs;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[test]
fn test_rust_parse_struct_and_generate_factory() {
    let code = r#"
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub timeout_ms: u64,
}
"#;
    let decl = parse_rust_struct_decl(code, "ConnectionConfig").unwrap();
    assert_eq!(decl.name, "ConnectionConfig");
    assert!(decl.is_pub);
    assert_eq!(decl.fields.len(), 3);
    assert_eq!(decl.fields[0].name, "host");
    assert_eq!(decl.fields[0].ty, "String");
    assert_eq!(decl.fields[1].name, "port");
    assert_eq!(decl.fields[1].ty, "u16");
    assert_eq!(decl.fields[2].name, "timeout_ms");
    assert_eq!(decl.fields[2].ty, "u64");

    let factory = generate_factory_code(&decl, "new");
    assert!(factory.contains("impl ConnectionConfig {"));
    assert!(factory.contains("pub fn new(host: String, port: u16, timeout_ms: u64) -> Self {"));
    assert!(factory.contains("Self {"));
    assert!(factory.contains("host,"));
    assert!(factory.contains("port,"));
    assert!(factory.contains("timeout_ms,"));
}

#[test]
fn test_rust_parse_struct_and_generate_builder() {
    let code = r#"
pub struct UserProfile {
    pub username: String,
    pub age: u32,
}
"#;
    let decl = parse_rust_struct_decl(code, "UserProfile").unwrap();
    let builder = generate_builder_code(&decl, "UserProfileBuilder");
    assert!(builder.contains("pub struct UserProfileBuilder {"));
    assert!(builder.contains("username: Option<String>,"));
    assert!(builder.contains("age: Option<u32>,"));
    assert!(builder.contains("pub fn username(mut self, value: String) -> Self"));
    assert!(builder.contains("pub fn age(mut self, value: u32) -> Self"));
    assert!(builder.contains("pub fn build(self) -> UserProfile"));
    assert!(builder.contains("pub fn builder() -> UserProfileBuilder"));
}

#[test]
fn test_rust_instantiation_rewriting_preserves_declared_order() {
    let code = r#"
struct Point {
    x: i32,
    y: i32,
}

fn test() {
    let p1 = Point { x: 10, y: 20 };
    let p2 = Point { y: 40, x: 30 }; // reversed order in literal
    let x = 5;
    let y = 6;
    let p3 = Point { x, y }; // shorthand
}
"#;
    let decl = parse_rust_struct_decl(code, "Point").unwrap();
    let (sites, blocked) = find_rust_instantiations(code, "Point", decl.decl_start, decl.decl_end);
    assert!(blocked.is_empty());
    assert_eq!(sites.len(), 3);

    let rw1 = rewrite_instantiation(&sites[0], &decl, ReplaceMode::Factory, "new").unwrap();
    assert_eq!(rw1, "Point::new(10, 20)");

    let rw2 = rewrite_instantiation(&sites[1], &decl, ReplaceMode::Factory, "new").unwrap();
    // Reversed fields mapped correctly to declared parameter order:
    assert_eq!(rw2, "Point::new(30, 40)");

    let rw3 = rewrite_instantiation(&sites[2], &decl, ReplaceMode::Factory, "new").unwrap();
    assert_eq!(rw3, "Point::new(x, y)");

    let b1 = rewrite_instantiation(&sites[0], &decl, ReplaceMode::Builder, "PointBuilder").unwrap();
    assert_eq!(b1, "Point::builder().x(10).y(20).build()");
}

#[test]
fn test_rust_struct_update_syntax_reported_as_blocked() {
    let code = r#"
struct Widget {
    id: u64,
    name: String,
}

fn main() {
    let old = Widget { id: 1, name: "old".into() };
    let updated = Widget { id: 2, ..old };
}
"#;
    let decl = parse_rust_struct_decl(code, "Widget").unwrap();
    let (sites, blocked) = find_rust_instantiations(code, "Widget", decl.decl_start, decl.decl_end);
    assert_eq!(sites.len(), 1);
    assert_eq!(blocked.len(), 1);
    assert!(blocked[0].contains("struct update syntax `..`"));
}

#[test]
fn test_polyglot_helpers() {
    // Rust fields
    let r_fields = parse_rust_struct_fields("pub a: u32, b: String");
    assert_eq!(r_fields.len(), 2);

    // Go fields & instantiations
    let go_code = "type Item struct {\n    Title string\n    Price float64\n}\nfunc f() {\n    it := &Item{Title: \"Book\", Price: 19.99}\n}";
    let g_decl = parse_struct_declaration(go_code, "Item", "go").unwrap();
    assert_eq!(g_decl.fields.len(), 2);
    let g_fields = parse_go_struct_fields("Title string\nPrice float64");
    assert_eq!(g_fields.len(), 2);
    let g_sites = find_go_instantiations(go_code, "Item", g_decl.decl_start, g_decl.decl_end);
    assert_eq!(g_sites.len(), 1);
    let g_rw =
        rewrite_instantiation(&g_sites[0], &g_decl, ReplaceMode::Factory, "NewItem").unwrap();
    assert_eq!(g_rw, "NewItem(\"Book\", 19.99)");

    // TypeScript fields & instantiations
    let ts_code = "class Greeter {\n    greeting: string;\n    constructor(greeting: string) {\n        this.greeting = greeting;\n    }\n}\nconst g = new Greeter(\"Hello\");";
    let ts_decl = parse_struct_declaration(ts_code, "Greeter", "typescript").unwrap();
    assert_eq!(ts_decl.fields.len(), 1);
    let ts_fields = parse_ts_fields("constructor(name: string, age: number)");
    assert_eq!(ts_fields.len(), 2);
    let ts_sites = find_ts_instantiations(ts_code, "Greeter", ts_decl.decl_start, ts_decl.decl_end);
    assert_eq!(ts_sites.len(), 1);
    let ts_rw =
        rewrite_instantiation(&ts_sites[0], &ts_decl, ReplaceMode::Factory, "create").unwrap();
    assert_eq!(ts_rw, "Greeter.create(\"Hello\")");

    // Python fields & instantiations
    let py_code = "class Order:\n    def __init__(self, id: int, item: str):\n        self.id = id\n        self.item = item\n\no = Order(1, \"Tea\")";
    let py_decl = parse_struct_declaration(py_code, "Order", "python").unwrap();
    assert_eq!(py_decl.fields.len(), 2);
    let py_fields = parse_python_fields("def __init__(self, x: int, y: int): pass");
    assert_eq!(py_fields.len(), 2);
    let py_sites =
        find_python_instantiations(py_code, "Order", py_decl.decl_start, py_decl.decl_end);
    assert_eq!(py_sites.len(), 1);
    let py_rw =
        rewrite_instantiation(&py_sites[0], &py_decl, ReplaceMode::Factory, "create").unwrap();
    assert_eq!(py_rw, "Order.create(1, \"Tea\")");
}

#[tokio::test]
async fn test_end_to_end_replace_constructor_with_factory_rust() {
    let initial_code = r#"pub struct ServerOpts {
    pub host: String,
    pub port: u16,
}

pub fn make_default_server() -> ServerOpts {
    ServerOpts {
        host: "127.0.0.1".to_string(),
        port: 8080,
    }
}
"#;
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", initial_code)]);
    let root = ws.root();
    let lib_rs = root.join("src/lib.rs");

    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let remote = gateway.addr();

    let result = replace_constructor_with_factory(
        remote,
        &root,
        &lib_rs,
        "ServerOpts",
        Some("new"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert!(result.applied);
    assert_eq!(result.instantiations_rewritten, 1);
    assert!(result.blocked.is_empty());

    let modified_code = fs::read_to_string(&lib_rs).unwrap();
    assert!(modified_code.contains("pub fn new(host: String, port: u16) -> Self"));
    assert!(modified_code.contains("ServerOpts::new(\"127.0.0.1\".to_string(), 8080)"));
}

#[tokio::test]
async fn test_end_to_end_replace_constructor_with_builder_rust() {
    let initial_code = r#"pub struct ClientConfig {
    pub endpoint: String,
    pub retries: u32,
}

pub fn make_client() -> ClientConfig {
    ClientConfig {
        endpoint: "https://api.prod.codes".to_string(),
        retries: 3,
    }
}
"#;
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", initial_code)]);
    let root = ws.root();
    let lib_rs = root.join("src/lib.rs");

    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let remote = gateway.addr();

    let result = replace_constructor_with_builder(
        remote,
        &root,
        &lib_rs,
        "ClientConfig",
        Some("ClientConfigBuilder"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert!(result.applied);
    assert_eq!(result.instantiations_rewritten, 1);
    assert!(result.blocked.is_empty());

    let modified_code = fs::read_to_string(&lib_rs).unwrap();
    assert!(modified_code.contains("pub struct ClientConfigBuilder"));
    assert!(modified_code.contains("pub fn builder() -> ClientConfigBuilder"));
    assert!(modified_code.contains("ClientConfig::builder().endpoint(\"https://api.prod.codes\".to_string()).retries(3).build()"));
}

#[tokio::test]
async fn test_mcp_tool_execution_replace_constructor() {
    let initial_code = r#"pub struct Task {
    pub id: u64,
    pub title: String,
}

pub fn make_task() -> Task {
    Task { id: 42, title: "Deliver Milestone 14".to_string() }
}
"#;
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", initial_code)]);
    let root = ws.root();
    let lib_rs = root.join("src/lib.rs");

    let gateway = ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let remote = gateway.addr();

    let args = serde_json::json!({
        "path": "src/lib.rs",
        "type_name": "Task",
        "apply": true,
    });

    let res = prod_code_mcp::tools::execute_tool(
        remote,
        &root,
        "code_replace_constructor_with_factory",
        args,
    )
    .await
    .unwrap();

    assert!(!res.is_error);
    let modified = fs::read_to_string(&lib_rs).unwrap();
    assert!(modified.contains("Task::new(42, \"Deliver Milestone 14\".to_string())"));
}

#[test]
fn rust_constructor_discovery_skips_comments_strings_and_foreign_self() {
    let source = r#"struct Config {
    value: i32,
}

impl Config {
    fn make() -> Self { Self { value: 1 } }
}

impl Other {
    fn make() -> Self { Self { value: 2 } }
}

fn test() {
    let _example = "Config { value: 3 }";
    // Config { value: 4 }
    let _actual = Config { value: 5 };
}
"#;
    let decl = parse_rust_struct_decl(source, "Config").unwrap();
    let (sites, blocked) =
        find_rust_instantiations(source, "Config", decl.decl_start, decl.decl_end);
    assert!(blocked.is_empty(), "{blocked:?}");
    assert_eq!(sites.len(), 2, "{sites:?}");
}

#[test]
fn rust_constructor_rewrite_refuses_side_effect_reordering() {
    let source = r#"struct Pair {
    second: i32,
    first: i32,
}
fn make() -> Pair { Pair { first: observe(), second: mutate() } }
"#;
    let decl = parse_rust_struct_decl(source, "Pair").unwrap();
    let (sites, blocked) = find_rust_instantiations(source, "Pair", decl.decl_start, decl.decl_end);
    assert!(blocked.is_empty());
    let err = rewrite_instantiation(&sites[0], &decl, ReplaceMode::Factory, "new").unwrap_err();
    assert!(err.to_string().contains("ordered differently"), "{err:#}");
}

#[test]
fn go_constructor_rewrite_preserves_omitted_and_positional_field_values() {
    let source = r#"package example
type User struct {
    Name string
    Age int
}
func make() {
    _ = &User{Name: "Ada"}
    _ = User{"Grace", 37}
}
"#;
    let decl = parse_struct_declaration(source, "User", "go").unwrap();
    let sites = find_go_instantiations(source, "User", decl.decl_start, decl.decl_end);
    assert_eq!(sites.len(), 2, "{sites:?}");
    assert_eq!(
        rewrite_instantiation(&sites[0], &decl, ReplaceMode::Factory, "NewUser").unwrap(),
        "NewUser(\"Ada\", 0)"
    );
    assert_eq!(
        rewrite_instantiation(&sites[1], &decl, ReplaceMode::Factory, "NewUser").unwrap(),
        "NewUser(\"Grace\", 37)"
    );
}

#[tokio::test]
async fn javascript_factory_is_inserted_in_class_and_preserves_constructor_arguments() {
    let source = r#"export class Point {
    constructor(public x: number, public y: number) {}
}
const point = new Point(computeX(), 2);
const example = "new Point(3, 4)";
// new Point(5, 6)
"#;
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("point.ts", source)]);
    let root = ws.root();
    let file = root.join("point.ts");
    let gateway = fake_gateway().await;

    let result = replace_constructor_with_factory(
        gateway.addr(),
        &root,
        &file,
        "Point",
        Some("create"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.instantiations_rewritten, 1);
    let content = fs::read_to_string(file).unwrap();
    let class_end = content.find("\n}").expect("class close");
    assert!(content[..class_end].contains("static create(x: number, y: number): Point"));
    assert!(content.contains("Point.create(computeX(), 2)"), "{content}");
    assert!(content.contains("\"new Point(3, 4)\""), "{content}");
    assert!(content.contains("// new Point(5, 6)"), "{content}");
}

#[tokio::test]
async fn javascript_factory_has_no_typescript_return_annotation() {
    let source = "class Greeter { constructor() {} }\nconst g = new Greeter();\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("greeter.js", source)]);
    let root = ws.root();
    let file = root.join("greeter.js");
    let gateway = fake_gateway().await;

    let result = replace_constructor_with_factory(
        gateway.addr(),
        &root,
        &file,
        "Greeter",
        Some("create"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.instantiations_rewritten, 1);
    let content = fs::read_to_string(file).unwrap();
    assert!(content.contains("static create() {"), "{content}");
    assert!(!content.contains("static create(): Greeter"), "{content}");
    assert!(content.contains("const g = Greeter.create();"), "{content}");
}

#[tokio::test]
async fn python_builder_uses_values_saved_by_its_setters() {
    let source = "class Person:\n    name: str\n    def __init__(self, name):\n        self.name = name\nperson = Person(\"Ada\")\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("person.py", source)]);
    let root = ws.root();
    let file = root.join("person.py");
    let gateway = fake_gateway().await;

    let result = replace_constructor_with_builder(
        gateway.addr(),
        &root,
        &file,
        "Person",
        Some("PersonBuilder"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.instantiations_rewritten, 1);
    let content = fs::read_to_string(file).unwrap();
    assert!(
        content.contains("PersonBuilder().name(\"Ada\").build()"),
        "{content}"
    );
    assert!(
        content.contains("return Person(name=self._name)"),
        "{content}"
    );
    assert!(!content.contains("return Person(name=name)"), "{content}");
}

#[tokio::test]
async fn cpp_and_swift_factory_discovery_preserves_constructor_arguments() {
    let cases = [
        (
            "point.cpp",
            "struct Point {\n    int x;\n    int y;\n};\nPoint make() { return Point{computeX(), 2}; }\nconst char* example = \"Point{3, 4}\";\n// Point{5, 6}\n",
            "cpp",
            "Point::create(computeX(), 2)",
        ),
        (
            "point.swift",
            "struct Point {\n    var x: Int\n    var y: Int\n}\nfunc make() -> Point { return Point(x: computeX(), y: 2) }\nlet example = \"Point(x: 3, y: 4)\"\n// Point(x: 5, y: 6)\n",
            "swift",
            "Point.create(x: computeX(), y: 2)",
        ),
    ];
    for (path, source, _, expected_call) in cases {
        let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), (path, source)]);
        let root = ws.root();
        let file = root.join(path);
        let gateway = fake_gateway().await;
        let result = replace_constructor_with_factory(
            gateway.addr(),
            &root,
            &file,
            "Point",
            Some("create"),
            true,
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.instantiations_rewritten, 1, "{path}");
        let content = fs::read_to_string(file).unwrap();
        assert!(content.contains(expected_call), "{path}: {content}");
        let type_close = if path.ends_with(".cpp") {
            content.find("\n};").unwrap()
        } else {
            content.find("\n}").unwrap()
        };
        assert!(
            content[..type_close].contains("static"),
            "{path}: {content}"
        );
        assert!(
            content.contains("\"Point{3, 4}\"") || content.contains("\"Point(x: 3, y: 4)\""),
            "{path}: {content}"
        );
        assert!(content.contains("// Point"), "{path}: {content}");
    }
}

#[tokio::test]
async fn cpp_and_swift_builders_route_constructor_arguments_through_setters() {
    let cases = [
        (
            "point.cpp",
            "struct Point {\n    int x;\n};\nPoint make() { return Point{loadX()}; }\n",
            "PointBuilder{}.x(loadX()).build()",
        ),
        (
            "point.swift",
            "struct Point {\n    var x: Int\n}\nfunc make() -> Point { return Point(x: loadX()) }\n",
            "PointBuilder().setX(loadX()).build()",
        ),
    ];
    for (path, source, expected_call) in cases {
        let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), (path, source)]);
        let root = ws.root();
        let file = root.join(path);
        let gateway = fake_gateway().await;
        let result = replace_constructor_with_builder(
            gateway.addr(),
            &root,
            &file,
            "Point",
            Some("PointBuilder"),
            true,
            false,
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.instantiations_rewritten, 1, "{path}");
        let content = fs::read_to_string(file).unwrap();
        assert!(content.contains(expected_call), "{path}: {content}");
    }
}

#[tokio::test]
async fn typescript_builder_routes_constructor_arguments_through_setters() {
    let source = "export class Person {\n    constructor(public name: string, public age: number) {}\n}\nconst p = new Person(loadName(), 37);\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("person.ts", source)]);
    let root = ws.root();
    let file = root.join("person.ts");
    let gateway = fake_gateway().await;

    let result = replace_constructor_with_builder(
        gateway.addr(),
        &root,
        &file,
        "Person",
        Some("PersonBuilder"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.instantiations_rewritten, 1);
    let content = fs::read_to_string(file).unwrap();
    assert!(
        content.contains("new PersonBuilder().name(loadName()).age(37).build()"),
        "{content}"
    );
}

#[tokio::test]
async fn javascript_builder_routes_assigned_constructor_properties_without_types() {
    let source = "class Person { constructor(name) { this.name = name; } }\nconst p = new Person(loadName());\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("person.js", source)]);
    let root = ws.root();
    let file = root.join("person.js");
    let gateway = fake_gateway().await;

    let result = replace_constructor_with_builder(
        gateway.addr(),
        &root,
        &file,
        "Person",
        Some("PersonBuilder"),
        true,
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.instantiations_rewritten, 1);
    let content = fs::read_to_string(file).unwrap();
    assert!(
        content.contains("new PersonBuilder().name(loadName()).build()"),
        "{content}"
    );
    assert!(content.contains("class PersonBuilder"), "{content}");
    assert!(!content.contains("private _name?"), "{content}");
    assert!(!content.contains("value: "), "{content}");
}
