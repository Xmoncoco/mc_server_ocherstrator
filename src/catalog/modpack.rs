use super::curseforge::curseforge_file_entry;
use super::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct MrIndex {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    dependencies: HashMap<String, String>,
    #[serde(default)]
    files: Vec<MrIndexFile>,
}

#[derive(Deserialize)]
struct MrIndexFile {
    path: String,
    #[serde(default)]
    hashes: HashMap<String, String>,
    #[serde(default)]
    env: Option<MrEnv>,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default, rename = "fileSize")]
    file_size: Option<u64>,
}

#[derive(Deserialize)]
struct MrEnv {
    #[serde(default)]
    server: Option<String>,
}

#[derive(Deserialize)]
struct CfManifest {
    minecraft: CfMinecraft,
    #[serde(default)]
    files: Vec<CfManifestFile>,
    #[serde(default)]
    overrides: Option<String>,
}

#[derive(Deserialize)]
struct CfMinecraft {
    version: String,
    #[serde(default, rename = "modLoaders")]
    mod_loaders: Vec<CfLoader>,
}

#[derive(Deserialize)]
struct CfLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
struct CfManifestFile {
    #[serde(rename = "projectID")]
    project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    #[serde(default = "default_true")]
    required: bool,
}

fn default_true() -> bool {
    true
}

pub async fn fetch_modpack(
    source: Source,
    project_id: &str,
    version_id: Option<&str>,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
    work_dir: &Path,
) -> Result<ModpackArchive, String> {
    let remote = resolve::remote_file(source, project_id, version_id, game_version, loader).await?;
    if !(remote.file_name.to_ascii_lowercase().ends_with(".zip")
        || remote.file_name.to_ascii_lowercase().ends_with(".mrpack"))
    {
        return Err(format!(
            "Le fichier « {} » n'est pas une archive de modpack (.zip/.mrpack).",
            remote.file_name
        ));
    }

    tokio::fs::create_dir_all(work_dir)
        .await
        .map_err(|e| e.to_string())?;
    let archive_path = work_dir.join(&remote.file_name);
    let client = client();
    download::download_file(&client, &remote, &archive_path).await?;

    match source {
        Source::Modrinth => {
            parse_modrinth_pack(project_id, &remote.version_id, &archive_path).await
        }
        Source::CurseForge => {
            parse_curseforge_pack(project_id, &remote.version_id, &archive_path).await
        }
    }
}

async fn parse_modrinth_pack(
    project_id: &str,
    version_id: &str,
    archive_path: &Path,
) -> Result<ModpackArchive, String> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive_path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("archive .mrpack invalide : {e}"))?;

    let index: MrIndex = {
        let mut entry = zip
            .by_name("modrinth.index.json")
            .map_err(|_| "modrinth.index.json absent de l'archive".to_string())?;
        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| format!("modrinth.index.json invalide : {e}"))?
    };

    let mc_version = index
        .dependencies
        .get("minecraft")
        .cloned()
        .ok_or_else(|| "Le modpack ne précise pas sa version Minecraft.".to_string())?;

    let loader_info = [
        ("neoforge", ModLoader::NeoForge),
        ("forge", ModLoader::Forge),
        ("quilt-loader", ModLoader::Quilt),
        ("quilt", ModLoader::Quilt),
        ("fabric-loader", ModLoader::Fabric),
        ("fabric", ModLoader::Fabric),
    ]
    .into_iter()
    .find_map(|(key, loader)| {
        index
            .dependencies
            .get(key)
            .map(|version| (loader, version.clone()))
    })
    .ok_or_else(|| {
        "Le modpack ne précise pas de mod loader (Fabric, Quilt, Forge ou NeoForge).".to_string()
    })?;

    let mut files = Vec::new();
    let mut skipped = 0usize;
    for file in index.files {
        if file.env.as_ref().and_then(|env| env.server.as_deref()) == Some("unsupported") {
            skipped += 1;
            continue;
        }
        let Some(url) = file.downloads.into_iter().next() else {
            skipped += 1;
            continue;
        };
        let file_name = Path::new(&file.path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .ok_or_else(|| format!("chemin de fichier invalide : {}", file.path))?;

        files.push(PackFile {
            relative_path: file.path,
            remote: RemoteFile {
                url,
                file_name,
                sha1: file.hashes.get("sha1").cloned(),
                sha512: file.hashes.get("sha512").cloned(),
                version_name: String::new(),
                version_id: String::new(),
                dependencies: Vec::new(),
            },
            size: file.file_size,
        });
    }

    let name = match index.name {
        Some(name) if !name.trim().is_empty() => name,
        _ => modrinth::project_title(project_id)
            .await
            .unwrap_or_else(|_| "Modpack".to_string()),
    };

    Ok(ModpackArchive {
        path: archive_path.to_path_buf(),
        source: Source::Modrinth,
        project_id: project_id.to_string(),
        version_id: version_id.to_string(),
        name,
        mc_version,
        loader: loader_info.0,
        loader_version: Some(loader_info.1),
        files,
        skipped,
        overrides_prefix: "overrides".to_string(),
    })
}

async fn parse_curseforge_pack(
    project_id: &str,
    version_id: &str,
    archive_path: &Path,
) -> Result<ModpackArchive, String> {
    let mut zip = zip::ZipArchive::new(fs::File::open(archive_path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("archive .zip invalide : {e}"))?;

    let manifest: CfManifest = {
        let mut entry = zip
            .by_name("manifest.json")
            .map_err(|_| "manifest.json absent de l'archive".to_string())?;
        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| format!("manifest.json invalide : {e}"))?
    };

    let loader_entry = manifest
        .minecraft
        .mod_loaders
        .iter()
        .find(|loader| loader.primary)
        .or_else(|| manifest.minecraft.mod_loaders.first())
        .ok_or_else(|| "Le modpack ne précise pas de mod loader.".to_string())?;

    let (loader_name, loader_version) = loader_entry
        .id
        .split_once('-')
        .map(|(name, version)| (name, Some(version.to_string())))
        .unwrap_or((loader_entry.id.as_str(), None));

    let loader = match loader_name.to_ascii_lowercase().as_str() {
        "forge" => ModLoader::Forge,
        "neoforge" => ModLoader::NeoForge,
        "fabric" => ModLoader::Fabric,
        "quilt" => ModLoader::Quilt,
        other => return Err(format!("Loader CurseForge non supporté : {other}")),
    };

    let project = cf_get(&client(), &format!("{CURSEFORGE_API}/mods/{project_id}"))
        .await
        .map_err(|e| format!("CurseForge : {e}"))?;
    let name = project["data"]["name"]
        .as_str()
        .unwrap_or("Modpack")
        .to_string();

    let mut files = Vec::new();
    let mut skipped = 0usize;
    for file in manifest.files {
        if !file.required {
            skipped += 1;
            continue;
        }

        let entry = curseforge_file_entry(
            Some(&file.project_id.to_string()),
            &file.file_id.to_string(),
        )
        .await?;

        let has_client = entry
            .game_versions
            .iter()
            .any(|version| version.eq_ignore_ascii_case("client"));
        let has_server = entry
            .game_versions
            .iter()
            .any(|version| version.eq_ignore_ascii_case("server"));
        if has_client && !has_server {
            skipped += 1;
            continue;
        }

        let Some(url) = entry.url.clone() else {
            return Err(format!(
                "Le fichier CurseForge « {} » n'a pas d'URL de téléchargement (l'auteur l'a peut-être désactivée).",
                entry.file_name.unwrap_or_else(|| file.file_id.to_string())
            ));
        };

        let file_name = entry
            .file_name
            .clone()
            .ok_or_else(|| format!("nom de fichier absent pour le fichier {}", file.file_id))?;
        let file_name = safe_file_name(&file_name).map_err(|e| e.to_string())?;

        files.push(PackFile {
            relative_path: format!("mods/{file_name}"),
            size: entry.file_size,
            remote: RemoteFile {
                url,
                file_name,
                sha1: entry.sha1,
                sha512: None,
                version_name: entry.name,
                version_id: entry.id,
                dependencies: Vec::new(),
            },
        });
    }

    Ok(ModpackArchive {
        path: archive_path.to_path_buf(),
        source: Source::CurseForge,
        project_id: project_id.to_string(),
        version_id: version_id.to_string(),
        name,
        mc_version: manifest.minecraft.version,
        loader,
        loader_version,
        files,
        skipped,
        overrides_prefix: manifest
            .overrides
            .unwrap_or_else(|| "overrides".to_string())
            .trim_end_matches('/')
            .to_string(),
    })
}

pub fn check_compatibility(server: &ServerInfo, archive: &ModpackArchive) -> Result<(), String> {
    let Some(server_mc) = server.mc_version.as_deref() else {
        return Err(
            "Ce serveur n'a pas de version Minecraft enregistrée : crée un nouveau serveur depuis le modpack."
                .to_string(),
        );
    };
    if server_mc != archive.mc_version {
        return Err(format!(
            "Ce modpack cible Minecraft {} ({}), mais le serveur est en Minecraft {} ({}). Crée un serveur dédié depuis le catalogue.",
            archive.mc_version,
            loader_label(archive.loader),
            server_mc,
            loader_label(server.mod_loader)
        ));
    }
    if server.mod_loader != archive.loader {
        return Err(format!(
            "Ce modpack utilise {}, mais le serveur utilise {}. Crée un serveur dédié depuis le catalogue.",
            loader_label(archive.loader),
            loader_label(server.mod_loader)
        ));
    }
    if archive.loader == ModLoader::Custom || archive.loader == ModLoader::Vanilla {
        return Err("Ce modpack n'utilise pas de mod loader compatible.".to_string());
    }
    Ok(())
}

fn loader_label(loader: ModLoader) -> &'static str {
    match loader {
        ModLoader::Vanilla => "Vanilla",
        ModLoader::Paper => "Paper",
        ModLoader::Fabric => "Fabric",
        ModLoader::Quilt => "Quilt",
        ModLoader::Forge => "Forge",
        ModLoader::NeoForge => "NeoForge",
        ModLoader::Custom => "un loader inconnu",
    }
}

pub async fn install_modpack(
    server_path: &Path,
    archive: &ModpackArchive,
) -> Result<ModpackReport, String> {
    let client = client();
    let mut installed = 0usize;
    let mut total_size = 0u64;

    for file in &archive.files {
        let relative = safe_relative_path(&file.relative_path)?;
        let dest = server_path.join(relative);
        let size = download::download_file(&client, &file.remote, &dest).await?;
        if let Some(expected) = file.size {
            if expected != size {
                return Err(format!(
                    "Taille inattendue pour {} ({} octets au lieu de {}).",
                    file.remote.file_name, size, expected
                ));
            }
        }
        total_size += size;
        installed += 1;
    }

    let overrides = extract_overrides(archive, server_path).await?;

    let marker_dir = server_path.join(".mcserv");
    fs::create_dir_all(&marker_dir).map_err(|e| e.to_string())?;
    let marker = json!({
        "name": archive.name,
        "source": archive.source.as_str(),
        "project_id": archive.project_id,
        "version_id": archive.version_id,
        "mc_version": archive.mc_version,
        "loader": archive.loader,
        "loader_version": archive.loader_version,
        "installed_at": SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    fs::write(
        marker_dir.join("modpack.json"),
        serde_json::to_string_pretty(&marker).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    Ok(ModpackReport {
        name: archive.name.clone(),
        mc_version: archive.mc_version.clone(),
        loader: archive.loader,
        loader_version: archive.loader_version.clone(),
        installed,
        skipped: archive.skipped,
        overrides,
        total_size,
    })
}

async fn extract_overrides(archive: &ModpackArchive, server_path: &Path) -> Result<usize, String> {
    let mut zip = zip::ZipArchive::new(fs::File::open(&archive.path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    let base = archive.overrides_prefix.trim_matches('/');
    if base.is_empty() {
        return Ok(0);
    }
    let prefix = format!("{base}/");
    let mut written = 0usize;

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().map_err(|e| e.to_string())?.to_string();
        if !name.starts_with(&prefix) {
            continue;
        }
        let relative_raw = &name[prefix.len()..];
        if relative_raw.is_empty() || relative_raw.ends_with('/') {
            continue;
        }
        let relative = safe_relative_path(relative_raw)?;

        // Les liens symboliques d'une archive ne sont jamais extraits.
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                continue;
            }
        }

        let target = server_path.join(&relative);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }

        let mut content = Vec::new();
        entry.read_to_end(&mut content).map_err(|e| e.to_string())?;
        tokio::fs::write(&target, content)
            .await
            .map_err(|e| e.to_string())?;
        written += 1;
    }

    Ok(written)
}

fn safe_relative_path(raw: &str) -> Result<PathBuf, String> {
    let normalized = raw.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(format!("chemin absolu interdit : {raw}"));
    }

    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part
                    .to_str()
                    .ok_or_else(|| format!("chemin non UTF-8 interdit : {raw}"))?;
                if text == ".mcserv" {
                    return Err("un modpack ne peut pas écrire dans .mcserv".to_string());
                }
                clean.push(part);
            }
            Component::CurDir => {}
            _ => return Err(format!("chemin interdit : {raw}")),
        }
    }

    if clean.as_os_str().is_empty() || clean.starts_with(".mcserv") {
        return Err(format!("chemin invalide : {raw}"));
    }
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chemins_relatifs() {
        assert!(safe_relative_path("mods/a.jar").is_ok());
        assert!(safe_relative_path("../evil.jar").is_err());
        assert!(safe_relative_path("/etc/passwd").is_err());
        assert!(safe_relative_path(".mcserv/modpack.json").is_err());
        assert!(safe_relative_path("config/../mods/a.jar").is_err());
    }

    #[test]
    fn parse_loader() {
        assert_eq!(modrinth_loader(ModLoader::Fabric), Some("fabric"));
        assert_eq!(modrinth_loader(ModLoader::Paper), None);
        assert_eq!(loader_from_cf_type(6), Some("neoforge"));
    }
}
