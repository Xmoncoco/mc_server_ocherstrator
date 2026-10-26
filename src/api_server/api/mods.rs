use rocket::data::{Data, ToByteUnit};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::serde::json::serde_json::{self, Value};
use rocket::{State, delete, get, post};

use crate::api_server::auth::auth_guard::KeyAuthUser;
use crate::server_management::loaders::{self, LoadersResponse};
use crate::server_management::server::{ModInfo, ServerManager};

use super::{ApiResult, api_err};

/// Liste des loaders supportés + versions Minecraft publiées par Mojang.
#[get("/loaders")]
pub(super) async fn loader_info(_auth: KeyAuthUser) -> Json<LoadersResponse> {
    Json(loaders::info().await)
}
#[get("/server/<id>/modpack")]
pub(super) fn server_modpack(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<Json<Value>> {
    let marker = mgr.modpack_marker(id).map_err(api_err)?;
    let value = marker
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or(Value::Null);
    Ok(Json(value))
}

#[get("/server/<id>/mods")]
pub(super) fn mod_list(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<Json<Vec<ModInfo>>> {
    mgr.list_mods(id).map(Json).map_err(api_err)
}

/// Upload brut d'un mod (`?filename=mon-mod.jar`), corps = contenu du jar.
#[post("/server/<id>/mods/upload?<filename>", data = "<data>")]
pub(super) async fn mod_upload(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    filename: String,
    data: Data<'_>,
) -> ApiResult<Json<Vec<ModInfo>>> {
    let temp = mgr.prepare_mod_upload(id, &filename).map_err(api_err)?;

    let mut stream = data.open(512.mebibytes());
    let mut file = rocket::tokio::fs::File::create(&temp)
        .await
        .map_err(|e| (Status::InternalServerError, e.to_string()))?;
    if let Err(e) = rocket::tokio::io::copy(&mut stream, &mut file).await {
        let _ = rocket::tokio::fs::remove_file(&temp).await;
        return Err((Status::BadRequest, format!("Upload interrompu : {e}")));
    }
    drop(file);

    let size = rocket::tokio::fs::metadata(&temp)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if size == 0 {
        let _ = rocket::tokio::fs::remove_file(&temp).await;
        return Err((Status::BadRequest, "Fichier vide".to_string()));
    }

    mgr.commit_mod_upload(id, &filename).map_err(api_err)?;
    mgr.list_mods(id).map(Json).map_err(api_err)
}

#[post("/server/<id>/mods/<name>/toggle")]
pub(super) fn mod_toggle(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    name: &str,
) -> ApiResult<Json<Vec<ModInfo>>> {
    mgr.toggle_mod(id, name).map(Json).map_err(api_err)
}

#[delete("/server/<id>/mods/<name>")]
pub(super) fn mod_delete(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    name: &str,
) -> ApiResult<Json<Vec<ModInfo>>> {
    mgr.delete_mod(id, name).map(Json).map_err(api_err)
}
