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

//! SSH, the manager's other transport (plans/Vault-Instances.md §4.4): the same operator commands as the link's, run
//! as `moqui operator …` by the instance's SSH admin shell or its host's sshd. The key it signs in with is the vault
//! item `AppInstance:{id}:sshKey` (type ssh-key), used under a lease or the environment's machine access and written
//! for the one call to a file of mode 0600 that is removed after; the instance's host key is pinned on its row
//! (`sshHostKey`), so an unknown host key is refused. A secret's value goes on standard input, never in the command.
//! Only status, seal and secret put are operator commands; the others are the link's.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error as LibError;
use moqui_vault_server::lease::LeaseRegistry;
use zeroize::Zeroizing;

use crate::actions;
use crate::common::{fail, field, leased};
use crate::plan::environment_of;

/// The longest an SSH call may take.
const TIMEOUT: Duration = Duration::from_secs(60);

/// An operator command over SSH.
pub enum SshCommand {
    Status,
    Seal,
    PutSecret {
        name: String,
        value: Zeroizing<String>,
        force: bool,
    },
}

/// The binding of an instance's SSH key.
#[must_use]
pub fn key_binding(instance_id: &str) -> String {
    format!("AppInstance:{instance_id}:sshKey")
}

/// The remote command of `command`: `moqui operator` and its words, each single-quoted for the remote shell. A secret
/// name with a quote, a space or a control character is refused, so nothing it holds reaches the shell.
pub fn remote_command(command: &SshCommand) -> Result<String, LibError> {
    let mut words = vec!["moqui".to_string(), "operator".to_string()];
    match command {
        SshCommand::Status => words.push("status".to_string()),
        SshCommand::Seal => words.push("seal".to_string()),
        SshCommand::PutSecret { name, force, .. } => {
            let safe = !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-_.@/[]".contains(ch));
            if !safe {
                return Err(fail(format!(
                    "the secret name {name:?} is not one an operator command takes over SSH"
                )));
            }
            words.extend(["secret".to_string(), "put".to_string(), name.clone()]);
            if *force {
                words.push("--force".to_string());
            }
        }
    }
    Ok(words
        .iter()
        .map(|word| format!("'{word}'"))
        .collect::<Vec<_>>()
        .join(" "))
}

/// Where and how to reach an instance over SSH.
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The pinned host key, as a known_hosts line's key type and base64.
    pub host_key: String,
}

impl SshTarget {
    /// The instance's from its row: `sshAddress` (host or host:port, 22 by default), `sshUser`, `sshHostKey`.
    pub fn of(row: &moqui_core::entity::EntityValue) -> Result<Self, LibError> {
        let id = field(row, "appInstanceId");
        let address = field(row, "sshAddress");
        let (host, port) = match address.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') || host.ends_with(']') => (
                host.trim_start_matches('[').trim_end_matches(']').to_string(),
                port.parse::<u16>()
                    .map_err(|_| fail(format!("AppInstance {id}: sshAddress {address} has no port")))?,
            ),
            _ => (address.clone(), 22),
        };
        let host_key = field(row, "sshHostKey");
        if host.is_empty() || host_key.is_empty() {
            return Err(fail(format!(
                "AppInstance {id} has no sshAddress or no pinned sshHostKey: SSH needs both (an unknown host key is refused)"
            )));
        }
        let target = Self {
            host,
            port,
            user: Some(field(row, "sshUser"))
                .filter(|user| !user.is_empty())
                .unwrap_or_else(|| "moqui".to_string()),
            host_key,
        };
        target
            .check()
            .map_err(|err| fail(format!("AppInstance {id}: {}", err.message())))?;
        Ok(target)
    }

    /// Refuses what ssh would read as an option or what would add a line to the pinned known_hosts file: a user that
    /// is not a plain name, a host that is not a host name or an IP address, a host key that is not one line of a key
    /// type and its base64.
    pub fn check(&self) -> Result<(), LibError> {
        let plain = |text: &str, extra: &str| {
            !text.is_empty()
                && !text.starts_with(['-', '.'])
                && text.chars().all(|ch| ch.is_ascii_alphanumeric() || extra.contains(ch))
        };
        if !plain(&self.user, "._-") || self.user.len() > 64 {
            return Err(fail(format!("the SSH user {:?} is not a user name", self.user)));
        }
        let host_name = plain(&self.host, ".-") && self.host.len() <= 253;
        if !host_name && self.host.parse::<std::net::IpAddr>().is_err() {
            return Err(fail(format!(
                "the SSH host {:?} is not a host name or an IP address",
                self.host
            )));
        }
        let words: Vec<&str> = self.host_key.split(' ').collect();
        let key_type = |text: &str| {
            text.starts_with("ssh-")
                || text.starts_with("ecdsa-sha2-")
                || text.starts_with("sk-ssh-")
                || text.starts_with("sk-ecdsa-")
        };
        let base64 =
            |text: &str| !text.is_empty() && text.chars().all(|ch| ch.is_ascii_alphanumeric() || "+/=".contains(ch));
        let one_key = matches!(words.as_slice(), [kind, key] if key_type(kind) && plain(kind, "-@.") && base64(key));
        if !one_key {
            return Err(fail(
                "the pinned SSH host key is not one known_hosts key (its type and base64, nothing else)",
            ));
        }
        Ok(())
    }
}

/// Write `bytes` to a new file of mode 0600 under `dir`.
fn private_file(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, LibError> {
    let path = dir.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|err| fail(format!("could not write {}: {err}", path.display())))?;
    file.write_all(bytes)
        .map_err(|err| fail(format!("could not write {}: {err}", path.display())))?;
    Ok(path)
}

/// Run `command` on `target` with `program` (`ssh`, or a test's) and the private key `key` (OpenSSH format): its
/// standard output. The key and the pinned host key are files of a directory of this call's alone, removed after.
pub fn run(program: &str, target: &SshTarget, key: &[u8], command: &SshCommand) -> Result<String, LibError> {
    target.check()?;
    let remote = remote_command(command)?;
    let mut nonce = [0_u8; 8];
    moqui_vault::rng::Rng::fill(&mut moqui_vault::rng::OsRng, &mut nonce);
    let dir = std::env::temp_dir().join(format!("moqui-ssh-{}", moqui_vault::text::hex(&nonce)));
    // `mode` is the only mutation, and it is Unix. On Windows the builder is used as created.
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = std::fs::DirBuilder::new();
    builder
        .create(&dir)
        .map_err(|err| fail(format!("could not make {}: {err}", dir.display())))?;
    let ran = (|| {
        let key_file = private_file(&dir, "key", key)?;
        let host = if target.port == 22 {
            target.host.clone()
        } else {
            format!("[{}]:{}", target.host, target.port)
        };
        let known = private_file(&dir, "known_hosts", format!("{host} {}\n", target.host_key).as_bytes())?;
        // no conf file of the user's or the system's, no known host but the pinned one, the user by -l, and the
        // destination after `--`, so nothing in a row is read as an option
        let mut child = Command::new(program)
            .arg("-T")
            .args(["-F", "/dev/null"])
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "GlobalKnownHostsFile=/dev/null",
            ])
            .arg("-o")
            .arg(format!("UserKnownHostsFile={}", known.display()))
            .arg("-i")
            .arg(&key_file)
            .arg("-p")
            .arg(target.port.to_string())
            .arg("-l")
            .arg(&target.user)
            .arg("--")
            .arg(&target.host)
            .arg(&remote)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| fail(format!("could not run {program}: {err}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            if let SshCommand::PutSecret { value, .. } = command {
                stdin
                    .write_all(value.as_bytes())
                    .and_then(|()| stdin.write_all(b"\n"))
                    .map_err(|err| fail(format!("could not write to {program}: {err}")))?;
            }
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(fail(format!("{program} did not finish in {} s", TIMEOUT.as_secs())));
                }
                Err(err) => return Err(fail(format!("{program} did not finish: {err}"))),
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|err| fail(format!("{program}: {err}")))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(fail(format!(
                "{program} to {}@{} failed ({}): {}",
                target.user,
                target.host,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    })();
    if let Err(err) = std::fs::remove_dir_all(&dir) {
        tracing::error!("SSH: the key directory {} was not removed: {err}", dir.display());
    }
    ran
}

/// Run `command` on `instance_id` over SSH (see the module doc), under `lease_id` or its environment's machine access:
/// what it printed.
pub fn call(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    instance_id: &str,
    command: &SshCommand,
    lease_id: Option<&str>,
) -> Result<String, LibError> {
    let row = crate::link::instance(facade, instance_id)?;
    crate::link::check_caller(facade, &row)?;
    let target = SshTarget::of(&row)?;
    let environment = environment_of(facade, &row)?;
    let binding = key_binding(instance_id);
    let opened = actions::open_authority(
        facade,
        registry,
        &field(&environment, "environmentId"),
        &field(&environment, "vaultId"),
        field(&environment, "machineAccess") == "Y",
        std::slice::from_ref(&binding),
        "ssh",
        lease_id,
    )?;
    let program = Some(moqui_core::context::conf_env::setting("manage@ssh-command"))
        .map(|program| program.trim().to_string())
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| "ssh".to_string());
    let body = || -> Result<String, LibError> {
        let key = leased(&binding, "privateKey")?;
        run(&program, &target, key.as_bytes(), command)
    };
    let ran = match &opened.lease_id {
        Some(lease_id) => moqui_vault_server::lease::run_under(lease_id, body),
        None => body(),
    };
    actions::end_lease(facade, registry, &opened);
    ran
}
