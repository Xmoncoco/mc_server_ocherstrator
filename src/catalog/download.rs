use super::resolve::remote_file;
use super::*;

pub(super) async fn download_file(
    client: &reqwest::Client,
    remote: &RemoteFile,
    dest: &Path,
) -> Result<u64, String> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("création de {} : {e}", parent.display()))?;
    }

    let part = PathBuf::from(format!("{}.part", dest.to_string_lossy()));
    let _ = tokio::fs::remove_file(&part).await;

    let result = async {
        let resp = client
            .get(&remote.url)
            .send()
            .await
            .map_err(|e| format!("téléchargement : {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("téléchargement : HTTP {status}"));
        }

        let mut file = tokio::fs::File::create(&part)
            .await
            .map_err(|e| format!("écriture de {} : {e}", part.display()))?;
        let mut sha1 = remote.sha1.as_ref().map(|_| Sha1::new());
        let mut sha512 = remote.sha512.as_ref().map(|_| Sha512::new());
        let mut size = 0u64;
        let mut resp = resp;

        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            if let Some(hasher) = sha1.as_mut() {
                hasher.update(&chunk);
            }
            if let Some(hasher) = sha512.as_mut() {
                hasher.update(&chunk);
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("écriture de {} : {e}", part.display()))?;
            size += chunk.len() as u64;
        }
        file.flush().await.map_err(|e| e.to_string())?;
        drop(file);

        if let Some(hasher) = sha1 {
            let actual = hex(&hasher.finalize());
            if let Some(expected) = &remote.sha1 {
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err("le fichier téléchargé est corrompu (SHA-1 invalide)".into());
                }
            }
        }
        if let Some(hasher) = sha512 {
            let actual = hex(&hasher.finalize());
            if let Some(expected) = &remote.sha512 {
                if !actual.eq_ignore_ascii_case(expected) {
                    return Err("le fichier téléchargé est corrompu (SHA-512 invalide)".into());
                }
            }
        }

        // Windows ne remplace pas une destination existante avec rename.
        #[cfg(windows)]
        let _ = tokio::fs::remove_file(dest).await;
        tokio::fs::rename(&part, dest)
            .await
            .map_err(|e| format!("finalisation de {} : {e}", dest.display()))?;
        Ok(size)
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

const MAX_DEPENDENCIES: usize = 40;
const MAX_DEPENDENCY_DEPTH: usize = 6;

struct InstalledMod {
    file_name: String,
    size: u64,
    version_name: String,
    version_id: String,
}

pub async fn install_mod(
    source: Source,
    project_id: &str,
    version_id: Option<&str>,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
    mods_dir: &Path,
) -> Result<ModInstallReport, String> {
    let mut visited = HashSet::new();
    let mut installed_files = Vec::new();
    let main = install_mod_recursive(
        source,
        project_id,
        version_id,
        game_version,
        loader,
        mods_dir,
        &mut visited,
        &mut installed_files,
        0,
    )
    .await?;

    Ok(ModInstallReport {
        file_name: main.file_name,
        size: main.size,
        version_name: main.version_name,
        version_id: main.version_id,
        installed_files,
    })
}

#[allow(clippy::too_many_arguments)]
async fn install_mod_recursive(
    source: Source,
    project_id: &str,
    version_id: Option<&str>,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
    mods_dir: &Path,
    visited: &mut HashSet<String>,
    installed_files: &mut Vec<String>,
    depth: usize,
) -> Result<InstalledMod, String> {
    let key = format!("{}:{}", source.as_str(), project_id);
    if !visited.insert(key) {
        return Err(format!(
            "Dépendance circulaire détectée autour de {project_id}."
        ));
    }
    if visited.len() > MAX_DEPENDENCIES || depth > MAX_DEPENDENCY_DEPTH {
        return Err("Trop de dépendances requises pour ce mod.".to_string());
    }

    let remote = remote_file(source, project_id, version_id, game_version, loader).await?;
    let client = client();
    let dest = mods_dir.join(&remote.file_name);
    let size = download_file(&client, &remote, &dest).await?;

    // Une ancienne version désactivée du même fichier ne doit pas créer de
    // doublon dans la liste.
    let disabled = mods_dir.join(format!("{}.disabled", remote.file_name));
    let _ = tokio::fs::remove_file(disabled).await;

    installed_files.push(remote.file_name.clone());
    let installed = InstalledMod {
        file_name: remote.file_name.clone(),
        size,
        version_name: remote.version_name.clone(),
        version_id: remote.version_id.clone(),
    };

    for dependency in &remote.dependencies {
        let Some(dependency_project) = dependency.project_id.as_deref() else {
            continue;
        };
        let dependency_key = format!("{}:{}", source.as_str(), dependency_project);
        if visited.contains(&dependency_key) {
            continue;
        }
        Box::pin(install_mod_recursive(
            source,
            dependency_project,
            dependency.version_id.as_deref(),
            game_version,
            loader,
            mods_dir,
            visited,
            installed_files,
            depth + 1,
        ))
        .await?;
    }

    Ok(installed)
}
