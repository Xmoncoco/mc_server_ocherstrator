use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use rocket::tokio::sync::broadcast;
use rocket::tokio::time::sleep;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server_management::loaders::{self, ModLoader};
use crate::server_management::tty::{self, PtyReader, PtySession};

/// Taille max de l'historique de console gardé en mémoire (octets).
const MAX_HISTORY: usize = 256 * 1024;
// ---------------------------------------------------------------------------
// Erreurs
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ServerError {
    NotFound,
    AlreadyRunning,
    NotRunning,
    Invalid(String),
    Internal(String),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::NotFound => write!(f, "élément introuvable"),
            ServerError::AlreadyRunning => write!(f, "déjà en cours d'exécution"),
            ServerError::NotRunning => write!(f, "n'est pas démarré"),
            ServerError::Invalid(message) => write!(f, "{message}"),
            ServerError::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<io::Error> for ServerError {
    fn from(e: io::Error) -> Self {
        ServerError::Internal(e.to_string())
    }
}

impl From<String> for ServerError {
    fn from(e: String) -> Self {
        ServerError::Internal(e)
    }
}

impl From<&str> for ServerError {
    fn from(e: &str) -> Self {
        ServerError::Internal(e.to_string())
    }
}

fn internal(e: impl std::fmt::Display) -> ServerError {
    ServerError::Internal(e.to_string())
}
/// Envoie un signal à tout le groupe de processus (l'enfant est leader de sa
/// session, donc pgid == pid).
#[cfg(unix)]
fn signal_group(pid: u32, sig: libc::c_int) -> Result<(), ServerError> {
    if unsafe { libc::killpg(pid as libc::pid_t, sig) } == -1 {
        let err = io::Error::last_os_error();
        // ESRCH : le groupe n'existe plus, rien à faire.
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(internal(format!("envoi du signal impossible : {err}")));
        }
    }
    Ok(())
}
// ---------------------------------------------------------------------------
// Noms de fichiers
// ---------------------------------------------------------------------------

/// Nettoie un nom de fichier destiné à l'instance (pas de traversée de
/// répertoire, pas de fichier caché).
pub fn safe_file_name(raw: &str) -> Result<String, ServerError> {
    if raw.is_empty()
        || raw.contains('/')
        || raw.contains('\\')
        || raw.contains('\0')
        || raw.starts_with('.')
        || raw == "."
        || raw == ".."
    {
        return Err(ServerError::Invalid("Nom de fichier invalide".into()));
    }

    let name = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| ServerError::Invalid("Nom de fichier invalide".into()))?;

    if name.is_empty() {
        return Err(ServerError::Invalid("Nom de fichier invalide".into()));
    }
    Ok(name.to_string())
}
// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub enum TypeOfServ {
    JavaBased,
    Executable,
}

#[derive(Serialize, Deserialize)]
pub struct Server {
    pub path: String,
    pub uuid: String,
    #[serde(default)]
    pub name: String,
    pub type_serv: TypeOfServ,
    pub java_path: Option<String>,
    pub executable: String,
    /// Mémoire en Mo (passée à Java via -Xms / -Xmx).
    pub min_ram: usize,
    pub max_ram: usize,
    /// Arguments passés au programme (après `-jar <jar>` pour Java).
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub mod_loader: ModLoader,
    #[serde(default)]
    pub mc_version: Option<String>,
    #[serde(default)]
    pub loader_version: Option<String>,
    /// Fichier d'arguments Java (Forge/NeoForge) : lancé avec `@fichier`.
    #[serde(default)]
    pub launch_args: Option<String>,

    // --- État d'exécution, jamais sauvegardé ---
    #[serde(skip)]
    pub session: Option<PtySession>,
    #[serde(skip)]
    pub console: Option<Arc<Console>>,
    #[serde(skip)]
    pub last_exit: Option<String>,
}

/// Vue sérialisable d'un serveur, renvoyée par l'API.
#[derive(Serialize)]
pub struct ServerInfo {
    pub uuid: String,
    pub name: String,
    pub type_serv: TypeOfServ,
    pub path: String,
    pub java_path: Option<String>,
    pub executable: String,
    pub launch_args: Option<String>,
    pub min_ram: usize,
    pub max_ram: usize,
    pub args: Vec<String>,
    pub mod_loader: ModLoader,
    pub mc_version: Option<String>,
    pub loader_version: Option<String>,
    pub running: bool,
    pub pid: Option<u32>,
    pub last_exit: Option<String>,
}
fn validate_ram(min_ram: usize, max_ram: usize) -> Result<(), ServerError> {
    if min_ram == 0 || max_ram == 0 {
        return Err(ServerError::Invalid(
            "La RAM doit être supérieure à 0".into(),
        ));
    }
    if min_ram > max_ram {
        return Err(ServerError::Invalid(
            "La RAM minimale ne peut pas dépasser la RAM maximale".into(),
        ));
    }
    if max_ram > 1024 * 1024 {
        return Err(ServerError::Invalid(
            "La RAM maximale ne peut pas dépasser 1 To".into(),
        ));
    }
    Ok(())
}

fn short_uuid(uuid: &str) -> &str {
    uuid.get(..8).unwrap_or(uuid)
}

fn default_name(uuid: &str) -> String {
    format!("Serveur {}", short_uuid(uuid))
}

fn write_eula(path: &Path) -> Result<(), ServerError> {
    fs::write(path.join("eula.txt"), "eula=true\n")?;
    Ok(())
}

fn is_inside(path: &Path, parent: &Path) -> bool {
    path.starts_with(parent)
}
impl Server {
    pub fn base_dir() -> PathBuf {
        crate::storage::data_dir()
    }

    pub fn meta_dir() -> PathBuf {
        Self::base_dir().join("meta")
    }

    pub fn uploads_dir() -> PathBuf {
        Self::base_dir().join("uploads")
    }

    pub fn instances_dir() -> PathBuf {
        Self::base_dir().join("instances")
    }

    fn normalize(&mut self) {
        if self.name.trim().is_empty() {
            self.name = default_name(&self.uuid);
        }
    }

    // Sauvegarde en TOML dans mcserv/meta/<uuid>.toml
    pub fn save_meta(&self) -> Result<(), Box<dyn std::error::Error>> {
        let meta_dir = Self::meta_dir();
        fs::create_dir_all(&meta_dir)?;

        let meta_file = meta_dir.join(format!("{}.toml", self.uuid));
        let toml_str = toml::to_string_pretty(self)?;
        fs::write(meta_file, toml_str)?;
        Ok(())
    }

    // Charge un serveur depuis un fichier .toml
    pub fn load_from_meta_file(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let mut server: Server = toml::from_str(&content)?;
        server.normalize();
        Ok(server)
    }

    /// Charge tous les serveurs de mcserv/meta/ (les fichiers illisibles sont ignorés).
    pub fn load_all_from_meta() -> Vec<Self> {
        let Ok(entries) = fs::read_dir(Self::meta_dir()) else {
            return Vec::new();
        };

        entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
            .filter_map(|p| match Self::load_from_meta_file(&p) {
                Ok(server) => Some(server),
                Err(e) => {
                    eprintln!("Meta ignorée ({}) : {e}", p.display());
                    None
                }
            })
            .collect()
    }

    /// Supprime les fichiers déposés par `POST /server/upload` il y a plus
    /// de 24 h et jamais utilisés.
    pub fn cleanup_uploads() {
        let dir = Self::uploads_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return;
        };
        let now = SystemTime::now();

        for entry in entries.flatten() {
            let path = entry.path();
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .map(|age| age > Duration::from_secs(24 * 60 * 60))
                .unwrap_or(false);
            if old {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }

    fn new_with(
        name: Option<String>,
        uuid: String,
        path: PathBuf,
        type_serv: TypeOfServ,
        java_path: Option<String>,
        executable: String,
        launch_args: Option<String>,
        min_ram: usize,
        max_ram: usize,
        args: Vec<String>,
        mod_loader: ModLoader,
        mc_version: Option<String>,
        loader_version: Option<String>,
    ) -> Self {
        let mut server = Self {
            path: path.to_string_lossy().to_string(),
            uuid,
            name: name.unwrap_or_default(),
            type_serv,
            java_path,
            executable,
            min_ram,
            max_ram,
            args,
            mod_loader,
            mc_version,
            loader_version,
            launch_args,
            session: None,
            console: None,
            last_exit: None,
        };
        server.normalize();
        server
    }

    /// Crée une instance à partir d'un fichier fourni à la main (chemin sur
    /// la machine serveur ou fichier déposé via `/server/upload`).
    #[allow(clippy::too_many_arguments)]
    pub fn create_manual(
        name: Option<String>,
        java_path: Option<String>,
        executable_path: &str,
        type_serv: TypeOfServ,
        min_ram: usize,
        max_ram: usize,
        args: Vec<String>,
        accept_eula: bool,
    ) -> Result<Self, ServerError> {
        let src_path = Path::new(executable_path);
        if !src_path.is_file() {
            return Err(ServerError::Invalid(format!(
                "Fichier introuvable : {executable_path}"
            )));
        }

        let exec_name = safe_file_name(
            src_path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| ServerError::Invalid("Chemin d'exécutable invalide".into()))?,
        )?;

        let uuid = Uuid::new_v4().to_string();
        let server_dir = Self::instances_dir().join(&uuid);
        fs::create_dir_all(&server_dir)?;
        let dest_path = server_dir.join(&exec_name);

        let result = (|| -> Result<Self, ServerError> {
            let from_upload = is_inside(src_path, &Self::uploads_dir());
            if from_upload {
                fs::rename(src_path, &dest_path).or_else(|_| {
                    fs::copy(src_path, &dest_path)?;
                    fs::remove_file(src_path)
                })?;
                // Nettoie le dossier d'upload s'il est maintenant vide.
                if let Some(parent) = src_path.parent() {
                    let _ = fs::remove_dir(parent);
                }
            } else {
                fs::copy(src_path, &dest_path)?;
            }

            fs::create_dir_all(server_dir.join("mods"))?;
            if accept_eula {
                write_eula(&server_dir)?;
            }

            let server = Self::new_with(
                name,
                uuid.clone(),
                server_dir.clone(),
                type_serv,
                java_path.filter(|p| !p.trim().is_empty()),
                exec_name,
                None,
                min_ram,
                max_ram,
                args,
                ModLoader::Custom,
                None,
                None,
            );
            server.save_meta().map_err(internal)?;
            Ok(server)
        })();

        if result.is_err() {
            let _ = fs::remove_dir_all(&server_dir);
        }
        result
    }

    /// Crée une instance en téléchargeant et installant le loader choisi.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_auto(
        name: Option<String>,
        mod_loader: ModLoader,
        mc_version: String,
        loader_version: Option<String>,
        java_path: Option<String>,
        min_ram: usize,
        max_ram: usize,
        args: Vec<String>,
        accept_eula: bool,
    ) -> Result<Self, ServerError> {
        if mod_loader == ModLoader::Custom {
            return Err(ServerError::Invalid(
                "Choisis un mod loader à installer".into(),
            ));
        }
        if mc_version.trim().is_empty() {
            return Err(ServerError::Invalid(
                "La version Minecraft est obligatoire".into(),
            ));
        }

        let uuid = Uuid::new_v4().to_string();
        let server_dir = Self::instances_dir().join(&uuid);
        fs::create_dir_all(&server_dir)?;

        let java = java_path
            .clone()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| "java".to_string());

        let installed = match loaders::install(
            mod_loader,
            mc_version.trim(),
            loader_version.as_deref().filter(|v| !v.trim().is_empty()),
            &java,
            &server_dir,
            accept_eula,
        )
        .await
        {
            Ok(installed) => installed,
            Err(e) => {
                let _ = fs::remove_dir_all(&server_dir);
                return Err(ServerError::Internal(e));
            }
        };

        fs::create_dir_all(server_dir.join("mods"))?;

        let server = Self::new_with(
            name,
            uuid,
            server_dir.clone(),
            TypeOfServ::JavaBased,
            Some(java),
            installed.executable,
            installed.launch_args,
            min_ram,
            max_ram,
            args,
            mod_loader,
            Some(mc_version.trim().to_string()),
            installed.loader_version,
        );

        if let Err(e) = server.save_meta() {
            let _ = fs::remove_dir_all(&server_dir);
            return Err(internal(e));
        }
        Ok(server)
    }

    // -----------------------------------------------------------------------
    // Exécution
    // -----------------------------------------------------------------------

    /// Programme + arguments à lancer.
    fn command_line(&self) -> (PathBuf, Vec<String>) {
        let exe = Path::new(&self.path).join(&self.executable);

        match self.type_serv {
            TypeOfServ::JavaBased => {
                let java = PathBuf::from(self.java_path.as_deref().unwrap_or("java"));
                let mut args = vec![
                    format!("-Xms{}M", self.min_ram),
                    format!("-Xmx{}M", self.max_ram),
                ];

                match &self.launch_args {
                    // Forge / NeoForge : le jar est décrit dans unix_args.txt.
                    Some(launch_args) => {
                        args.push(format!(
                            "@{}",
                            Path::new(&self.path).join(launch_args).to_string_lossy()
                        ));
                    }
                    None => {
                        args.push("-jar".to_string());
                        args.push(exe.to_string_lossy().into_owned());
                    }
                }

                args.extend(self.args.iter().cloned());
                (java, args)
            }
            TypeOfServ::Executable => (exe, self.args.clone()),
        }
    }

    /// La console (créée à la demande, conservée entre les redémarrages).
    pub fn console(&mut self) -> Arc<Console> {
        Arc::clone(self.console.get_or_insert_with(Console::new))
    }

    pub fn pid(&self) -> Option<u32> {
        self.session.as_ref().map(|s| s.child.id())
    }

    /// Le processus tourne-t-il encore ? Nettoie la session s'il est terminé.
    pub fn is_running(&mut self) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };

        match session.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                self.last_exit = Some(status.to_string());
                self.session = None;
                false
            }
            Err(_) => true,
        }
    }

    fn running_session(&mut self) -> Result<&mut PtySession, ServerError> {
        if !self.is_running() {
            return Err(ServerError::NotRunning);
        }
        self.session.as_mut().ok_or(ServerError::NotRunning)
    }

    pub fn start(&mut self) -> Result<(), ServerError> {
        if self.is_running() {
            return Err(ServerError::AlreadyRunning);
        }

        fs::create_dir_all(self.mods_dir())?;

        let (program, args) = self.command_line();
        let session =
            tty::spawn_in_pty(&program, Path::new(&self.path), &args, tty::winsize(24, 80))
                .map_err(|e| {
                    internal(format!("impossible de lancer {} : {e}", program.display()))
                })?;

        let console = self.console();
        let reader = session.reader()?;
        thread::Builder::new()
            .name(format!("pty-{}", short_uuid(&self.uuid)))
            .spawn(move || pump(reader, console))?;

        self.last_exit = None;
        self.session = Some(session);
        Ok(())
    }

    /// Arrêt propre : SIGTERM sur tout le groupe de processus.
    pub fn stop(&mut self) -> Result<(), ServerError> {
        #[cfg(unix)]
        {
            let pid = self.running_session()?.child.id();
            return signal_group(pid, libc::SIGTERM);
        }

        // Windows n'a pas de SIGTERM : on demande l'arrêt coopératif au
        // serveur Minecraft via sa console. `kill` reste disponible pour
        // forcer l'arrêt d'un programme qui l'ignore.
        #[cfg(windows)]
        {
            Ok(self.running_session()?.write_input(b"stop\n")?)
        }
    }

    /// Arrêt brutal : SIGKILL sur tout le groupe de processus.
    pub fn kill(&mut self) -> Result<(), ServerError> {
        Ok(self.running_session()?.kill()?)
    }

    /// Envoie des octets sur l'entrée du programme.
    pub fn write_input(&mut self, data: &[u8]) -> Result<(), ServerError> {
        Ok(self.running_session()?.write_input(data)?)
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<(), ServerError> {
        Ok(self.running_session()?.resize(tty::winsize(rows, cols))?)
    }

    pub fn info(&mut self) -> ServerInfo {
        let running = self.is_running();
        ServerInfo {
            uuid: self.uuid.clone(),
            name: self.name.clone(),
            type_serv: self.type_serv,
            path: self.path.clone(),
            java_path: self.java_path.clone(),
            executable: self.executable.clone(),
            launch_args: self.launch_args.clone(),
            min_ram: self.min_ram,
            max_ram: self.max_ram,
            args: self.args.clone(),
            mod_loader: self.mod_loader,
            mc_version: self.mc_version.clone(),
            loader_version: self.loader_version.clone(),
            running,
            pid: if running { self.pid() } else { None },
            last_exit: self.last_exit.clone(),
        }
    }
    /// Supprime le dossier d'instance et le fichier meta.
    fn remove_files(&self) -> Result<(), ServerError> {
        let dir = Path::new(&self.path);
        let instances = Self::instances_dir();
        let suspicious = !dir.starts_with(&instances)
            || dir.components().any(|c| matches!(c, Component::ParentDir));
        if suspicious {
            return Err(ServerError::Invalid(
                "Chemin d'instance suspect, suppression refusée".into(),
            ));
        }

        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        let meta = Self::meta_dir().join(format!("{}.toml", self.uuid));
        if meta.exists() {
            fs::remove_file(meta)?;
        }
        Ok(())
    }
}

mod console;
mod manager;
mod mods;

pub use console::Console;
use console::pump;
pub use manager::ServerManager;
pub use mods::ModInfo;
