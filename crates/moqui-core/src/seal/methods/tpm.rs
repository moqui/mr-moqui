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
//! nowhere else. The program is `seal@systemd-creds-command` when the seal is made, and is recorded with it: a conf
//! that names another program later is ignored and logged, as the method is (tests point it at a stub script).

use std::io::Write as _;
use std::process::{Command, Stdio};

use serde_json::{json, Map, Value};
use zeroize::Zeroizing;

use super::{bytes_field, Method, MethodKind};
use crate::context::Error;
use crate::seal::{encode, RootKey, SealConf};

/// The credential's name, which `systemd-creds` binds into it.
const NAME: &str = "moqui-seal";

/// The `tpm` method.
pub struct Tpm;

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
    let output = child
        .wait_with_output()
        .map_err(|err| Error::new(format!("{command} did not finish")).with_source(err))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::new(format!(
            "{command} {} failed ({}): {}",
            args.first().copied().unwrap_or_default(),
            output.status,
            stderr.trim()
        )));
    }
    Ok(Zeroizing::new(output.stdout))
}

impl Method for Tpm {
    fn kind(&self) -> MethodKind {
        MethodKind::Tpm
    }

    fn wrap(&self, root: &RootKey, conf: &SealConf) -> Result<Map<String, Value>, Error> {
        let command = conf.systemd_creds_command.as_str();
        let name = format!("--name={NAME}");
        let credential = run(command, &["encrypt", "--with-key=tpm2", &name, "-", "-"], root.expose())?;
        let Value::Object(metadata) = json!({
            "command": command,
            "name": NAME,
            "credential": encode(&credential),
        }) else {
            unreachable!("a json! object literal is an object");
        };
        Ok(metadata)
    }

    fn unwrap(&self, metadata: &Map<String, Value>, conf: &SealConf) -> Result<RootKey, Error> {
        let command = metadata
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("systemd-creds");
        if conf.systemd_creds_command != command {
            tracing::warn!(
                "seal@systemd-creds-command is {}, but this seal was made with {command}: the recorded one is used",
                conf.systemd_creds_command
            );
        }
        let name = format!(
            "--name={}",
            metadata.get("name").and_then(Value::as_str).unwrap_or(NAME)
        );
        let root = run(
            command,
            &["decrypt", &name, "-", "-"],
            &bytes_field(metadata, "credential")?,
        )?;
        RootKey::from_slice(&root)
    }
}
