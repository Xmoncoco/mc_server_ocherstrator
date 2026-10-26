use super::*;

pub(super) async fn install_vanilla(
    client: &reqwest::Client,
    mc_version: &str,
    dir: &Path,
) -> Result<Installed, String> {
    let manifest: Manifest = get_json(client, MOJANG_MANIFEST)
        .await
        .map_err(|e| format!("Mojang : {e}"))?;
    let entry = manifest
        .versions
        .iter()
        .find(|v| v.id == mc_version)
        .ok_or_else(|| format!("La version Minecraft {mc_version} n'existe pas"))?;

    let version: Value = get_json(client, &entry.url)
        .await
        .map_err(|e| format!("Mojang {mc_version} : {e}"))?;
    let url = version
        .pointer("/downloads/server/url")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Pas de serveur téléchargeable pour {mc_version}"))?;

    download(client, url, &dir.join("server.jar")).await?;
    Ok(Installed {
        executable: "server.jar".into(),
        launch_args: None,
        loader_version: None,
    })
}

pub(super) async fn install_paper(
    client: &reqwest::Client,
    mc_version: &str,
    dir: &Path,
) -> Result<Installed, String> {
    let url =
        format!("https://fill.papermc.io/v3/projects/paper/versions/{mc_version}/builds/latest");
    let build: Value = get_json(client, &url)
        .await
        .map_err(|e| format!("Paper {mc_version} : {e}"))?;
    let server = build
        .pointer("/downloads/server:default")
        .ok_or_else(|| format!("Paper {mc_version} : aucun build disponible"))?;
    let name = server
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("paper.jar");
    let download_url = server
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Paper : URL de téléchargement absente".to_string())?;
    let name = safe_file_name(name)?;

    download(client, download_url, &dir.join(&name)).await?;
    Ok(Installed {
        executable: name,
        launch_args: None,
        loader_version: None,
    })
}
