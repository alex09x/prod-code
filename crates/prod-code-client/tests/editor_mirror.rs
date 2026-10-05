use futures_util::{SinkExt, StreamExt};
use prod_code_client::editor_files::RemoteFiles;
use prod_code_protocol::{ProdCodeCodec, ReadFileResponse, WireMessage, path::file_uri};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::codec::Framed;

struct Node {
    addr: SocketAddr,
    reads: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Node {
    async fn start(files: HashMap<String, String>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&reads);
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(WireMessage::ReadFileRequest(request))) = framed.next().await {
                    count.fetch_add(1, Ordering::Relaxed);
                    let content = files
                        .get(&request.path)
                        .map(|text| text.as_bytes().to_vec());
                    let error = content.is_none().then(|| "absent fixture file".to_string());
                    framed
                        .send(WireMessage::ReadFileResponse(ReadFileResponse {
                            path: request.path,
                            content,
                            truncated: false,
                            is_executable: Some(false),
                            error,
                        }))
                        .await
                        .unwrap();
                }
            }
        });
        Self { addr, reads, task }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[test]
fn outgoing_mirror_locations_preserve_document_text() {
    let cache = tempfile::tempdir().unwrap();
    let files = RemoteFiles::new(
        "127.0.0.1:9400".parse().unwrap(),
        Path::new("/client"),
        Path::new("/node/project"),
        cache.path(),
    );
    let original = Path::new("/node/source.rs");
    let local = files.mirror_path(original).unwrap();
    let source = format!("const CACHE: &str = {:?};", local.to_str().unwrap());
    for method in ["textDocument/didOpen", "textDocument/didChange"] {
        let message = json!({"method":method,"params":{
            "textDocument":{"uri":file_uri(&local),"text":source},
            "contentChanges":[{"text":file_uri(&local)}]
        }});
        let mapped: Value = serde_json::from_str(&files.to_node(&message.to_string())).unwrap();
        assert_eq!(mapped["params"]["textDocument"]["uri"], file_uri(original));
        assert_eq!(
            mapped["params"]["textDocument"]["text"], source,
            "source bytes changed during {method}"
        );
        assert_eq!(
            mapped["params"]["contentChanges"][0]["text"],
            file_uri(&local)
        );
    }
}

#[test]
fn outgoing_mirror_preserves_sibling_paths_and_invalid_json() {
    let cache = tempfile::tempdir().unwrap();
    let files = RemoteFiles::new(
        "127.0.0.1:9400".parse().unwrap(),
        Path::new("/client"),
        Path::new("/node/project"),
        cache.path(),
    );
    let mirror = files.mirror_path(Path::new("/")).unwrap();
    let sibling = format!(
        "{}-other/source.rs",
        mirror.display().to_string().trim_end_matches('/')
    );
    let message =
        json!({"params":{"uri":file_uri(Path::new(&sibling)),"arguments":[sibling]}}).to_string();
    assert_eq!(
        files.to_node(&message),
        message,
        "a component prefix is not a mirror descendant"
    );
    let invalid = format!("not JSON: {}", mirror.display());
    assert_eq!(files.to_node(&invalid), invalid);
}

#[test]
fn encoded_mirror_uris_round_trip_to_the_node() {
    let cache = tempfile::tempdir().unwrap();
    let special = cache.path().join("cache # 100% λ");
    let files = RemoteFiles::new(
        "127.0.0.1:9400".parse().unwrap(),
        Path::new("/client"),
        Path::new("/node/project"),
        &special,
    );
    let original = Path::new("/node/lib name%41#λ.rs");
    let local = files.mirror_path(original).unwrap();
    let message =
        json!({"method":"textDocument/hover","params":{"textDocument":{"uri":file_uri(&local)}}});
    let mapped: Value = serde_json::from_str(&files.to_node(&message.to_string())).unwrap();
    assert_eq!(mapped["params"]["textDocument"]["uri"], file_uri(original));
}

#[tokio::test]
async fn incoming_text_fields_are_neither_fetched_nor_rewritten() {
    let node = Node::start(HashMap::from([(
        "/node/source.rs".to_string(),
        "remote bytes".to_string(),
    )]))
    .await;
    let checkout = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let files = RemoteFiles::new(
        node.addr,
        checkout.path(),
        Path::new("/node/project"),
        cache.path(),
    );
    let uri = "file:///node/source.rs";
    let message = json!({"result":{
        "text":uri,"newText":uri,"insertText":uri,"filterText":uri,"sortText":uri,
        "documentation":{"kind":"markdown","value":uri},"contents":uri,
        "message":uri,"label":uri,"detail":uri,"title":uri,"tooltip":uri
    }})
    .to_string();
    assert_eq!(files.to_editor(message.clone()).await, message);
    assert_eq!(
        node.reads.load(Ordering::Relaxed),
        0,
        "source/documentation caused a remote file read"
    );
    assert!(!files.mirror_path(Path::new("/node/source.rs")).unwrap().exists());
}

#[tokio::test]
async fn rebalance_retargets_external_file_reads_to_the_new_node() {
    let old_node = Node::start(HashMap::new()).await;
    let new_root = Path::new("/server/new-workspace");
    let new_node_path = new_root.join("src/only-node.rs");
    let new_node = Node::start(HashMap::from([(
        new_node_path.to_string_lossy().into_owned(),
        "new node bytes".to_string(),
    )]))
    .await;
    let checkout = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let files = RemoteFiles::new(
        old_node.addr,
        checkout.path(),
        Path::new("/server/old-workspace"),
        cache.path(),
    );
    files.set_node(new_node.addr, new_root);

    let client_path = checkout.path().join("src/only-node.rs");
    let message = json!({"result":{"location":{"uri":file_uri(&client_path)}}});
    let shown: Value = serde_json::from_str(&files.to_editor(message.to_string()).await).unwrap();
    let local_copy = files.mirror_path(&new_node_path).unwrap();
    let expected_uri = file_uri(&local_copy);

    assert_eq!(shown["result"]["location"]["uri"], expected_uri);
    assert_eq!(std::fs::read(&local_copy).unwrap(), b"new node bytes");
    assert_eq!(old_node.reads.load(Ordering::Relaxed), 0);
    assert_eq!(new_node.reads.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn mirror_maps_workspace_edit_keys_arguments_and_label_locations() {
    let node_path = Path::new("/node/source # 100% λ.rs");
    let uri = file_uri(node_path);
    let node = Node::start(HashMap::from([(
        node_path.to_str().unwrap().to_string(),
        "remote bytes".to_string(),
    )]))
    .await;
    let checkout = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let files = RemoteFiles::new(
        node.addr,
        checkout.path(),
        Path::new("/node/project"),
        &cache.path().join("cache # λ"),
    );
    let local = file_uri(&files.mirror_path(node_path).unwrap());
    let message = json!({"result":{
        "changes":{(uri.clone()):[{"range":{},"newText":uri}]},
        "arguments":[uri],
        "label":[{"value":uri,"location":{"uri":uri,"range":{}}}]
    }});
    let shown: Value = serde_json::from_str(&files.to_editor(message.to_string()).await).unwrap();
    assert!(
        shown["result"]["changes"].get(&local).is_some(),
        "WorkspaceEdit URI keys were not mapped: {shown}"
    );
    assert_eq!(shown["result"]["changes"][&local][0]["newText"], uri);
    assert_eq!(shown["result"]["arguments"][0], local);
    assert_eq!(shown["result"]["label"][0]["value"], uri);
    assert_eq!(shown["result"]["label"][0]["location"]["uri"], local);
    assert_eq!(
        node.reads.load(Ordering::Relaxed),
        1,
        "the same file should be fetched once"
    );
    assert_eq!(
        std::fs::read(files.mirror_path(node_path).unwrap()).unwrap(),
        b"remote bytes"
    );
    let back: Value = serde_json::from_str(&files.to_node(&shown.to_string())).unwrap();
    assert_eq!(
        back, message,
        "structured mirror round trip changed the payload"
    );
}

#[tokio::test]
async fn mirror_rejects_path_traversal_locations() {
    let node = Node::start(HashMap::from([(
        "/node/secret.rs".to_string(),
        "forbidden".to_string(),
    )]))
    .await;
    let checkout = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let files = RemoteFiles::new(
        node.addr,
        checkout.path(),
        Path::new("/node/project"),
        cache.path(),
    );

    assert!(files.mirror_path(Path::new("/node/../../etc/passwd")).is_none());
    assert!(files.mirror_path(Path::new("../../../../etc/shadow")).is_none());

    let traversal_uris = [
        "file:///node/../../etc/passwd",
        "file:///../../../../etc/shadow",
        "file:///../../../secret",
    ];
    for uri in traversal_uris {
        let message = json!({"result":{"location":{"uri":uri}}});
        let shown: Value =
            serde_json::from_str(&files.to_editor(message.to_string()).await).unwrap();
        assert_eq!(shown["result"]["location"]["uri"], uri);
    }
    assert_eq!(node.reads.load(Ordering::Relaxed), 0);
}

#[test]
#[cfg(unix)]
fn mirror_write_rejects_symlinks() {
    let cache = tempfile::tempdir().unwrap();
    let target = cache.path().join("sub/file.txt");
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("secret.txt");
    std::fs::write(&outside_file, b"initial").unwrap();

    std::fs::create_dir_all(cache.path().join("sub")).unwrap();
    std::os::unix::fs::symlink(&outside_file, &target).unwrap();

    let res = prod_code_client::editor_files::write_read_only(
        cache.path(),
        &target,
        b"overwritten",
    );
    assert!(res.is_err());
    assert_eq!(std::fs::read(&outside_file).unwrap(), b"initial");
}

#[test]
#[cfg(unix)]
fn mirror_write_rejects_symlink_parent_directory() {
    let cache = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let escaped_parent = cache.path().join("escaped");
    std::os::unix::fs::symlink(outside.path(), &escaped_parent).unwrap();

    let target = escaped_parent.join("file.txt");
    let res = prod_code_client::editor_files::write_read_only(
        cache.path(),
        &target,
        b"overwritten",
    );
    assert!(res.is_err());
    assert!(!outside.path().join("file.txt").exists());
}
