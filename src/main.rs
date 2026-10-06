#[macro_use]
extern crate rocket;

use crate::server_management::server::ServerManager;
use rocket::fairing::AdHoc;
use std::time::Duration;

mod server_management {
    pub mod server;
    pub mod tty;
}

use crate::api_server::api;

mod api_server {
    pub mod api;
    pub mod auth;
}

/*
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Démarrage du gestionnaire de serveur...");

    let mut server = server_management::server::Server::new(
        None,
        "/home/coco/dev/pumpkin/pumpkin-X64-Linux".to_string(),
        server::TypeOfServ::Executable,
        0,
        0,
    )?;

    // Chemin absolu vers l'exécutable copié dans l'instance
    let exec_path = Path::new(&server.path).join(&server.executable);
    let work_dir = Path::new(&server.path);

    // Lance le process dans le pseudo-terminal avec le bon dossier de travail
    let session = spawn_in_pty(&exec_path, work_dir, &[])?;

    println!("Serveur actif sur : {}", session.pts_path);
    println!("Connexion interactive : screen {}", session.pts_path);

    server.child = Some(session.child);

    // Bloque jusqu'à l'arrêt du serveur
    if let Some(mut child) = server.child.take() {
        tokio::task::spawn_blocking(move || child.wait()).await??;
    }

    Ok(())
}*/

#[launch]
fn rocket() -> _ {
    rocket::build()
        .manage(ServerManager::load())
        .attach(AdHoc::on_shutdown("Arrêt des serveurs", |rocket| {
            Box::pin(async move {
                if let Some(mgr) = rocket.state::<ServerManager>() {
                    mgr.shutdown_all(Duration::from_secs(30)).await;
                }
            })
        }))
        .mount("/", api::get_routes())
}
