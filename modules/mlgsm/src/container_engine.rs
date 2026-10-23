//! Container engine access for MLGSM external module

use super::*;
use bollard::API_DEFAULT_VERSION;
use bollard::Docker;
use bollard::models::ContainerSummary;
use bollard::query_parameters::{ListContainersOptionsBuilder, RemoveContainerOptionsBuilder};

const DEFAULT_TIMEOUT: u64 = 30;

/// Connect to Podman root WSL machine via HTTP tunnel.
pub fn connect_podman_root() -> Option<Docker> {
    let version = *API_DEFAULT_VERSION;
    Docker::connect_with_http("http://127.0.0.1:2376", DEFAULT_TIMEOUT, &version).ok()
}

/// Connect to Podman user WSL machine via HTTP tunnel.
pub fn connect_podman_user() -> Option<Docker> {
    let version = *API_DEFAULT_VERSION;
    Docker::connect_with_http("http://127.0.0.1:2377", DEFAULT_TIMEOUT, &version).ok()
}

/// Connect to Docker Desktop named pipe.
pub fn connect_docker() -> Option<Docker> {
    for pipe in ["npipe:////./pipe/dockerDesktopLinuxEngine", "npipe:////./pipe/docker_engine"] {
        if let Ok(client) = Docker::connect_with_named_pipe(pipe, 30, &*API_DEFAULT_VERSION) {
            return Some(client);
        }
    }
    None
}

/// Try to connect to any available container engine.
pub fn connect_local_client() -> Option<Docker> {
    connect_podman_root().or_else(connect_podman_user).or_else(connect_docker)
}

/// Engine set with root and user engines.
pub struct EngineSet {
    pub root: Option<Docker>,
    pub user: Option<Docker>,
}

impl EngineSet {
    pub fn connect() -> Self {
        Self {
            root: connect_podman_root(),
            user: connect_podman_user(),
        }
    }
}

/// List all containers from an engine set (both root and user engines).
pub async fn list_all_containers(set: &EngineSet) -> Vec<ContainerSummary> {
    let mut all = Vec::new();
    if let Some(ref engine) = set.root {
        match engine.list_containers(Some(ListContainersOptionsBuilder::default().all(true).build())).await {
            Ok(list) => all.extend(list),
            Err(_) => {}
        }
    }
    if let Some(ref engine) = set.user {
        match engine.list_containers(Some(ListContainersOptionsBuilder::default().all(true).build())).await {
            Ok(list) => all.extend(list),
            Err(_) => {}
        }
    }
    all
}

/// Remove a container by name.
pub async fn remove_container(engine: &Docker, name: &str) -> MlgsmResult<()> {
    engine.stop_container(name, None).await.ok();
    engine
        .remove_container(
            name,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await
        .map_err(|e| Error::Internal(format!("Container removal failed: {e}")))
}