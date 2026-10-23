//! Game-server file-management types (volume browser / editor).

use serde::{Deserialize, Serialize};

/// One entry inside a server volume directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    #[serde(default)]
    pub mtime: String,
}

/// Result of listing one server-volume directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

/// `POST /games/files/{id}/write` — overwrite / create one file.
#[derive(Debug, Clone, Deserialize)]
pub struct WriteFileRequest {
    /// Volume-relative path (e.g. `cfg/autoexec.cfg`).
    pub path: String,
    /// Raw file content as base64 (binary-safe).
    pub content_base64: String,
}

/// `POST /games/files/{id}/mkdir` — create one directory.
#[derive(Debug, Clone, Deserialize)]
pub struct MkdirRequest {
    pub path: String,
}

/// `POST /games/files/{id}/rename` — rename or move a file / directory.
#[derive(Debug, Clone, Deserialize)]
pub struct RenameRequest {
    pub from: String,
    pub to: String,
}

/// Uniform response for destructive / ack-only file ops.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpResult {
    pub ok: bool,
    #[serde(default)]
    pub message: String,
}
