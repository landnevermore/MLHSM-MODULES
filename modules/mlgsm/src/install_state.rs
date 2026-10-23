//! Install-progress files: one small JSON per server id, written
// by the installer loop and polled by the API while a server is installing.

use super::*;
// â”€â”€ Install progress â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Directory holding live install-progress files, one per server id.
pub fn game_install_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("game-install")
}

pub(crate) fn install_state_path(data_dir: &std::path::Path, server_id: &str) -> PathBuf {
    game_install_dir(data_dir).join(format!("{server_id}.json"))
}

/// Read the live install progress for a server (None if not installing).
pub fn read_install_state(data_dir: &std::path::Path, server_id: &str) -> Option<InstallProgress> {
    let bytes = std::fs::read(install_state_path(data_dir, server_id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn write_install_state(data_dir: &std::path::Path, server_id: &str, progress: &InstallProgress) {
    let path = install_state_path(data_dir, server_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(bytes) = serde_json::to_vec(progress) else {
        return;
    };
    let _ = std::fs::write(&path, bytes);
}

/// Remove the install-progress file (server done / deleted).
pub fn clear_install_state(data_dir: &std::path::Path, server_id: &str) {
    let _ = std::fs::remove_file(install_state_path(data_dir, server_id));
}

/// Extract a download percentage from an installer log line. Handles
/// steamcmd (Update state (0x61) downloading, progress: 45.21 (...)),
/// wget/curl progress bars (45.2%) and plain NN% echo lines.
pub(crate) fn parse_percent(line: &str) -> Option<f64> {
    // steamcmd: "progress: 45.21" / "progress = 45.21"
    if let Some(idx) = line.find("progress") {
        let tail = &line[idx..];
        if let Some(colon) = tail.find([':', '=']) {
            let num = &tail[colon + 1..];
            if let Some(v) = first_float(num) {
                if (0.0..=100.0).contains(&v) {
                    return Some(v);
                }
            }
        }
    }
    // wget/curl/"NN%": last NN.N% token in the line.
    let mut best: Option<f64> = None;
    let mut rest = line;
    while let Some(pct) = rest.find('%') {
        let before = &rest[..pct];
        let start = before
            .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
            .map(|i| i + 1)
            .unwrap_or(0);
        if let Ok(v) = before[start..].parse::<f64>() {
            if (0.0..=100.0).contains(&v) {
                best = Some(v);
            }
        }
        rest = &rest[pct + 1..];
    }
    best
}

/// Parse the first float at the start of s (allowing a leading dot .5).
fn first_float(s: &str) -> Option<f64> {
    let mut digits = String::new();
    let mut seen_dot = false;
    let mut start = 0;
    while start < s.len() && !s.as_bytes()[start].is_ascii_digit() {
        start += 1;
    }
    for c in s[start..].chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else if c == '.' && !seen_dot {
            seen_dot = true;
            digits.push(c);
        } else {
            break;
        }
    }
    if digits.is_empty() || digits == "." {
        return None;
    }
    digits.parse::<f64>().ok()
}

// â”€â”€ Docker lifecycle â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€


