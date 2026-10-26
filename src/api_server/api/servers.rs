use std::time::Duration;

use rocket::data::{Data, ToByteUnit};
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::serde::json::serde_json::{Value, json};
use rocket::{State, delete, get, post};
use serde::Deserialize;
use uuid::Uuid;

use crate::api_server::auth::auth_guard::KeyAuthUser;
use crate::server_management::loaders::ModLoader;
use crate::server_management::server::{
    Server, ServerInfo, ServerManager, TypeOfServ, safe_file_name,
};

use super::{ApiResult, api_err};

/// Délai accordé à un arrêt propre avant d'abandonner un redémarrage.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct NewServer {
    /// `auto` : télécharge et installe le loader ; `path` : fichier fourni.
    #[serde(default = "default_install_mode")]
    install: String,
    name: Option<String>,
    #[serde(default)]
    mod_loader: Option<ModLoader>,
    #[serde(default)]
    mc_version: Option<String>,
    #[serde(default)]
    loader_version: Option<String>,
    #[serde(default)]
    java_path: Option<String>,
    #[serde(default)]
    executable_path: Option<String>,
    #[serde(default)]
    type_serv: Option<TypeOfServ>,
    min_ram: usize,
    max_ram: usize,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    accept_eula: bool,
}

fn default_install_mode() -> String {
    "auto".to_string()
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct ServerSettings {
    name: Option<String>,
    java_path: Option<String>,
    min_ram: Option<usize>,
    max_ram: Option<usize>,
    args: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct SetLaunchFile {
    /// Chemin relatif au dossier de l'instance.
    path: String,
}
#[get("/servers")]
pub(super) fn server_list(_auth: KeyAuthUser, mgr: &State<ServerManager>) -> Json<Vec<ServerInfo>> {
    Json(mgr.list())
}

#[post("/server/create", format = "json", data = "<req>")]
pub(super) async fn server_create(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    req: Json<NewServer>,
) -> ApiResult<Json<ServerInfo>> {
    let r = req.into_inner();
    let args = r.args.unwrap_or_default();

    match r.install.as_str() {
        "path" => {
            let executable_path = r.executable_path.ok_or_else(|| {
                (
                    Status::BadRequest,
                    "executable_path est obligatoire en mode manuel".to_string(),
                )
            })?;
            let type_serv = r.type_serv.unwrap_or(TypeOfServ::JavaBased);
            mgr.create_manual(
                r.name,
                r.java_path,
                &executable_path,
                type_serv,
                r.min_ram,
                r.max_ram,
                args,
                r.accept_eula,
            )
            .map(Json)
            .map_err(api_err)
        }
        "auto" => {
            let mod_loader = r.mod_loader.ok_or_else(|| {
                (
                    Status::BadRequest,
                    "Choisis un mod loader à installer".to_string(),
                )
            })?;
            let mc_version = r.mc_version.ok_or_else(|| {
                (
                    Status::BadRequest,
                    "La version Minecraft est obligatoire".to_string(),
                )
            })?;
            mgr.create_auto(
                r.name,
                mod_loader,
                mc_version,
                r.loader_version,
                r.java_path,
                r.min_ram,
                r.max_ram,
                args,
                r.accept_eula,
            )
            .await
            .map(Json)
            .map_err(api_err)
        }
        other => Err((
            Status::BadRequest,
            format!("Mode d'installation inconnu : {other}"),
        )),
    }
}

/// Dépose un fichier (jar ou exécutable) sur la machine serveur. Le chemin
/// renvoyé peut ensuite être passé à `/server/create` en mode `path`.
#[post("/server/upload?<filename>", data = "<data>")]
pub(super) async fn server_upload(
    _auth: KeyAuthUser,
    filename: String,
    data: Data<'_>,
) -> ApiResult<Json<Value>> {
    let name = safe_file_name(&filename).map_err(api_err)?;
    let dir = Server::uploads_dir().join(Uuid::new_v4().to_string());
    rocket::tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| (Status::InternalServerError, e.to_string()))?;
    let dest = dir.join(&name);

    let mut stream = data.open(1024.mebibytes());
    let mut file = rocket::tokio::fs::File::create(&dest)
        .await
        .map_err(|e| (Status::InternalServerError, e.to_string()))?;
    rocket::tokio::io::copy(&mut stream, &mut file)
        .await
        .map_err(|e| (Status::BadRequest, format!("Upload interrompu : {e}")))?;

    let size = rocket::tokio::fs::metadata(&dest)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if size == 0 {
        let _ = rocket::tokio::fs::remove_dir_all(&dir).await;
        return Err((Status::BadRequest, "Fichier vide".to_string()));
    }

    Ok(Json(json!({
        "path": dest.to_string_lossy(),
        "filename": name,
        "size": size,
    })))
}
#[get("/server/<id>")]
pub(super) fn server_info(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<Json<ServerInfo>> {
    mgr.info(id).map(Json).map_err(api_err)
}

#[get("/server/<id>/launch-files")]
pub(super) fn server_launch_files(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<Json<Vec<String>>> {
    mgr.launch_files(id).map(Json).map_err(api_err)
}

#[post("/server/<id>/launch-file", format = "json", data = "<req>")]
pub(super) fn server_set_launch_file(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<SetLaunchFile>,
) -> ApiResult<Json<ServerInfo>> {
    mgr.set_launch_file(id, &req.into_inner().path)
        .map(Json)
        .map_err(api_err)
}

#[delete("/server/<id>")]
pub(super) fn server_delete(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<String> {
    mgr.delete(id).map_err(api_err)?;
    Ok("ok".to_string())
}

#[post("/server/<id>/settings", format = "json", data = "<req>")]
pub(super) fn server_settings(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<ServerSettings>,
) -> ApiResult<Json<ServerInfo>> {
    let r = req.into_inner();
    mgr.update_settings(id, r.name, r.java_path, r.min_ram, r.max_ram, r.args)
        .map(Json)
        .map_err(api_err)
}
#[post("/server/<id>/start")]
pub(super) fn server_start(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<Json<ServerInfo>> {
    mgr.start(id).map(Json).map_err(api_err)
}

/// Arrêt propre (SIGTERM). Renvoie tout de suite ; suis l'état avec GET /server/<id>.
#[post("/server/<id>/stop")]
pub(super) fn server_stop(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<String> {
    mgr.stop(id).map_err(api_err)?;
    Ok("ok".to_string())
}

/// Arrêt brutal (SIGKILL).
#[post("/server/<id>/kill")]
pub(super) fn server_kill(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<String> {
    mgr.kill(id).map_err(api_err)?;
    Ok("ok".to_string())
}

#[post("/server/<id>/restart")]
pub(super) async fn server_restart(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<Json<ServerInfo>> {
    if mgr.is_running(id).map_err(api_err)? {
        mgr.stop(id).map_err(api_err)?;
        let stopped = mgr.wait_stopped(id, STOP_TIMEOUT).await.map_err(api_err)?;
        if !stopped {
            return Err((
                Status::InternalServerError,
                "Le serveur ne s'est pas arrêté à temps, utilise /kill".to_string(),
            ));
        }
    }
    mgr.start(id).map(Json).map_err(api_err)
}
