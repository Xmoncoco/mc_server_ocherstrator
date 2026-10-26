use super::*;

pub(super) async fn install_fabric(
    client: &reqwest::Client,
    mc_version: &str,
    loader_version: Option<&str>,
    dir: &Path,
) -> Result<Installed, String> {
    let url = format!("{FABRIC_META}/versions/loader/{mc_version}");
    let loaders: Vec<Value> = get_json(client, &url)
        .await
        .map_err(|e| format!("Fabric {mc_version} : {e}"))?;
    if loaders.is_empty() {
        return Err(format!("Fabric ne supporte pas Minecraft {mc_version}"));
    }

    let loader = match loader_version {
        Some(v) => v.to_string(),
        None => loaders
            .iter()
            .find(|e| e.pointer("/loader/stable").and_then(Value::as_bool) == Some(true))
            .or_else(|| loaders.first())
            .and_then(|e| e.pointer("/loader/version").and_then(Value::as_str))
            .ok_or_else(|| "Fabric : version de loader introuvable".to_string())?
            .to_string(),
    };

    let installers: Vec<Value> = get_json(client, &format!("{FABRIC_META}/versions/installer"))
        .await
        .map_err(|e| format!("Fabric (installer) : {e}"))?;
    let installer = installers
        .iter()
        .find(|e| e.get("stable").and_then(Value::as_bool) == Some(true))
        .or_else(|| installers.first())
        .and_then(|e| e.get("version").and_then(Value::as_str))
        .ok_or_else(|| "Fabric : version d'installer introuvable".to_string())?;

    let jar_url =
        format!("{FABRIC_META}/versions/loader/{mc_version}/{loader}/{installer}/server/jar");
    download(client, &jar_url, &dir.join("fabric-server-launch.jar")).await?;

    Ok(Installed {
        executable: "fabric-server-launch.jar".into(),
        launch_args: None,
        loader_version: Some(loader),
    })
}

pub(super) async fn install_quilt(
    client: &reqwest::Client,
    mc_version: &str,
    loader_version: Option<&str>,
    java_path: &str,
    dir: &Path,
) -> Result<Installed, String> {
    let loaders: Vec<Value> = get_json(
        client,
        &format!("{QUILT_META}/versions/loader/{mc_version}"),
    )
    .await
    .map_err(|e| format!("Quilt {mc_version} : {e}"))?;
    if loaders.is_empty() {
        return Err(format!("Quilt ne supporte pas Minecraft {mc_version}"));
    }

    let loader = match loader_version {
        Some(v) => v.to_string(),
        None => loaders
            .iter()
            .filter(|e| {
                e.pointer("/loader/version")
                    .and_then(Value::as_str)
                    .map(|v| !is_prerelease(v))
                    .unwrap_or(false)
            })
            .max_by_key(|e| {
                version_key(
                    e.pointer("/loader/version")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                )
            })
            .or_else(|| loaders.first())
            .and_then(|e| e.pointer("/loader/version").and_then(Value::as_str))
            .ok_or_else(|| "Quilt : version de loader introuvable".to_string())?
            .to_string(),
    };

    // Quilt ne fournit pas de jar serveur prêt à l'emploi : on passe par
    // l'installateur officiel, qui crée un sous-dossier `server/`.
    let installers: Vec<Value> = get_json(client, &format!("{QUILT_META}/versions/installer"))
        .await
        .map_err(|e| format!("Quilt (installer) : {e}"))?;
    let installer = installers
        .first()
        .ok_or_else(|| "Quilt : aucun installateur disponible".to_string())?;
    let installer_url = installer
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Quilt : URL d'installateur absente".to_string())?;

    let installer_path = dir.join(".mcserv-quilt-installer.jar");
    download(client, installer_url, &installer_path).await?;

    let output = run_java(
        java_path,
        dir,
        &installer_path,
        &[
            "install",
            "server",
            mc_version,
            &loader,
            "--download-server",
        ],
        INSTALL_TIMEOUT,
    )
    .await
    .map_err(|e| format!("Quilt : {e}"))?;

    let nested = dir.join("server");
    if !nested.is_dir() {
        return Err(format!(
            "Quilt : l'installateur n'a pas créé de dossier server/. Sortie : {}",
            tail(&output)
        ));
    }
    move_dir_contents(&nested, dir).await?;
    let _ = tokio::fs::remove_dir_all(&nested).await;
    let _ = tokio::fs::remove_file(&installer_path).await;

    if !dir.join("quilt-server-launch.jar").is_file() {
        return Err(format!(
            "Quilt : quilt-server-launch.jar absent après installation. Sortie : {}",
            tail(&output)
        ));
    }

    Ok(Installed {
        executable: "quilt-server-launch.jar".into(),
        launch_args: None,
        loader_version: Some(loader),
    })
}

pub(super) async fn install_forge(
    client: &reqwest::Client,
    mc_version: &str,
    loader_version: Option<&str>,
    java_path: &str,
    dir: &Path,
) -> Result<Installed, String> {
    let all = maven_versions(client, &format!("{FORGE_MAVEN}/maven-metadata.xml")).await?;
    let full = pick_forge_version(&all, mc_version, loader_version)
        .ok_or_else(|| format!("Forge n'a pas de version pour Minecraft {mc_version}"))?;

    let installer_url = format!("{FORGE_MAVEN}/{full}/forge-{full}-installer.jar");
    let installer = dir.join(format!("forge-{full}-installer.jar"));
    download(client, &installer_url, &installer).await?;

    let output = run_java(
        java_path,
        dir,
        &installer,
        &["--installServer"],
        INSTALL_TIMEOUT,
    )
    .await
    .map_err(|e| format!("Forge : {e}"))?;

    let args_file = find_launch_args(dir, "minecraftforge")
        .await
        .ok_or_else(|| {
            format!(
                "Forge : installation terminée mais {} est introuvable. Sortie : {}",
                launch_args_name(),
                tail(&output)
            )
        })?;
    let _ = tokio::fs::remove_file(&installer).await;
    tokio::fs::create_dir_all(dir.join("mods"))
        .await
        .map_err(|e| format!("Forge : dossier mods : {e}"))?;

    Ok(Installed {
        executable: args_file.clone(),
        launch_args: Some(args_file),
        loader_version: Some(full),
    })
}

pub(super) async fn install_neoforge(
    client: &reqwest::Client,
    mc_version: &str,
    loader_version: Option<&str>,
    java_path: &str,
    dir: &Path,
) -> Result<Installed, String> {
    let all = maven_versions(client, &format!("{NEOFORGE_MAVEN}/maven-metadata.xml")).await?;
    let full = pick_neoforge_version(&all, mc_version, loader_version)
        .ok_or_else(|| format!("NeoForge n'a pas de version pour Minecraft {mc_version}"))?;

    let installer_url = format!("{NEOFORGE_MAVEN}/{full}/neoforge-{full}-installer.jar");
    let installer = dir.join(format!("neoforge-{full}-installer.jar"));
    download(client, &installer_url, &installer).await?;

    let output = run_java(
        java_path,
        dir,
        &installer,
        &["--installServer"],
        INSTALL_TIMEOUT,
    )
    .await
    .map_err(|e| format!("NeoForge : {e}"))?;

    let args_file = find_launch_args(dir, "neoforged").await.ok_or_else(|| {
        format!(
            "NeoForge : installation terminée mais {} est introuvable. Sortie : {}",
            launch_args_name(),
            tail(&output)
        )
    })?;
    let _ = tokio::fs::remove_file(&installer).await;
    tokio::fs::create_dir_all(dir.join("mods"))
        .await
        .map_err(|e| format!("NeoForge : dossier mods : {e}"))?;

    Ok(Installed {
        executable: args_file.clone(),
        launch_args: Some(args_file),
        loader_version: Some(full),
    })
}
async fn maven_versions(client: &reqwest::Client, url: &str) -> Result<Vec<String>, String> {
    let xml = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    Ok(xml
        .split("<version>")
        .skip(1)
        .filter_map(|part| part.split("</version>").next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect())
}

/// Choisit `{mc}-{forge}` (la version fournie peut être complète ou non).
pub(super) fn pick_forge_version(
    all: &[String],
    mc_version: &str,
    loader_version: Option<&str>,
) -> Option<String> {
    let prefix = format!("{mc_version}-");
    if let Some(v) = loader_version {
        let full = if v.starts_with(&prefix) {
            v.to_string()
        } else {
            format!("{prefix}{v}")
        };
        return all.iter().find(|candidate| **candidate == full).cloned();
    }
    all.iter()
        .filter(|v| v.starts_with(&prefix))
        .filter(|v| !v.contains("-beta") && !v.contains("-pre"))
        .max_by_key(|v| version_key(v))
        .cloned()
}

/// Préfixe de version NeoForge correspondant à une version Minecraft.
pub(super) fn neoforge_prefix(mc_version: &str) -> String {
    if let Some(rest) = mc_version.strip_prefix("1.") {
        let mut parts = rest.split('.');
        let major = parts.next().unwrap_or(rest);
        match parts.next() {
            Some(minor) => format!("{major}.{minor}"),
            None => format!("{major}.0"),
        }
    } else {
        mc_version.to_string()
    }
}

fn pick_neoforge_version(
    all: &[String],
    mc_version: &str,
    loader_version: Option<&str>,
) -> Option<String> {
    if let Some(v) = loader_version {
        return all.iter().find(|candidate| **candidate == v).cloned();
    }
    let prefix = format!("{}.", neoforge_prefix(mc_version));
    all.iter()
        .filter(|v| v.starts_with(&prefix))
        .filter(|v| !v.contains("-beta") && !v.contains("-pre") && !v.contains("-alpha"))
        .max_by_key(|v| version_key(v))
        .cloned()
}

fn is_prerelease(version: &str) -> bool {
    ["-beta", "-alpha", "-pre", "-rc", "-snapshot"]
        .iter()
        .any(|suffix| version.contains(suffix))
}

fn version_key(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// `unix_args.txt` sous Unix, `win_args.txt` sous Windows.
fn launch_args_name() -> &'static str {
    if cfg!(windows) {
        "win_args.txt"
    } else {
        "unix_args.txt"
    }
}

/// Cherche le fichier d'arguments généré par l'installateur Forge/NeoForge.
async fn find_launch_args(dir: &Path, vendor: &str) -> Option<String> {
    let libraries = dir.join("libraries");
    let mut stack = vec![libraries];
    let mut found: Option<PathBuf> = None;

    while let Some(path) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&path).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let entry_path = entry.path();
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(entry_path);
            } else if entry.file_name().to_string_lossy() == launch_args_name()
                && entry_path.to_string_lossy().contains(vendor)
            {
                found = Some(entry_path);
            }
        }
    }

    found.and_then(|p| {
        p.strip_prefix(dir)
            .ok()
            .map(|rel| rel.to_string_lossy().to_string())
    })
}

async fn move_dir_contents(src: &Path, dst: &Path) -> Result<(), String> {
    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| format!("lecture de {} : {e}", src.display()))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("lecture de {} : {e}", src.display()))?
    {
        let target = dst.join(entry.file_name());
        if target.is_dir() {
            let _ = tokio::fs::remove_dir_all(&target).await;
        } else if target.exists() {
            let _ = tokio::fs::remove_file(&target).await;
        }
        tokio::fs::rename(entry.path(), &target)
            .await
            .map_err(|e| format!("déplacement vers {} : {e}", target.display()))?;
    }
    Ok(())
}
