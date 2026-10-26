use super::*;

pub(super) async fn search(
    kind: Kind,
    query: &str,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
    offset: usize,
    limit: usize,
) -> Result<CatalogPage, String> {
    let class_id = match kind {
        Kind::Mod => CF_MODS_CLASS_ID,
        Kind::Modpack => CF_MODPACKS_CLASS_ID,
    };

    let mut url =
        reqwest::Url::parse(&format!("{CURSEFORGE_API}/mods/search")).map_err(|e| e.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs
            .append_pair("gameId", &CF_MINECRAFT_GAME_ID.to_string())
            .append_pair("classId", &class_id.to_string())
            .append_pair("searchFilter", query)
            .append_pair("index", &offset.to_string())
            .append_pair("pageSize", &limit.to_string())
            .append_pair("sortField", "2")
            .append_pair("sortOrder", "desc");
        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            pairs.append_pair("gameVersion", version);
        }
        if let Some(loader_type) = loader.and_then(cf_loader_type) {
            pairs.append_pair("modLoaderType", &loader_type.to_string());
        }
    }

    let data = cf_get(&client(), url.as_str())
        .await
        .map_err(|e| format!("CurseForge : {e}"))?;

    let entries = data["data"].as_array().cloned().unwrap_or_default();
    let items = entries
        .iter()
        .map(|entry| normalize_curseforge_hit(entry, kind))
        .collect();

    Ok(CatalogPage {
        items,
        total: data["pagination"]["totalCount"].as_u64().unwrap_or(0),
        offset,
        limit,
        source: Source::CurseForge.as_str().to_string(),
        kind: kind.as_str().to_string(),
    })
}

fn normalize_curseforge_hit(hit: &Value, kind: Kind) -> CatalogItem {
    let slug = hit["slug"].as_str().unwrap_or_default().to_string();
    let id = hit["id"]
        .as_u64()
        .map(|v| v.to_string())
        .unwrap_or_default();

    let mut loaders = Vec::new();
    let mut versions = Vec::new();
    if let Some(indexes) = hit["latestFilesIndexes"].as_array() {
        for index in indexes {
            if let Some(loader) = index["modLoader"].as_u64().and_then(loader_from_cf_type) {
                if !loaders.iter().any(|v| v == loader) {
                    loaders.push(loader.to_string());
                }
            }
            if let Some(version) = index["gameVersion"].as_str() {
                if !versions.iter().any(|v| v == version) {
                    versions.push(version.to_string());
                }
            }
        }
    }

    let url = hit["links"]["websiteUrl"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            let section = match kind {
                Kind::Mod => "mc-mods",
                Kind::Modpack => "modpacks",
            };
            format!("https://www.curseforge.com/minecraft/{section}/{slug}")
        });

    CatalogItem {
        id,
        source: Source::CurseForge.as_str().to_string(),
        kind: kind.as_str().to_string(),
        slug,
        name: hit["name"].as_str().unwrap_or_default().to_string(),
        author: hit["authors"]
            .as_array()
            .and_then(|authors| authors.first())
            .and_then(|author| author["name"].as_str())
            .unwrap_or_default()
            .to_string(),
        summary: hit["summary"].as_str().unwrap_or_default().to_string(),
        icon_url: hit["logo"]["url"].as_str().map(str::to_string),
        url,
        downloads: hit["downloadCount"].as_u64().unwrap_or(0),
        follows: hit["thumbsUpCount"].as_u64().unwrap_or(0),
        loaders,
        game_versions: versions,
        server_side: None,
        client_side: None,
        updated: hit["dateModified"].as_str().map(str::to_string),
    }
}

pub(super) async fn versions(
    project_id: &str,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
) -> Result<Vec<CatalogVersion>, String> {
    let mut url = reqwest::Url::parse(&format!("{CURSEFORGE_API}/mods/{project_id}/files"))
        .map_err(|e| e.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs
            .append_pair("index", "0")
            .append_pair("pageSize", "50");
        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            pairs.append_pair("gameVersion", version);
        }
        if let Some(loader_type) = loader.and_then(cf_loader_type) {
            pairs.append_pair("modLoaderType", &loader_type.to_string());
        }
    }

    let data = cf_get(&client(), url.as_str())
        .await
        .map_err(|e| format!("CurseForge : {e}"))?;
    let entries = data["data"].as_array().cloned().unwrap_or_default();

    let wanted_loader = loader.and_then(modrinth_loader);
    let mut versions = Vec::new();

    for entry in entries {
        let game_versions = strings(&entry["gameVersions"]);
        let loaders = game_versions
            .iter()
            .filter_map(|value| loader_from_name(value))
            .map(str::to_string)
            .collect::<Vec<_>>();

        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            if !game_versions.iter().any(|v| v == version) {
                continue;
            }
        }
        if let Some(wanted) = wanted_loader {
            if !loaders.iter().any(|v| v == wanted) && !loaders.is_empty() {
                continue;
            }
        }

        let release_type = match entry["releaseType"].as_u64().unwrap_or(1) {
            1 => "release",
            2 => "beta",
            3 => "alpha",
            _ => "release",
        };

        let sha1 = entry["hashes"]
            .as_array()
            .and_then(|hashes| {
                hashes
                    .iter()
                    .find(|hash| hash["algo"].as_u64() == Some(1))
                    .and_then(|hash| hash["value"].as_str())
            })
            .map(str::to_string);

        versions.push(CatalogVersion {
            id: entry["id"]
                .as_u64()
                .map(|v| v.to_string())
                .unwrap_or_default(),
            name: entry["displayName"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            version_number: entry["displayName"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            game_versions,
            loaders,
            downloads: entry["downloadCount"].as_u64().unwrap_or(0),
            published: entry["fileDate"].as_str().map(str::to_string),
            file_name: entry["fileName"].as_str().map(str::to_string),
            file_size: entry["fileLength"].as_u64(),
            release_type: release_type.to_string(),
            url: entry["downloadUrl"].as_str().map(str::to_string),
            sha1,
            sha512: None,
            dependencies: curseforge_dependencies(&entry),
        });
    }

    versions.sort_by(|a, b| b.published.cmp(&a.published));
    Ok(versions)
}

fn curseforge_dependencies(entry: &Value) -> Vec<CatalogDependency> {
    entry["dependencies"]
        .as_array()
        .map(|dependencies| {
            dependencies
                .iter()
                .filter_map(|dependency| {
                    if dependency["relationType"].as_u64() != Some(3) {
                        return None;
                    }
                    Some(CatalogDependency {
                        project_id: dependency["modId"].as_u64().map(|id| id.to_string()),
                        version_id: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) async fn curseforge_file_entry(
    project_id: Option<&str>,
    file_id: &str,
) -> Result<CatalogVersion, String> {
    let url = match project_id {
        Some(project_id) => {
            format!("{CURSEFORGE_API}/mods/{project_id}/files/{file_id}")
        }
        None => format!("{CURSEFORGE_API}/mods/files/{file_id}"),
    };
    let data = cf_get(&client(), &url)
        .await
        .map_err(|e| format!("CurseForge : {e}"))?;
    let entry = &data["data"];

    let game_versions = strings(&entry["gameVersions"]);
    let loaders = game_versions
        .iter()
        .filter_map(|value| loader_from_name(value))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let release_type = match entry["releaseType"].as_u64().unwrap_or(1) {
        1 => "release",
        2 => "beta",
        3 => "alpha",
        _ => "release",
    };
    let sha1 = entry["hashes"]
        .as_array()
        .and_then(|hashes| {
            hashes
                .iter()
                .find(|hash| hash["algo"].as_u64() == Some(1))
                .and_then(|hash| hash["value"].as_str())
        })
        .map(str::to_string);

    Ok(CatalogVersion {
        id: entry["id"]
            .as_u64()
            .map(|v| v.to_string())
            .unwrap_or_default(),
        name: entry["displayName"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        version_number: entry["displayName"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        game_versions,
        loaders,
        downloads: entry["downloadCount"].as_u64().unwrap_or(0),
        published: entry["fileDate"].as_str().map(str::to_string),
        file_name: entry["fileName"].as_str().map(str::to_string),
        file_size: entry["fileLength"].as_u64(),
        release_type: release_type.to_string(),
        url: entry["downloadUrl"].as_str().map(str::to_string),
        sha1,
        sha512: None,
        dependencies: curseforge_dependencies(entry),
    })
}
