//! Egg loading, variable resolution and port allocation.
//
// The egg is the declarative description of a game: its install script, its
// startup command, its variable schema. Everything else in this module either
// reads eggs or decides which port/image a server gets.

use super::*;
use hsm_model::game_server::Egg;
use bollard::models::ContainerSummary;
pub const GAME_VOLUME_PREFIX: &str = "hsm-game-data-";

/// Directory that holds egg JSON files.
pub fn eggs_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("game-eggs")
}

/// Engine volume that holds this server's game data.
///
/// Preferred over a bind mount into data_dir: the engine volume sits on the
/// Linux root filesystem, which srcds (Source engine) needs to locate its
/// gamemodes. The volume is created implicitly by the engine when the installer
/// or game container is created.
pub fn game_volume_name(server_id: &str) -> String {
    format!("{GAME_VOLUME_PREFIX}{server_id}")
}

// Ã¢â€â‚¬Ã¢â€â‚¬ Egg loading Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Load all eggs from game-eggs (recursive, *.json), sorted by name.
pub fn load_eggs(data_dir: &std::path::Path) -> MlgsmResult<Vec<Egg>> {
    let dir = eggs_dir(data_dir);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut eggs = Vec::new();
    for entry in read_dir_recursive(&dir) {
        let Some(ext) = entry.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !ext.eq_ignore_ascii_case("json") {
            continue;
        }
        let bytes = std::fs::read(&entry).map_err(|e| {
            Error::Internal(format!("Egg lesen {} fehlgeschlagen: {e}", entry.display()))
        })?;
        match serde_json::from_slice::<Egg>(&bytes) {
            Ok(mut egg) => {
                // PTDL_v1 eggs carry images (plain list) instead of the v2
                // docker_images map. Normalize so the rest of the pipeline
                // only ever deals with the map form.
                if egg.docker_images.is_empty() && !egg.images.is_empty() {
                    for (i, img) in egg.images.iter().enumerate() {
                        let label = if i == 0 {
                            "Default".to_string()
                        } else {
                            format!("Image {}", i + 1)
                        };
                        egg.docker_images.insert(label, img.clone());
                    }
                }
                egg.category = egg_category(entry.parent());
                eggs.push(egg);
            }
            Err(e) => {
                return Err(Error::Internal(format!(
                    "UngÃƒÂ¼ltiges Egg {}: {e}",
                    entry.display()
                )))
            }
        }
    }
    eggs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(eggs)
}

/// Derive the egg's category from its directory relative to game-eggs.
/// The top-level folder is the category, with a few known remaps so the
/// dropdown groups games sanely regardless of where the file sits.
pub(crate) fn egg_category(dir: Option<&std::path::Path>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = dir.map(|p| p.to_path_buf());
    while let Some(path) = cur {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            parts.push(name.to_string());
        }
        cur = path.parent().map(|p| p.to_path_buf());
    }
    let rel = parts;
    if rel.is_empty() {
        return String::new();
    }
    let top = &rel[0];
    match top.as_str() {
        "minecraft" => "Minecraft".to_string(),
        "rust" => "Rust".to_string(),
        "voice-servers" => "Voice".to_string(),
        "source-engine" | "games" => {
            if rel.iter().any(|p| p.to_lowercase().contains("ark")) {
                "Survival".to_string()
            } else {
                "Source Engine".to_string()
            }
        }
        other => other
            .split(['-', '_'])
            .filter(|s| !s.is_empty())
            .map(|s| {
                let mut c = s.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Find an egg by file name (without .json) or by name.
pub fn find_egg<'a>(eggs: &'a [Egg], id: &str) -> Option<&'a Egg> {
    eggs.iter().find(|e| e.name.eq_ignore_ascii_case(id))
}

pub(crate) fn read_dir_recursive(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(read_dir_recursive(&path));
            } else {
                out.push(path);
            }
        }
    }
    out
}

// Ã¢â€â‚¬Ã¢â€â‚¬ Variable resolution Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬Ã¢â€â‚¬

/// Merge egg.default_value under user-supplied values; return env map.
pub fn resolve_variables(egg: &Egg, user: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for var in &egg.variables {
        let value = user
            .get(&var.env_variable)
            .cloned()
            .unwrap_or_else(|| var.default_value.clone());
        env.insert(var.env_variable.clone(), value);
    }
    env
}

/// Substitute {{VAR}} placeholders in the egg startup command.
pub fn render_startup(startup: &str, env: &BTreeMap<String, String>) -> String {
    let mut out = startup.to_string();
    for (k, v) in env {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
}

/// Pick a free game-server port starting at 27015, skipping used ones.
pub(crate) fn next_free_port(used: &std::collections::HashSet<String>) -> String {
    (27015u16..=27999)
        .find(|p| !used.contains(&p.to_string()))
        .unwrap_or(27015)
        .to_string()
}

/// Host ports currently published by any container Ã¢â‚¬â€ including orphaned or
/// manually created ones that have no game_servers.json record. Auto
/// allocation merges these in so it can never collide with something real.
pub async fn container_used_ports() -> MlgsmResult<std::collections::HashSet<String>> {
    let set = super::container_engine::EngineSet::connect();
    if set.root.is_none() && set.user.is_none() {
        return Err(Error::ServiceUnavailable("Docker nicht erreichbar".into()));
    }
    let list = super::container_engine::list_all_containers(&set).await;
    let mut ports = std::collections::HashSet::new();
    for c in list {
        if let Some(port_list) = c.ports {
            for p in port_list {
                if let Some(host) = p.public_port {
                    ports.insert(host.to_string());
                }
            }
        }
    }
    Ok(ports)
}





