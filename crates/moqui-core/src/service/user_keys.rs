/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! A user's own credentials for git and SSH, in `org.moqui.impl.UserServices`: SSH public keys (`UserSshKey`, which
//! the SSH server matches by SHA-256 fingerprint) and access tokens (a `UserLoginKey` with a description, shown once
//! when it is made, taken as the password of git over HTTPS or as an `api_key`), and app passwords (`UserAppPassword`, one
//! per device, taken by the DAV servers, SFTP to drives and git, not by the web login). No Java counterpart. Each service
//! works on the current user's rows only, so it reads and writes them with authorization off.

use std::collections::BTreeMap;

use base64::Engine;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use sha2::Digest;

use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};

const USER_SSH_KEY: &str = "moqui.security.UserSshKey";
const USER_LOGIN_KEY: &str = "moqui.security.UserLoginKey";
const USER_APP_PASSWORD: &str = "moqui.security.UserAppPassword";

/// The key types the SSH server takes.
const KEY_TYPES: [&str; 7] = [
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "ssh-rsa",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

/// An access token lasts this long when no expiry is given.
const DEFAULT_TOKEN_DAYS: f64 = 90.0;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, create_ssh_key)?;
    rust_service!(map, delete_ssh_key)?;
    rust_service!(map, create_access_token)?;
    rust_service!(map, revoke_access_token)?;
    rust_service!(map, create_app_password)?;
    rust_service!(map, revoke_app_password)?;
    rust_service!(map, get_app_passwords)?;
    Ok(())
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn out(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn current_user() -> Option<String> {
    let user = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty());
    if user.is_none() {
        add_error("log in to manage your keys");
    }
    user
}

/// The type and SHA-256 fingerprint (`SHA256:` and unpadded base64, as `ssh-keygen -l` prints) of an OpenSSH public
/// key line, `ssh-ed25519 AAAA... comment`, and the line without its comment.
pub(crate) fn ssh_key_fingerprint(line: &str) -> Result<(String, String, String), String> {
    let mut parts = line.split_whitespace();
    let (Some(kind), Some(encoded)) = (parts.next(), parts.next()) else {
        return Err("an SSH public key is a line like: ssh-ed25519 AAAA... comment".into());
    };
    if !KEY_TYPES.contains(&kind) {
        return Err(format!(
            "SSH key type {kind} is not taken; use ssh-ed25519 (ssh-keygen -t ed25519)"
        ));
    }
    let blob = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "the SSH public key is not valid base64".to_string())?;
    // the blob starts with its own type, length-prefixed
    let named = blob
        .get(..4)
        .map(|length| u32::from_be_bytes([length[0], length[1], length[2], length[3]]) as usize)
        .and_then(|length| blob.get(4..4 + length))
        .map(|name| String::from_utf8_lossy(name).into_owned());
    if named.as_deref() != Some(kind) {
        return Err("the SSH public key data does not match its type".into());
    }
    let digest = sha2::Sha256::digest(&blob);
    let fingerprint = format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    );
    Ok((kind.to_string(), fingerprint, format!("{kind} {encoded}")))
}

/// `create#UserSshKey`: add an SSH public key for the current user. Out: `sshKeyId`, `fingerprint`.
fn create_ssh_key(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(BTreeMap::new());
    };
    let Some(line) = text(parameters, "publicKey") else {
        add_error("publicKey is required");
        return Ok(BTreeMap::new());
    };
    let (_, fingerprint, _) = match ssh_key_fingerprint(&line) {
        Ok(parsed) => parsed,
        Err(message) => {
            add_error(message);
            return Ok(BTreeMap::new());
        }
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let now = crate::user::now_value();
    let taken = facade
        .find(USER_SSH_KEY)?
        .eq("fingerprint", Value::string(&fingerprint))
        .list()?
        .iter()
        .any(|row| {
            row.get("thruDate").is_none_or(|thru| {
                matches!(thru, Value::Null)
                    || moqui_lib::object::compare_obj(thru, &now).is_ok_and(|order| order.is_gt())
            })
        });
    if taken {
        add_error(format!("SSH key {fingerprint} is already in use"));
        return Ok(BTreeMap::new());
    }
    let mut row = EntityValue::make(facade, USER_SSH_KEY)?;
    row.set_sequenced_id_primary(facade)?;
    let id = row.get("sshKeyId").map(to_plain_string).unwrap_or_default();
    row.set("userId", Value::string(&user_id));
    row.set("publicKey", Value::string(line.trim()));
    row.set("fingerprint", Value::string(&fingerprint));
    row.set("fromDate", now);
    if let Some(description) = text(parameters, "description") {
        row.set("description", Value::string(description));
    }
    row.create(facade)?;
    Ok(out([
        ("sshKeyId", Value::string(id)),
        ("fingerprint", Value::string(fingerprint)),
    ]))
}

/// `delete#UserSshKey`: end one of the current user's SSH keys (its `thruDate` is now).
fn delete_ssh_key(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(BTreeMap::new());
    };
    let Some(id) = text(parameters, "sshKeyId") else {
        add_error("sshKeyId is required");
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let updated = facade
        .find(USER_SSH_KEY)?
        .eq("sshKeyId", Value::string(&id))
        .eq("userId", Value::string(&user_id))
        .update_all([("thruDate".to_string(), crate::user::now_value())])?;
    if updated == 0 {
        add_error(format!("you have no SSH key {id}"));
    }
    Ok(BTreeMap::new())
}

/// `create#UserAccessToken`: a new access token for the current user. Out: `accessToken` (shown only now; only its hash
/// is kept), `loginKey` (that hash, which names the token), `thruDate`.
fn create_access_token(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if current_user().is_none() {
        return Ok(BTreeMap::new());
    }
    let days = text(parameters, "expireDays")
        .and_then(|days| days.parse::<f64>().ok())
        .filter(|days| *days > 0.0)
        .unwrap_or(DEFAULT_TOKEN_DAYS);
    let token = crate::user::get_login_key(Some(days * 24.0))?;
    let hashed = crate::user::hash_login_key(&token)?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let row = facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(&hashed))
        .one()?;
    let thru = row
        .as_ref()
        .and_then(|row| row.get("thruDate").cloned())
        .unwrap_or(Value::Null);
    // a description marks the key as an access token (the user's other login keys are sessions')
    let description = text(parameters, "description").unwrap_or_else(|| "Access token".to_string());
    facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(&hashed))
        .update_all([("description".to_string(), Value::string(description))])?;
    Ok(out([
        ("accessToken", Value::string(token)),
        ("loginKey", Value::string(hashed)),
        ("thruDate", thru),
    ]))
}

/// `revoke#UserAccessToken`: end one of the current user's access tokens, by its `loginKey` (the stored hash).
fn revoke_access_token(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(BTreeMap::new());
    };
    let Some(key) = text(parameters, "loginKey") else {
        add_error("loginKey is required");
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let updated = facade
        .find(USER_LOGIN_KEY)?
        .eq("loginKey", Value::string(&key))
        .eq("userId", Value::string(&user_id))
        .update_all([("thruDate".to_string(), crate::user::now_value())])?;
    if updated == 0 {
        add_error("you have no such access token");
    }
    Ok(BTreeMap::new())
}

/// `create#UserAppPassword`: a new app password for the current user. Out: `appPasswordId`, `password` (shown only now).
fn create_app_password(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(BTreeMap::new());
    };
    let Some(name) = text(parameters, "name") else {
        add_error("name is required: the device or app the password is for");
        return Ok(BTreeMap::new());
    };
    let password = crate::user::new_app_password()?;
    let hashed = crate::user::hash_login_key(&crate::user::app_password_key(&password))?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = EntityValue::make(facade, USER_APP_PASSWORD)?;
    row.set_sequenced_id_primary(facade)?;
    let id = row.get("appPasswordId").map(to_plain_string).unwrap_or_default();
    row.set("userId", Value::string(&user_id));
    row.set("name", Value::string(name.chars().take(255).collect::<String>()));
    row.set("passwordHash", Value::string(hashed));
    row.set("createdDate", crate::user::now_value());
    row.create(facade)?;
    Ok(out([
        ("appPasswordId", Value::string(id)),
        ("password", Value::string(password)),
    ]))
}

/// `revoke#UserAppPassword`: revoke one of the current user's app passwords (its `revokedDate` is now).
fn revoke_app_password(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(BTreeMap::new());
    };
    let Some(id) = text(parameters, "appPasswordId") else {
        add_error("appPasswordId is required");
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let updated = facade
        .find(USER_APP_PASSWORD)?
        .eq("appPasswordId", Value::string(&id))
        .eq("userId", Value::string(&user_id))
        .condition(moqui_db::sql::Condition::is_null("revokedDate"))
        .update_all([("revokedDate".to_string(), crate::user::now_value())])?;
    if updated == 0 {
        add_error("you have no such app password, or it is revoked already");
    }
    Ok(BTreeMap::new())
}

/// `get#UserAppPasswords`: the current user's app passwords, newest first, without their hashes.
fn get_app_passwords(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(out([("appPasswordList", Value::List(Vec::new()))]));
    };
    let include_revoked = parameters
        .get("includeRevoked")
        .is_some_and(|value| to_plain_string(value) == "true");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut find = facade
        .find(USER_APP_PASSWORD)?
        .eq("userId", Value::string(&user_id))
        .order_by("createdDate", true);
    if !include_revoked {
        find = find.condition(moqui_db::sql::Condition::is_null("revokedDate"));
    }
    let list = find
        .list()?
        .iter()
        .map(|row| {
            let mut map = indexmap::IndexMap::new();
            for field in ["appPasswordId", "name", "createdDate", "lastUsedDate", "revokedDate"] {
                map.insert(field.to_string(), row.get(field).cloned().unwrap_or(Value::Null));
            }
            Value::Map(map)
        })
        .collect();
    Ok(out([("appPasswordList", Value::List(list))]))
}

#[cfg(test)]
mod tests {
    use super::ssh_key_fingerprint;

    #[test]
    fn a_key_line_gives_the_fingerprint_ssh_keygen_prints() {
        assert!(ssh_key_fingerprint("ssh-dss AAAA").is_err());
        assert!(ssh_key_fingerprint("not a key").is_err());
        // the blob of an RSA key under the ed25519 type
        assert!(ssh_key_fingerprint("ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQAB").is_err());
        let dir = std::env::temp_dir().join(format!("moqui-ssh-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("id");
        let made = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "test@example", "-f"])
            .arg(&key)
            .status();
        if !made.is_ok_and(|status| status.success()) {
            eprintln!("skipped the comparison: ssh-keygen is not on the PATH");
            return;
        }
        let line = std::fs::read_to_string(dir.join("id.pub")).unwrap();
        let listed = std::process::Command::new("ssh-keygen")
            .arg("-lf")
            .arg(dir.join("id.pub"))
            .output()
            .unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
        let (kind, fingerprint, bare) = ssh_key_fingerprint(&line).unwrap();
        assert_eq!(kind, "ssh-ed25519");
        assert_eq!(listed.split_whitespace().nth(1), Some(fingerprint.as_str()), "{listed}");
        assert!(line.starts_with(&bare) && !bare.contains("test@example"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
