//! The editor proxy must treat one malformed child frame as the end of that editor session.

use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::editor_proxy::{run, EditorServers, ServerCommand};
use prod_code_protocol::{PathTranslator, ProdCodeCodec, WireMessage};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

const WAIT: Duration = Duration::from_secs(3);

fn fake_editor(dir: &Path, frame: &str, close_stdout: bool) -> (ServerCommand, std::path::PathBuf) {
    let program = dir.join("malformed-editor.sh");
    let pid = dir.join("editor.pid");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$EDITOR_PID\"\nprintf '{frame}'\n{}while IFS= read -r _; do :; done\n",
            if close_stdout { "exec 1>&-\n" } else { "" },
        ),
    )
    .expect("write fake editor");
    let mut permissions = std::fs::metadata(&program)
        .expect("fake editor metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&program, permissions).expect("make fake editor executable");
    (
        ServerCommand {
            program: program.to_string_lossy().into_owned(),
            args: Vec::new(),
            env: vec![("EDITOR_PID".to_string(), pid.to_string_lossy().into_owned())],
        },
        pid,
    )
}

async fn wait_for_pid(path: &Path) -> i32 {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Ok(pid) = std::fs::read_to_string(path)
            && let Ok(pid) = pid.trim().parse()
        {
            return pid;
        }
        assert!(tokio::time::Instant::now() < deadline, "fake editor never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_exit(pid: i32) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "owned fake editor {pid} survived");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn malformed_stdout_closes_session(frame: &str, close_stdout: bool) {
    let temp = tempfile::tempdir().expect("temporary editor root");
    let root = temp.path().to_path_buf();
    let (command, pid_path) = fake_editor(&root, frame, close_stdout);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
    let addr = listener.local_addr().expect("proxy address");
    let servers = Arc::new(EditorServers::default());
    let session_servers = Arc::clone(&servers);
    let session_root = root.clone();
    let session = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept editor");
        run(
            Framed::new(socket, ProdCodeCodec::new()),
            PathTranslator::new("/client", &session_root.to_string_lossy()),
            command,
            &session_root,
            session_servers.as_ref(),
            57,
        )
        .await
    });
    let mut editor = Framed::new(
        TcpStream::connect(addr).await.expect("connect editor"),
        ProdCodeCodec::new(),
    );
    let pid = wait_for_pid(&pid_path).await;
    let received = tokio::time::timeout(WAIT, editor.next())
        .await
        .expect("malformed stdout must close the editor connection");
    if received.is_some() {
        let _ = editor
            .send(WireMessage::Disconnect {
                reason: "test cleanup".to_string(),
            })
            .await;
    }
    drop(editor);
    tokio::time::timeout(WAIT, session)
        .await
        .expect("editor session finishes")
        .expect("editor session task")
        .expect("editor session result");
    assert_eq!(servers.count(), 0, "closed session unregisters its editor");
    wait_for_exit(pid).await;
    assert!(
        received.is_none(),
        "malformed stdout must close instead of forwarding a payload: {received:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_editor_stdout_closes_the_owned_session_without_lossy_text() {
    for (frame, close_stdout) in [
        ("Content-Length: 2\\r\\nContent-Length: 2\\r\\n\\r\\n{}", false),
        ("Content-Length: 268435457\\r\\n\\r\\n", false),
        ("Content-Length: 2\\r\\n\\r\\n{", true),
        ("Content-Length: 1\\r\\n\\r\\n\\377", false),
    ] {
        malformed_stdout_closes_session(frame, close_stdout).await;
    }
}
