//! Game-server data types.
//!
//! `Egg` is MLHSM's native install template. The on-disk format is the widely
//! used `PTDL_v2` JSON template schema, so existing third-party template
//! collections can be imported without conversion. MLHSM's game-server engine
//! is fully native: no external panel is bundled, contacted or required.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

/// Deserializer: accept explicit JSON `null` or missing → `T::default()`.
/// Handles real-world `PTDL_v2` exports where optional fields appear as
/// `null` rather than being absent.
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

// ── Install template (PTDL_v2 compatible) ──────────────────────────────────

/// An install template: defines how a game server is installed and started.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Egg {
    #[serde(default)]
    pub meta: EggMeta,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, deserialize_with = "null_default")]
    pub features: Vec<String>,
    /// Label -> container image reference, e.g. "Java 21" -> "ghcr.io/pterodactyl/yolks:java_21".
    #[serde(default, deserialize_with = "null_default")]
    pub docker_images: BTreeMap<String, String>,
    /// PTDL_v1 eggs use a plain image list instead of `docker_images`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    /// Display grouping for the egg picker (set from the folder layout).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    /// The run command template; `{{VARNAME}}` placeholders are substituted.
    #[serde(default)]
    pub startup: String,
    #[serde(default)]
    pub config: EggConfig,
    #[serde(default, deserialize_with = "null_default")]
    pub scripts: EggScripts,
    #[serde(default, deserialize_with = "null_default")]
    pub variables: Vec<EggVariable>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EggMeta {
    #[serde(default)]
    pub version: String,
    #[serde(rename = "update_url", default)]
    pub update_url: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EggConfig {
    /// Stop command sent to the server console.
    #[serde(default)]
    pub stop: String,
    /// Parser configuration for on-disk config files (e.g. `server.properties`).
    /// Kept raw; HSM does not rewrite arbitrary config files yet.
    #[serde(default)]
    pub files: String,
    /// Startup-log parse rules ("done" detection).
    #[serde(default)]
    pub startup: String,
    #[serde(default)]
    pub logs: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EggScripts {
    #[serde(default)]
    pub installation: EggInstallation,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EggInstallation {
    /// Shell script run inside the installer container to fetch files.
    #[serde(default)]
    pub script: String,
    /// Installer container image (e.g. `ghcr.io/pterodactyl/installers:alpine`).
    #[serde(default)]
    pub container: String,
    /// Entrypoint for the installer container, typically `ash`.
    #[serde(default)]
    pub entrypoint: String,
}

/// A server-scoped configuration variable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EggVariable {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "env_variable", default)]
    pub env_variable: String,
    #[serde(rename = "default_value", default)]
    pub default_value: String,
    #[serde(rename = "user_viewable", default)]
    pub user_viewable: bool,
    #[serde(rename = "user_editable", default)]
    pub user_editable: bool,
    #[serde(default)]
    pub rules: String,
    #[serde(rename = "field_type", default)]
    pub field_type: String,
}

// ── Running server ─────────────────────────────────────────────────────────

/// Live install progress for a game server being deployed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InstallProgress {
    /// Current deploy phase: "pull" (image), "install" (installer),
    /// "start" (game container).
    pub phase: String,
    /// Download percent from the installer log, when parseable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    /// Estimated seconds remaining, derived from elapsed time + percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
    /// Last non-empty installer log line (trimmed).
    #[serde(default)]
    pub last_log: String,
}

/// A deployed game server: an egg + chosen image + resolved variables.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GameServer {
    pub id: String,
    pub name: String,
    pub egg_name: String,
    pub image: String,
    /// Env-variable values (egg defaults already merged in).
    pub variables: BTreeMap<String, String>,
    pub status: GameServerStatus,
    /// Container name; also the docker-compose/lifecycle identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// For dev copies: id of the parent (main) server this data was cloned from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// `"main"` for regular servers, `"dev"` for dev copies of a main server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameServerStatus {
    Installing,
    Running,
    Offline,
    Error,
}

impl GameServerStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GameServerStatus::Installing => "installing",
            GameServerStatus::Running => "running",
            GameServerStatus::Offline => "offline",
            GameServerStatus::Error => "error",
        }
    }

    /// Whether this status represents a server that is genuinely live — the
    /// engine container is running right now. `Installing` is deliberately
    /// excluded: installing is not serving.
    pub fn is_running(&self) -> bool {
        matches!(self, GameServerStatus::Running)
    }
}

/// Request to deploy a new game server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CreateGameServerRequest {
    pub name: String,
    /// Egg to use; matched by file name without extension.
    pub egg: String,
    /// Docker image label from the egg, e.g. "Java 21". Defaults to the first.
    /// Empty string selects the egg default.
    #[serde(default)]
    pub image: String,
    /// Values for the egg's editable variables. Missing variables fall back
    /// to the egg default_value.
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
}

// ── Guided-install presets ────────────────────────────────────────────────

/// A guided preset rendered by the no-knowledge creation wizard. Each preset
/// maps to exactly one egg plus a curated set of variables presented as
/// plain-language fields.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GamePreset {
    pub preset_id: String,
    pub label: String,
    pub subtitle: String,
    /// Egg name the preset deploys.
    pub egg: String,
    /// Docker image label from the egg; empty string = egg default.
    pub image: String,
    /// Category used for the game logo lookup.
    pub category: String,
    /// Curated fields shown in the wizard (env_variable → input).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<PresetField>,
    /// Whether the wizard offers "Dev-Server" copies for this preset.
    pub dev_supported: bool,
}

/// One wizard field. `key` is the egg env_variable written to `variables`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetField {
    pub key: String,
    pub label: String,
    /// `"text"` | `"number"` | `"select"` | `"boolean"`.
    pub kind: String,
    pub default: String,
    /// For `"select"` fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<PresetOption>,
    /// Plain-language hint shown under the field.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub helper: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetOption {
    pub value: String,
    pub label: String,
}

/// Request to clone a main server into an isolated dev copy.
/// Everything is optional: name defaults to "<parent> (Dev)", the port to the
/// parent's port + 1 (next free).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CreateDevServerRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "opt_string_or_number")]
    pub port: Option<String>,
}

/// Accepts `null`, a JSON number or a string for a port field — the desktop
/// GUI sends numbers, external callers may send strings.
fn opt_string_or_number<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserializer.deserialize_any(StringOrNumberVisitor)
}

struct StringOrNumberVisitor;

impl<'de> serde::de::Visitor<'de> for StringOrNumberVisitor {
    type Value = Option<String>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a string, a number or null")
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
        Ok(Some(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
        Ok(Some(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
        Ok(Some(v.to_string()))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
        Ok(Some(v.to_string()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
        Ok(Some((v as u64).to_string()))
    }
}
