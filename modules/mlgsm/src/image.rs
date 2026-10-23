//! Image pull with progress. Kept apart from the deploy flow because
// the pull stream behaves differently on Podman and Docker and is the one
// place where a container engine quirk leaked into a long function.

use super::*;
pub const CONT_NAME_PREFIX: &str = "hsm-game-";
pub(crate) fn container_name(server_id: &str) -> String {
    format!("{CONT_NAME_PREFIX}{server_id}")
}

/// Pull an image if it is not already local. Best effort: errors surface on
/// the subsequent create call if the image is really unreachable.
///
/// Consumes the whole pull stream so downloads actually complete; emits
/// progress into `on_progress(image, percent, status)` as the stream runs.
pub async fn pull_image(
    docker: &EngineClient,
    image: &str,
    mut on_progress: impl FnMut(Option<f64>, String),
) {
    let image = if image.contains(':') {
        image.to_string()
    } else {
        format!("{image}:latest")
    };
    // Podman keeps the pull stream open after "Download complete" instead of
    // closing it or sending "Pull complete". Skip the whole pull when the
    // image already exists locally.
    if docker.inspect_image(image.as_str()).await.is_ok() {
        on_progress(Some(100.0), "Bild ist vorhanden".into());
        return;
    }
    let options = CreateImageOptionsBuilder::default()
        .from_image(image.as_str())
        .build();
    let mut stream = docker.create_image(Some(options), None, None);
    let mut last_status = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(info) => {
                let status = info.status.unwrap_or_default();
                if !status.is_empty() {
                    last_status = status.clone();
                }
                let percent = info
                    .progress_detail
                    .as_ref()
                    .filter(|d| d.total.is_some() && d.total != Some(0))
                    .map(|d| {
                        let cur = d.current.unwrap_or(0) as f64;
                        let tot = d.total.unwrap_or(1) as f64;
                        (cur / tot) * 100.0
                    });
                on_progress(percent, last_status.clone());
            }
            Err(e) => {
                tracing::warn!(image, error = %e, "image pull skipped");
                return;
            }
        }
    }
}

/// Simple pull with no progress reporting.
pub async fn ensure_image(docker: &EngineClient, image: &str) {
    pull_image(docker, image, |_, _| {}).await;
}



