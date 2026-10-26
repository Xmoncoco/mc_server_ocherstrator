//! Emplacement des données de mcserv, portable Linux / macOS / Windows.
//!
//! Priorité à `MCSSERV_DATA_DIR`, puis aux variables standard de la
//! plateforme. Le module NixOS et les déploiements Windows peuvent ainsi
//! choisir un dossier dédié sans toucher au code.

use std::env;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    if let Some(dir) = env::var_os("MCSSERV_DATA_DIR") {
        return PathBuf::from(dir);
    }

    #[cfg(windows)]
    {
        if let Some(dir) = env::var_os("LOCALAPPDATA").or_else(|| env::var_os("APPDATA")) {
            return PathBuf::from(dir).join("mcserv");
        }
    }

    #[cfg(not(windows))]
    {
        // Compatibilité : les installations existantes sont dans
        // ~/.local/share/mcserv, même si XDG_DATA_HOME est défini.
        if let Some(home) = env::var_os("HOME") {
            let legacy = PathBuf::from(&home).join(".local/share/mcserv");
            if legacy.exists() {
                return legacy;
            }
        }
        if let Some(dir) = env::var_os("XDG_DATA_HOME") {
            return PathBuf::from(dir).join("mcserv");
        }
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home).join(".local/share/mcserv");
        }
    }

    PathBuf::from("mcserv")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dossier_non_vide() {
        let dir = data_dir();
        assert!(!dir.as_os_str().is_empty());
    }
}
