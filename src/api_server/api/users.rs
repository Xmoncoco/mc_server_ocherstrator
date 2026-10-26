use rocket::http::Status;
use rocket::post;
use rocket::serde::json::Json;
use serde::Deserialize;

use crate::api_server::auth::auth_guard::KeyAuthUser;
use crate::api_server::auth::{User, Userdb};

// ---------------------------------------------------------------------------
// Utilisateurs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct NewUser {
    name: String,
    username: String,
    password: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct ChangeUser {
    old_username: String,
    username: Option<String>,
    name: Option<String>,
    password: Option<String>,
}

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct RemoveUser {
    username: String,
}

#[post("/user/create", format = "json", data = "<user_data>")]
pub(super) fn user_profile(user_data: Json<NewUser>) -> Result<String, (Status, String)> {
    let user_info = user_data.into_inner();
    let mut user = User::new(user_info.name, user_info.username);
    if let Some(pwd) = user_info.password {
        user.change_password(pwd);
    }
    let mut db = Userdb::new().map_err(|_| {
        (
            Status::InternalServerError,
            "Erreur lors de l'ouverture de la db".to_string(),
        )
    })?;
    match db.add(user) {
        Ok(_) => Ok("ok".to_string()),
        Err(e) => Err((Status::BadRequest, e)),
    }
}

#[post("/user/remove", format = "json", data = "<req>")]
pub(super) fn user_remove(
    auth: KeyAuthUser,
    req: Json<RemoveUser>,
) -> Result<String, (Status, String)> {
    let username_to_remove = req.into_inner().username;

    if auth.username != username_to_remove {
        return Err((
            Status::Forbidden,
            "Tu ne peux supprimer que ton propre compte".to_string(),
        ));
    }

    let mut db =
        Userdb::new().map_err(|_| (Status::InternalServerError, "Erreur db".to_string()))?;

    match db.remove(username_to_remove) {
        Ok(_) => Ok("ok".to_string()),
        Err(e) => Err((Status::NotFound, e)),
    }
}

#[post("/user/modify", format = "json", data = "<u>")]
pub(super) fn user_modify(
    auth: KeyAuthUser,
    u: Json<ChangeUser>,
) -> Result<String, (Status, String)> {
    let change = u.into_inner();

    if auth.username != change.old_username {
        return Err((
            Status::Forbidden,
            "Tu ne peux modifier que ton propre compte".to_string(),
        ));
    }

    let mut db =
        Userdb::new().map_err(|_| (Status::InternalServerError, "Erreur db".to_string()))?;

    match db.update_user_fields(
        &change.old_username,
        change.username,
        change.name,
        change.password,
    ) {
        Ok(_) => Ok("ok".to_string()),
        Err(e) => Err((Status::BadRequest, e)),
    }
}
