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

//! The `tpm` method (plans/Sealed-Secrets.md §3.3, the default for BOSS): the root key encrypted by `systemd-creds`
//! with the host's TPM2, bound to its PCRs, so the server unseals itself at an unattended reboot of the same host and
//! nowhere else. The program is never taken from `seal.json`, which a restored bundle supplies: it is
//! `seal@systemd-creds-command`, an absolute path read only from the conf files ([`SealConf::from_gate`]), else the
//! first of [`PROGRAMS`] there is. A call that takes longer than a minute is ended.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use super::{bytes_field, Method, MethodKind};
use crate::context::Error;
use crate::seal::{encode, RootKey, SealConf};

/// The credential's name, which `systemd-creds` binds into it.
const NAME: &str = "moqui-seal";

/// Where `systemd-creds` is when `seal@systemd-creds-command` is empty.
pub const PROGRAMS: [&str; 2] = ["/usr/bin/systemd-creds", "/bin/systemd-creds"];

/// The longest a `systemd-creds` call may take.
const TIMEOUT: Duration = Duration::from_secs(60);

/// The `tpm` method.
pub struct Tpm;

/// The program to run.
fn program(conf: &SealConf) -> Result<String, Error> {
    if !conf.systemd_creds_command.is_empty() {
        return Ok(conf.systemd_creds_command.clone());
    }
    PROGRAMS
        .iter()
        .find(|program| Path::new(program).is_file())
        .map(|program| (*program).to_string())
        .ok_or_else(|| {
            Error::new(format!(
                "no systemd-creds at {}: the tpm method needs systemd with a TPM2",
                PROGRAMS.join(" or ")
            ))
        })
}

/// Run `command` with `args`, `input` on its standard input, and give its standard output.
fn run(command: &str, args: &[&str], input: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| Error::new(format!("could not run {command}")).with_source(err))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .map_err(|err| Error::new(format!("could not write to {command}")).with_source(err))?;
    }
    // its output is a credential or a key, far under a pipe's buffer, so it is read once the program has ended
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::new(format!(
                    "{command} did not finish in {} s",
                    TIMEOUT.as_secs()
                )));
            }
            Err(err) => return Err(Error::new(format!("{command} did not finish")).with_source(err)),
        }
    };
    let mut stdout = Zeroizing::new(Vec::new());
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut stdout)
            .map_err(|err| Error::new(format!("could not read {command}")).with_source(err))?;
    }
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        return Err(Error::new(format!(
            "{command} {} failed ({status}): {}",
            args.first().copied().unwrap_or_default(),
            stderr.trim()
        )));
    }
    Ok(stdout)
}

/// The program of `command`, an absolute path, else the first of [`PROGRAMS`] there is.
fn program_of(command: &str) -> Result<String, Error> {
    let mut conf = SealConf::with_purpose("");
    conf.systemd_creds_command = command.trim().to_string();
    program(&conf)
}

/// `data` as a credential named `name`, encrypted by `systemd-creds` with the host's TPM2: for a key that must open
/// while the server is sealed and only on this host (the manager link's key, plans/archive/Vault-Instances.md §4.4).
///
/// # Errors
///
/// No `systemd-creds`, or it failed.
pub fn encrypt_credential(command: &str, name: &str, data: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let name = format!("--name={name}");
    run(
        &program_of(command)?,
        &["encrypt", "--with-key=tpm2", &name, "-", "-"],
        data,
    )
}

/// A credential [`encrypt_credential`] made, opened.
///
/// # Errors
///
/// No `systemd-creds`, or the credential does not open on this host.
pub fn decrypt_credential(command: &str, name: &str, credential: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let name = format!("--name={name}");
    run(&program_of(command)?, &["decrypt", &name, "-", "-"], credential)
}

impl Method for Tpm {
    fn kind(&self) -> MethodKind {
        MethodKind::Tpm
    }

    fn wrap(&self, root: &RootKey, conf: &SealConf) -> Result<Map<String, Value>, Error> {
        let name = format!("--name={NAME}");
        let credential = run(
            &program(conf)?,
            &["encrypt", "--with-key=tpm2", &name, "-", "-"],
            root.expose(),
        )?;
        let Value::Object(metadata) = json!({
            "name": NAME,
            "credential": encode(&credential),
        }) else {
            unreachable!("a json! object literal is an object");
        };
        Ok(metadata)
    }

    fn unwrap(&self, metadata: &Map<String, Value>, conf: &SealConf) -> Result<RootKey, Error> {
        let name = format!("--name={NAME}");
        let root = run(
            &program(conf)?,
            &["decrypt", &name, "-", "-"],
            &bytes_field(metadata, "credential")?,
        )?;
        RootKey::from_slice(&root)
    }
}
