use base64::{Engine as _, engine::general_purpose::STANDARD as b64};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use rocket::http::Status;
use rocket::request::{FromRequest, Outcome, Request};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::api_server::auth::Userdb;

pub struct KeyAuthUser {
    pub username: String,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for KeyAuthUser {
    type Error = String;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let headers = req.headers();

        let username = headers.get_one("X-Username");
        let timestamp_str = headers.get_one("X-Timestamp");
        let signature_b64 = headers.get_one("X-Signature");

        match (username, timestamp_str, signature_b64) {
            (Some(uname), Some(ts_str), Some(sig_b64)) => {
                // 1. Vérification du timestamp (anti-rejeu)
                let client_ts: u64 = match ts_str.parse() {
                    Ok(t) => t,
                    Err(_) => {
                        return Outcome::Error((
                            Status::BadRequest,
                            "Timestamp invalide".to_string(),
                        ));
                    }
                };

                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                if now.abs_diff(client_ts) > 10 {
                    // Tolérance de 10 secondes
                    return Outcome::Error((Status::Unauthorized, "Requête expirée".to_string()));
                }

                // 2. Récupération de l'utilisateur
                let db = match Userdb::new() {
                    Ok(db) => db,
                    Err(_) => {
                        return Outcome::Error((
                            Status::InternalServerError,
                            "Erreur base de données".to_string(),
                        ));
                    }
                };
                eprintln!(
                    "DEBUG: {} utilisateur(s) chargé(s), usernames = {:?}",
                    db.db.len(),
                    db.db.iter().map(|u| &u.username).collect::<Vec<_>>()
                );
                let user = match db.db.iter().find(|u| u.username == uname) {
                    Some(u) => u,
                    None => {
                        return Outcome::Error((
                            Status::Unauthorized,
                            "Utilisateur introuvable".to_string(),
                        ));
                    }
                };

                // Si l'utilisateur n'a pas de clé publique configurée
                if user.publickey.is_empty() {
                    return Outcome::Error((
                        Status::Unauthorized,
                        "Aucune clé publique pour cet utilisateur".to_string(),
                    ));
                }

                // 3. Vérification de la signature Ed25519
                if verify_ed25519_signature(&user.publickey, ts_str, sig_b64) {
                    Outcome::Success(KeyAuthUser {
                        username: uname.to_string(),
                    })
                } else {
                    Outcome::Error((Status::Unauthorized, "Signature invalide".to_string()))
                }
            }
            _ => Outcome::Error((
                Status::Unauthorized,
                "Headers d'authentification manquants".to_string(),
            )),
        }
    }
}

/// Vérifie que le `payload` (le timestamp) a bien été signé par la clé privée correspondant à `pub_key_b64`.
fn verify_ed25519_signature(pub_key_b64: &str, payload: &str, signature_b64: &str) -> bool {
    // 1. Décoder la clé publique Base64 (doit faire 32 bytes)
    let pub_key_bytes: [u8; 32] = match b64.decode(pub_key_b64).ok().and_then(|b| b.try_into().ok())
    {
        Some(bytes) => bytes,
        None => return false,
    };

    let verifying_key = match VerifyingKey::from_bytes(&pub_key_bytes) {
        Ok(k) => k,
        Err(_) => return false,
    };

    // 2. Décoder la signature Base64 (doit faire 64 bytes)
    let sig_bytes: [u8; 64] = match b64
        .decode(signature_b64)
        .ok()
        .and_then(|b| b.try_into().ok())
    {
        Some(bytes) => bytes,
        None => return false,
    };

    let signature = Signature::from_bytes(&sig_bytes);

    // 3. Vérifier cryptographiquement
    verifying_key.verify(payload.as_bytes(), &signature).is_ok()
}
