use super::*;

pub(super) async fn remote_file(
    source: Source,
    project_id: &str,
    version_id: Option<&str>,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
) -> Result<RemoteFile, String> {
    let version = match version_id.filter(|v| !v.is_empty()) {
        Some(id) => direct_version(source, project_id, id).await?,
        None => pick_version(&versions(source, project_id, game_version, loader).await?)
            .cloned()
            .ok_or_else(|| {
                "Aucune version compatible n'a été trouvée pour ce projet.".to_string()
            })?,
    };

    let url = version.url.clone().ok_or_else(|| {
        if source == Source::CurseForge {
            "Ce fichier CurseForge n'a pas d'URL de téléchargement (l'auteur l'a peut-être désactivée). Ouvre la page du projet pour le récupérer manuellement.".to_string()
        } else {
            "Aucun fichier téléchargeable pour cette version.".to_string()
        }
    })?;
    let file_name = version
        .file_name
        .clone()
        .ok_or_else(|| "Le nom du fichier est absent des métadonnées de la version.".to_string())?;
    let file_name = safe_file_name(&file_name).map_err(|e| e.to_string())?;

    Ok(RemoteFile {
        url,
        file_name,
        sha1: version.sha1.clone(),
        sha512: version.sha512.clone(),
        version_name: version.name.clone(),
        version_id: version.id.clone(),
        dependencies: version.dependencies.clone(),
    })
}

/// Récupère une version précise sans passer par la liste (utile si la liste
/// filtrée ne la contient pas).

async fn direct_version(
    source: Source,
    project_id: &str,
    version_id: &str,
) -> Result<CatalogVersion, String> {
    match source {
        Source::Modrinth => modrinth::direct_version(version_id).await,
        Source::CurseForge => curseforge::curseforge_file_entry(Some(project_id), version_id).await,
    }
}
