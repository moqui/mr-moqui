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
//! when it is made, taken as the password of git over HTTPS or as an `api_key`), app passwords (`UserAppPassword`, one
//! per device, taken by the DAV servers, SFTP to drives and git, not by the web login), and the keys of the desktop and
//! mobile clients (a `UserLoginKey` with a `deviceName`). No Java counterpart. Each service
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
    rust_service!(map, find_ssh_keys)?;
    rust_service!(map, find_access_tokens)?;
    rust_service!(map, create_app_password)?;
    rust_service!(map, revoke_app_password)?;
    rust_service!(map, get_app_passwords)?;
    rust_service!(map, get_device_keys)?;
    rust_service!(map, revoke_device_key)?;
    rust_service!(map, renew_device_key)?;
    rust_service!(map, get_scram_credentials)?;
    rust_service!(map, check_plain_password)?;
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
        .first_chunk::<4>()
        .map(|length| u32::from_be_bytes(*length) as usize)
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
        ("tokenId", Value::string(access_token_id(&hashed))),
        ("loginKey", Value::string(hashed)),
        ("thruDate", thru),
    ]))
}

/// Whether a row with `thru_date` is still in date at `now` (milliseconds).
fn in_date(row: &EntityValue, now: i64) -> bool {
    row.get("thruDate")
        .and_then(crate::user::value_millis)
        .is_none_or(|thru| thru > now)
}

/// A row's fields as a Map, `Null` for the ones it lacks.
fn row_map<S: std::hash::BuildHasher + Default>(
    row: &EntityValue,
    fields: &[&str],
) -> indexmap::IndexMap<String, Value, S> {
    fields
        .iter()
        .map(|field| ((*field).to_string(), row.get(field).cloned().unwrap_or(Value::Null)))
        .collect()
}

/// `find#UserSshKeys`: the current user's SSH keys, newest first, each `sshKeyId`, `description`, `keyType`,
/// `fingerprint`, `fromDate` and `thruDate`; ended ones only with `includeEnded`. When a key was last used is not
/// kept.
fn find_ssh_keys(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(out([("sshKeyList", Value::List(Vec::new()))]));
    };
    let include_ended = text(parameters, "includeEnded").is_some_and(|value| value == "true");
    let now = crate::user::now_millis();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find(USER_SSH_KEY)?
        .eq("userId", Value::string(&user_id))
        .order_by("fromDate", true)
        .list()?;
    let list = rows
        .iter()
        .filter(|row| include_ended || in_date(row, now))
        .map(|row| {
            let mut map = row_map(row, &["sshKeyId", "description", "fingerprint", "fromDate", "thruDate"]);
            let key_type = row
                .get("publicKey")
                .map(to_plain_string)
                .and_then(|line| line.split_whitespace().next().map(str::to_string));
            map.insert("keyType".to_string(), key_type.map_or(Value::Null, Value::string));
            Value::Map(map)
        })
        .collect();
    Ok(out([("sshKeyList", Value::List(list))]))
}

/// What names an access token to its user in lists and for revoke: a digest of the stored hash, so a list never holds
/// a value a login is looked up by.
fn access_token_id(login_key: &str) -> String {
    let digest = sha2::Sha256::digest(format!("moqui-access-token-id:{login_key}").as_bytes());
    digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect()
}

/// The user's access tokens: the `UserLoginKey` rows with a description and no device (a device's key is
/// `get#UserDeviceKeys`', a session's has no description).
fn token_rows(facade: &EntityFacade, user_id: &str) -> Result<Vec<EntityValue>, LibError> {
    let rows = facade
        .find(USER_LOGIN_KEY)?
        .eq("userId", Value::string(user_id))
        .condition(moqui_db::sql::Condition::is_not_null("description"))
        .condition(moqui_db::sql::Condition::is_null("deviceName"))
        .order_by("fromDate", true)
        .list()?;
    Ok(rows.iter().cloned().collect())
}

/// `find#UserAccessTokens`: the current user's access tokens, newest first, each `tokenId` (for
/// `revoke#UserAccessToken`), `description`, `fromDate` and `thruDate`; ended ones only with `includeEnded`. Never
/// the token, which is shown once when it is made, or its stored hash.
fn find_access_tokens(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(out([("accessTokenList", Value::List(Vec::new()))]));
    };
    let include_ended = text(parameters, "includeEnded").is_some_and(|value| value == "true");
    let now = crate::user::now_millis();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let list = token_rows(facade, &user_id)?
        .iter()
        .filter(|row| include_ended || in_date(row, now))
        .map(|row| {
            let mut map = row_map(row, &["description", "fromDate", "thruDate"]);
            let key = row.get("loginKey").map(to_plain_string).unwrap_or_default();
            map.insert("tokenId".to_string(), Value::string(access_token_id(&key)));
            Value::Map(map)
        })
        .collect();
    Ok(out([("accessTokenList", Value::List(list))]))
}

/// `revoke#UserAccessToken`: end one of the current user's access tokens, by its `tokenId` (as `find#UserAccessTokens`
/// gives it) or its `loginKey` (the stored hash, as `create#UserAccessToken` gives it).
fn revoke_access_token(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(given) = text(parameters, "tokenId").or_else(|| text(parameters, "loginKey")) else {
        add_error("tokenId or loginKey is required");
        return Ok(BTreeMap::new());
    };
    // the id find#UserAccessTokens gives is looked for among the user's own tokens; any other value is a stored hash
    let key = token_rows(facade, &user_id)?
        .iter()
        .filter_map(|row| row.get("loginKey").map(to_plain_string))
        .find(|key| access_token_id(key) == given)
        .unwrap_or(given);
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
    // its XMPP sign-in (SCRAM, plans/archive/Comms-Framework.md §4.3): the only place the password is in the clear
    scram_fields(facade, &user_id, &password, &mut row)?;
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

/// The iteration count of new SCRAM verifiers.
const SCRAM_ITERATIONS: u32 = 10_000;

/// An app password's SCRAM-SHA-256 verifiers, for the XMPP server: one for the password as shown (with its dashes, in
/// lower case) and one for its key (without them), since SCRAM hashes what the client typed. A user's app passwords
/// share one salt and iteration count, so the server's one challenge fits each of them (moqui-xmpp `scram.rs`).
fn scram_fields(facade: &EntityFacade, user_id: &str, password: &str, row: &mut EntityValue) -> Result<(), LibError> {
    let engine = base64::engine::general_purpose::STANDARD;
    let shared = facade
        .find(USER_APP_PASSWORD)?
        .eq("userId", Value::string(user_id))
        .condition(moqui_db::sql::Condition::is_null("revokedDate"))
        .list()?
        .iter()
        .find_map(|existing| {
            let salt = existing
                .get("scramSalt")
                .map(to_plain_string)
                .filter(|salt| !salt.is_empty())?;
            let iterations = existing
                .get("scramIterations")
                .map(to_plain_string)
                .and_then(|count| count.parse::<u32>().ok())?;
            Some((engine.decode(salt).ok()?, iterations))
        });
    let (salt, iterations) = match shared {
        Some(found) => found,
        None => {
            let mut salt = vec![0_u8; 16];
            getrandom::fill(&mut salt).map_err(|err| LibError::new(format!("random source failed: {err}")))?;
            (salt, SCRAM_ITERATIONS)
        }
    };
    let mut stored = Vec::new();
    let mut server = Vec::new();
    for form in [password.to_lowercase(), crate::user::app_password_key(password)] {
        let (stored_key, server_key) = crate::user::scram_sha256_verifier(&form, &salt, iterations);
        stored.push(engine.encode(stored_key));
        server.push(engine.encode(server_key));
    }
    row.set("scramSalt", Value::string(engine.encode(&salt)));
    row.set("scramIterations", Value::I64(i64::from(iterations)));
    row.set("scramStoredKeys", Value::string(stored.join(" ")));
    row.set("scramServerKeys", Value::string(server.join(" ")));
    Ok(())
}

/// `org.moqui.chat.XmppServices.get#ScramCredentials` (internal): the salt, iteration count and verifiers of a user's
/// active app passwords that have them, for SCRAM sign-in; nothing for an unknown user, a disabled one, or one with
/// no such password.
fn get_scram_credentials(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(username) = text(parameters, "username") else {
        return Ok(BTreeMap::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(account) = facade
        .find("moqui.security.UserAccount")?
        .eq("username", Value::string(&username))
        .one()?
    else {
        return Ok(BTreeMap::new());
    };
    if account.get("disabled").map(to_plain_string).as_deref() == Some("Y") {
        return Ok(BTreeMap::new());
    }
    let user_id = account.get("userId").map(to_plain_string).unwrap_or_default();
    let rows = facade
        .find(USER_APP_PASSWORD)?
        .eq("userId", Value::string(&user_id))
        .condition(moqui_db::sql::Condition::is_null("revokedDate"))
        .order_by("createdDate", false)
        .list()?;
    let mut salt = String::new();
    let mut iterations = String::new();
    let mut stored = Vec::new();
    let mut server = Vec::new();
    let mut ids = Vec::new();
    for row in rows.iter() {
        let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
        if field("scramSalt").is_empty() {
            continue;
        }
        // the first salt is the user's; a row made with another (none should be) is left out
        if salt.is_empty() {
            salt = field("scramSalt");
            iterations = field("scramIterations");
        } else if field("scramSalt") != salt {
            continue;
        }
        let (keys, servers) = (field("scramStoredKeys"), field("scramServerKeys"));
        for (stored_key, server_key) in keys.split_whitespace().zip(servers.split_whitespace()) {
            stored.push(Value::string(stored_key));
            server.push(Value::string(server_key));
            ids.push(Value::string(field("appPasswordId")));
        }
    }
    if stored.is_empty() {
        return Ok(BTreeMap::new());
    }
    Ok(out([
        ("userId", Value::string(user_id)),
        ("salt", Value::string(salt)),
        ("iterations", Value::string(iterations)),
        ("storedKeyList", Value::List(stored)),
        ("serverKeyList", Value::List(server)),
        ("appPasswordIdList", Value::List(ids)),
    ]))
}

/// `org.moqui.chat.XmppServices.check#PlainPassword` (internal): PLAIN sign-in, which takes an app password only
/// (`login_app_password`: a refusal never counts toward the account's lockout); the account's userId and username.
fn check_plain_password(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let (Some(username), Some(password)) = (text(parameters, "username"), text(parameters, "password")) else {
        add_error("not an app password");
        return Ok(BTreeMap::new());
    };
    if !crate::user::looks_like_app_password(&password)
        || crate::user::login_app_password(&username, &password).is_err()
    {
        add_error("not an app password");
        return Ok(BTreeMap::new());
    }
    let user_id = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    let username = crate::user::current_username().ok().flatten().unwrap_or(username);
    // which app password it was, so the session and its FAST tokens go with it when it is revoked
    let hashed = crate::user::hash_login_key(&crate::user::app_password_key(&password))?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let app_password_id = facade
        .find(USER_APP_PASSWORD)?
        .eq("passwordHash", Value::string(hashed))
        .one()?
        .and_then(|row| row.get("appPasswordId").map(to_plain_string))
        .unwrap_or_default();
    Ok(out([
        ("userId", Value::string(user_id)),
        ("username", Value::string(username)),
        ("appPasswordId", Value::string(app_password_id)),
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
            let mut map = indexmap::IndexMap::default();
            for field in ["appPasswordId", "name", "createdDate", "lastUsedDate", "revokedDate"] {
                map.insert(field.to_string(), row.get(field).cloned().unwrap_or(Value::Null));
            }
            Value::Map(map)
        })
        .collect();
    Ok(out([("appPasswordList", Value::List(list))]))
}

/// `get#UserDeviceKeys`: the current user's devices signed in by key (a `UserLoginKey` with a `deviceName`: the
/// desktop and mobile clients), unexpired, most recently used first. `loginKey` is the stored hash, which names the
/// device for revoke; `current` marks the device of this request's `login_key` header.
fn get_device_keys(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(user_id) = current_user() else {
        return Ok(out([("deviceList", Value::List(Vec::new()))]));
    };
    let current = crate::web::current_request()
        .and_then(|request| request.header("login_key").map(str::to_string))
        .filter(|key| !key.is_empty())
        .and_then(|key| crate::user::hash_login_key(&key).ok());
    let now = crate::user::now_millis();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut rows: Vec<(i64, Value)> = facade
        .find(USER_LOGIN_KEY)?
        .eq("userId", Value::string(&user_id))
        .condition(moqui_db::sql::Condition::is_not_null("deviceName"))
        .list()?
        .iter()
        .filter(|row| {
            row.get("thruDate")
                .and_then(crate::user::value_millis)
                .is_none_or(|thru| thru > now)
        })
        .map(|row| {
            let mut map = indexmap::IndexMap::default();
            for field in ["loginKey", "deviceName", "fromDate", "lastUsedDate", "thruDate"] {
                map.insert(field.to_string(), row.get(field).cloned().unwrap_or(Value::Null));
            }
            let key = row.get("loginKey").map(to_plain_string);
            map.insert("current".to_string(), Value::Bool(key.is_some() && key == current));
            let used = ["lastUsedDate", "fromDate"]
                .iter()
                .find_map(|field| row.get(field).and_then(crate::user::value_millis))
                .unwrap_or(0);
            (used, Value::Map(map))
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    Ok(out([(
        "deviceList",
        Value::List(rows.into_iter().map(|(_, row)| row).collect()),
    )]))
}

/// `revoke#UserDeviceKey`: sign one of the current user's devices out, by its `loginKey` (the stored hash). Its next
/// request is refused, and its notification socket closes within a minute.
fn revoke_device_key(
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
        .condition(moqui_db::sql::Condition::is_not_null("deviceName"))
        .update_all([("thruDate".to_string(), crate::user::now_value())])?;
    if updated == 0 {
        add_error("you have no such device");
    } else {
        // the device gets no more pushes (plans/Mobile-Desktop.md §5, native push)
        super::web_push::forget_device_key(facade, &key);
    }
    Ok(BTreeMap::new())
}

/// `renew#UserDeviceKey`: a new key for the device of this request's `login_key` header, with the same name and a full
/// lifetime, given once as `loginKey` with its `thruDate`; the header's key ends a few minutes later
/// (`user::renew_device_key`). The desktop client renews before its key runs out.
fn renew_device_key(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let key = crate::web::current_request()
        .and_then(|request| request.header("login_key").map(str::to_string))
        .unwrap_or_default();
    match crate::user::renew_device_key(&key)? {
        Some((renewed, thru)) => Ok(out([
            ("loginKey", Value::string(renewed)),
            ("thruDate", crate::user::stamp_value(thru)),
        ])),
        None => {
            add_error("this request was not signed in by a device key");
            Ok(BTreeMap::new())
        }
    }
}

#[cfg(test)]
#[path = "user_keys_tests.rs"]
mod tests;
