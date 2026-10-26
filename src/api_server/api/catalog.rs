use std::path::Path;

use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::{State, get, post};
use serde::Deserialize;

use crate::api_server::auth::auth_guard::KeyAuthUser;
use crate::catalog::{
    self, CatalogConfig, CatalogPage, CatalogSearchRequest, ModInstallReport, ModpackReport, Source,
};
use crate::server_management::loaders::ModLoader;
use crate::server_management::server::{ServerInfo, ServerManager};

use super::{ApiResult, api_err};

// ---------------------------------------------------------------------------
// Catalogue Modrinth / CurseForge
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct CatalogKeyBody {
    #[serde(default)]
    curseforge_api_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct InstallModBody {
    source: Source,
    project_id: String,
    #[serde(default)]
    version_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct InstallModpackBody {
    source: Source,
    project_id: String,
    #[serde(default)]
    version_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct NewServerFromModpack {
    source: Source,
    project_id: String,
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    java_path: Option<String>,
    min_ram: usize,
    max_ram: usize,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    accept_eula: bool,
}

fn catalog_err(message: String) -> (Status, String) {
    (Status::BadRequest, message)
}

fn modded_loader(loader: ModLoader) -> bool {
    matches!(
        loader,
        ModLoader::Fabric | ModLoader::Quilt | ModLoader::Forge | ModLoader::NeoForge
    )
}

#[get("/catalog/config")]
pub(super) fn catalog_config(_auth: KeyAuthUser) -> Json<CatalogConfig> {
    Json(catalog::config_info())
}

#[post("/catalog/config", format = "json", data = "<body>")]
pub(super) fn catalog_set_config(
    _auth: KeyAuthUser,
    body: Json<CatalogKeyBody>,
) -> ApiResult<Json<CatalogConfig>> {
    catalog::set_curseforge_key(body.into_inner().curseforge_api_key)
        .map(Json)
        .map_err(catalog_err)
}

#[post("/catalog/search", format = "json", data = "<req>")]
pub(super) async fn catalog_search(
    _auth: KeyAuthUser,
    req: Json<CatalogSearchRequest>,
) -> ApiResult<Json<CatalogPage>> {
    catalog::search(req.into_inner())
        .await
        .map(Json)
        .map_err(catalog_err)
}

#[post("/server/<id>/mods/install", format = "json", data = "<req>")]
pub(super) async fn mod_install_catalog(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<InstallModBody>,
) -> ApiResult<Json<ModInstallReport>> {
    let body = req.into_inner();
    let info = mgr.info(id).map_err(api_err)?;
    if !modded_loader(info.mod_loader) {
        return Err((
            Status::BadRequest,
            "Ce serveur ne charge pas les mods. Utilise Fabric, Quilt, Forge ou NeoForge."
                .to_string(),
        ));
    }

    let mods_dir = Path::new(&info.path).join("mods");
    rocket::tokio::fs::create_dir_all(&mods_dir)
        .await
        .map_err(|e| (Status::InternalServerError, e.to_string()))?;

    catalog::install_mod(
        body.source,
        &body.project_id,
        body.version_id.as_deref(),
        info.mc_version.as_deref(),
        Some(info.mod_loader),
        &mods_dir,
    )
    .await
    .map(Json)
    .map_err(catalog_err)
}

#[post("/server/<id>/modpack", format = "json", data = "<req>")]
pub(super) async fn server_install_modpack(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<InstallModpackBody>,
) -> ApiResult<Json<ModpackReport>> {
    let body = req.into_inner();
    let info = mgr.info(id).map_err(api_err)?;

    if !modded_loader(info.mod_loader) || info.mc_version.is_none() {
        return Err((
            Status::BadRequest,
            "Ce serveur n'a pas de mod loader compatible avec les modpacks.".to_string(),
        ));
    }

    let temp = catalog::new_temp_dir().map_err(catalog_err)?;
    let result = async {
        let archive = catalog::fetch_modpack(
            body.source,
            &body.project_id,
            body.version_id.as_deref(),
            info.mc_version.as_deref(),
            Some(info.mod_loader),
            &temp,
        )
        .await
        .map_err(catalog_err)?;

        catalog::check_compatibility(&info, &archive).map_err(catalog_err)?;
        catalog::install_modpack(Path::new(&info.path), &archive)
            .await
            .map(Json)
            .map_err(catalog_err)
    }
    .await;

    let _ = rocket::tokio::fs::remove_dir_all(&temp).await;
    result
}

#[post("/server/create/from-modpack", format = "json", data = "<req>")]
pub(super) async fn server_create_from_modpack(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    req: Json<NewServerFromModpack>,
) -> ApiResult<Json<ServerInfo>> {
    let body = req.into_inner();
    let temp = catalog::new_temp_dir().map_err(catalog_err)?;

    let result = async {
        let archive = catalog::fetch_modpack(
            body.source,
            &body.project_id,
            body.version_id.as_deref(),
            None,
            None,
            &temp,
        )
        .await
        .map_err(catalog_err)?;

        if !modded_loader(archive.loader) {
            return Err(catalog_err(
                "Ce modpack n'utilise pas de mod loader compatible.".to_string(),
            ));
        }

        let name = body
            .name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .or_else(|| Some(archive.name.clone()));

        let info = mgr
            .create_auto(
                name,
                archive.loader,
                archive.mc_version.clone(),
                archive.loader_version.clone(),
                body.java_path,
                body.min_ram,
                body.max_ram,
                body.args.unwrap_or_default(),
                body.accept_eula,
            )
            .await
            .map_err(api_err)?;

        if let Err(error) = catalog::install_modpack(Path::new(&info.path), &archive).await {
            let _ = mgr.delete(&info.uuid);
            return Err(catalog_err(error));
        }

        Ok(Json(info))
    }
    .await;

    let _ = rocket::tokio::fs::remove_dir_all(&temp).await;
    result
}
