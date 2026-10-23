//! Remove, power and live status. live_status deliberately reports the
// engine's actual state, never the persisted wish - that is what makes the
// dashboard honest about a container that exited on its own.

use super::*;
use crate::image::container_name;
use crate::servers::{load_servers, save_servers};
use crate::eggs::{game_volume_name, find_egg};
use crate::deploy::{start_game_container};
/// Stop + remove the game container, keeping the data volume. Used by
/// reinstall so the deploy step can recreate the container.
pub async fn remove_game_container(server_id: &str) -> MlgsmResult<()> {
    let cname = container_name(server_id);
    remove_named_container(&cname).await
}

pub(crate) async fn remove_named_container(cname: &str) -> MlgsmResult<()> {
    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".to_string()))?;
    let _ = docker.stop_container(cname, None).await;
    let _ = docker
        .remove_container(
            cname,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await;
    Ok(())
}

/// Stop + remove the game container, its engine volume (server data) and â€”
/// for a main server â€” every dev copy below it. Removes the matching records
/// from game_servers.json. Returns the ids whose install state should be
/// cleared (the server itself plus any removed dev copies).
pub async fn remove_server(data_dir: &std::path::Path, server_id: &str) -> MlgsmResult<Vec<String>> {
    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".to_string()))?;
    let servers = load_servers(data_dir)?;
    // Dev copies of a main server are removed with it (they own a full game
    // data copy and would otherwise be orphaned).
    let remove_ids = [server_id.to_string()]
        .into_iter()
        .chain(
            servers
                .iter()
                .filter(|s| s.parent_id.as_deref() == Some(server_id))
                .map(|s| s.id.clone()),
        )
        .collect::<Vec<_>>();
    for id in &remove_ids {
        let cname = container_name(id);
        let _ = docker.stop_container(&cname, None).await;
        let _ = docker
            .remove_container(
                &cname,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await;
        // Remove the engine volume holding this server's game data.
        let vol = game_volume_name(id);
        let _ = docker
            .remove_volume(&vol, None::<bollard::query_parameters::RemoveVolumeOptions>)
            .await;
    }
    let remaining: Vec<GameServer> = servers
        .into_iter()
        .filter(|s| !remove_ids.contains(&s.id))
        .collect();
    save_servers(data_dir, &remaining)?;
    Ok(remove_ids)
}

/// Start / stop / restart a game container.
pub async fn power(data_dir: &std::path::Path, server_id: &str, action: &str) -> MlgsmResult<()> {
    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".to_string()))?;
    let cname = container_name(server_id);

    // Health-Gate: only ever transition from a state that makes sense. Starting
    // an already-running server or stopping one that is already stopped is an
    // invalid transition â€” reject it loudly instead of silently succeeding.
    let live = live_status(server_id).await;
    let invalid = match action {
        "start" => live.is_running(),
        "stop" => !live.is_running(),
        // restart may run from either state.
        "restart" => false,
        _ => false,
    };
    if invalid {
        return Err(Error::Conflict(format!(
            "Aktion '{action}' fÃ¼r '{server_id}' ist im aktuellen Zustand ('{}') nicht erlaubt â€” no-op vermieden.",
            live.as_str()
        )));
    }

    match action {
        "start" | "restart" => {
            // Env-Reconciliation: the container env is frozen at create time,
            // while update_server_variables only edits the registry. Rebuild
            // the container when its env no longer matches the registry, so
            // changed variables (e.g. STEAM_ACC) apply on the next boot.
            let servers = load_servers(data_dir)?;
            let server = servers
                .iter()
                .find(|s| s.id == server_id)
                .ok_or_else(|| Error::NotFound(format!("Game-Server {server_id}")))?;
            let eggs = load_eggs(data_dir)?;
            let egg = find_egg(&eggs, &server.egg_name)
                .ok_or_else(|| Error::NotFound(format!("Egg '{}'", server.egg_name)))?;
            let wanted_env = server.variables.clone();

            let (exists, env_stale) = match docker.inspect_container(&cname, None).await {
                Ok(resp) => {
                    let stale = resp
                        .config
                        .and_then(|c| c.env)
                        .map(|cur| {
                            let cur_env: std::collections::BTreeMap<String, String> = cur
                                .into_iter()
                                .filter_map(|kv| {
                                    kv.split_once('=')
                                        .map(|(k, v)| (k.to_owned(), v.to_owned()))
                                })
                                .collect();
                            cur_env != wanted_env
                        })
                        .unwrap_or(true);
                    (true, stale)
                }
                Err(_) => (false, true),
            };

            if !exists || env_stale {
                if exists {
                    super::container_engine::remove_container(&docker, &cname)
                        .await
                        .map_err(|e| {
                            Error::ServiceUnavailable(format!(
                                "Container entfernen fehlgeschlagen: {e}"
                            ))
                        })?;
                }
                start_game_container(&docker, server_id, egg, &server.image, &wanted_env)
                    .await
                    .map_err(|e| {
                        Error::ServiceUnavailable(format!(
                            "Game-Container neu erstellen fehlgeschlagen: {e}"
                        ))
                    })?;
            } else if action == "start" {
                docker
                    .start_container(&cname, None)
                    .await
                    .map_err(|e| Error::ServiceUnavailable(format!("Start fehlgeschlagen: {e}")))?;
            } else {
                docker.restart_container(&cname, None).await.map_err(|e| {
                    Error::ServiceUnavailable(format!("Restart fehlgeschlagen: {e}"))
                })?;
            }
        }
        "stop" => {
            docker
                .stop_container(&cname, None)
                .await
                .map_err(|e| Error::ServiceUnavailable(format!("Stop fehlgeschlagen: {e}")))?;
        }
        other => {
            return Err(Error::Internal(format!("Unbekannte Aktion: {other}")));
        }
    }
    Ok(())
}

/// Update a deployed server's egg variables (upsert) and persist the registry.
/// Missing keys are kept; unknown new keys are added as-is. The container env
/// is not touched here â€” the new values apply on the next start/restart.
pub fn update_server_variables(
    data_dir: &std::path::Path,
    server_id: &str,
    variables: &std::collections::BTreeMap<String, String>,
) -> MlgsmResult<GameServer> {
    let mut servers = load_servers(data_dir)?;
    let idx = servers
        .iter()
        .position(|s| s.id == server_id)
        .ok_or_else(|| Error::NotFound(format!("Game-Server {server_id}")))?;
    for (key, value) in variables {
        servers[idx].variables.insert(key.clone(), value.clone());
    }
    let updated = servers[idx].clone();
    save_servers(data_dir, &servers)?;
    Ok(updated)
}

/// Resolve the live status for a deployed server record.
///
/// This describes the *actual* running state, never the persisted wish. A
/// container that merely exists (created / exited) is NOT "Running" â€” only a
/// container whose engine state is running counts. This is what makes the
/// dashboard honest: an exited GMod container shows Stopped, and a started
/// one shows Running, regardless of what the JSON wrapper says.
pub async fn live_status(server_id: &str) -> GameServerStatus {
    let Some(docker) = super::container_engine::connect_local_client() else {
        return GameServerStatus::Offline;
    };
    let resp = match docker
        .inspect_container(&container_name(server_id), None)
        .await
    {
        Ok(resp) => resp,
        Err(_) => return GameServerStatus::Offline,
    };
    match resp.state.and_then(|st| st.running) {
        Some(true) => GameServerStatus::Running,
        _ => GameServerStatus::Offline,
    }
}




