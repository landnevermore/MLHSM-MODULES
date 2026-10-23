//! Container logs and the interactive console attach/resize.

use super::*;
use crate::container_engine::connect_local_client;
// â”€â”€ Runtime logs â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Fetch the last `tail` lines of container stdout+stderr for a running game server.
pub async fn container_logs(server_id: &str, tail: u64) -> MlgsmResult<Vec<String>> {
    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".into()))?;
    let cname = container_name(server_id);
    let opts = LogsOptionsBuilder::default()
        .stdout(true)
        .stderr(true)
        .tail(&tail.to_string())
        .build();
    let mut stream = docker.logs(&cname, Some(opts));
    let mut lines = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(out) => {
                let bytes = out.into_bytes();
                let text = String::from_utf8_lossy(&bytes);
                for chunk in text.split('\r').flat_map(|c| c.split('\n')) {
                    let t = chunk.trim();
                    if !t.is_empty() {
                        lines.push(t.to_string());
                    }
                }
            }
            Err(_) => break,
        }
    }
    Ok(lines)
}

/// Return a raw `bollard` attach handle for live console streaming.
/// Caller is responsible for bridging the streams (e.g. to a WebSocket).
pub async fn attach_console(server_id: &str) -> MlgsmResult<bollard::container::AttachContainerResults> {
    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".into()))?;
    let cname = container_name(server_id);
    let opts = AttachContainerOptionsBuilder::default()
        .stdin(true)
        .stdout(true)
        .stderr(true)
        .stream(true)
        .build();
    docker
        .attach_container(&cname, Some(opts))
        .await
        .map_err(|e| Error::ServiceUnavailable(format!("Attach fehlgeschlagen: {e}")))
}

/// Resize the TTY of a running game container.
pub async fn resize_console(server_id: &str, cols: u16, rows: u16) -> MlgsmResult<()> {
    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".into()))?;
    let cname = container_name(server_id);
    let opts = ResizeContainerTTYOptionsBuilder::default()
        .w(i32::from(cols))
        .h(i32::from(rows))
        .build();
    docker
        .resize_container_tty(&cname, opts)
        .await
        .map_err(|e| Error::ServiceUnavailable(format!("Resize fehlgeschlagen: {e}")))
}



