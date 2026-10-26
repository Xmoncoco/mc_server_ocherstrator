use rocket::http::Status;
use rocket::{Route, get, routes};

use crate::server_management::server::ServerError;

pub(crate) type ApiResult<T> = Result<T, (Status, String)>;

pub(crate) fn api_err(e: ServerError) -> (Status, String) {
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

#[get("/api")]
fn index() -> &'static str {
    "Welcome, this is an api"
}

mod catalog;
mod console;
mod mods;
mod servers;
mod users;

use catalog::*;
use console::*;
use mods::*;
use servers::*;
use users::*;

pub fn get_routes() -> Vec<Route> {
    routes![
        index,
        user_profile,
        user_remove,
        user_modify,
        server_list,
        server_create,
        server_upload,
        server_info,
        server_launch_files,
        server_set_launch_file,
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
        loader_info,
        mod_list,
        mod_upload,
        mod_toggle,
        mod_delete,
        catalog_config,
        catalog_set_config,
        catalog_search,
        mod_install_catalog,
        server_install_modpack,
        server_create_from_modpack,
        server_modpack,
    ]
}
