use super::*;

pub(super) async fn search(
    kind: Kind,
    query: &str,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
    include_client: bool,
    offset: usize,
    limit: usize,
) -> Result<CatalogPage, String> {
    let mut groups: Vec<Vec<String>> = vec![vec![format!("project_type:{}", kind.as_str())]];

    if !include_client {
        groups.push(vec![
            "server_side:required".to_string(),
            "server_side:optional".to_string(),
        ]);
    }
    if let Some(version) = game_version.filter(|v| !v.is_empty()) {
        groups.push(vec![format!("versions:{version}")]);
    }
    if let Some(loader) = loader.and_then(modrinth_loader) {
        groups.push(vec![format!("categories:{loader}")]);
    }

    let facets = Value::Array(
        groups
            .into_iter()
            .map(|group| Value::Array(group.into_iter().map(Value::String).collect()))
            .collect(),
    );

    let mut url =
        reqwest::Url::parse(&format!("{MODRINTH_API}/search")).map_err(|e| e.to_string())?;
    let index = if query.trim().is_empty() {
        "downloads"
    } else {
        "relevance"
    };
    url.query_pairs_mut()
        .append_pair("query", query)
        .append_pair("limit", &limit.to_string())
        .append_pair("offset", &offset.to_string())
        .append_pair("index", index)
        .append_pair("facets", &facets.to_string());

    let data = get_json(&client(), url.as_str())
        .await
        .map_err(|e| format!("Modrinth : {e}"))?;

    let hits = data["hits"].as_array().cloned().unwrap_or_default();
    let items = hits.iter().map(normalize_modrinth_hit).collect::<Vec<_>>();

    Ok(CatalogPage {
        items,
        total: data["total_hits"].as_u64().unwrap_or(0),
        offset,
        limit,
        source: Source::Modrinth.as_str().to_string(),
        kind: kind.as_str().to_string(),
    })
}

fn normalize_modrinth_hit(hit: &Value) -> CatalogItem {
    let categories: Vec<String> = hit["categories"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let loaders = categories
        .iter()
        .filter_map(|category| loader_from_name(category))
        .map(str::to_string)
        .collect::<Vec<_>>();

    let game_versions = hit["versions"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let project_type = hit["project_type"].as_str().unwrap_or("mod");
    let slug = hit["slug"].as_str().unwrap_or_default().to_string();

    CatalogItem {
        id: hit["project_id"].as_str().unwrap_or_default().to_string(),
        source: Source::Modrinth.as_str().to_string(),
        kind: project_type.to_string(),
        slug: slug.clone(),
        name: hit["title"].as_str().unwrap_or_default().to_string(),
        author: hit["author"].as_str().unwrap_or_default().to_string(),
        summary: hit["description"].as_str().unwrap_or_default().to_string(),
        icon_url: hit["icon_url"].as_str().map(str::to_string),
        url: format!("https://modrinth.com/{project_type}/{slug}"),
        downloads: hit["downloads"].as_u64().unwrap_or(0),
        follows: hit["follows"].as_u64().unwrap_or(0),
        loaders,
        game_versions,
        server_side: hit["server_side"].as_str().map(str::to_string),
        client_side: hit["client_side"].as_str().map(str::to_string),
        updated: hit["date_modified"].as_str().map(str::to_string),
    }
}

pub(super) async fn versions(
    project_id: &str,
    game_version: Option<&str>,
    loader: Option<ModLoader>,
) -> Result<Vec<CatalogVersion>, String> {
    let mut url = reqwest::Url::parse(&format!("{MODRINTH_API}/project/{project_id}/version"))
        .map_err(|e| e.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            pairs.append_pair("game_versions", &json!([version]).to_string());
        }
        if let Some(loader) = loader.and_then(modrinth_loader) {
            pairs.append_pair("loaders", &json!([loader]).to_string());
        }
    }

    let data = get_json(&client(), url.as_str())
        .await
        .map_err(|e| format!("Modrinth : {e}"))?;
    let entries = data.as_array().cloned().unwrap_or_default();

    let wanted_loader = loader.and_then(modrinth_loader);
    let mut versions = Vec::new();

    for entry in entries {
        let game_versions = strings(&entry["game_versions"]);
        let loaders = strings(&entry["loaders"]);

        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            if !game_versions.iter().any(|v| v == version) {
                continue;
            }
        }
        if let Some(wanted) = wanted_loader {
            if !loaders.iter().any(|v| v == wanted) {
                continue;
            }
        }

        let files = entry["files"].as_array().cloned().unwrap_or_default();
        let file = files
            .iter()
            .find(|f| f["primary"].as_bool() == Some(true))
            .or_else(|| files.first());

        versions.push(CatalogVersion {
            id: entry["id"].as_str().unwrap_or_default().to_string(),
            name: entry["name"].as_str().unwrap_or_default().to_string(),
            version_number: entry["version_number"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            game_versions,
            loaders,
            downloads: entry["downloads"].as_u64().unwrap_or(0),
            published: entry["date_published"].as_str().map(str::to_string),
            file_name: file
                .and_then(|f| f["filename"].as_str())
                .map(str::to_string),
            file_size: file.and_then(|f| f["size"].as_u64()),
            release_type: entry["version_type"]
                .as_str()
                .unwrap_or("release")
                .to_string(),
            url: file.and_then(|f| f["url"].as_str()).map(str::to_string),
            sha1: file
                .and_then(|f| f["hashes"]["sha1"].as_str())
                .map(str::to_string),
            sha512: file
                .and_then(|f| f["hashes"]["sha512"].as_str())
                .map(str::to_string),
            dependencies: modrinth_dependencies(&entry),
        });
    }

    versions.sort_by(|a, b| b.published.cmp(&a.published));
    Ok(versions)
}

fn modrinth_dependencies(entry: &Value) -> Vec<CatalogDependency> {
    entry["dependencies"]
        .as_array()
        .map(|dependencies| {
            dependencies
                .iter()
                .filter_map(|dependency| {
                    if dependency["dependency_type"].as_str() != Some("required") {
                        return None;
                    }
                    let project_id = dependency["project_id"].as_str().map(str::to_string)?;
                    Some(CatalogDependency {
                        project_id: Some(project_id),
                        version_id: dependency["version_id"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) async fn direct_version(version_id: &str) -> Result<CatalogVersion, String> {
    let entry = get_json(&client(), &format!("{MODRINTH_API}/version/{version_id}"))
        .await
        .map_err(|e| format!("Modrinth : {e}"))?;

    let files = entry["files"].as_array().cloned().unwrap_or_default();
    let file = files
        .iter()
        .find(|f| f["primary"].as_bool() == Some(true))
        .or_else(|| files.first());

    Ok(CatalogVersion {
        id: entry["id"].as_str().unwrap_or_default().to_string(),
        name: entry["name"].as_str().unwrap_or_default().to_string(),
        version_number: entry["version_number"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        game_versions: strings(&entry["game_versions"]),
        loaders: strings(&entry["loaders"]),
        downloads: entry["downloads"].as_u64().unwrap_or(0),
        published: entry["date_published"].as_str().map(str::to_string),
        file_name: file
            .and_then(|f| f["filename"].as_str())
            .map(str::to_string),
        file_size: file.and_then(|f| f["size"].as_u64()),
        release_type: entry["version_type"]
            .as_str()
            .unwrap_or("release")
            .to_string(),
        url: file.and_then(|f| f["url"].as_str()).map(str::to_string),
        sha1: file
            .and_then(|f| f["hashes"]["sha1"].as_str())
            .map(str::to_string),
        sha512: file
            .and_then(|f| f["hashes"]["sha512"].as_str())
            .map(str::to_string),
        dependencies: modrinth_dependencies(&entry),
    })
}

pub(super) async fn project_title(project_id: &str) -> Result<String, String> {
    let data = get_json(&client(), &format!("{MODRINTH_API}/project/{project_id}")).await?;
    Ok(data["title"].as_str().unwrap_or("Modpack").to_string())
}
