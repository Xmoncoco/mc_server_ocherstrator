use super::*;

// ---------------------------------------------------------------------------
// ServerManager : état partagé Rocket
// ---------------------------------------------------------------------------

pub struct ServerManager {
    servers: Mutex<HashMap<String, Server>>,
}

impl ServerManager {
    pub fn load() -> Self {
        Server::cleanup_uploads();
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
        infos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        infos
    }

    pub fn info(&self, id: &str) -> Result<ServerInfo, ServerError> {
        self.with(id, |s| Ok(s.info()))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_manual(
        &self,
        name: Option<String>,
        java_path: Option<String>,
        executable_path: &str,
        type_serv: TypeOfServ,
        min_ram: usize,
        max_ram: usize,
        args: Vec<String>,
        accept_eula: bool,
    ) -> Result<ServerInfo, ServerError> {
        validate_ram(min_ram, max_ram)?;
        let mut server = Server::create_manual(
            name,
            java_path,
            executable_path,
            type_serv,
            min_ram,
            max_ram,
            args,
            accept_eula,
        )?;
        let info = server.info();
        self.lock().insert(server.uuid.clone(), server);
        Ok(info)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_auto(
        &self,
        name: Option<String>,
        mod_loader: ModLoader,
        mc_version: String,
        loader_version: Option<String>,
        java_path: Option<String>,
        min_ram: usize,
        max_ram: usize,
        args: Vec<String>,
        accept_eula: bool,
    ) -> Result<ServerInfo, ServerError> {
        validate_ram(min_ram, max_ram)?;
        let mut server = Server::create_auto(
            name,
            mod_loader,
            mc_version,
            loader_version,
            java_path,
            min_ram,
            max_ram,
            args,
            accept_eula,
        )
        .await?;
        let info = server.info();
        self.lock().insert(server.uuid.clone(), server);
        Ok(info)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_settings(
        &self,
        id: &str,
        name: Option<String>,
        java_path: Option<String>,
        min_ram: Option<usize>,
        max_ram: Option<usize>,
        args: Option<Vec<String>>,
    ) -> Result<ServerInfo, ServerError> {
        self.with(id, |s| {
            validate_ram(min_ram.unwrap_or(s.min_ram), max_ram.unwrap_or(s.max_ram))?;

            if let Some(v) = name {
                let v = v.trim().to_string();
                if v.is_empty() {
                    return Err(ServerError::Invalid(
                        "Le nom du serveur ne peut pas être vide".into(),
                    ));
                }
                s.name = v;
            }
            if let Some(v) = java_path {
                if !v.trim().is_empty() {
                    s.java_path = Some(v);
                }
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

    // --- Mods ------------------------------------------------------------

    pub fn list_mods(&self, id: &str) -> Result<Vec<ModInfo>, ServerError> {
        self.with(id, |s| s.list_mods())
    }

    pub fn prepare_mod_upload(&self, id: &str, filename: &str) -> Result<PathBuf, ServerError> {
        self.with(id, |s| s.prepare_mod_upload(filename))
    }

    pub fn commit_mod_upload(&self, id: &str, filename: &str) -> Result<(), ServerError> {
        self.with(id, |s| s.commit_mod_upload(filename))
    }

    pub fn toggle_mod(&self, id: &str, name: &str) -> Result<Vec<ModInfo>, ServerError> {
        self.with(id, |s| s.toggle_mod(name))
    }

    pub fn delete_mod(&self, id: &str, name: &str) -> Result<Vec<ModInfo>, ServerError> {
        self.with(id, |s| s.delete_mod(name))
    }

    pub fn modpack_marker(&self, id: &str) -> Result<Option<String>, ServerError> {
        self.with(id, |s| Ok(s.modpack_marker()))
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
