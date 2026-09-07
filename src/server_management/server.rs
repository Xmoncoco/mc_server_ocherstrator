use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Clone)]
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
    pub min_ram: usize,
    pub max_ram: usize,

    #[serde(skip)]
    pub child: Option<Child>,
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
        let mut server: Server = toml::from_str(&content)?;
        server.child = None;
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
            .expect("Chemin d'exécutable invalide")
            .to_string_lossy()
            .to_string();

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
            child: None,
        };

        server.save_meta()?;

        Ok(server)
    }
}