use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::protocol::{DaemonEvent, UICommand};

/// Capacity for the IPC command mpsc channel.
const IPC_COMMAND_CAPACITY: usize = 64;

/// Default socket path: ~/Library/Application Support/aura/daemon.sock
pub fn default_socket_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("aura")
        .join("daemon.sock")
}

/// Start the IPC server as a background task.
///
/// Accepts Unix socket connections and:
/// - Sends all `DaemonEvent`s to each client as JSONL
/// - Reads `UICommand`s from each client and forwards them via `cmd_tx`
///
/// Returns the `mpsc::Receiver<UICommand>` that the caller uses to process
/// commands from connected clients.
///
/// The server removes any stale socket file on startup and cleans up on
/// cancellation.
pub fn start_ipc_server(
    event_tx: broadcast::Sender<DaemonEvent>,
    cancel: CancellationToken,
) -> mpsc::Receiver<UICommand> {
    let (cmd_tx, cmd_rx) = mpsc::channel(IPC_COMMAND_CAPACITY);
    let socket_path = default_socket_path();

    tokio::spawn(run_server(event_tx, cmd_tx, socket_path, cancel));

    cmd_rx
}

async fn run_server(
    event_tx: broadcast::Sender<DaemonEvent>,
    cmd_tx: mpsc::Sender<UICommand>,
    socket_path: PathBuf,
    cancel: CancellationToken,
) {
    if let Err(e) = run_server_inner(&event_tx, &cmd_tx, &socket_path, &cancel).await {
        tracing::error!("IPC server error: {e}");
    }

    // Clean up the socket file on exit
    cleanup_socket(&socket_path);
}

async fn run_server_inner(
    event_tx: &broadcast::Sender<DaemonEvent>,
    cmd_tx: &mpsc::Sender<UICommand>,
    socket_path: &Path,
    cancel: &CancellationToken,
) -> Result<()> {
    // Remove stale socket file from a previous run
    cleanup_socket(socket_path);

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create IPC socket directory")?;
    }

    let listener = UnixListener::bind(socket_path).context("Failed to bind IPC Unix socket")?;

    tracing::info!(path = %socket_path.display(), "IPC server listening");

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("IPC server shutting down");
                break;
            }
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        tracing::debug!("IPC client connected");
                        let client_event_rx = event_tx.subscribe();
                        let client_cmd_tx = cmd_tx.clone();
                        let client_cancel = cancel.clone();
                        tokio::spawn(handle_client(
                            stream,
                            client_event_rx,
                            client_cmd_tx,
                            client_cancel,
                        ));
                    }
                    Err(e) => {
                        tracing::warn!("IPC accept error: {e}");
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle a single connected IPC client.
///
/// Reads UICommand JSONL from the client and forwards DaemonEvent JSONL to it.
async fn handle_client(
    stream: UnixStream,
    mut event_rx: broadcast::Receiver<DaemonEvent>,
    cmd_tx: mpsc::Sender<UICommand>,
    cancel: CancellationToken,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,

            // Read commands from client
            line = lines.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<UICommand>(&text) {
                            Ok(cmd) => {
                                tracing::debug!(?cmd, "IPC command received");
                                if cmd_tx.send(cmd).await.is_err() {
                                    tracing::warn!("IPC command channel closed");
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!(input = %text, "Invalid IPC command: {e}");
                            }
                        }
                    }
                    Ok(None) => {
                        // Client disconnected
                        tracing::debug!("IPC client disconnected");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("IPC read error: {e}");
                        break;
                    }
                }
            }

            // Forward events to client
            event = event_rx.recv() => {
                match event {
                    Ok(evt) => {
                        let mut json = match serde_json::to_string(&evt) {
                            Ok(j) => j,
                            Err(e) => {
                                tracing::warn!("Failed to serialize IPC event: {e}");
                                continue;
                            }
                        };
                        json.push('\n');
                        if writer.write_all(json.as_bytes()).await.is_err() {
                            tracing::debug!("IPC client write failed, disconnecting");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "IPC client lagged — events dropped");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

/// Remove the socket file if it exists. Errors are logged and ignored.
fn cleanup_socket(path: &Path) {
    if path.exists() {
        if let Err(e) = std::fs::remove_file(path) {
            tracing::warn!(path = %path.display(), "Failed to remove stale socket: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn test_ipc_server_sends_events() {
        let cancel = CancellationToken::new();
        let (event_tx, _) = broadcast::channel(16);

        // Use a temp path for the socket
        let tmp_dir = tempfile::tempdir().unwrap();
        let socket_path = tmp_dir.path().join("test.sock");

        let cmd_tx_pair = mpsc::channel(16);
        let event_tx_clone = event_tx.clone();
        let socket_clone = socket_path.clone();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let _ = run_server_inner(
                &event_tx_clone,
                &cmd_tx_pair.0,
                &socket_clone,
                &cancel_clone,
            )
            .await;
        });

        // Give the server a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect a client
        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let (mut reader, _writer) = stream.into_split();

        // Wait for the server to accept and subscribe the client to broadcast
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Send an event
        let _ = event_tx.send(DaemonEvent::ConnectionState {
            state: crate::protocol::ConnectionState::Connected,
            message: "test".into(),
        });

        // Read the event line
        let mut buf = vec![0u8; 1024];
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), reader.read(&mut buf))
            .await
            .unwrap()
            .unwrap();

        let line = String::from_utf8_lossy(&buf[..n]);
        assert!(line.contains("ConnectionState"));
        assert!(line.contains("connected"));

        cancel.cancel();
    }

    #[tokio::test]
    async fn test_ipc_server_receives_commands() {
        let cancel = CancellationToken::new();
        let (event_tx, _) = broadcast::channel(16);

        let tmp_dir = tempfile::tempdir().unwrap();
        let socket_path = tmp_dir.path().join("test_cmd.sock");

        let (cmd_tx, mut cmd_rx) = mpsc::channel(16);
        let event_tx_clone = event_tx.clone();
        let socket_clone = socket_path.clone();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let _ = run_server_inner(&event_tx_clone, &cmd_tx, &socket_clone, &cancel_clone).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Connect and send a command
        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let (_reader, mut writer) = stream.into_split();

        let cmd_json = serde_json::to_string(&UICommand::SendText {
            text: "hello".into(),
        })
        .unwrap();
        writer
            .write_all(format!("{cmd_json}\n").as_bytes())
            .await
            .unwrap();

        // Receive the command
        let cmd = tokio::time::timeout(std::time::Duration::from_secs(2), cmd_rx.recv())
            .await
            .unwrap()
            .unwrap();

        match cmd {
            UICommand::SendText { text } => assert_eq!(text, "hello"),
            _ => panic!("unexpected command"),
        }

        cancel.cancel();
    }
}
