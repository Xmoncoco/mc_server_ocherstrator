use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use rocket::tokio::sync::broadcast;
use rocket::tokio::time::sleep;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

impl From<io::Error> for ServerError {
    fn from(e: io::Error) -> Self {
        ServerError::Internal(e.to_string())
    }
}

fn internal(e: impl std::fmt::Display) -> ServerError {
    ServerError::Internal(e.to_string())
}

// ---------------------------------------------------------------------------
// Console : historique + diffusion en direct de la sortie du programme
// ---------------------------------------------------------------------------

pub struct Console {
    history: Mutex<VecDeque<u8>>,
    tx: broadcast::Sender<Vec<u8>>,
}

impl Console {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(256);
        Arc::new(Self {
            history: Mutex::new(VecDeque::new()),
            tx,
        })
    }

    fn push(&self, chunk: &[u8]) {
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        history.extend(chunk);
        if history.len() > MAX_HISTORY {
            let excess = history.len() - MAX_HISTORY;
            drop(history.drain(..excess));
        }
        // On envoie sous le verrou : `subscribe` ne peut ainsi ni rater
        // ni dupliquer de données. Une erreur = personne n'écoute.
        let _ = self.tx.send(chunk.to_vec());
    }

    /// Tout l'historique conservé.
    pub fn snapshot(&self) -> Vec<u8> {
        let history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        history.iter().copied().collect()
    }

    /// Historique actuel + un récepteur pour la suite, sans trou ni doublon.
    pub fn subscribe(&self) -> (Vec<u8>, broadcast::Receiver<Vec<u8>>) {
        let history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let rx = self.tx.subscribe();
        (history.iter().copied().collect(), rx)
    }
}

/// Longueur du plus long préfixe qui ne coupe pas un caractère UTF-8 en deux.
fn utf8_complete_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => bytes.len(), // octets invalides : on laisse passer
    }
}

/// Lit le pty jusqu'à la fin du programme et alimente la console.
fn pump(mut reader: PtyReader, console: Arc<Console>) {
    let mut buf = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::new();

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                let cut = utf8_complete_len(&pending);
                if cut > 0 {
                    console.push(&pending[..cut]);
                    drop(pending.drain(..cut));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if !pending.is_empty() {
        console.push(&pending);
    }
}

/// Envoie un signal à tout le groupe de processus (l'enfant est leader de sa
/// session, donc pgid == pid).
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
    pub type_serv: TypeOfServ,
    pub java_path: Option<String>,
    pub executable: String,
    /// Mémoire en Mo (passée à Java via -Xms / -Xmx).
    pub min_ram: usize,
    pub max_ram: usize,
    /// Arguments passés au programme (après `-jar <jar>` pour Java).
    #[serde(default)]
    pub args: Vec<String>,

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
    pub type_serv: TypeOfServ,
    pub path: String,
    pub java_path: Option<String>,
    pub executable: String,
    pub min_ram: usize,
    pub max_ram: usize,
    pub args: Vec<String>,
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
            "min_ram ne peut pas dépasser max_ram".into(),
        ));
    }
    Ok(())
}

impl Server {
    pub fn base_dir() -> PathBuf {
        let home = env::var("HOME").expect("Variable $HOME introuvable");
        PathBuf::from(home).join(".local/share/mcserv")
    }

    pub fn meta_dir() -> PathBuf {
        Self::base_dir().join("meta")
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
        let server: Server = toml::from_str(&content)?;
        Ok(server)
    }

    // Cherche le premier .toml existant dans mcserv/meta/
    pub fn load_first_from_meta() -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let meta_dir = Self::meta_dir();
        if !meta_dir.exists() {
            return Ok(None);
        }

        for entry in fs::read_dir(meta_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                return Ok(Some(Self::load_from_meta_file(&path)?));
            }
        }

        Ok(None)
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

    pub fn new(
        java_path: Option<String>,
        executable_path: String,
        type_serv: TypeOfServ,
        min_ram: usize,
        max_ram: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let uuid = Uuid::new_v4().to_string();
        let server_dir = Self::base_dir().join("instances").join(&uuid);

        fs::create_dir_all(&server_dir)?;

        let src_path = Path::new(&executable_path);
        let exec_name = src_path
            .file_name()
            .ok_or("Chemin d'exécutable invalide")?
            .to_string_lossy()
            .to_string();

        //to implement : check if the type_of_serv is

        let dest_path = server_dir.join(&exec_name);
        fs::copy(src_path, dest_path)?;

        let server = Self {
            path: server_dir.to_string_lossy().to_string(),
            uuid,
            type_serv,
            java_path,
            executable: exec_name,
            min_ram,
            max_ram,
            args: Vec::new(),
            session: None,
            console: None,
            last_exit: None,
        };

        server.save_meta()?;

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
                    "-jar".to_string(),
                    exe.to_string_lossy().into_owned(),
                ];
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

        let (program, args) = self.command_line();
        let session =
            tty::spawn_in_pty(&program, Path::new(&self.path), &args, tty::winsize(24, 80))
                .map_err(|e| {
                    internal(format!("impossible de lancer {} : {e}", program.display()))
                })?;

        let console = self.console();
        let reader = session.reader()?;
        thread::Builder::new()
            .name(format!("pty-{}", &self.uuid[..8]))
            .spawn(move || pump(reader, console))?;

        self.last_exit = None;
        self.session = Some(session);
        Ok(())
    }

    /// Arrêt propre : SIGTERM sur tout le groupe de processus.
    pub fn stop(&mut self) -> Result<(), ServerError> {
        let pid = self.running_session()?.child.id();
        signal_group(pid, libc::SIGTERM)
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
            type_serv: self.type_serv,
            path: self.path.clone(),
            java_path: self.java_path.clone(),
            executable: self.executable.clone(),
            min_ram: self.min_ram,
            max_ram: self.max_ram,
            args: self.args.clone(),
            running,
            pid: if running { self.pid() } else { None },
            last_exit: self.last_exit.clone(),
        }
    }

    /// Supprime le dossier d'instance et le fichier meta.
    fn remove_files(&self) -> Result<(), ServerError> {
        let dir = Path::new(&self.path);
        let instances = Self::base_dir().join("instances");
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

// ---------------------------------------------------------------------------
// ServerManager : état partagé Rocket
// ---------------------------------------------------------------------------

pub struct ServerManager {
    servers: Mutex<HashMap<String, Server>>,
}

impl ServerManager {
    pub fn load() -> Self {
        let servers = Server::load_all_from_meta()
            .into_iter()
            .map(|s| (s.uuid.clone(), s))
            .collect();
        Self {
            servers: Mutex::new(servers),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Server>> {
        self.servers.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn with<T>(
        &self,
        id: &str,
        f: impl FnOnce(&mut Server) -> Result<T, ServerError>,
    ) -> Result<T, ServerError> {
        let mut map = self.lock();
        let server = map.get_mut(id).ok_or(ServerError::NotFound)?;
        f(server)
    }

    pub fn list(&self) -> Vec<ServerInfo> {
        let mut map = self.lock();
        let mut infos: Vec<ServerInfo> = map.values_mut().map(Server::info).collect();
        infos.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        infos
    }

    pub fn info(&self, id: &str) -> Result<ServerInfo, ServerError> {
        self.with(id, |s| Ok(s.info()))
    }

    pub fn create(
        &self,
        java_path: Option<String>,
        executable_path: String,
        type_serv: TypeOfServ,
        min_ram: usize,
        max_ram: usize,
        args: Vec<String>,
    ) -> Result<ServerInfo, ServerError> {
        validate_ram(min_ram, max_ram)?;
        if !Path::new(&executable_path).is_file() {
            return Err(ServerError::Invalid(format!(
                "Fichier introuvable : {executable_path}"
            )));
        }

        let mut server = Server::new(java_path, executable_path, type_serv, min_ram, max_ram)
            .map_err(internal)?;
        if !args.is_empty() {
            server.args = args;
            server.save_meta().map_err(internal)?;
        }

        let info = server.info();
        self.lock().insert(server.uuid.clone(), server);
        Ok(info)
    }

    pub fn update_settings(
        &self,
        id: &str,
        java_path: Option<String>,
        min_ram: Option<usize>,
        max_ram: Option<usize>,
        args: Option<Vec<String>>,
    ) -> Result<ServerInfo, ServerError> {
        self.with(id, |s| {
            validate_ram(min_ram.unwrap_or(s.min_ram), max_ram.unwrap_or(s.max_ram))?;

            if let Some(v) = java_path {
                s.java_path = Some(v);
            }
            if let Some(v) = min_ram {
                s.min_ram = v;
            }
            if let Some(v) = max_ram {
                s.max_ram = v;
            }
            if let Some(v) = args {
                s.args = v;
            }
            s.save_meta().map_err(internal)?;
            Ok(s.info()) // les changements s'appliquent au prochain démarrage
        })
    }

    pub fn delete(&self, id: &str) -> Result<(), ServerError> {
        let mut map = self.lock();
        let server = map.get_mut(id).ok_or(ServerError::NotFound)?;
        if server.is_running() {
            return Err(ServerError::AlreadyRunning);
        }
        server.remove_files()?;
        map.remove(id);
        Ok(())
    }

    pub fn start(&self, id: &str) -> Result<ServerInfo, ServerError> {
        self.with(id, |s| {
            s.start()?;
            Ok(s.info())
        })
    }

    pub fn stop(&self, id: &str) -> Result<(), ServerError> {
        self.with(id, |s| s.stop())
    }

    pub fn kill(&self, id: &str) -> Result<(), ServerError> {
        self.with(id, |s| s.kill())
    }

    pub fn is_running(&self, id: &str) -> Result<bool, ServerError> {
        self.with(id, |s| Ok(s.is_running()))
    }

    pub fn write_input(&self, id: &str, data: &[u8]) -> Result<(), ServerError> {
        self.with(id, |s| s.write_input(data))
    }

    pub fn resize(&self, id: &str, rows: u16, cols: u16) -> Result<(), ServerError> {
        self.with(id, |s| s.resize(rows, cols))
    }

    pub fn console(&self, id: &str) -> Result<Arc<Console>, ServerError> {
        self.with(id, |s| Ok(s.console()))
    }

    /// Attend l'arrêt du serveur. `Ok(false)` si le délai est dépassé.
    pub async fn wait_stopped(&self, id: &str, timeout: Duration) -> Result<bool, ServerError> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.is_running(id)? {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    /// Arrête proprement tous les serveurs (SIGTERM), puis tue les retardataires.
    /// À appeler à l'arrêt de Rocket : sinon la fermeture du pty tuerait les
    /// programmes sans leur laisser le temps de sauvegarder.
    pub async fn shutdown_all(&self, timeout: Duration) {
        {
            let mut map = self.lock();
            for server in map.values_mut() {
                if server.is_running() {
                    let _ = server.stop();
                }
            }
        }

        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let any_running = {
                let mut map = self.lock();
                map.values_mut().fold(false, |acc, s| s.is_running() || acc)
            };
            if !any_running {
                return;
            }
            sleep(Duration::from_millis(200)).await;
        }

        let mut map = self.lock();
        for server in map.values_mut() {
            if server.is_running() {
                let _ = server.kill();
            }
        }
    }
}
