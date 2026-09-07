use std::fs::File;
use std::os::fd::{AsFd, AsRawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use nix::pty::openpty;

use crate::server_management::server;

mod api_server {
    pub mod auth {
        pub mod auth;
    }
}

mod server_management {
    pub mod server;
}

pub struct PtySession {
    pub pts_path: String,
    pub child: Child,
    pub master: File,
}

pub fn spawn_in_pty(
    program: &Path,
    working_dir: &Path,
    args: &[&str],
) -> Result<PtySession, Box<dyn std::error::Error>> {
    let pty = openpty(None, None)?;

    let pts_path = std::fs::read_link(format!("/proc/self/fd/{}", pty.slave.as_raw_fd()))?
        .to_string_lossy()
        .into_owned();

    let slave_in: Stdio = File::from(pty.slave.as_fd().try_clone_to_owned()?).into();
    let slave_out: Stdio = File::from(pty.slave.as_fd().try_clone_to_owned()?).into();
    let slave_err: Stdio = File::from(pty.slave).into();

    let child = Command::new(program)
        .current_dir(working_dir)
        .args(args)
        .stdin(slave_in)
        .stdout(slave_out)
        .stderr(slave_err)
        .spawn()?;

    let master = File::from(pty.master);

    Ok(PtySession {
        pts_path,
        child,
        master,
    })
}

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
}