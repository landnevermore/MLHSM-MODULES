//! Reinstall: stop, then re-run the installer (main server) or re-copy the
// parent volume (dev server).

use super::*;
use crate::eggs::next_free_port;
use crate::servers::save_servers;
use crate::lifecycle::{remove_game_container, remove_named_container};
// â”€â”€ Reinstall â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Stop a game server, then re-run its egg installer.
/// deploy_server handles the full installer + start flow.
pub async fn reinstall_server(data_dir: &std::path::Path, server_id: &str) -> MlgsmResult<()> {
    let servers = load_servers(data_dir)?;
    let server = servers
        .iter()
        .find(|s| s.id == server_id)
        .ok_or_else(|| Error::NotFound(format!("Server {server_id} nicht gefunden")))?;

    let eggs = load_eggs(data_dir)?;
    let egg = eggs
        .iter()
        .find(|e| e.name == server.egg_name)
        .ok_or_else(|| Error::Internal(format!("Egg '{}' nicht gefunden", server.egg_name)))?;

    let mut variables = server.variables.clone();
    if !variables.contains_key("SERVER_PORT")
        || variables
            .get("SERVER_PORT")
            .is_none_or(|v| v.trim().is_empty())
    {
        let used: std::collections::HashSet<String> = servers
            .iter()
            .flat_map(|s| s.variables.get("SERVER_PORT").cloned())
            .filter(|p| !p.trim().is_empty())
            .collect();
        variables.insert("SERVER_PORT".into(), next_free_port(&used));
        let mut updated = load_servers(data_dir)?;
        if let Some(s) = updated.iter_mut().find(|s| s.id == server_id) {
            s.variables = variables.clone();
        }
        save_servers(data_dir, &updated)?;
    }

    // Stop + remove containers if they exist (deploy recreates them).
    let _ = power(data_dir, server_id, "stop").await;
    let _ = remove_game_container(server_id).await;
    // Also remove leftover install containers from previous deploys.
    let install_cname = format!("hsm-game-install-{server_id}");
    let _ = remove_named_container(&install_cname).await;

    // Dev copies are rebuilt from the live parent instead of re-downloading
    // the game data (a dev reinstall means "reset to parent state").
    if server.role.as_deref() == Some("dev") {
        return deploy_dev_copy(data_dir, server_id).await;
    }

    // Re-deploy with the same config.
    deploy_server(data_dir, egg, &server.image, &variables, server_id).await
}



