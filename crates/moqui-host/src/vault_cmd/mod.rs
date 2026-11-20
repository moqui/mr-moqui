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

//! `moqui vault` (plans/Vault-Instances.md §3.5, §3.6; docs/security/Vaults.md): a vault client on the user's own
//! machine, against a server's vault services over REST. The crypto is the vault crate's own session ([`client`]),
//! the one the browser's worker runs, so the key passphrase, the personal key and every vault key stay in this
//! process. Each run signs in with the password and second factor ([`remote`]), keeps nothing of the sign-in, and
//! remembers each vault's highest accepted key version and member list ([`state`], decision (d)).
//!
//! - `key init`: make the personal key here; prints its recovery code and fingerprint.
//! - `key show`: this user's key fingerprint.
//! - `key check <username>`: compare a user's fingerprint, then store the check signed by this user's key.
//! - `list`: this user's vaults.
//! - `open <vaultId> [--item <id>]`: open a vault through its member-list chain and list its items, or print one.
//! - `recovery ...`: an organization recovery holder's steps, and turning recovery on ([`recovery`]).
//! - `export-open FILE [--item <id>]`: open an encrypted export file of Commons › Vault here, with its password.
//! - `leases [--vault <id>]`: the leases waiting for this user's approval and those they asked for, or a vault's.
//! - `approve <leaseId>`: show what the signed lease names, check the server's lease key once, and approve it here
//!   (§3.7): the item keys unwrapped under the lease's own names and sealed to it, signed by this user's key.

pub(crate) mod client;
mod recovery;
mod remote;
mod state;

use std::io::Write as _;

use moqui_lib::error::Error;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use client::VaultClient;
use remote::{Login, Remote};
use state::State;

const USAGE: &str = "\
Usage: moqui vault --server URL --user USERNAME <command>

  key init                  Make your personal vault key here: a key passphrase (twice), then its recovery code
  key show                  Your personal key's fingerprint
  key check <username>      Compare a user's key fingerprint with them, then store your signed check
  list                      Your vaults
  open <vaultId> [--item ID]  Open a vault through its member list and list its items, or print one item
  recovery list             The organization's recovery keys, and the recoveries you can seal a share to
  recovery make --k K [--group G] <username>...
                            Make a recovery key split K of these holders (each key checked)
  recovery on <vaultId> [--key ID]
                            As a manager: turn recovery on for a vault (the recovery key's maker checked)
  recovery start <vaultId> <username> [--key ID]
                            As a holder: recover the vault to the member's new key (checked); waits for the shares
  recovery share <recoverySessionId>
                            As another holder: seal your share to a recovery
  recovery replace <vaultId> --key ID
                            As a manager: list a new recovery key in place of a used one (rotates the vault key)
  export-open FILE [--item ID]
                            Open an encrypted export file (Commons › Vault) here, with its password: no server
  leases [--vault ID]       Leases waiting for your approval and those you asked for, or one vault's leases
  approve <leaseId>         Show what a lease names and approve it here, with your personal key

Each run signs in with your password, and your second factor's code when your account has one: the server sends your
wrapped personal key only after such a sign-in. The key passphrase never leaves this process. What this client has
seen of each vault's member list is kept in ~/.config/moqui/vault/state.json, and a server that hands back an older
one is refused.";

/// `moqui vault ...`.
pub(crate) fn dispatch(args: &[String]) -> Result<(), Error> {
    let mut server = String::new();
    let mut user = String::new();
    let mut item = String::new();
    let mut vault = String::new();
    let mut words = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .cloned()
                .ok_or_else(|| Error::new(format!("{name} needs a value")))
        };
        match arg.as_str() {
            "--server" => server = value("--server")?,
            "--user" => user = value("--user")?,
            "--item" => item = value("--item")?,
            "--vault" => vault = value("--vault")?,
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => words.push(other.to_string()),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    // an export file opens here, with its password: no server, no sign-in
    if let ["export-open", file] = words.as_slice() {
        return export_open(file, &item);
    }
    if server.is_empty() || user.is_empty() || words.is_empty() {
        return Err(Error::new(USAGE));
    }
    let mut remote = Remote::new(&server)?;
    sign_in(&mut remote, &user)?;
    let mut client = VaultClient::default();
    match words.as_slice() {
        ["key", "init"] => key_init(&mut remote, &mut client),
        ["key", "show"] => {
            let key = remote.get("/personalKey", &[])?;
            if key["hasKey"] != json!(true) {
                return Err(Error::new("You have no personal vault key yet: moqui vault key init"));
            }
            println!("{}  {}", text(&key, "personalKeyId"), text(&key, "fingerprint"));
            Ok(())
        }
        ["key", "check", username] => key_check(&mut remote, &mut client, username),
        ["list"] => list(&mut remote),
        ["open", vault_id] => open(&mut remote, &mut client, vault_id, &item),
        ["recovery", rest @ ..] => recovery::dispatch(&mut remote, &mut client, &user, rest),
        ["leases"] => leases(&mut remote, &vault),
        ["approve", lease_id] => approve(&mut remote, &mut client, lease_id),
        _ => Err(Error::new(USAGE)),
    }
}

/// A server's text for the terminal: a control character (a newline, an escape that moves the cursor or recolors) is
/// shown escaped, so a name cannot forge a line of the approval prompt.
fn shown(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// A field of a server's answer as text for the terminal ([`shown`]).
fn shown_field(value: &Value, name: &str) -> String {
    shown(&text(value, name))
}

fn text(value: &Value, name: &str) -> String {
    match &value[name] {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn vault_err(err: String) -> Error {
    Error::new(err)
}

/// Asks for a line on the terminal; a secret is read with echo off when the input is a terminal. The answer is
/// cleared from memory when dropped.
fn ask(label: &str, secret: bool) -> Result<Zeroizing<String>, Error> {
    eprint!("{label}: ");
    let _ = std::io::stderr().flush();
    #[cfg(unix)]
    let restore = if secret { echo_off() } else { None };
    let mut line = Zeroizing::new(String::with_capacity(256));
    let read = std::io::stdin().read_line(&mut line);
    #[cfg(unix)]
    if let Some(saved) = restore {
        use std::os::fd::AsFd as _;
        let _ = rustix::termios::tcsetattr(std::io::stdin().as_fd(), rustix::termios::OptionalActions::Now, &saved);
        eprintln!();
    }
    #[cfg(not(unix))]
    let _ = secret;
    read.map_err(|err| Error::new(format!("{label}: {err}")))?;
    let trimmed = line.trim_end_matches(['\r', '\n']).len();
    line.truncate(trimmed);
    Ok(line)
}

/// Turns the terminal's echo off; the settings to put back, or none when the input is not a terminal.
#[cfg(unix)]
fn echo_off() -> Option<rustix::termios::Termios> {
    use std::os::fd::AsFd as _;
    let stdin = std::io::stdin();
    if !rustix::termios::isatty(stdin.as_fd()) {
        return None;
    }
    let saved = rustix::termios::tcgetattr(stdin.as_fd()).ok()?;
    let mut quiet = saved.clone();
    quiet.local_modes.remove(rustix::termios::LocalModes::ECHO);
    rustix::termios::tcsetattr(stdin.as_fd(), rustix::termios::OptionalActions::Now, &quiet).ok()?;
    Some(saved)
}

/// The REST login with the password, then the second factor's code when the account has one.
fn sign_in(remote: &mut Remote, user: &str) -> Result<(), Error> {
    let password = ask(&format!("Password for {user} at {}", remote.base()), true)?;
    let mut answer = remote.login(user, &password, "")?;
    for _ in 0..3 {
        let Login::CodeNeeded(prompt) = answer else {
            return Ok(());
        };
        let code = ask(&prompt, false)?;
        answer = remote.login(user, &password, &code)?;
    }
    match answer {
        Login::SignedIn => Ok(()),
        Login::CodeNeeded(_) => Err(Error::new("Sign-in refused: the code was not valid")),
    }
}

/// A path segment of a vault REST path: letters, digits, `-`, `_` and `.` as they are, any other byte %-encoded.
fn segment(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

fn new_id(prefix: &str) -> String {
    let mut bytes = [0_u8; 12];
    moqui_vault::rng::Rng::fill(&mut moqui_vault::rng::OsRng, &mut bytes);
    format!("{prefix}-{}", moqui_vault::text::hex(&bytes))
}

/// `key init`: the personal key made here, its public halves and wraps set up on the server.
fn key_init(remote: &mut Remote, client: &mut VaultClient) -> Result<(), Error> {
    let key = remote.get("/personalKey", &[])?;
    if key["hasKey"] == json!(true) {
        return Err(Error::new("You have a personal vault key already"));
    }
    let passphrase = ask("New key passphrase (at least 12 characters, not your password)", true)?;
    if passphrase.chars().count() < 12 {
        return Err(Error::new("A key passphrase is at least 12 characters"));
    }
    if *ask("The key passphrase again", true)? != *passphrase {
        return Err(Error::new("The two passphrases differ"));
    }
    let key_id = new_id("pk");
    let made = client
        .call(
            "personal_make",
            json!({ "personal_key_id": key_id, "user_id": text(&key, "userId"), "passphrase": passphrase.as_str() }),
        )
        .map_err(vault_err)?;
    remote.post(
        "/personalKey",
        &json!({ "personalKeyId": key_id, "public": made["public"], "passphraseWrap": made["passphrase_wrap"],
                 "recoveryWrap": made["recovery_wrap"] }),
    )?;
    println!("Your personal vault key is set up.");
    println!("Fingerprint:   {}", text(&made, "fingerprint"));
    println!("Recovery code: {}", text(&made, "recovery_code"));
    println!("Print or save the recovery code outside this server: with it, a lost passphrase is not a lost key.");
    Ok(())
}

/// Opens this user's personal key in `client`: the wrap, fetched after this run's strong sign-in, opened with the key
/// passphrase; a wrap under the Argon2id target is wrapped again and stored.
fn open_key(remote: &mut Remote, client: &mut VaultClient) -> Result<(), Error> {
    let wrapped = remote.get("/personalKey/wrapped", &[])?;
    let passphrase = ask("Key passphrase", true)?;
    let opened = client
        .call(
            "personal_open",
            json!({ "personal_key_id": wrapped["personalKeyId"], "user_id": wrapped["userId"],
                    "public": wrapped["public"], "passphrase_wrap": wrapped["passphraseWrap"], "passphrase": passphrase.as_str() }),
        )
        .map_err(vault_err)?;
    if opened["rewrapped"].is_object() {
        remote.post(
            "/personalKey/rewrap",
            &json!({ "personalKeyId": wrapped["personalKeyId"], "passphraseWrap": opened["rewrapped"] }),
        )?;
    }
    Ok(())
}

/// `key check <username>`: the user's fingerprint shown to compare with them by another channel, and on a yes the
/// check signed by this user's key and stored.
fn key_check(remote: &mut Remote, client: &mut VaultClient, username: &str) -> Result<(), Error> {
    let key = remote.get("/publicKey", &[("username", username)])?;
    if key["hasKey"] != json!(true) {
        return Err(Error::new(format!("{username} has no personal vault key")));
    }
    println!("{username}'s key fingerprint:\n  {}", text(&key, "fingerprint"));
    let same = ask(&format!("Does {username} read you the same words? [y/N]"), false)?;
    if !matches!(same.trim(), "y" | "Y" | "yes") {
        return Err(Error::new("Not checked"));
    }
    open_key(remote, client)?;
    let check = client
        .call(
            "key_check_make",
            json!({ "checked_key_id": key["personalKeyId"], "checked_user_id": key["userId"], "checked_public": key["public"],
                    "fingerprint": key["fingerprint"], "checked_at": now_millis() }),
        )
        .map_err(vault_err)?;
    remote.post("/keyChecks", &json!({ "check": check }))?;
    println!("Checked: {username}'s key {}", text(&key, "personalKeyId"));
    Ok(())
}

/// `list`: this user's vaults.
fn list(remote: &mut Remote) -> Result<(), Error> {
    let vaults = remote.get("/vaults", &[])?;
    for vault in vaults["vaultList"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let name = match text(vault, "vaultName") {
            name if name.is_empty() => "(name sealed)".to_string(),
            name => name,
        };
        println!(
            "{}  {}  {}  {}  {} items  {name}",
            text(vault, "vaultId"),
            text(vault, "vaultKindEnumId"),
            text(vault, "lockEnumId"),
            text(vault, "roleEnumId"),
            text(vault, "itemCount"),
        );
    }
    Ok(())
}

/// A shared-key vault's key of its current version, opened in `client` through the vault's chain (checked against
/// this user's checks and what was remembered); the personal key is open already.
fn open_shared(remote: &mut Remote, client: &mut VaultClient, vault_id: &str, vault: &Value) -> Result<(), Error> {
    let key_version = vault["vault"]["keyVersion"].as_u64().unwrap_or(0);
    let checks = remote.get("/keyChecks", &[])?;
    let path = state::state_file()?;
    let mut state = State::read(&path);
    let wrap = vault["wraps"]
        .as_array()
        .and_then(|wraps| {
            wraps
                .iter()
                .find(|wrap| wrap["key_version"].as_u64() == Some(key_version))
        })
        .cloned()
        .ok_or_else(|| Error::new("You hold no wrap of this vault's current key version"))?;
    let opened = client
        .call(
            "vault_key_open",
            json!({ "entries": vault["entries"], "key_checks": checks["keyCheckList"],
                    "remembered": state.remembered(remote.base(), vault_id), "wrap": wrap }),
        )
        .map_err(vault_err)?;
    state.remember(remote.base(), vault_id, &opened["remembered"]);
    state.write(&path)?;
    if vault["wraps"].as_array().is_some_and(|wraps| wraps.len() > 1) {
        remote.post(
            &format!("/vaults/{}/ack", segment(vault_id)),
            &json!({ "keyVersion": key_version }),
        )?;
    }
    Ok(())
}

/// `leases`: the leases waiting for this user's approval and those they asked for, or one vault's.
fn leases(remote: &mut Remote, vault_id: &str) -> Result<(), Error> {
    let query: Vec<(&str, &str)> = if vault_id.is_empty() {
        Vec::new()
    } else {
        vec![("vaultId", vault_id)]
    };
    let leases = remote.get("/leases", &query)?;
    for lease in leases["leaseList"].as_array().map(Vec::as_slice).unwrap_or_default() {
        println!(
            "{}  {}  {}  {}  {}",
            shown_field(lease, "leaseId"),
            shown_field(lease, "statusId"),
            shown_field(lease, "vaultId"),
            shown_field(lease, "operation"),
            shown_field(lease, "bindings"),
        );
    }
    Ok(())
}

/// The time limit of a lease, as a count of minutes from now.
fn minutes_left(expires_at: i64) -> i64 {
    (expires_at - now_millis()) / 60_000
}

/// `approve <leaseId>`: what the signed lease names (never a column), the lease key checked once by its fingerprint,
/// then on a yes the approval made here and stored. The server needs the approve role and this run's sign-in.
fn approve(remote: &mut Remote, client: &mut VaultClient, lease_id: &str) -> Result<(), Error> {
    let got = remote.get(&format!("/leases/{}", segment(lease_id)), &[])?;
    if text(&got, "statusId") != "VlsRequested" {
        return Err(Error::new(format!("The lease {lease_id} is not waiting for approval")));
    }
    let lease = &got["lease"];
    let vault_id = text(lease, "vault_id");
    println!(
        "Lease {} of vault {}, for: {}",
        shown(lease_id),
        shown(&vault_id),
        shown_field(lease, "operation")
    );
    for item in lease["items"].as_array().map(Vec::as_slice).unwrap_or_default() {
        println!(
            "  {}  ({}, {})",
            shown_field(item, "name"),
            shown_field(item, "binding"),
            shown_field(item, "item_type")
        );
    }
    let expires_at = lease["expires_at"].as_i64().unwrap_or(0);
    println!(
        "Runs on {} for at most {} minutes more.",
        shown_field(lease, "cluster_member"),
        minutes_left(expires_at)
    );
    println!("These names are checked: an item that is not the one named does not unwrap.");
    open_key(remote, client)?;
    let lease_key = text(&got, "leaseKey");
    let checks = remote.get("/keyChecks", &[("checkedKeyId", "lease-key")])?;
    let checked = checks["keyCheckList"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| text(row, "checked_verify") == lease_key));
    if !checked {
        println!(
            "You have not checked this server's lease key. Its fingerprint:
  {}",
            text(&got, "leaseKeyFingerprint")
        );
        let same = ask("Does the server's operator read you the same words? [y/N]", false)?;
        if !matches!(same.trim(), "y" | "Y" | "yes") {
            return Err(Error::new("Not approved: the lease key is not checked"));
        }
        let check = client
            .call(
                "key_check_make",
                json!({ "lease_key": lease_key, "fingerprint": got["leaseKeyFingerprint"], "checked_at": now_millis() }),
            )
            .map_err(vault_err)?;
        remote.post("/keyChecks", &json!({ "check": check }))?;
    }
    let vault = remote.get(&format!("/vaults/{}", segment(&vault_id)), &[])?;
    open_shared(remote, client, &vault_id, &vault)?;
    let yes = ask("Approve this lease? [y/N]", false)?;
    if !matches!(yes.trim(), "y" | "Y" | "yes") {
        return Err(Error::new("Not approved"));
    }
    let checks = remote.get("/keyChecks", &[])?;
    let approval = client
        .call(
            "lease_approve",
            json!({ "lease": lease, "lease_key": lease_key, "key_checks": checks["keyCheckList"],
                    "wrapped_item_keys": got["wrappedItemKeys"], "now": now_millis() }),
        )
        .map_err(vault_err)?;
    remote.post(
        &format!("/leases/{}/approve", segment(lease_id)),
        &json!({ "approval": approval }),
    )?;
    println!("Approved: lease {lease_id}");
    Ok(())
}

/// `open <vaultId>`: the vault key opened through the vault's chain (checked against this user's checks and what was
/// remembered), each item's name and type, or one item's body.
fn open(remote: &mut Remote, client: &mut VaultClient, vault_id: &str, item_id: &str) -> Result<(), Error> {
    let vault = remote.get(&format!("/vaults/{}", segment(vault_id)), &[])?;
    let key_version = vault["vault"]["keyVersion"].as_u64().unwrap_or(0);
    if text(&vault["vault"], "lockEnumId") == "VlSingleSecret" {
        if text(&vault["vault"], "secretKindEnumId") == "VsGenerated" {
            let generated = ask("Generated key", true)?;
            client
                .call(
                    "vault_key_generated_open",
                    json!({ "vault_id": vault_id, "key_version": key_version, "wrap": vault["secretWrap"],
                            "generated": generated.trim() }),
                )
                .map_err(vault_err)?;
        } else {
            let password = ask("Vault password", true)?;
            client
                .call(
                    "vault_key_password_open",
                    json!({ "vault_id": vault_id, "key_version": key_version, "wrap": vault["secretWrap"], "password": password.as_str() }),
                )
                .map_err(vault_err)?;
        }
    } else {
        open_key(remote, client)?;
        open_shared(remote, client, vault_id, &vault)?;
    }
    let items = remote.get(&format!("/vaults/{}/items", segment(vault_id)), &[])?;
    for item in items["itemList"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let id = text(item, "vaultItemId");
        if !item_id.is_empty() && id != item_id {
            continue;
        }
        let label = match text(item, "itemName") {
            name if name.is_empty() => Value::Null,
            name => {
                json!({ "item_type": text(item, "vaultItemTypeId"), "binding": text(item, "bindingKey"), "name": name })
            }
        };
        let opened = client.call(
            "item_open",
            json!({ "vault_id": vault_id, "item_id": id, "key_version": item["keyVersion"], "label": label,
                    "wrapped_item_key": item["wrappedItemKey"], "sealed_body": item["bodyCipher"] }),
        );
        let body: Value = match opened {
            Ok(opened) => serde_json::from_str(opened["body"].as_str().unwrap_or("")).unwrap_or(Value::Null),
            Err(err) => {
                eprintln!("{id}: {err}");
                continue;
            }
        };
        if item_id.is_empty() {
            println!("{id}  {}  {}", text(&body, "type"), text(&body, "name"));
        } else {
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
        }
    }
    Ok(())
}

/// `export-open FILE`: an encrypted export file of Commons › Vault (`{"format": "moqui-vault-export", "v": 1,
/// "export_id", "key_version", "wrap", "items"}`: a vault key wrapped by the export's password, each item sealed under
/// it), opened here with its password: each item's type and name, or one item's body.
fn export_open(file: &str, item_id: &str) -> Result<(), Error> {
    let text_of = std::fs::read_to_string(file).map_err(|err| Error::new(format!("{file}: {err}")))?;
    let export: Value = serde_json::from_str(&text_of).map_err(|_| Error::new(format!("{file} is not JSON")))?;
    if export["format"] != json!("moqui-vault-export") || export["v"] != json!(1) {
        return Err(Error::new(format!(
            "{file} is not a vault export (moqui-vault-export, v 1)"
        )));
    }
    let export_id = text(&export, "export_id");
    let key_version = export["key_version"].as_u64().unwrap_or(1);
    let password = ask("Export password", true)?;
    let mut client = VaultClient::default();
    client
        .call(
            "vault_key_password_open",
            json!({ "vault_id": export_id, "key_version": key_version, "wrap": export["wrap"], "password": password.as_str() }),
        )
        .map_err(vault_err)?;
    for row in export["items"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let id = text(row, "item_id");
        if !item_id.is_empty() && id != item_id {
            continue;
        }
        let opened = client.call(
            "item_open",
            json!({ "vault_id": export_id, "item_id": id, "key_version": key_version,
                    "wrapped_item_key": row["wrapped_item_key"], "sealed_body": row["sealed_body"] }),
        );
        let body: Value = match opened {
            Ok(opened) => serde_json::from_str(opened["body"].as_str().unwrap_or("")).unwrap_or(Value::Null),
            Err(err) => {
                eprintln!("{id}: {err}");
                continue;
            }
        };
        if item_id.is_empty() {
            println!("{id}  {}  {}", text(&body, "type"), text(&body, "name"));
        } else {
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
        }
    }
    Ok(())
}
