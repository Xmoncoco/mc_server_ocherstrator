//! Téléchargement et installation des serveurs Minecraft et de leurs
//! mod loaders (Vanilla, Paper, Fabric, Quilt, Forge, NeoForge).
//!
//! Toutes les informations de version viennent des API officielles : rien
//! n'est codé en dur, ce qui évite de casser au prochain changement de
//! numérotation de Minecraft.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rocket::serde::json::serde_json::Value;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

const USER_AGENT: &str = "mcserv/0.1 (gestionnaire de serveurs Minecraft)";
const MOJANG_MANIFEST: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";
const FABRIC_META: &str = "https://meta.fabricmc.net/v2";
const QUILT_META: &str = "https://meta.quiltmc.org/v3";
const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

// ---------------------------------------------------------------------------
// Mod loaders
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModLoader {
    /// Serveur fourni à la main (jar ou exécutable), sans loader connu.
    #[default]
    Custom,
    Vanilla,
    Paper,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

// ---------------------------------------------------------------------------
// Réponse de l'endpoint /loaders
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct LoaderDesc {
    pub id: &'static str,
    pub name: &'static str,
    /// Peut être installé automatiquement par le serveur.
    pub auto: bool,
    /// Charge des mods depuis le dossier `mods/`.
    pub mods: bool,
    pub note: &'static str,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GameVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Serialize)]
pub struct LoadersResponse {
    pub loaders: Vec<LoaderDesc>,
    pub game_versions: Vec<GameVersion>,
    pub latest: Option<String>,
    /// Renseigné si l'API Mojang n'a pas pu être jointe.
    pub warning: Option<String>,
}

pub fn supported_loaders() -> Vec<LoaderDesc> {
    vec![
        LoaderDesc {
            id: "vanilla",
            name: "Vanilla",
            auto: true,
            mods: false,
            note: "Serveur officiel Mojang, sans mods.",
        },
        LoaderDesc {
            id: "paper",
            name: "Paper",
            auto: true,
            mods: false,
            note: "Performant, supporte les plugins (pas les mods).",
        },
        LoaderDesc {
            id: "fabric",
            name: "Fabric",
            auto: true,
            mods: true,
            note: "Léger et rapide, idéal pour les mods modernes.",
        },
        LoaderDesc {
            id: "quilt",
            name: "Quilt",
            auto: true,
            mods: true,
            note: "Successeur communautaire de Fabric.",
        },
        LoaderDesc {
            id: "forge",
            name: "Forge",
            auto: true,
            mods: true,
            note: "Le loader historique, large compatibilité.",
        },
        LoaderDesc {
            id: "neoforge",
            name: "NeoForge",
            auto: true,
            mods: true,
            note: "Fork moderne de Forge pour les versions récentes.",
        },
    ]
}

#[derive(Deserialize)]
struct Manifest {
    latest: ManifestLatest,
    versions: Vec<ManifestVersion>,
}

#[derive(Deserialize)]
struct ManifestLatest {
    release: String,
}

#[derive(Deserialize)]
struct ManifestVersion {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

/// Versions de jeu publiées par Mojang (uniquement les releases).
pub async fn game_versions() -> (Vec<GameVersion>, Option<String>, Option<String>) {
    let client = client();
    match get_json::<Manifest>(&client, MOJANG_MANIFEST).await {
        Ok(manifest) => {
            let versions = manifest
                .versions
                .into_iter()
                .filter(|v| v.kind == "release")
                .map(|v| GameVersion {
                    id: v.id,
                    kind: v.kind,
                })
                .collect();
            (versions, Some(manifest.latest.release), None)
        }
        Err(e) => (
            Vec::new(),
            None,
            Some(format!("Impossible de récupérer les versions Mojang : {e}")),
        ),
    }
}

pub async fn info() -> LoadersResponse {
    let (game_versions, latest, warning) = game_versions().await;
    LoadersResponse {
        loaders: supported_loaders(),
        game_versions,
        latest,
        warning,
    }
}

// ---------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------

pub struct Installed {
    /// Nom du fichier à lancer, relatif au dossier de l'instance.
    pub executable: String,
    /// Fichier d'arguments Java (Forge/NeoForge) : lancé avec `@fichier`.
    pub launch_args: Option<String>,
    pub loader_version: Option<String>,
}

/// Installe le loader dans `dir` et renvoie quoi lancer.
pub async fn install(
    loader: ModLoader,
    mc_version: &str,
    loader_version: Option<&str>,
    java_path: &str,
    dir: &Path,
    accept_eula: bool,
) -> Result<Installed, String> {
    if !valid_version(mc_version) {
        return Err(format!("Version Minecraft invalide : {mc_version}"));
    }
    if let Some(v) = loader_version {
        if !valid_version(v) {
            return Err(format!("Version de loader invalide : {v}"));
        }
    }

    let client = client();
    let installed = match loader {
        ModLoader::Vanilla => install_vanilla(&client, mc_version, dir).await?,
        ModLoader::Paper => install_paper(&client, mc_version, dir).await?,
        ModLoader::Fabric => install_fabric(&client, mc_version, loader_version, dir).await?,
        ModLoader::Quilt => {
            install_quilt(&client, mc_version, loader_version, java_path, dir).await?
        }
        ModLoader::Forge => {
            install_forge(&client, mc_version, loader_version, java_path, dir).await?
        }
        ModLoader::NeoForge => {
            install_neoforge(&client, mc_version, loader_version, java_path, dir).await?
        }
        ModLoader::Custom => {
            return Err("Aucun loader à installer pour un serveur personnalisé".into());
        }
    };

    if accept_eula {
        write_eula(dir).await?;
    }

    Ok(installed)
}

// ---------------------------------------------------------------------------
// Aides
// ---------------------------------------------------------------------------

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .unwrap_or_default()
}

async fn get_json<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> Result<T, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    resp.json::<T>()
        .await
        .map_err(|e| format!("réponse invalide : {e}"))
}

async fn download(client: &reqwest::Client, url: &str, dest: &Path) -> Result<(), String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("téléchargement de {url} : {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("téléchargement de {url} : HTTP {status}"));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("téléchargement de {url} : {e}"))?;
    if bytes.is_empty() {
        return Err(format!("téléchargement de {url} : fichier vide"));
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    tokio::fs::write(dest, &bytes)
        .await
        .map_err(|e| format!("écriture de {} : {e}", dest.display()))?;
    Ok(())
}

async fn run_java(
    java_path: &str,
    dir: &Path,
    jar: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let jar_name = jar
        .file_name()
        .ok_or_else(|| "nom d'installateur invalide".to_string())?;

    let output = tokio::time::timeout(
        timeout,
        Command::new(java_path)
            .current_dir(dir)
            .arg("-jar")
            .arg(jar_name)
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| {
        format!(
            "délai dépassé ({} min) — vérifie que Java est installé et que la machine a accès à Internet",
            timeout.as_secs() / 60
        )
    })?
    .map_err(|e| format!("impossible de lancer « {java_path} » : {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        return Err(format!(
            "l'installateur a échoué ({}) : {}",
            output.status,
            tail(&combined)
        ));
    }
    Ok(combined)
}

async fn write_eula(dir: &Path) -> Result<(), String> {
    tokio::fs::write(dir.join("eula.txt"), "eula=true\n")
        .await
        .map_err(|e| format!("écriture de eula.txt : {e}"))
}

fn tail(text: &str) -> String {
    const MAX: usize = 3000;
    let mut chars: Vec<char> = text.chars().rev().take(MAX).collect();
    chars.reverse();
    chars.into_iter().collect()
}

fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

fn safe_file_name(raw: &str) -> Result<String, String> {
    let name = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "nom de fichier invalide".to_string())?;
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err("nom de fichier invalide".into());
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixe_neoforge() {
        assert_eq!(modded::neoforge_prefix("1.21"), "21.0");
        assert_eq!(modded::neoforge_prefix("1.21.1"), "21.1");
        assert_eq!(modded::neoforge_prefix("1.20.4"), "20.4");
        assert_eq!(modded::neoforge_prefix("26.3"), "26.3");
    }

    #[test]
    fn versions_valides() {
        assert!(valid_version("1.20.1"));
        assert!(valid_version("26.3"));
        assert!(!valid_version("../etc/passwd"));
        assert!(!valid_version(""));
    }

    #[test]
    fn choix_forge() {
        let all = vec![
            "1.21-51.0.30".to_string(),
            "1.21-51.0.33".to_string(),
            "1.20.1-47.2.0".to_string(),
            "26.3-66.0.13".to_string(),
        ];
        assert_eq!(
            modded::pick_forge_version(&all, "1.21", None).as_deref(),
            Some("1.21-51.0.33")
        );
        assert_eq!(
            modded::pick_forge_version(&all, "1.21", Some("51.0.30")).as_deref(),
            Some("1.21-51.0.30")
        );
    }
}

mod modded;
mod vanilla;

use modded::{install_fabric, install_forge, install_neoforge, install_quilt};
use vanilla::{install_paper, install_vanilla};
