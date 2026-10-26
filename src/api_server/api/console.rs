use std::time::Duration;

use rocket::futures::{SinkExt, StreamExt};
use rocket::http::Status;
use rocket::response::stream::{Event, EventStream};
use rocket::serde::json::Json;
use rocket::serde::json::serde_json::{self, json};
use rocket::tokio::select;
use rocket::tokio::sync::broadcast::error::RecvError;
use rocket::tokio::time::interval;
use rocket::{Shutdown, State, get, post};
use rocket_ws::{Channel, Message, WebSocket};
use serde::Deserialize;

use crate::api_server::auth::auth_guard::KeyAuthUser;
use crate::server_management::server::ServerManager;

use super::{ApiResult, api_err};

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
/// Envoie une commande au programme (le retour à la ligne est ajouté).
#[post("/server/<id>/command", format = "json", data = "<req>")]
pub(super) fn server_command(
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
pub(super) fn server_input(
    _auth: KeyAuthUser,
    mgr: &State<ServerManager>,
    id: &str,
    req: Json<InputBody>,
) -> ApiResult<String> {
    mgr.write_input(id, req.data.as_bytes()).map_err(api_err)?;
    Ok("ok".to_string())
}

#[post("/server/<id>/resize", format = "json", data = "<req>")]
pub(super) fn server_resize(
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
pub(super) fn server_logs(
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
pub(super) fn server_console(
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
pub(super) fn server_ws<'r>(
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
