//! Dev copies: an isolated clone of a main server. The record is
// created without touching data; deploy_dev_copy copies the parent's volume
// afterwards.

use super::*;
use crate::eggs::next_free_port;
use crate::servers::{load_servers, save_servers};
use crate::eggs::game_volume_name;
use crate::deploy::{make_progress, copy_game_volume, finish_deploy};
/// Create a dev server record: an isolated clone of a parent (main) server.
/// The data volume is not touched here â€” deploy_dev_copy copies it. The
/// port defaults to the parent's port + 1.
pub fn create_dev_server(
    data_dir: &std::path::Path,
    parent_id: &str,
    req: &CreateDevServerRequest,
    container_ports: &std::collections::HashSet<String>,
) -> MlgsmResult<GameServer> {
    let mut servers = load_servers(data_dir)?;
    let parent = servers
        .iter()
        .find(|s| s.id == parent_id)
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("Server {parent_id} nicht gefunden")))?;
    if parent.role.as_deref() == Some("dev") {
        return Err(Error::Internal(
            "Aus einem Dev-Server kann kein weiterer Dev-Server entstehen.".into(),
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let name = req
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| format!("{} (Dev)", parent.name));

    // Pick the dev port: explicit request, else parent + 1, else first free.
    // A busy explicit port falls back to the next free one.
    let mut used: std::collections::HashSet<String> = servers
        .iter()
        .flat_map(|s| s.variables.get("SERVER_PORT").cloned())
        .filter(|p| !p.trim().is_empty())
        .collect();
    used.extend(container_ports.iter().cloned());
    let port = match req.port.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) if used.contains(p) => next_free_port(&used),
        Some(p) => p.to_string(),
        None => {
            let base = parent
                .variables
                .get("SERVER_PORT")
                .and_then(|p| p.trim().parse::<u32>().ok());
            match base {
                Some(b) if !used.contains(&(b + 1).to_string()) => (b + 1).to_string(),
                _ => next_free_port(&used),
            }
        }
    };

    let mut variables = parent.variables.clone();
    variables.insert("SERVER_PORT".to_string(), port.clone());

    let server = GameServer {
        id: id.clone(),
        name,
        egg_name: parent.egg_name.clone(),
        image: parent.image.clone(),
        variables,
        status: GameServerStatus::Installing,
        container: Some(container_name(&id)),
        created_at: Some(chrono::Utc::now().to_rfc3339()),
        parent_id: Some(parent_id.to_string()),
        role: Some("dev".to_string()),
    };
    servers.push(server.clone());
    save_servers(data_dir, &servers)?;
    Ok(server)
}

/// Deploy a dev server: copy the parent's data volume, then start the game
/// container. No installer run â€” the game files already exist.
pub async fn deploy_dev_copy(data_dir: &std::path::Path, server_id: &str) -> MlgsmResult<()> {
    let servers = load_servers(data_dir)?;
    let server = servers
        .iter()
        .find(|s| s.id == server_id)
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("Server {server_id} nicht gefunden")))?;
    let parent_id = server
        .parent_id
        .clone()
        .ok_or_else(|| Error::Internal("Kein Eltern-Server fÃ¼r Dev-Server.".into()))?;

    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".to_string()))?;
    let mut write_progress = make_progress(data_dir, server_id);
    write_progress("install", None, "Game-Daten werden kopiertâ€¦");

    copy_game_volume(
        &docker,
        &game_volume_name(&parent_id),
        &game_volume_name(server_id),
        &mut write_progress,
    )
    .await?;

    let egg = load_eggs(data_dir)?
        .into_iter()
        .find(|e| e.name.eq_ignore_ascii_case(&server.egg_name))
        .ok_or_else(|| Error::Internal(format!("Egg '{}' nicht gefunden", server.egg_name)))?;
    finish_deploy(data_dir, server_id, &egg, &server.image, &server.variables).await
}




