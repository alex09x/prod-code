//! Loading ONNX Runtime, in a process of its own: once a library is loaded, `ort` keeps it for
//! the life of the process, so a test in the library's own test binary could not see the load
//! fail after another test had loaded the real one.

use prod_code_gateway::embed::{RUNTIME_LIBRARY, open_runtime};
use std::path::Path;

#[test]
fn a_missing_or_broken_runtime_library_is_an_error_not_a_panic() {
    let missing = open_runtime(Path::new("/no/such/libonnxruntime.so")).unwrap_err();
    assert!(missing.contains("no ONNX Runtime library"), "{missing}");
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join(RUNTIME_LIBRARY);
    std::fs::write(&broken, b"not a shared library").unwrap();
    let err = open_runtime(&broken).unwrap_err();
    assert!(err.starts_with("loading "), "{err}");
}
