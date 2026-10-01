use prod_code_mcp::diagnostics::{is_swift_cross_target_candidate, swift_target_name};
use prod_code_mcp::verify::{detect_tools, plan_command_with, VerifyKind};
use std::path::Path;

#[test]
fn swift_cross_target_candidate_detection() {
    let positive_samples = [
        "value of type 'RemoteTerminalSurfaceCache.Configuration' has no member 'sessionIdentity'",
        "cannot find type 'NewTerminalClass' in scope",
        "cannot find 'activeSessionToken' in scope",
        "module 'ProdApp' has no member named 'TakoSurface'",
        "'Configuration' is not a member type of class 'ProdUI.SurfaceCache'",
        "extra argument 'sessionIdentity' in call",
        "incorrect argument label in call (have 'sessionIdentity:', expected 'id:')",
        "missing argument for parameter 'sessionIdentity' in call",
    ];

    for sample in positive_samples {
        assert!(
            is_swift_cross_target_candidate(sample),
            "Expected '{sample}' to be classified as a Swift cross-target candidate error"
        );
    }

    let negative_samples = [
        "expected '}' in class declaration",
        "consecutive statements on a line must be separated by ';'",
        "cannot convert value of type 'String' to specified type 'Int'",
        "missing return in a function expected to return 'Bool'",
    ];

    for sample in negative_samples {
        assert!(
            !is_swift_cross_target_candidate(sample),
            "Expected '{sample}' NOT to be classified as a Swift cross-target candidate error"
        );
    }
}

#[test]
fn swift_target_name_resolution() {
    assert_eq!(
        swift_target_name(Path::new("clients/macos/ProdUI/Sources/ProdApp/WorkspaceTerminalView.swift")),
        Some("ProdApp".to_string())
    );
    assert_eq!(
        swift_target_name(Path::new("clients/macos/ProdUI/Sources/ProdUI/ProdTerminalSurfacePolicy.swift")),
        Some("ProdUI".to_string())
    );
    assert_eq!(
        swift_target_name(Path::new("clients/macos/ProdUI/Tests/ProdUITests/RemoteTerminalSurfaceCacheTests.swift")),
        Some("ProdUITests".to_string())
    );
    assert_eq!(
        swift_target_name(Path::new("Package.swift")),
        None
    );
    assert_eq!(
        swift_target_name(Path::new("docs/architecture.md")),
        None
    );
}

#[test]
fn swift_check_command_includes_build_tests() {
    let tools = detect_tools(Path::new("."));
    let cmd = plan_command_with(&tools, "swift", VerifyKind::Check, None)
        .expect("plan swift check command");
    assert_eq!(
        cmd,
        vec!["swift", "build", "--build-tests"],
        "Swift check command must build test targets alongside source targets"
    );
}
