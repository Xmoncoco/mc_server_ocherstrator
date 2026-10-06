pub mod auth_guard;

use std::{env, fs, path::PathBuf};

use argon2::password_hash::PasswordHash;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct Userdb {
    db: Vec<User>,
    nb: usize,
}

#[derive(Serialize, Deserialize)]
pub struct User {
    name: String,
    username: String,
    profile_picture: String,
    hash: String,
    publickey: String,
    uuid: String, //the minecraft one
}

pub fn base_dir() -> PathBuf {
    let home = env::var("HOME").expect("Variable $HOME introuvable");
    PathBuf::from(home).join(".local/share/mcserv")
}

impl User {
    pub fn new(name: String, username: String) -> Self {
        Self {
            name,
            username,
            profile_picture: String::new(),
            hash: String::new(),
            publickey: String::new(),
            uuid: String::new(),
        }
    }

    pub fn change_password(&mut self, password: String) {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .unwrap()
            .to_string();
        self.hash = password_hash;
    }

    pub fn verify_password(&self, password: &str) -> Result<bool, String> {
        // 1. On parse la chaîne PHC stockée dans self.hash
        let parsed_hash = match PasswordHash::new(&self.hash) {
            Ok(hash) => hash,
            Err(_) => return Ok(false), // Format de hash invalide
        };

        // 2. On vérifie le mot de passe avec l'instance Argon2 par défaut
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }
}

impl Userdb {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let path = base_dir().join("userdb.toml");

        let userdb = if fs::exists(&path)? {
            let content = fs::read_to_string(&path)?;
            toml::from_str(&content)?
        } else {
            Userdb {
                db: Vec::new(),
                nb: 0,
            }
        };

        Ok(userdb)
    }

    fn find_user(&mut self, u: &User) -> Option<&User> {
        let mut iter = self.db.iter();
        iter.find(|&t| t.username == u.username)
    }

    pub fn update_user_fields(
        &mut self,
        old_username: &str,
        new_username: Option<String>,
        new_name: Option<String>,
        new_password: Option<String>,
    ) -> Result<(), String> {
        for e in &mut self.db {
            if e.username == old_username {
                if let Some(name) = new_name {
                    e.name = name;
                }
                if let Some(uname) = new_username {
                    e.username = uname;
                }
                if let Some(pwd) = new_password {
                    e.change_password(pwd);
                }

                return match self.save_to_disk() {
                    Ok(_) => Ok(()),
                    Err(_) => Err("err while saving to disk".to_string()),
                };
            }
        }
        Err("MODIFY : user not found".to_string())
    }
    pub fn save_to_disk(&self) -> std::io::Result<()> {
        let path = base_dir().join("userdb.toml");
        let json_data = toml::to_string_pretty(self).expect("idk");
        fs::write(path, json_data)?;

        Ok(())
    }

    pub fn add(&mut self, u: User) -> Result<bool, String> {
        match self.find_user(&u) {
            None => {
                self.db.push(u);
                self.nb = self.nb + 1;

                let _ = self.save_to_disk();

                return Ok(true);
            }
            Some(_) => return Err("the user already exists !".to_string()),
        }
    }

    pub fn remove(&mut self, uname: String) -> Result<(), String> {
        if let Some(index) = self.db.iter().position(|e| e.username == uname) {
            self.db.remove(index);
            let _ = self.save_to_disk();
            return Ok(());
        } else {
            Err(std::string::String::from(
                "REMOVE : no user correspond to this",
            )) //damn auto fix
        }
    }
}
