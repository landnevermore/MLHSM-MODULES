//! The deploy flow: run the egg's installer, then create and start the
// game container. The largest module here and the one that still carries a
// 570-line function - see the note in the commit message.

use super::*;
pub use crate::image::ensure_image;
use crate::eggs::{game_volume_name, render_startup};
use crate::install_state::{parse_percent, write_install_state};
use crate::image::pull_image;
use crate::servers::save_servers;
pub const ALPINE_IMAGE: &str = "docker.io/library/alpine:latest";
pub const DEFAULT_SERVER_PROPERTIES: &str = "#Minecraft server properties
server-ip=
server-port=__PORT__
gamemode=survival
difficulty=easy
max-players=20
motd=A Minecraft Server
online-mode=true
spawn-protection=16
max-tick-time=60000
";
use bollard::models::VolumeCreateRequest;
use std::time::Instant;
use bollard::container::LogOutput;
use bollard::query_parameters::{AttachContainerOptionsBuilder, CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder, ResizeContainerTTYOptionsBuilder, WaitContainerOptionsBuilder};
use bollard::models::{ContainerCreateBody, HostConfig, PortBinding};
use bollard::Docker as EngineClient;
use futures_util::stream::StreamExt;
use tokio::io::AsyncWriteExt;

/// Create + start the game container for a server that already has its data
/// populated (by the installer) in the server's engine volume.
pub(crate) async fn start_game_container(
    docker: &EngineClient,
    server_id: &str,
    egg: &Egg,
    image: &str,
    env: &BTreeMap<String, String>,
) -> MlgsmResult<()> {
    ensure_image(docker, image).await;
    // Game data now lives in an engine volume (native ext4), never on the
    // NTFS/9p bind mount that breaks srcds's gamemode scan.
    let binds = vec![format!("{}:/home/container", game_volume_name(server_id))];

    let entrypoint = render_startup(&egg.startup, env);
    let mut port_bindings: std::collections::HashMap<String, Option<Vec<PortBinding>>> =
        std::collections::HashMap::new();
    if let Some(port) = env.get("SERVER_PORT").and_then(|p| p.parse::<u32>().ok()) {
        for proto in ["tcp", "udp"] {
            let key = format!("{port}/{proto}");
            let hb = PortBinding {
                host_ip: Some("0.0.0.0".into()),
                host_port: Some(port.to_string()),
            };
            port_bindings.insert(key, Some(vec![hb]));
        }
    }
    let config = ContainerCreateBody {
        image: Some(image.to_string()),
        // Run via /bin/sh -c so quoted args, pipes and && chains survive.
        entrypoint: Some(vec!["/bin/sh".to_string(), "-c".to_string(), entrypoint]),
        cmd: None,
        tty: Some(true),
        open_stdin: Some(true),
        stdin_once: Some(false),
        env: Some(
            env.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>(),
        ),
        host_config: Some(HostConfig {
            binds: Some(binds),
            port_bindings: Some(port_bindings),
            ..Default::default()
        }),
        working_dir: Some("/home/container".into()),
        ..Default::default()
    };
    let cname = container_name(server_id);
    let options = CreateContainerOptionsBuilder::default()
        .name(&cname)
        .build();
    docker
        .create_container(Some(options), config)
        .await
        .map_err(|e| {
            Error::ServiceUnavailable(format!("Game-Container anlegen fehlgeschlagen: {e}"))
        })?;
    docker.start_container(&cname, None).await.map_err(|e| {
        Error::ServiceUnavailable(format!("Game-Container starten fehlgeschlagen: {e}"))
    })?;
    Ok(())
}

/// Outcome of one installer-container run.
pub(crate) struct InstallerOutcome {
    code: i64,
    percent: Option<f64>,
    last_log: String,
    /// SteamCMD first runs frequently exit 0 but never fetched the depot
    /// ("Failed to install app ... (Missing configuration)").
    needs_retry: bool,
}

/// Build the installer container definition (mount the server engine volume to /mnt/server).
pub(crate) fn installer_cfg(
    image: &str,
    script: &str,
    entrypoint: &str,
    env: &BTreeMap<String, String>,
    data_source: &str,
) -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(image.to_string()),
        cmd: Some(vec!["-c".into(), script.to_string()]),
        entrypoint: if entrypoint.is_empty() {
            None
        } else {
            Some(vec![entrypoint.to_string()])
        },
        env: Some(env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        host_config: Some(HostConfig {
            // data_source is the server's engine volume; installer writes into it.
            binds: Some(vec![format!("{data_source}:/mnt/server")]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Create, start, stream logs, wait for, then remove one installer container.
pub(crate) async fn run_installer_container(
    docker: &EngineClient,
    cname: &str,
    cfg: ContainerCreateBody,
    write_progress: &mut impl FnMut(&str, Option<f64>, &str),
) -> InstallerOutcome {
    let options = CreateContainerOptionsBuilder::default().name(cname).build();
    if let Err(e) = docker.create_container(Some(options), cfg).await {
        return InstallerOutcome {
            code: i64::MIN,
            percent: None,
            last_log: format!("Installer-Container anlegen fehlgeschlagen: {e}"),
            needs_retry: false,
        };
    }
    if let Err(e) = docker.start_container(cname, None).await {
        let _ = docker
            .remove_container(
                cname,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await;
        return InstallerOutcome {
            code: i64::MIN,
            percent: None,
            last_log: format!("Installer-Container starten fehlgeschlagen: {e}"),
            needs_retry: false,
        };
    }

    write_progress("install", None, "Spiel-Dateien werden installiertâ€¦");

    // Stream installer logs while waiting for exit; parse download %.
    let wait_options = WaitContainerOptionsBuilder::default()
        .condition("not-running")
        .build();
    let mut wait_stream = docker.wait_container(cname, Some(wait_options));
    let logs_options = LogsOptionsBuilder::default()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .tail("all")
        .build();
    let mut log_stream = docker.logs(cname, Some(logs_options));

    // Sentinel so code has a well-defined value before the select loop is
    // entered (the -1/"no exit event yet" default is what bad retry paths see).
    #[allow(unused_assignments)]
    let mut code = -1i64;
    let mut percent: Option<f64> = None;
    let mut last_log = String::new();
    let mut needs_retry = false;

    // Track whether each stream has ended.  When both close without an exit
    // event (observed over the SSH maintenance tunnel â€” Podman sometimes
    // drops the wait signal), fall back to inspect_container to detect
    // termination.  A probe branch polls independently of the streams, so a
    // hot/dangling log stream can never starve termination detection.
    let mut log_ended = false;
    let mut wait_ended = false;
    let mut probe = tokio::time::interval(std::time::Duration::from_secs(3));

    loop {
        tokio::select! {
            wait = wait_stream.next(), if !wait_ended => {
                match wait {
                    Some(Ok(resp)) => { code = resp.status_code; break; }
                    Some(Err(e)) => {
                        code = i64::MIN;
                        last_log = format!("Installer-Container warten fehlgeschlagen: {e}");
                        break;
                    }
                    None => { wait_ended = true; }
                }
            }
            log = log_stream.next(), if !log_ended => {
                match log {
                    Some(Ok(LogOutput::StdOut { message }))
                    | Some(Ok(LogOutput::StdErr { message }))
                    | Some(Ok(LogOutput::Console { message })) => {
                        let text = String::from_utf8_lossy(&message);
                        for chunk in text.split('\r').flat_map(|c| c.split('\n')) {
                            let trimmed = chunk.trim();
                            if trimmed.is_empty() { continue; }
                            if trimmed.contains("Missing configuration")
                                || trimmed.contains("Failed to install app") {
                                needs_retry = true;
                            }
                            if let Some(p) = parse_percent(trimmed) {
                                if percent.map(|old| (p - old).abs() >= 0.1).unwrap_or(true) {
                                    percent = Some(p);
                                    write_progress("install", percent, trimmed);
                                }
                            } else if (trimmed.len() > last_log.len() || !trimmed.contains('%'))
                                && trimmed.len() <= 160 {
                                    last_log = trimmed.to_string();
                                    if !trimmed.contains('%') && !trimmed.starts_with("Update state") {
                                        write_progress("install", percent, &last_log);
                                    }
                                }
                        }
                    }
                    Some(Ok(LogOutput::StdIn { .. })) | Some(Err(_)) | None => {
                        log_ended = true;
                    }
                }
            }
            _ = probe.tick() => {
                // The engine over the tunnel can drop both streams after the
                // container exits without ever delivering an exit event.  Re-
                // check the container state directly and break as soon as it
                // terminated â€” regardless of what the streams are doing.
                match docker.inspect_container(cname, None).await {
                    Ok(info) => {
                        if let Some(state) = &info.state {
                            let status = state.status.as_ref().map(|s| s.to_string());
                            if status.as_deref() == Some("exited")
                                || status.as_deref() == Some("dead")
                            {
                                code = state.exit_code.unwrap_or(0);
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        // Container gone entirely (removed externally):
                        // nothing left to wait for.
                        code = 0;
                        break;
                    }
                }
            }
        }
    }

    let _ = docker
        .remove_container(
            cname,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await;
    InstallerOutcome {
        code,
        percent,
        last_log,
        needs_retry,
    }
}

/// Build the install-progress writer for one server deploy round-trip.
pub(crate) fn make_progress(
    data_dir: &std::path::Path,
    server_id: &str,
) -> impl FnMut(&str, Option<f64>, &str) {
    let data_dir = data_dir.to_path_buf();
    let server_id = server_id.to_string();
    let started = Instant::now();
    move |phase: &str, percent: Option<f64>, last_log: &str| {
        let elapsed = started.elapsed().as_secs_f64();
        let eta_secs = percent
            .filter(|p| *p > 1.0)
            .map(|p| ((elapsed / p) * (100.0 - p)) as u64);
        let log = sanitize_log(last_log);
        write_install_state(
            &data_dir,
            &server_id,
            &InstallProgress {
                phase: phase.into(),
                percent,
                eta_secs,
                last_log: log,
            },
        );
    }
}

/// Run the egg's install script in the installer container, then start the
/// game container. Completes when the install script exits 0.
///
/// Progress is streamed into <data_dir>/game-install/<server_id>.json
/// (phase + download percent when parseable + rough ETA), which the API reads
/// while the server status is installing.
pub async fn deploy_server(
    data_dir: &std::path::Path,
    egg: &Egg,
    image: &str,
    env: &BTreeMap<String, String>,
    server_id: &str,
) -> MlgsmResult<()> {
    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".to_string()))?;

    let mut write_progress = make_progress(data_dir, server_id);

    // 1. Installer run (fetch game files into the server engine volume).
    if !egg.scripts.installation.container.is_empty() {
        write_progress("pull", None, "Bild wird heruntergeladenâ€¦");
        let data_host = game_volume_name(server_id);
        let script = egg
            .scripts
            .installation
            .script
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let installer_image = egg.scripts.installation.container.clone();
        let entrypoint = egg.scripts.installation.entrypoint.clone();
        pull_image(&docker, &installer_image, |percent, status| {
            let text = if status.is_empty() {
                percent
                    .map(|p| format!("Bild wird heruntergeladenâ€¦ {p:.1} %"))
                    .unwrap_or_else(|| "Bild wird heruntergeladenâ€¦".to_string())
            } else if percent.is_some() {
                format!("{status}: {:.1} %", percent.unwrap())
            } else {
                status.to_string()
            };
            write_progress("pull", percent, &text);
        })
        .await;

        let cfg = installer_cfg(&installer_image, &script, &entrypoint, env, &data_host);
        let cname = format!("hsm-game-install-{server_id}");

        let mut outcome = run_installer_container(&docker, &cname, cfg, &mut write_progress).await;
        // SteamCMD's first anonymous run often fails with
        // "Failed to install app ... (Missing configuration)" because the
        // depot config is not loaded yet. Retry once: the second run pulls.
        if outcome.code == 0 && outcome.needs_retry {
            let retry_cfg = installer_cfg(&installer_image, &script, &entrypoint, env, &data_host);
            write_progress("install", None, "Install erneut versuchtâ€¦");
            outcome =
                run_installer_container(&docker, &cname, retry_cfg, &mut write_progress).await;
        }
        let code = outcome.code;
        let percent = outcome.percent;
        let last_log = outcome.last_log;
        if code != 0 {
            let msg = if code == i64::MIN {
                last_log.clone()
            } else if last_log.is_empty() {
                format!("Installer endete mit Exit-Code {code}. Spiel-Dateien nicht bereit.")
            } else {
                format!("Installer fehlgeschlagen (Exit-Code {code}): {last_log}")
            };
            write_progress("error", percent, &last_log);
            return Err(Error::ServiceUnavailable(msg));
        }

        // Eggs that read a server.properties (vanilla Minecraft) need the
        // EULA + the allocated port set before the first boot.
        apply_egg_runtime_config(&docker, server_id, egg, env, &mut write_progress).await?;
    }

    // 2. Game container.
    finish_deploy(data_dir, server_id, egg, image, env).await
}

/// Start the game container for an already-populated server and persist the
/// resulting status. Shared by fresh installs (deploy_server) and dev-copy
/// deploys (deploy_dev_copy).
pub(crate) async fn finish_deploy(
    data_dir: &std::path::Path,
    server_id: &str,
    egg: &Egg,
    image: &str,
    env: &BTreeMap<String, String>,
) -> MlgsmResult<()> {
    let docker = super::container_engine::connect_local_client()
        .ok_or_else(|| Error::ServiceUnavailable("Docker nicht erreichbar".to_string()))?;
    let mut write_progress = make_progress(data_dir, server_id);
    write_progress("start", None, "Container wird gestartetâ€¦");
    let result = start_game_container(&docker, server_id, egg, image, env).await;
    clear_install_state(data_dir, server_id);
    // Persist the server's live status so the list endpoint shows the real
    // state instead of a stale "installing".
    if let Err(e) = set_server_status(data_dir, server_id, &result).await {
        tracing::warn!(id = %server_id, error = %e, "Server-Status nicht aktualisiert");
    }
    result
}

/// Update a server's stored status after a deploy round-trip: Running on
/// success, Offline when the container could not be started.
pub(crate) async fn set_server_status(
    data_dir: &std::path::Path,
    server_id: &str,
    deploy: &MlgsmResult<()>,
) -> MlgsmResult<()> {
    let status = match deploy {
        Ok(()) => GameServerStatus::Running,
        Err(_) => GameServerStatus::Offline,
    };
    let mut servers = load_servers(data_dir)?;
    if let Some(s) = servers.iter_mut().find(|s| s.id == server_id) {
        s.status = status;
    }
    save_servers(data_dir, &servers)
}

/// Collapse whitespace/newlines in a log tail into a single display line.
pub(crate) fn sanitize_log(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// For eggs whose config declares a server.properties (vanilla Minecraft):
/// write the EULA and ensure server-port matches the allocated
/// SERVER_PORT before the first game-container boot. The existing
/// server.properties is preserved when already present (a reinstall keeps
/// the world config); only the port line is normalized then.
pub(crate) async fn apply_egg_runtime_config(
    docker: &EngineClient,
    server_id: &str,
    egg: &Egg,
    env: &BTreeMap<String, String>,
    write_progress: &mut impl FnMut(&str, Option<f64>, &str),
) -> MlgsmResult<()> {
    if !egg.config.files.contains("server.properties") {
        return Ok(());
    }
    let Some(port) = env
        .get("SERVER_PORT")
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
    else {
        return Ok(());
    };
    write_progress("install", None, "Server-Konfiguration wird vorbereitetâ€¦");
    let defaults = DEFAULT_SERVER_PROPERTIES.replace("__PORT__", port);
    let script = format!(
        "cat > /to/eula.txt <<'EOF'\neula=true\nEOF\n\
         if [ ! -f /to/server.properties ]; then\n\
         cat > /to/server.properties <<'EOF'\n{defaults}\nEOF\n\
         else\n\
         sed -i -E 's/^server-port=[0-9]+/server-port={port}/' /to/server.properties || true\n\
         grep -q '^server-port=' /to/server.properties || printf 'server-port={port}\\n' >> /to/server.properties\n\
         fi"
    );
    let cfg = ContainerCreateBody {
        image: Some(ALPINE_IMAGE.to_string()),
        entrypoint: Some(vec!["/bin/sh".to_string()]),
        cmd: Some(vec!["-c".to_string(), script]),
        env: None,
        host_config: Some(HostConfig {
            binds: Some(vec![format!("{}:/to", game_volume_name(server_id))]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let cname = format!("hsm-game-cfg-{server_id}");
    let outcome = run_installer_container(docker, &cname, cfg, write_progress).await;
    if outcome.code == 0 {
        Ok(())
    } else if outcome.code == i64::MIN && outcome.last_log.is_empty() {
        Err(Error::ServiceUnavailable(
            "Server-Konfiguration vorbereiten fehlgeschlagen.".to_string(),
        ))
    } else {
        Err(Error::ServiceUnavailable(format!(
            "Server-Konfiguration vorbereiten fehlgeschlagen (Exit-Code {}): {}",
            outcome.code, outcome.last_log
        )))
    }
}

/// Copy one named volume's contents into a second, freshly created volume.
/// Runs an ephemeral helper container (cp -a) so the copy stays on the
/// engine's native filesystem. Best-effort create: an already-existing
/// destination volume is reused as-is.
pub(crate) async fn copy_game_volume(
    docker: &EngineClient,
    src_vol: &str,
    dst_vol: &str,
    write_progress: &mut impl FnMut(&str, Option<f64>, &str),
) -> MlgsmResult<()> {
    // Create only when missing: Podman errors on create of an existing
    // volume, and a dev reinstall runs against a volume that already exists.
    if docker.inspect_volume(dst_vol).await.is_err() {
        docker
            .create_volume(VolumeCreateRequest {
                name: Some(dst_vol.into()),
                ..Default::default()
            })
            .await
            .map_err(|e| {
                Error::ServiceUnavailable(format!("Volume anlegen fehlgeschlagen: {e}"))
            })?;
    }
    let cfg = ContainerCreateBody {
        image: Some(ALPINE_IMAGE.to_string()),
        entrypoint: Some(vec!["/bin/sh".to_string()]),
        cmd: Some(vec!["-c".to_string(), "cp -a /from/. /to/".to_string()]),
        env: None,
        host_config: Some(HostConfig {
            binds: Some(vec![
                format!("{src_vol}:/from:ro"),
                format!("{dst_vol}:/to"),
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };
    // cname carries the destination id (unique per dev server).
    let cname = format!("hsm-game-copy-{dst_vol}");
    let outcome = run_installer_container(docker, &cname, cfg, write_progress).await;
    if outcome.code == 0 {
        Ok(())
    } else {
        Err(Error::ServiceUnavailable(format!(
            "Volumen-Kopie fehlgeschlagen (Exit-Code {}): {}",
            outcome.code, outcome.last_log
        )))
    }
}








