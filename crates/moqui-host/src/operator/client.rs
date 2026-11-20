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

//! `moqui operator …` (plans/Sealed-Secrets.md §3.3): the client of a server's operator socket, run at the console or
//! through the host's own sshd. Passphrases and shares are read with the terminal's echo off (from standard input, a
//! line each, when it is not a terminal), and the shares `init` issues are printed once. The socket is
//! [`UNIT_SOCKET`] or `--socket`, never one a conf, `runtime/.env` or the environment names, so whoever can write those
//! cannot point root's passphrase at a socket of their own. `moqui operator bind` makes the socket as root for a
//! server that drops to its own user (a container's entrypoint).

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};

use moqui_lib::error::Error;
use serde_json::{json, Value};
use zeroize::{Zeroize as _, Zeroizing};

/// The operator socket the socket unit (`systemd/moqui-operator.socket`) and a container's entrypoint make, in a
/// directory of root's.
pub const UNIT_SOCKET: &str = "/run/moqui-operator/moqui.sock";

/// Clear every text in `value`: a request or an answer that carried a passphrase or shares.
pub fn scrub(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(items) => {
            for item in items {
                scrub(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                scrub(item);
            }
        }
        _ => {}
    }
}

/// `moqui operator <command>`.
///
/// # Errors
///
/// A usage error, no server on the socket, or a refused command.
pub fn dispatch(args: &[String]) -> Result<(), Error> {
    if let [command, path, rest @ ..] = args {
        if command == "bind" {
            return bind_and_exec(path, rest);
        }
    }
    let mut socket: Option<PathBuf> = None;
    let mut rest = Vec::new();
    let mut words = args.iter();
    while let Some(arg) = words.next() {
        if arg == "--socket" {
            socket = Some(
                words
                    .next()
                    .ok_or_else(|| crate::usage::command_usage("operator"))?
                    .into(),
            );
        } else if let Some(path) = arg.strip_prefix("--socket=") {
            socket = Some(path.into());
        } else {
            rest.push(arg.as_str());
        }
    }
    let socket = socket.unwrap_or_else(socket_path);
    match rest.as_slice() {
        ["status"] => {
            let answer = call(&socket, json!({"command": "status"}))?;
            print_status(&answer["status"]);
            Ok(())
        }
        ["seal"] => {
            let answer = call(&socket, json!({"command": "seal"}))?;
            println!("Sealed: the root key is dropped from memory and the listeners close.");
            print_status(&answer["status"]);
            Ok(())
        }
        ["init", options @ ..] => init(&socket, options),
        ["unseal", options @ ..] => unseal(&socket, options),
        ["rekey", "--method", method] => {
            let mut request = json!({"command": "rekey", "method": method});
            if *method == "passphrase" {
                request["passphrase"] = Value::String(new_passphrase()?.to_string());
            }
            with_recovery_shares(&socket, &mut request)?;
            let mut answer = call(&socket, request)?;
            print_shares(&answer);
            print_status(&answer["status"]);
            scrub(&mut answer);
            println!("Export the recovery bundle again: moqui operator export-seal <file>");
            Ok(())
        }
        ["rotate-root"] => {
            let mut request = json!({"command": "rotate-root"});
            let status = call(&socket, json!({"command": "status"}))?;
            if status["status"]["method"] == "passphrase" {
                request["passphrase"] = Value::String(read_secret("Passphrase: ")?.to_string());
            }
            with_recovery_shares(&socket, &mut request)?;
            let mut answer = call(&socket, request)?;
            print_shares(&answer);
            print_status(&answer["status"]);
            scrub(&mut answer);
            println!("Export the recovery bundle again: moqui operator export-seal <file>");
            Ok(())
        }
        ["secret", "put", name, options @ ..] => {
            let force = options.contains(&"--force");
            let options: Vec<&str> = options.iter().copied().filter(|option| *option != "--force").collect();
            let value = match options.as_slice() {
                ["--file", path] => Zeroizing::new(
                    std::fs::read_to_string(path)
                        .map_err(|err| Error::new(format!("could not read {path}")).with_source(err))?,
                ),
                [] => secret_value()?,
                _ => return Err(crate::usage::command_usage("operator")),
            };
            let request = json!({"command": "secret-put", "name": name, "value": value.as_str(), "force": force});
            let answer = call(&socket, request)?;
            println!(
                "Kept the secret {} (its value is never shown).",
                answer["name"].as_str().unwrap_or(name)
            );
            Ok(())
        }
        ["secret", "list"] => {
            let answer = call(&socket, json!({"command": "secret-list"}))?;
            for listed in answer["secrets"].as_array().into_iter().flatten() {
                println!(
                    "{}  {}  generation {}",
                    listed["name"].as_str().unwrap_or_default(),
                    listed["kind"].as_str().unwrap_or_default(),
                    listed["generation"]
                );
            }
            Ok(())
        }
        ["secret", "rm", name] => {
            let answer = call(&socket, json!({"command": "secret-rm", "name": name}))?;
            if answer["removed"] == Value::Bool(true) {
                println!("Removed the secret {name}.");
            } else {
                println!("There is no secret {name}.");
            }
            Ok(())
        }
        ["export-seal", path] => {
            let answer = call(&socket, json!({"command": "export-seal"}))?;
            write_private(Path::new(path), answer["bundle"].as_str().unwrap_or_default())?;
            println!("Wrote the recovery bundle to {path}: keep it with the offline backups, apart from the shares.");
            if let Some(warning) = answer["warning"].as_str() {
                eprintln!("Warning: {warning}");
            }
            Ok(())
        }
        _ => Err(crate::usage::command_usage("operator")),
    }
}

/// The socket a client reaches with no `--socket`: [`UNIT_SOCKET`], always for root; for another user, when it is
/// not there, `db/operator.sock` in the runtime (a development server that binds its own; no conf is read).
fn socket_path() -> PathBuf {
    let unit = Path::new(UNIT_SOCKET);
    if unit.exists() || rustix::process::geteuid().is_root() {
        return unit.to_path_buf();
    }
    let (runtime, _) = crate::runtime_framework_dirs();
    runtime.join("db").join("operator.sock")
}

/// `moqui operator bind <path> [--group NAME] -- <command> ...`, as root: make the operator socket at `path` (its
/// directory root's, mode 0755; the socket root's, mode 0660 with the group or 0600 with none), then run `command` in
/// this process with the socket open on it, named by `MOQUI_OPERATOR_FD` and this process's id: a container's
/// entrypoint runs it before `setpriv` drops to the server's user, which keeps the process and the descriptor.
fn bind_and_exec(path: &str, rest: &[String]) -> Result<(), Error> {
    let usage = || crate::usage::command_usage("operator");
    let (group, command) = match rest {
        [flag, name, dash, command @ ..] if flag == "--group" && dash == "--" => (Some(name.as_str()), command),
        [dash, command @ ..] if dash == "--" => (None, command),
        _ => return Err(usage()),
    };
    let [program, args @ ..] = command else {
        return Err(usage());
    };
    if !rustix::process::geteuid().is_root() {
        return Err(Error::new(
            "moqui operator bind runs as root, before the server drops to its own user",
        ));
    }
    let path = Path::new(path);
    let Some(dir) = path.parent().filter(|_| path.is_absolute()) else {
        return Err(Error::new("moqui operator bind takes an absolute path"));
    };
    let gid = match group {
        Some(name) => Some(resolve_group(name).ok_or_else(|| Error::new(format!("no group {name} on this host")))?),
        None => None,
    };
    let failed = |what: &str, err: std::io::Error| Error::new(format!("{what} {}", path.display())).with_source(err);
    std::fs::create_dir_all(dir).map_err(|err| failed("could not make the directory of", err))?;
    rustix::fs::chown(dir, Some(rustix::fs::Uid::ROOT), Some(rustix::fs::Gid::ROOT))
        .map_err(|err| failed("could not give root the directory of", err.into()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
        .map_err(|err| failed("could not set the mode of the directory of", err))?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(Error::new(format!("{} is there and is not a socket", path.display())));
        }
        std::fs::remove_file(path).map_err(|err| failed("could not remove", err))?;
    }
    let listener = std::os::unix::net::UnixListener::bind(path).map_err(|err| failed("could not bind", err))?;
    rustix::fs::chown(path, Some(rustix::fs::Uid::ROOT), gid.map(rustix::fs::Gid::from_raw))
        .map_err(|err| failed("could not give root", err.into()))?;
    let mode = if gid.is_some() { 0o660 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|err| failed("could not set the mode of", err))?;
    // kept open across the exec, for the server to take
    rustix::io::fcntl_setfd(&listener, rustix::io::FdFlags::empty())
        .map_err(|err| failed("could not keep open", err.into()))?;
    let err = std::process::Command::new(program)
        .args(args)
        .env("MOQUI_OPERATOR_FD", listener.as_raw_fd().to_string())
        .env("MOQUI_OPERATOR_PID", std::process::id().to_string())
        .exec();
    Err(Error::new(format!("could not run {program}")).with_source(err))
}

/// A group's id: a number as given, else the name's line in `/etc/group`.
fn resolve_group(name: &str) -> Option<u32> {
    let name = name.trim();
    if let Ok(gid) = name.parse() {
        return Some(gid);
    }
    let groups = std::fs::read_to_string("/etc/group").ok()?;
    groups.lines().find_map(|line| {
        let mut fields = line.split(':');
        (fields.next() == Some(name))
            .then(|| fields.nth(1).and_then(|gid| gid.parse().ok()))
            .flatten()
    })
}

/// Send one request to the socket at `socket` and read its answer: the answer when `ok`, else its error. The request's
/// texts are cleared once sent.
///
/// # Errors
///
/// No server on the socket, a connection that ends early, or a refused command (its error text).
pub fn call(socket: &Path, mut request: Value) -> Result<Value, Error> {
    let answer = send(socket, &request);
    scrub(&mut request);
    answer
}

fn send(socket: &Path, request: &Value) -> Result<Value, Error> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket).map_err(|err| {
        Error::new(format!(
            "no server answers on the operator socket {} (--socket names another)",
            socket.display()
        ))
        .with_source(err)
    })?;
    let mut text = Zeroizing::new(request.to_string());
    text.push('\n');
    stream
        .write_all(text.as_bytes())
        .map_err(|err| Error::new("could not send to the operator socket").with_source(err))?;
    let mut line = Zeroizing::new(String::new());
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|err| Error::new("could not read the operator socket's answer").with_source(err))?;
    let answer: Value = serde_json::from_str(&line)
        .map_err(|err| Error::new(format!("the operator socket's answer is not JSON: {err}")))?;
    if answer["ok"] == Value::Bool(true) {
        Ok(answer)
    } else {
        Err(Error::new(answer["error"].as_str().unwrap_or("refused").to_string()))
    }
}

fn init(socket: &Path, options: &[&str]) -> Result<(), Error> {
    let mut request = json!({"command": "init"});
    let mut words = options.iter();
    while let Some(option) = words.next() {
        match *option {
            "--take-shares" => request["take_shares"] = Value::Bool(true),
            "--method" => {
                let method = words.next().ok_or_else(|| crate::usage::command_usage("operator"))?;
                request["method"] = Value::String((*method).to_string());
            }
            "--from" => {
                let path = words.next().ok_or_else(|| crate::usage::command_usage("operator"))?;
                let bundle = std::fs::read_to_string(path)
                    .map_err(|err| Error::new(format!("could not read the bundle {path}")).with_source(err))?;
                request["bundle"] = Value::String(bundle);
            }
            _ => return Err(crate::usage::command_usage("operator")),
        }
    }
    if request["method"] == "passphrase" {
        request["passphrase"] = Value::String(new_passphrase()?.to_string());
    }
    let mut answer = call(socket, request)?;
    print_shares(&answer);
    print_status(&answer["status"]);
    scrub(&mut answer);
    Ok(())
}

fn unseal(socket: &Path, options: &[&str]) -> Result<(), Error> {
    let recovery = match options {
        [] => false,
        ["--recovery"] => true,
        _ => return Err(crate::usage::command_usage("operator")),
    };
    let status = call(socket, json!({"command": "status"}))?;
    let method = status["status"]["method"].as_str().unwrap_or_default().to_string();
    let request = if recovery {
        json!({"command": "unseal", "recovery": true, "share": read_secret("Recovery share: ")?.as_str()})
    } else if method == "passphrase" {
        json!({"command": "unseal", "passphrase": read_secret("Passphrase: ")?.as_str()})
    } else if method == "shamir" {
        json!({"command": "unseal", "share": read_secret("Share: ")?.as_str()})
    } else {
        json!({"command": "unseal"})
    };
    let answer = call(socket, request)?;
    print_status(&answer["status"]);
    Ok(())
}

/// Read the threshold of recovery shares the server's seal asks for into `request["shares"]`.
fn with_recovery_shares(socket: &Path, request: &mut Value) -> Result<(), Error> {
    let status = call(socket, json!({"command": "status"}))?;
    let threshold = status["status"]["recovery"][0].as_u64().unwrap_or(0);
    if threshold == 0 {
        return Err(Error::new("this seal has no recovery shares (the dev seal)"));
    }
    let mut shares = Vec::new();
    for number in 1..=threshold {
        shares.push(Value::String(
            read_secret(&format!("Recovery share {number} of {threshold}: "))?.to_string(),
        ));
    }
    request["shares"] = Value::Array(shares);
    Ok(())
}

/// Write `text` to `path`, readable by its owner only.
fn write_private(path: &Path, text: &str) -> Result<(), Error> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| Error::new(format!("could not write {}", path.display())).with_source(err))?;
    out.write_all(text.as_bytes())
        .and_then(|()| out.write_all(b"\n"))
        .map_err(|err| Error::new(format!("could not write {}", path.display())).with_source(err))
}

/// A new passphrase, entered twice.
fn new_passphrase() -> Result<Zeroizing<String>, Error> {
    let first = read_secret("New passphrase: ")?;
    if first.chars().count() < 12 {
        return Err(Error::new("a passphrase of at least 12 characters"));
    }
    let again = read_secret("Again: ")?;
    if first != again {
        return Err(Error::new("the two passphrases differ"));
    }
    Ok(first)
}

/// One secret line: from the terminal with its echo off, else a line of standard input.
fn read_secret(prompt: &str) -> Result<Zeroizing<String>, Error> {
    let stdin = std::io::stdin();
    let terminal = rustix::termios::isatty(&stdin);
    let saved = if terminal {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let saved = rustix::termios::tcgetattr(&stdin).ok();
        if let Some(mut quiet) = saved.clone() {
            quiet.local_modes.remove(rustix::termios::LocalModes::ECHO);
            let _ = rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &quiet);
        }
        saved
    } else {
        None
    };
    let mut line = Zeroizing::new(String::new());
    let read = stdin.lock().read_line(&mut line);
    if let Some(saved) = saved {
        let _ = rustix::termios::tcsetattr(&stdin, rustix::termios::OptionalActions::Now, &saved);
        eprintln!();
    }
    read.map_err(|err| Error::new("could not read the secret").with_source(err))?;
    let trimmed = Zeroizing::new(line.trim_end_matches(['\r', '\n']).to_string());
    if trimmed.is_empty() {
        return Err(Error::new("nothing was entered"));
    }
    Ok(trimmed)
}

/// A secret's value: one line from the terminal with its echo off, else all of standard input (a PEM key piped in).
fn secret_value() -> Result<Zeroizing<String>, Error> {
    let stdin = std::io::stdin();
    if rustix::termios::isatty(&stdin) {
        return read_secret("Value: ");
    }
    let mut text = Zeroizing::new(String::new());
    std::io::Read::read_to_string(&mut stdin.lock(), &mut text)
        .map_err(|err| Error::new("could not read the value").with_source(err))?;
    if text.trim().is_empty() {
        return Err(Error::new("nothing was entered"));
    }
    Ok(text)
}

fn print_shares(answer: &Value) {
    for (field, title) in [
        ("method_shares", "Unseal shares of the shamir method"),
        ("recovery_shares", "Recovery shares"),
    ] {
        let Some(shares) = answer[field].as_array().filter(|shares| !shares.is_empty()) else {
            continue;
        };
        println!("{title} (shown once; give each to a different holder, and keep none on this server):");
        for share in shares {
            println!("  {}", share.as_str().unwrap_or_default());
        }
    }
}

fn print_status(status: &Value) {
    let text = |name: &str| status[name].as_str().unwrap_or_default().to_string();
    let state = text("state");
    let method = text("method");
    println!(
        "State: {state}{}",
        if method.is_empty() {
            " (no seal yet)".to_string()
        } else {
            format!(", method {method}")
        }
    );
    if state == "unsealing" {
        println!("Shares: {} of {} entered", status["entered"], status["needed"]);
    }
    if let Some(recovery) = status["recovery"].as_array() {
        println!(
            "Recovery: {} of {} shares",
            recovery.first().unwrap_or(&Value::Null),
            recovery.get(1).unwrap_or(&Value::Null)
        );
    }
    let reason = text("reason");
    if !reason.is_empty() {
        println!("Reason: {reason}");
    }
    if status["retry_after_ms"].as_u64().unwrap_or(0) > 0 {
        println!("Next attempt in {} ms", status["retry_after_ms"]);
    }
    if status["shares_waiting"] == Value::Bool(true) {
        println!("Recovery shares are held in memory: run `moqui operator init --take-shares`");
    }
    if status["ephemeral"] == Value::Bool(true) {
        println!("No seal: this process holds a root key in memory only");
    }
}
