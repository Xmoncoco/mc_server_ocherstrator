use super::*;
use serde::Serialize;

#[derive(Serialize)]
pub struct ModInfo {
    /// Nom affiché, sans le suffixe `.disabled`.
    pub name: String,
    pub size: u64,
    pub enabled: bool,
    pub modified: Option<u64>,
}

impl Server {
    // -----------------------------------------------------------------------
    // Mods
    // -----------------------------------------------------------------------

    pub fn mods_dir(&self) -> PathBuf {
        Path::new(&self.path).join("mods")
    }

    /// Contenu du marqueur `.mcserv/modpack.json` s'il existe.
    pub fn modpack_marker(&self) -> Option<String> {
        fs::read_to_string(Path::new(&self.path).join(".mcserv").join("modpack.json")).ok()
    }

    pub fn list_mods(&self) -> Result<Vec<ModInfo>, ServerError> {
        let dir = self.mods_dir();
        fs::create_dir_all(&dir)?;

        let mut mods = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_file() {
                continue;
            }

            let raw = entry.file_name().to_string_lossy().to_string();
            if raw.starts_with('.') {
                continue;
            }

            let (name, enabled) = match raw.strip_suffix(".disabled") {
                Some(stripped) => (stripped.to_string(), false),
                None => (raw.clone(), true),
            };

            let metadata = entry.metadata()?;
            let modified = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());

            mods.push(ModInfo {
                name,
                size: metadata.len(),
                enabled,
                modified,
            });
        }

        mods.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(mods)
    }

    /// Chemin temporaire d'upload (`.part`). Le mod existant n'est remplacé
    /// qu'une fois l'envoi terminé, via `commit_mod_upload`.
    pub fn prepare_mod_upload(&self, filename: &str) -> Result<PathBuf, ServerError> {
        let name = safe_file_name(filename)?;
        if !name.to_ascii_lowercase().ends_with(".jar") {
            return Err(ServerError::Invalid(
                "Seuls les fichiers .jar sont acceptés".into(),
            ));
        }

        let dir = self.mods_dir();
        fs::create_dir_all(&dir)?;

        let temp = dir.join(format!("{name}.part"));
        if temp.exists() {
            fs::remove_file(&temp)?;
        }
        Ok(temp)
    }

    /// Valide l'upload : remplace la destination et retire l'ancienne version
    /// désactivée pour éviter les doublons.
    pub fn commit_mod_upload(&self, filename: &str) -> Result<(), ServerError> {
        let name = safe_file_name(filename)?;
        let dir = self.mods_dir();
        let temp = dir.join(format!("{name}.part"));
        let final_path = dir.join(&name);

        if !temp.is_file() {
            return Err(ServerError::Invalid(
                "Fichier temporaire d'upload introuvable".into(),
            ));
        }
        // Windows ne remplace pas une destination existante avec rename.
        #[cfg(windows)]
        if final_path.is_file() {
            fs::remove_file(&final_path)?;
        }
        fs::rename(&temp, &final_path)?;

        let disabled = dir.join(format!("{name}.disabled"));
        if disabled.is_file() {
            fs::remove_file(disabled)?;
        }
        Ok(())
    }

    pub fn toggle_mod(&self, name: &str) -> Result<Vec<ModInfo>, ServerError> {
        let name = safe_file_name(name)?;
        if !name.to_ascii_lowercase().ends_with(".jar") {
            return Err(ServerError::Invalid(
                "Seuls les fichiers .jar peuvent être activés/désactivés".into(),
            ));
        }

        let dir = self.mods_dir();
        let enabled = dir.join(&name);
        let disabled = dir.join(format!("{name}.disabled"));

        if enabled.is_file() {
            fs::rename(&enabled, &disabled)?;
        } else if disabled.is_file() {
            fs::rename(&disabled, &enabled)?;
        } else {
            return Err(ServerError::NotFound);
        }

        self.list_mods()
    }

    pub fn delete_mod(&self, name: &str) -> Result<Vec<ModInfo>, ServerError> {
        let name = safe_file_name(name)?;
        let dir = self.mods_dir();
        let enabled = dir.join(&name);
        let disabled = dir.join(format!("{name}.disabled"));

        let mut found = false;
        if enabled.is_file() {
            fs::remove_file(&enabled)?;
            found = true;
        }
        if disabled.is_file() {
            fs::remove_file(&disabled)?;
            found = true;
        }
        if !found {
            return Err(ServerError::NotFound);
        }

        self.list_mods()
    }
}
