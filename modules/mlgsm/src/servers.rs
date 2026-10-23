//! The server registry: game_servers.json plus the record operations
// (create, ensure exists, update variables).

use super::*;
use crate::eggs::{resolve_variables, next_free_port};
use crate::image::container_name;
// â”€â”€ Persistence helpers (JSON per server) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Config file holding server records.
pub fn servers_file(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("game_servers.json")
}

pub fn load_servers(data_dir: &std::path::Path) -> MlgsmResult<Vec<GameServer>> {
    let path = servers_file(data_dir);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| Error::Internal(format!("Server-Liste lesen fehlgeschlagen: {e}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| Error::Internal(format!("Server-Liste beschÃ¤digt: {e}")))
}

pub fn save_servers(data_dir: &std::path::Path, servers: &[GameServer]) -> MlgsmResult<()> {
    let bytes = serde_json::to_vec_pretty(servers)
        .map_err(|e| Error::Internal(format!("Server-Liste serialisieren fehlgeschlagen: {e}")))?;
    std::fs::write(servers_file(data_dir), bytes)
        .map_err(|e| Error::Internal(format!("Server-Liste schreiben fehlgeschlagen: {e}")))
}

/// Validate that a server id exists in the persisted registry. Used by
/// file-management endpoints to 404 unknown servers.
pub fn ensure_server(data_dir: &std::path::Path, id: &str) -> MlgsmResult<()> {
    let servers = load_servers(data_dir)?;
    if servers.iter().any(|s| s.id == id) {
        Ok(())
    } else {
        Err(Error::NotFound(format!("Server '{id}' nicht gefunden.")))
    }
}

pub fn create_server(
    data_dir: &std::path::Path,
    req: &CreateGameServerRequest,
    container_ports: &std::collections::HashSet<String>,
) -> MlgsmResult<GameServer> {
    let mut servers = load_servers(data_dir)?;
    let id = uuid::Uuid::new_v4().to_string();

    let egg = load_eggs(data_dir)?
        .into_iter()
        .find(|e| e.name.eq_ignore_ascii_case(&req.egg))
        .ok_or_else(|| Error::Internal(format!("Egg '{}' nicht gefunden", req.egg)))?;

    let image_label = if req.image.is_empty() {
        egg.docker_images.keys().next().cloned().unwrap_or_default()
    } else {
        req.image.clone()
    };
    let image = egg
        .docker_images
        .get(&image_label)
        .cloned()
        .ok_or_else(|| Error::Internal(format!("Bild '{}' nicht im Egg", image_label)))?;

    let mut env = resolve_variables(&egg, &req.variables);
    // Built-in placeholders: allocate a host port for the game.
    // An explicitly requested port that is already taken falls back to the
    // next free one so creation never collides.
    let mut used: std::collections::HashSet<String> = servers
        .iter()
        .flat_map(|s| s.variables.get("SERVER_PORT").cloned())
        .collect();
    used.extend(container_ports.iter().cloned());
    let requested = env.get("SERVER_PORT").map(String::as_str).unwrap_or("");
    if requested.trim().is_empty() || used.contains(requested.trim()) {
        env.insert("SERVER_PORT".into(), next_free_port(&used));
    }
    let server = GameServer {
        id: id.clone(),
        name: req.name.clone(),
        egg_name: egg.name.clone(),
        image: image.clone(),
        variables: env,
        status: GameServerStatus::Installing,
        container: Some(container_name(&id)),
        created_at: Some(chrono::Utc::now().to_rfc3339()),
        parent_id: None,
        role: None,
    };

    // No host directory is created: game data lives in the server's engine
    // volume, created lazily by the engine on the installer container start.
    servers.push(server);
    save_servers(data_dir, &servers)?;
    Ok(servers
        .into_iter()
        .find(|s| s.id == id)
        .expect("just pushed"))
}



