//! Catalogue Modrinth / CurseForge : recherche, téléchargement de mods et
//! installation de modpacks (.mrpack ou .zip CurseForge).
//!
//! Modrinth ne demande pas de clé. CurseForge exige une clé API, lue depuis
//! `CURSEFORGE_API_KEY` ou `~/.local/share/mcserv/config.toml`.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use rocket::serde::json::serde_json::{self, Value, json};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use sha2::Sha512;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::server_management::loaders::ModLoader;
use crate::server_management::server::{Server, ServerInfo, safe_file_name};

const USER_AGENT: &str = "mcserv/0.1 (gestionnaire de serveurs Minecraft)";
const MODRINTH_API: &str = "https://api.modrinth.com/v2";
const CURSEFORGE_API: &str = "https://api.curseforge.com/v1";
const CF_MINECRAFT_GAME_ID: u32 = 432;
const CF_MODS_CLASS_ID: u32 = 6;
const CF_MODPACKS_CLASS_ID: u32 = 4471;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

// ---------------------------------------------------------------------------
// Types publics
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Modrinth,
    CurseForge,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Modrinth => "modrinth",
            Source::CurseForge => "curseforge",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Mod,
    Modpack,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Mod => "mod",
            Kind::Modpack => "modpack",
        }
    }
}

#[derive(Serialize)]
pub struct CatalogConfig {
    pub curseforge_configured: bool,
    pub curseforge_source: Option<&'static str>,
}

#[derive(Serialize, Clone)]
pub struct CatalogItem {
    pub id: String,
    pub source: String,
    pub kind: String,
    pub slug: String,
    pub name: String,
    pub author: String,
    pub summary: String,
    pub icon_url: Option<String>,
    pub url: String,
    pub downloads: u64,
    pub follows: u64,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    pub server_side: Option<String>,
    pub client_side: Option<String>,
    pub updated: Option<String>,
}

#[derive(Serialize)]
pub struct CatalogPage {
    pub items: Vec<CatalogItem>,
    pub total: u64,
    pub offset: usize,
    pub limit: usize,
    pub source: String,
    pub kind: String,
}

#[derive(Deserialize)]
pub struct CatalogSearchRequest {
    pub source: Source,
    pub kind: Kind,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub game_version: Option<String>,
    #[serde(default)]
    pub loader: Option<ModLoader>,
    #[serde(default)]
    pub include_client: bool,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Serialize, Clone)]
pub struct CatalogVersion {
    pub id: String,
    pub name: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub downloads: u64,
    pub published: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub release_type: String,

    // Informations de téléchargement, jamais envoyées au client.
    #[serde(skip)]
    url: Option<String>,
    #[serde(skip)]
    sha1: Option<String>,
    #[serde(skip)]
    sha512: Option<String>,
    #[serde(skip)]
    dependencies: Vec<CatalogDependency>,
}

#[derive(Clone)]
struct CatalogDependency {
    project_id: Option<String>,
    version_id: Option<String>,
}

#[derive(Serialize)]
pub struct ModInstallReport {
    pub file_name: String,
    pub size: u64,
    pub version_name: String,
    pub version_id: String,
    /// Fichier principal + dépendances requises effectivement installées.
    pub installed_files: Vec<String>,
}

#[derive(Serialize)]
pub struct ModpackReport {
    pub name: String,
    pub mc_version: String,
    pub loader: ModLoader,
    pub loader_version: Option<String>,
    pub installed: usize,
    pub skipped: usize,
    pub overrides: usize,
    pub total_size: u64,
}

struct RemoteFile {
    url: String,
    file_name: String,
    sha1: Option<String>,
    sha512: Option<String>,
    version_name: String,
    version_id: String,
    dependencies: Vec<CatalogDependency>,
}

struct PackFile {
    relative_path: String,
    remote: RemoteFile,
    size: Option<u64>,
}

pub struct ModpackArchive {
    pub path: PathBuf,
    pub source: Source,
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    pub mc_version: String,
    pub loader: ModLoader,
    pub loader_version: Option<String>,
    files: Vec<PackFile>,
    skipped: usize,
    overrides_prefix: String,
}
// ---------------------------------------------------------------------------
// Configuration (clé CurseForge)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
struct StoredConfig {
    #[serde(default)]
    curseforge_api_key: String,
}

fn config_path() -> PathBuf {
    Server::base_dir().join("config.toml")
}

fn stored_config() -> StoredConfig {
    let path = config_path();
    fs::read_to_string(path)
        .ok()
        .and_then(|content| toml::from_str(&content).ok())
        .unwrap_or_default()
}

fn env_curseforge_key() -> Option<String> {
    env::var("CURSEFORGE_API_KEY")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn curseforge_key() -> Option<String> {
    env_curseforge_key().or_else(|| {
        let key = stored_config().curseforge_api_key.trim().to_string();
        (!key.is_empty()).then_some(key)
    })
}

pub fn config_info() -> CatalogConfig {
    let source = if env_curseforge_key().is_some() {
        Some("env")
    } else if curseforge_key().is_some() {
        Some("config")
    } else {
        None
    };
    CatalogConfig {
        curseforge_configured: source.is_some(),
        curseforge_source: source,
    }
}

pub fn set_curseforge_key(key: Option<String>) -> Result<CatalogConfig, String> {
    let mut config = stored_config();
    config.curseforge_api_key = key.unwrap_or_default().trim().to_string();

    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let toml = toml::to_string_pretty(&config).map_err(|e| e.to_string())?;
    fs::write(&path, toml).map_err(|e| e.to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    Ok(config_info())
}

// ---------------------------------------------------------------------------
// Dossiers temporaires
// ---------------------------------------------------------------------------

pub fn temp_dir() -> PathBuf {
    Server::base_dir().join("tmp")
}

pub fn new_temp_dir() -> Result<PathBuf, String> {
    let dir = temp_dir().join(Uuid::new_v4().to_string());
    fs::create_dir_all(&dir).map_err(|e| format!("dossier temporaire : {e}"))?;
    Ok(dir)
}

pub fn cleanup_temp() {
    let dir = temp_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|age| age > Duration::from_secs(24 * 60 * 60))
            .unwrap_or(false);
        if old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers HTTP
// ---------------------------------------------------------------------------

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .unwrap_or_default()
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status} : {}", truncate(&text)));
    }
    serde_json::from_str(&text).map_err(|e| format!("réponse invalide : {e}"))
}

async fn cf_get(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let key = curseforge_key()
        .ok_or_else(|| "clé API manquante. Ajoute-la dans l'onglet Catalogue.".to_string())?;
    let resp = client
        .get(url)
        .header("x-api-key", key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status} : {}", truncate(&text)));
    }
    serde_json::from_str(&text).map_err(|e| format!("réponse CurseForge invalide : {e}"))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn truncate(text: &str) -> String {
    const MAX: usize = 400;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let mut out: String = text.chars().take(MAX).collect();
    out.push('…');
    out
}
fn modrinth_loader(loader: ModLoader) -> Option<&'static str> {
    match loader {
        ModLoader::Fabric => Some("fabric"),
        ModLoader::Quilt => Some("quilt"),
        ModLoader::Forge => Some("forge"),
        ModLoader::NeoForge => Some("neoforge"),
        _ => None,
    }
}

fn loader_from_name(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "fabric" => Some("fabric"),
        "quilt" => Some("quilt"),
        "forge" => Some("forge"),
        "neoforge" => Some("neoforge"),
        "liteloader" => Some("liteloader"),
        _ => None,
    }
}

fn cf_loader_type(loader: ModLoader) -> Option<u32> {
    match loader {
        ModLoader::Forge => Some(1),
        ModLoader::Fabric => Some(4),
        ModLoader::Quilt => Some(5),
        ModLoader::NeoForge => Some(6),
        _ => None,
    }
}

fn loader_from_cf_type(value: u64) -> Option<&'static str> {
    match value {
        1 => Some("forge"),
        4 => Some("fabric"),
        5 => Some("quilt"),
        6 => Some("neoforge"),
        _ => None,
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn pick_version(versions: &[CatalogVersion]) -> Option<&CatalogVersion> {
    versions
        .iter()
        .find(|v| v.release_type == "release")
        .or_else(|| versions.first())
}

// ---------------------------------------------------------------------------
// Recherche
// ---------------------------------------------------------------------------

pub async fn search(request: CatalogSearchRequest) -> Result<CatalogPage, String> {
    let limit = request.limit.unwrap_or(20).clamp(1, 50);
    let offset = request.offset.unwrap_or(0).min(10_000);
    match request.source {
        Source::Modrinth => {
            modrinth::search(
                request.kind,
                &request.query,
                request.game_version.as_deref(),
                request.loader,
                request.include_client,
                offset,
                limit,
            )
            .await
        }
        Source::CurseForge => {
            curseforge::search(
                request.kind,
                &request.query,
                request.game_version.as_deref(),
                request.loader,
                offset,
                limit,
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Versions
// ---------------------------------------------------------------------------

pub async fn versions(
    source: Source,
    project_id: &str,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
) -> Result<Vec<CatalogVersion>, String> {
    match source {
        Source::Modrinth => modrinth::versions(project_id, game_version, loader).await,
        Source::CurseForge => curseforge::versions(project_id, game_version, loader).await,
    }
}

mod curseforge;
mod download;
mod modpack;
mod modrinth;
mod resolve;

pub use download::install_mod;
pub use modpack::{check_compatibility, fetch_modpack, install_modpack};
