#![cfg(unix)]

//! The editor proxy must treat one malformed child frame as the end of that editor session.

use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::editor_proxy::{EditorServers, ServerCommand, run};
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

async fn wait_for_pid(path: &Path) -> Result<i32, String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Ok(pid) = std::fs::read_to_string(path)
            && let Ok(pid) = pid.trim().parse()
        {
            return Ok(pid);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("fake editor never started".to_string());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_exit(pid: i32) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("owned fake editor {pid} survived"));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn finish_session(
    session: &mut tokio::task::JoinHandle<anyhow::Result<()>>,
) -> Result<(), String> {
    match tokio::time::timeout(WAIT, &mut *session).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => Err(format!("editor session returned an error: {error:#}")),
        Ok(Err(error)) => Err(format!("editor session task failed: {error}")),
        Err(_) => {
            session.abort();
            match tokio::time::timeout(WAIT, &mut *session).await {
                Ok(Err(error)) if error.is_cancelled() => {
                    Err("editor session timed out and was cancelled".to_string())
                }
                Ok(Ok(Ok(()))) => Err("editor session completed after timing out".to_string()),
                Ok(Ok(Err(error))) => {
                    Err(format!("editor session failed after timing out: {error:#}"))
                }
                Ok(Err(error)) => Err(format!(
                    "editor session task failed after cancellation: {error}"
                )),
                Err(_) => Err("editor session did not cancel".to_string()),
            }
        }
    }
}

async fn retire_owned_child(pid: i32) -> Result<(), String> {
    if let Err(wait_error) = wait_for_exit(pid).await {
        let signal = unsafe { libc::kill(pid, libc::SIGKILL) };
        if signal == -1 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            return Err(format!(
                "{wait_error}; could not kill owned fake editor {pid}"
            ));
        }
        wait_for_exit(pid)
            .await
            .map_err(|kill_error| format!("{wait_error}; {kill_error}"))?;
        return Err(wait_error);
    }
    Ok(())
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
    let mut session = tokio::spawn(async move {
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
    let mut editor = None;
    let mut pid = None;
    let mut received = None;
    let outcome = async {
        editor = Some(Framed::new(
            TcpStream::connect(addr)
                .await
                .map_err(|error| format!("connect editor: {error}"))?,
            ProdCodeCodec::new(),
        ));
        pid = Some(wait_for_pid(&pid_path).await?);
        let read = match editor.as_mut() {
            Some(editor) => tokio::time::timeout(WAIT, editor.next())
                .await
                .map_err(|_| "malformed stdout did not close the editor connection".to_string())?,
            None => return Err("editor connection was not retained".to_string()),
        };
        received = Some(read);
        if received.as_ref().is_some_and(|read| read.is_some())
            && let Some(editor) = editor.as_mut()
        {
            let _ = editor
                .send(WireMessage::Disconnect {
                    reason: "test cleanup".to_string(),
                })
                .await;
        }
        Ok(())
    }
    .await;
    drop(editor);
    let session_cleanup = finish_session(&mut session).await;
    let child_cleanup = match pid {
        Some(pid) => retire_owned_child(pid).await,
        None => Ok(()),
    };
    let mut failures = Vec::new();
    if let Err(error) = outcome {
        failures.push(error);
    }
    if let Err(error) = session_cleanup {
        failures.push(error);
    }
    if let Err(error) = child_cleanup {
        failures.push(error);
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
    assert_eq!(servers.count(), 0, "closed session unregisters its editor");
    assert!(
        matches!(received, Some(None)),
        "malformed stdout must close instead of forwarding a payload: {received:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_editor_stdout_closes_the_owned_session_without_lossy_text() {
    for (frame, close_stdout) in [
        (
            "Content-Length: 2\\r\\nContent-Length: 2\\r\\n\\r\\n{}",
            false,
        ),
        ("Content-Length: 268435457\\r\\n\\r\\n", false),
        ("Content-Length: 2\\r\\n\\r\\n{", true),
        ("Content-Length: 1\\r\\n\\r\\n\\377", false),
    ] {
        malformed_stdout_closes_session(frame, close_stdout).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cleanup_reports_a_child_that_needed_forced_retirement() {
    let mut child = tokio::process::Command::new("sleep")
        .arg("30")
        .kill_on_drop(true)
        .spawn()
        .expect("start an owned, deliberately surviving child");
    let pid = i32::try_from(child.id().expect("owned child pid")).expect("PID fits i32");
    let mut reap = tokio::spawn(async move { child.wait().await });
    let outcome = retire_owned_child(pid).await;
    let reaped = tokio::time::timeout(WAIT, &mut reap).await;
    if reaped.is_err() {
        reap.abort();
        let _ = tokio::time::timeout(WAIT, &mut reap).await;
    }
    assert!(
        matches!(&reaped, Ok(Ok(Ok(status))) if !status.success()),
        "the cleanup must reap its forcibly retired child: {reaped:?}"
    );
    assert!(
        outcome.is_err(),
        "forcing cleanup must report the original child-retirement failure"
    );
}
