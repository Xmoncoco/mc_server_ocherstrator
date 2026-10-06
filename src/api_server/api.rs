use std::time::Duration;

use rocket::futures::{SinkExt, StreamExt};
use rocket::http::Status;
use rocket::response::stream::{Event, EventStream};
use rocket::serde::json::Json;
use rocket::serde::json::serde_json::{self, json};
use rocket::tokio::select;
use rocket::tokio::sync::broadcast::error::RecvError;
use rocket::tokio::time::interval;
use rocket::{Route, Shutdown, State, delete, get, post, routes};
use rocket_ws::{Channel, Message, WebSocket};
use serde::Deserialize;

use crate::api_server::auth::auth_guard::KeyAuthUser;
use crate::api_server::auth::{User, Userdb};

use crate::server_management::server::{ServerError, ServerInfo, ServerManager, TypeOfServ};

// ---------------------------------------------------------------------------
// Utilisateurs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct NewUser {
    name: String,
    username: String,
    password: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct ChangeUser {
    old_username: String,
    username: Option<String>,
    name: Option<String>,
    password: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct RemoveUser {
    username: String,
}

#[get("/api")]
fn index() -> &'static str {
    "Welcome, this is an api"
}

#[post("/user/create", format = "json", data = "<user_data>")]
fn user_profile(user_data: Json<NewUser>) -> Result<String, (Status, String)> {
    let user_info = user_data.into_inner();
    let mut user = User::new(user_info.name, user_info.username);
    if let Some(pwd) = user_info.password {
        user.change_password(pwd);
    }
    let mut db = Userdb::new().map_err(|_| {
        (
            Status::InternalServerError,
            "Erreur lors de l'ouverture de la db".to_string(),
        )
    })?;
    match db.add(user) {
        Ok(_) => Ok("ok".to_string()),
        Err(e) => Err((Status::BadRequest, e)),
    }
}

#[post("/user/remove", format = "json", data = "<req>")]
fn user_remove(auth: KeyAuthUser, req: Json<RemoveUser>) -> Result<String, (Status, String)> {
    let username_to_remove = req.into_inner().username;

    if auth.username != username_to_remove {
        return Err((
            Status::Forbidden,
            "Tu ne peux supprimer que ton propre compte".to_string(),
        ));
    }

    let mut db =
        Userdb::new().map_err(|_| (Status::InternalServerError, "Erreur db".to_string()))?;

    match db.remove(username_to_remove) {
        Ok(_) => Ok("ok".to_string()),
        Err(e) => Err((Status::NotFound, e)),
    }
}

#[post("/user/modify", format = "json", data = "<u>")]
fn user_modify(auth: KeyAuthUser, u: Json<ChangeUser>) -> Result<String, (Status, String)> {
    let change = u.into_inner();

    if auth.username != change.old_username {
        return Err((
            Status::Forbidden,
            "Tu ne peux modifier que ton propre compte".to_string(),
        ));
    }

    let mut db =
        Userdb::new().map_err(|_| (Status::InternalServerError, "Erreur db".to_string()))?;

    match db.update_user_fields(
        &change.old_username,
        change.username,
        change.name,
        change.password,
    ) {
        Ok(_) => Ok("ok".to_string()),
        Err(e) => Err((Status::BadRequest, e)),
    }
}

// ---------------------------------------------------------------------------
// Serveurs
// ---------------------------------------------------------------------------

type ApiResult<T> = Result<T, (Status, String)>;

/// Délai accordé à un arrêt propre avant d'abandonner un redémarrage.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

fn api_err(e: ServerError) -> (Status, String) {
    match e {
        ServerError::NotFound => (Status::NotFound, "Serveur introuvable".into()),
        ServerError::AlreadyRunning => (
            Status::Conflict,
            "Le serveur est déjà en cours d'exécution".into(),
        ),
        ServerError::NotRunning => (Status::Conflict, "Le serveur n'est pas démarré".into()),
        ServerError::Invalid(msg) => (Status::BadRequest, msg),
        ServerError::Internal(msg) => (Status::InternalServerError, msg),
    }
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct NewServer {
    java_path: Option<String>,
    executable_path: String,
    type_serv: TypeOfServ,
    min_ram: usize,
    max_ram: usize,
    args: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct ServerSettings {
    java_path: Option<String>,
    min_ram: Option<usize>,
    max_ram: Option<usize>,
    args: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct CommandBody {
    command: String,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct InputBody {
    data: String,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct ResizeBody {
    rows: u16,
    cols: u16,
}

/// Ajoute le retour à la ligne qui valide la commande.
fn command_line(command: &str) -> String {
    format!("{}\n", command.trim_end_matches(['\r', '\n']))
}

#[get("/servers")]
fn server_list(_auth: KeyAuthUser, mgr: &State<ServerManager>) -> Json<Vec<ServerInfo>> {
    Json(mgr.list())
}

#[post("/server/create", format = "json", data = "<req>")]
fn server_create(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    req: Json<NewServer>,
) -> ApiResult<Json<ServerInfo>> {
    let r = req.into_inner();
    mgr.create(
        r.java_path,
        r.executable_path,
        r.type_serv,
        r.min_ram,
        r.max_ram,
        r.args.unwrap_or_default(),
    )
    .map(Json)
    .map_err(api_err)
}

#[get("/server/<id>")]
fn server_info(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<Json<ServerInfo>> {
    mgr.info(id).map(Json).map_err(api_err)
}

#[delete("/server/<id>")]
fn server_delete(_auth: KeyAuthUser, mgr: &State<ServerManager>, id: &str) -> ApiResult<String> {
    mgr.delete(id).map_err(api_err)?;
    Ok("ok".to_string())
}

#[post("/server/<id>/settings", format = "json", data = "<req>")]
fn server_settings(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<ServerSettings>,
) -> ApiResult<Json<ServerInfo>> {
    let r = req.into_inner();
    mgr.update_settings(id, r.java_path, r.min_ram, r.max_ram, r.args)
        .map(Json)
        .map_err(api_err)
}

#[post("/server/<id>/start")]
fn server_start(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
) -> ApiResult<Json<ServerInfo>> {
    mgr.start(id).map(Json).map_err(api_err)
}

/// Arrêt propre (SIGTERM). Renvoie tout de suite ; suis l'état avec GET /server/<id>.
#[post("/server/<id>/stop")]
fn server_stop(_auth: KeyAuthUser, mgr: &State<ServerManager>, id: &str) -> ApiResult<String> {
    mgr.stop(id).map_err(api_err)?;
    Ok("ok".to_string())
}

/// Arrêt brutal (SIGKILL).
#[post("/server/<id>/kill")]
fn server_kill(_auth: KeyAuthUser, mgr: &State<ServerManager>, id: &str) -> ApiResult<String> {
    mgr.kill(id).map_err(api_err)?;
    Ok("ok".to_string())
}

#[post("/server/<id>/restart")]
async fn server_restart(
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

/// Envoie une commande au programme (le retour à la ligne est ajouté).
#[post("/server/<id>/command", format = "json", data = "<req>")]
fn server_command(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<CommandBody>,
) -> ApiResult<String> {
    mgr.write_input(id, command_line(&req.command).as_bytes())
        .map_err(api_err)?;
    Ok("ok".to_string())
}

/// Envoie du texte brut tel quel (sans retour à la ligne ajouté) : pour un
/// terminal interactif, par exemple xterm.js.
#[post("/server/<id>/input", format = "json", data = "<req>")]
fn server_input(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<InputBody>,
) -> ApiResult<String> {
    mgr.write_input(id, req.data.as_bytes()).map_err(api_err)?;
    Ok("ok".to_string())
}

#[post("/server/<id>/resize", format = "json", data = "<req>")]
fn server_resize(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<ResizeBody>,
) -> ApiResult<String> {
    if req.rows == 0 || req.cols == 0 {
        return Err((
            Status::BadRequest,
            "rows et cols doivent être > 0".to_string(),
        ));
    }
    mgr.resize(id, req.rows, req.cols).map_err(api_err)?;
    Ok("ok".to_string())
}

/// Historique de la console (dernières `tail` lignes si précisé).
#[get("/server/<id>/logs?<tail>")]
fn server_logs(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    tail: Option<usize>,
) -> ApiResult<String> {
    let bytes = mgr.console(id).map_err(api_err)?.snapshot();
    let text = String::from_utf8_lossy(&bytes);

    Ok(match tail {
        Some(n) => {
            let mut lines: Vec<&str> = text.lines().rev().take(n).collect();
            lines.reverse();
            lines.join("\n")
        }
        None => text.into_owned(),
    })
}

/// Console en direct (Server-Sent Events). Le premier événement (`history`)
/// contient l'historique, les suivants la nouvelle sortie. Chaque `data:` est
/// une chaîne JSON, à décoder avec JSON.parse.
#[get("/server/<id>/console")]
fn server_console(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    mut end: Shutdown,
) -> ApiResult<EventStream![]> {
    let console = mgr.console(id).map_err(api_err)?;
    let (history, mut rx) = console.subscribe();

    Ok(EventStream! {
        if !history.is_empty() {
            yield Event::json(&String::from_utf8_lossy(&history)).event("history");
        }

        loop {
            let chunk = select! {
                msg = rx.recv() => match msg {
                    Ok(chunk) => chunk,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                },
                _ = &mut end => break,
            };
            yield Event::json(&String::from_utf8_lossy(&chunk));
        }
    })
}

// ---------------------------------------------------------------------------
// WebSocket : console bidirectionnelle
// ---------------------------------------------------------------------------
//
// Trames texte JSON.
//   serveur -> client : {"type":"history"|"data"|"error","data":"..."}
//   client -> serveur : {"type":"command","data":"say salut"}   (\n ajouté)
//                       {"type":"input","data":"texte brut"}      (tel quel)
//                       {"type":"resize","rows":40,"cols":120}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde", tag = "type", rename_all = "lowercase")]
enum WsIn {
    Command { data: String },
    Input { data: String },
    Resize { rows: u16, cols: u16 },
}

fn ws_frame(kind: &str, data: &str) -> Message {
    Message::text(json!({ "type": kind, "data": data }).to_string())
}

fn handle_ws_input(mgr: &ServerManager, id: &str, raw: &str) -> Result<(), String> {
    let msg: WsIn = serde_json::from_str(raw).map_err(|_| "Message invalide".to_string())?;

    match msg {
        WsIn::Command { data } => mgr.write_input(id, command_line(&data).as_bytes()),
        WsIn::Input { data } => mgr.write_input(id, data.as_bytes()),
        WsIn::Resize { rows, cols } => {
            if rows == 0 || cols == 0 {
                return Err("rows et cols doivent être > 0".to_string());
            }
            mgr.resize(id, rows, cols)
        }
    }
    .map_err(|e| api_err(e).1)
}

/// Console en direct dans les deux sens. L'authentification (`KeyAuthUser`)
/// se fait sur la requête d'ouverture, avec les en-têtes signés habituels.
#[get("/server/<id>/ws")]
fn server_ws<'r>(
    _auth: KeyAuthUser,
    ws: WebSocket,
    mgr: &'r State<ServerManager>,
    id: &'r str,
    mut end: Shutdown,
) -> ApiResult<Channel<'r>> {
    let console = mgr.console(id).map_err(api_err)?;
    let (history, mut rx) = console.subscribe();
    let id = id.to_string();

    Ok(ws.channel(move |mut stream| {
        Box::pin(async move {
            if !history.is_empty() {
                let text = String::from_utf8_lossy(&history);
                stream.send(ws_frame("history", &text)).await?;
            }

            // Battement de cœur : garde la connexion vivante à travers les
            // NAT / proxys et détecte les clients disparus.
            let mut heartbeat = interval(Duration::from_secs(25));
            heartbeat.tick().await; // le premier tick est immédiat

            loop {
                select! {
                    out = rx.recv() => match out {
                        Ok(chunk) => {
                            let text = String::from_utf8_lossy(&chunk);
                            stream.send(ws_frame("data", &text)).await?;
                        }
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => break,
                    },
                    incoming = stream.next() => match incoming {
                        Some(Ok(Message::Text(text))) => {
                            if let Err(msg) = handle_ws_input(mgr, &id, &text) {
                                stream.send(ws_frame("error", &msg)).await?;
                            }
                        }
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                        Some(Ok(_)) => {} // ping / pong : gérés par la bibliothèque
                    },
                    _ = heartbeat.tick() => {
                        stream.send(Message::Ping(Default::default())).await?;
                    }
                    _ = &mut end => break,
                }
            }
            Ok(())
        })
    }))
}

pub fn get_routes() -> Vec<Route> {
    routes![
        index,
        user_profile,
        user_remove,
        user_modify,
        server_list,
        server_create,
        server_info,
        server_delete,
        server_settings,
        server_start,
        server_stop,
        server_kill,
        server_restart,
        server_command,
        server_input,
        server_resize,
        server_logs,
        server_console,
        server_ws,
    ]
}
