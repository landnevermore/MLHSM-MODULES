//! MLGameServerManager (MLGSM) - external module for game server management

mod container_engine;
mod deploy;
mod dev;
mod eggs;
mod error;
mod image;
mod install_state;
mod lifecycle;
mod logs;
mod reinstall;
mod servers;

use error::{Error, MlgsmResult};


use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{ws::WebSocketUpgrade, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use bollard::query_parameters::{
    AttachContainerOptionsBuilder, CreateContainerOptionsBuilder, CreateImageOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, ResizeContainerTTYOptionsBuilder, WaitContainerOptionsBuilder,
};
use bollard::models::{ContainerCreateBody, HostConfig, PortBinding};
use bollard::Docker as EngineClient;
use futures_util::stream::StreamExt;
use futures_util::SinkExt;
use hsm_model::game_server::{
    CreateDevServerRequest, CreateGameServerRequest, Egg, GamePreset, GameServer,
    GameServerStatus, InstallProgress, PresetField, PresetOption,
};
use hsm_model::game_files::{DirListing, FileOpResult, MkdirRequest, RenameRequest, WriteFileRequest};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::{error, info, warn};

use deploy::deploy_server;
use dev::deploy_dev_copy;
use eggs::{load_eggs, container_used_ports};
use image::container_name;
use install_state::{read_install_state, clear_install_state};
use lifecycle::{remove_server, power, update_server_variables, live_status};
use logs::{container_logs, attach_console, resize_console};
use reinstall::reinstall_server;
use servers::{load_servers, ensure_server};

const DEFAULT_MLGSM_PORT: u16 = 19091;

#[derive(Clone)]
struct MlgsmState { data_dir: PathBuf }
impl MlgsmState { fn new(data_dir: PathBuf) -> Self { Self { data_dir } } }

#[derive(Debug, Deserialize)]
pub(crate) struct GameServerPowerRequest { action: String }

#[derive(Debug, Deserialize)]
pub(crate) struct GameServerVariablesRequest { variables: BTreeMap<String, String> }

#[derive(Debug, Deserialize)]
pub(crate) struct GameServerLogsQuery {
    #[serde(default = "default_log_tail")] tail: u64,
}
fn default_log_tail() -> u64 { 200 }

#[derive(Debug, Serialize)]
pub(crate) struct ActionResponse { ok: bool, message: String }

#[derive(Debug, Deserialize)]
pub(crate) struct GameServerFileQuery {
    path: Option<String>,
    name: Option<String>,
}

fn game_presets() -> Vec<GamePreset> {
    vec![GamePreset {
        preset_id: "garrys-mod".into(), label: "Garrys Mod".into(),
        subtitle: "Sandbox-Server".into(),
        egg: "Garrys Mod".into(), image: String::new(),
        category: "Garrys Mod".into(),
        fields: vec![
            PresetField { key: "GAMEMODE".into(), label: "Gamemode".into(),
                kind: "select".into(), default: "sandbox".into(),
                options: vec![PresetOption { value: "sandbox".into(), label: "Sandbox".into() }],
                helper: "Der Spielmodus.".into(), },
            PresetField { key: "SRCDS_MAP".into(), label: "Startkarte".into(),
                kind: "text".into(), default: "gm_flatgrass".into(),
                options: Vec::new(), helper: "Die Startkarte.".into(), },
            PresetField { key: "MAX_PLAYERS".into(), label: "Spieler".into(),
                kind: "number".into(), default: "32".into(),
                options: Vec::new(), helper: "Max Spieler.".into(), },
            PresetField { key: "SERVER_PORT".into(), label: "Port".into(),
                kind: "number".into(), default: "27015".into(),
                options: Vec::new(), helper: "Netzwerk-Port.".into(), },
        ],
        dev_supported: true,
    }]
}

#[axum::debug_handler]async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "healthy", "module": "mlgsm" }))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let port: u16 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_MLGSM_PORT);
    let data_dir: PathBuf = std::env::var("HSM_DATA_DIR").ok().map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("data"));

    info!(port, "MLGSM starting");
    std::fs::create_dir_all(&data_dir).expect("data dir");
    let state = Arc::new(MlgsmState::new(data_dir));

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/games/eggs", get(list_game_eggs))
        .route("/api/v1/games/presets", get(list_game_presets))
        .route("/api/v1/games/servers", get(list_game_servers).post(create_game_server))
        .route("/api/v1/games/servers/{id}", get(get_game_server).delete(delete_game_server))
        .route("/api/v1/games/servers/{id}/dev", post(create_game_dev_server))
        .route("/api/v1/games/servers/{id}/install", get(get_game_server_install))
        .route("/api/v1/games/servers/{id}/power", post(game_server_power))
        .route("/api/v1/games/servers/{id}/variables", put(update_game_server_variables))
        .route("/api/v1/games/servers/{id}/logs", get(get_game_server_logs))
        .route("/api/v1/games/servers/{id}/reinstall", post(reinstall_game_server))
        .route("/api/v1/games/servers/{id}/console", get(game_server_console_ws))
        .route("/api/v1/games/servers/{id}/files", get(list_game_files))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    println!("MLGSM_PORT={port}");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("error");
}

#[axum::debug_handler]async fn list_game_eggs(State(state): State<Arc<MlgsmState>>) -> MlgsmResult<Json<Vec<Egg>>> {
    let eggs = load_eggs(&state.data_dir)?;
    Ok(Json(eggs))
}

#[axum::debug_handler]async fn list_game_presets(State(state): State<Arc<MlgsmState>>) -> MlgsmResult<Json<Vec<GamePreset>>> {
    let egg_names: Vec<String> = load_eggs(&state.data_dir)?
        .into_iter().map(|e| e.name).collect();
    Ok(Json(game_presets().into_iter().filter(|p| egg_names.iter().any(|n| n == &p.egg)).collect()))
}

#[axum::debug_handler]async fn list_game_servers(State(state): State<Arc<MlgsmState>>) -> MlgsmResult<Json<Vec<GameServer>>> {
    let mut servers = load_servers(&state.data_dir)?;
    for server in servers.iter_mut() { server.status = live_status(&server.id).await; }
    Ok(Json(servers))
}

#[axum::debug_handler]async fn get_game_server(State(state): State<Arc<MlgsmState>>, Path(id): Path<String>) -> MlgsmResult<Json<GameServer>> {
    let mut servers = load_servers(&state.data_dir)?;
    let server = servers.iter_mut().find(|s| s.id == id).ok_or_else(|| Error::NotFound("not found".into()))?;
    server.status = live_status(&server.id).await;
    Ok(Json(server.clone()))
}

#[axum::debug_handler]async fn get_game_server_install(State(state): State<Arc<MlgsmState>>, Path(id): Path<String>) -> MlgsmResult<Json<Option<InstallProgress>>> {
    Ok(Json(read_install_state(&state.data_dir, &id)))
}

#[axum::debug_handler]async fn create_game_server(State(state): State<Arc<MlgsmState>>, Json(req): Json<CreateGameServerRequest>) -> MlgsmResult<Json<GameServer>> {
    let container_ports = container_used_ports().await.unwrap_or_default();
    let mut server = crate::servers::create_server(&state.data_dir, &req, &container_ports)
        ?;
    let data_dir = state.data_dir.clone();
    let egg_name = server.egg_name.clone();
    let image = server.image.clone();
    let vars = server.variables.clone();
    let sid = server.id.clone();
    tokio::spawn(async move {
        let eggs = load_eggs(&data_dir).unwrap_or_default();
        let Some(egg) = eggs.iter().find(|e| e.name == egg_name) else {
            error!(egg = %egg_name, "Egg not found"); return;
        };
        match deploy_server(&data_dir, egg, &image, &vars, &sid).await {
            Ok(()) => info!(id = %sid, "installed"),
            Err(e) => error!(id = %sid, error = %e, "deploy failed"),
        }
    });
    server.status = live_status(&server.id).await;
    Ok(Json(server))
}

#[axum::debug_handler]async fn delete_game_server(State(state): State<Arc<MlgsmState>>, Path(id): Path<String>) -> MlgsmResult<Json<ActionResponse>> {
    let removed = remove_server(&state.data_dir, &id).await?;
    for rid in removed { clear_install_state(&state.data_dir, &rid); }
    Ok(Json(ActionResponse { ok: true, message: "removed".to_string() }))
}

#[axum::debug_handler]async fn create_game_dev_server(State(state): State<Arc<MlgsmState>>, Path(id): Path<String>, Json(req): Json<CreateDevServerRequest>) -> MlgsmResult<Json<GameServer>> {
    let container_ports = container_used_ports().await.unwrap_or_default();
    let server = crate::dev::create_dev_server(&state.data_dir, &id, &req, &container_ports)
        ?;
    let data_dir = state.data_dir.clone();
    let sid = server.id.clone();
    tokio::spawn(async move {
        match deploy_dev_copy(&data_dir, &sid).await {
            Ok(()) => info!(id = %sid, "dev deployed"),
            Err(e) => error!(id = %sid, error = %e, "dev deploy failed"),
        }
    });
    Ok(Json(server))
}

#[axum::debug_handler]async fn game_server_power(State(state): State<Arc<MlgsmState>>, Path(id): Path<String>, Json(req): Json<GameServerPowerRequest>) -> MlgsmResult<Json<ActionResponse>> {
    power(&state.data_dir, &id, &req.action).await?;
    Ok(Json(ActionResponse { ok: true, message: format!("action {} performed", req.action) }))
}

#[axum::debug_handler]async fn update_game_server_variables(State(state): State<Arc<MlgsmState>>, Path(id): Path<String>, Json(req): Json<GameServerVariablesRequest>) -> MlgsmResult<Json<GameServer>> {
    let server = update_server_variables(&state.data_dir, &id, &req.variables)
        ?;
    Ok(Json(server))
}

#[axum::debug_handler]async fn get_game_server_logs(Path(id): Path<String>, Query(q): Query<GameServerLogsQuery>) -> MlgsmResult<Json<Vec<String>>> {
    let lines = container_logs(&id, q.tail.min(2000)).await?;
    Ok(Json(lines))
}

#[axum::debug_handler]async fn reinstall_game_server(State(state): State<Arc<MlgsmState>>, Path(id): Path<String>) -> MlgsmResult<Json<ActionResponse>> {
    let data_dir = state.data_dir.clone();
    let sid = id.clone();
    tokio::spawn(async move {
        match reinstall_server(&data_dir, &sid).await {
            Ok(()) => info!(id = %sid, "reinstalled"),
            Err(e) => error!(id = %sid, error = %e, "reinstall failed"),
        }
    });
    Ok(Json(ActionResponse { ok: true, message: "reinstall started".to_string() }))
}

const RESIZE_PREFIX: &str = "__resize:";

#[axum::debug_handler]async fn game_server_console_ws(Path(id): Path<String>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| console_socket(socket, id))
}

async fn console_socket(socket: axum::extract::ws::WebSocket, id: String) {
    let attach = match attach_console(&id).await {
        Ok(a) => a,
        Err(e) => { warn!(id = %id, error = %e, "console fail"); return; }
    };
    let (mut tx, mut rx) = socket.split();
    let mut from_container = attach.output;
    let mut to_container = attach.input;
    let sender = tokio::spawn(async move {
        while let Some(item) = from_container.next().await {
            if let Ok(out) = item {
                let bytes = out.into_bytes();
                let text = String::from_utf8_lossy(&bytes);
            } else { break; }
        }
    });
    let receiver = tokio::spawn(async move {
        while let Some(Ok(msg)) = rx.next().await {
            match msg {
                axum::extract::ws::Message::Text(text) => {
                    if text.starts_with(RESIZE_PREFIX) {
                        if let Some(rest) = text.strip_prefix(RESIZE_PREFIX) {
                            let parts: Vec<&str> = rest.split(':').collect();
                            if parts.len() == 2 {
                                if let (Ok(cols), Ok(rows)) = (parts[0].parse::<u16>(), parts[1].parse::<u16>()) {
                                    let _ = resize_console(&id, cols, rows).await;
                                }
                            }
                        }
                    } else {
                        let bytes = text.as_bytes().to_vec();
                        let _ = to_container.write_all(&bytes).await;
                        let _ = to_container.flush().await;
                    }
                }
                axum::extract::ws::Message::Binary(bytes) => {
                    let _ = to_container.write_all(&bytes).await;
                    let _ = to_container.flush().await;
                }
                axum::extract::ws::Message::Close(_) => break,
                _ => {}
            }
        }
    });
    let _ = sender.await;
    let _ = receiver.await;
}

#[axum::debug_handler]async fn list_game_files(
    State(state): State<Arc<MlgsmState>>,
    Path(id): Path<String>,
    Query(q): Query<GameServerFileQuery>,
) -> MlgsmResult<Json<DirListing>> {
    ensure_server(&state.data_dir, &id)?;
    let path = q.path.as_deref().unwrap_or("");
    Ok(Json(DirListing { path: path.to_string(), entries: vec![] }))
}