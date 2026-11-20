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
//! line each, when it is not a terminal), and the shares `init` issues are printed once.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};

use moqui_lib::error::Error;
use serde_json::{json, Value};
use zeroize::Zeroizing;

/// `moqui operator <command>`.
///
/// # Errors
///
/// A usage error, no server on the socket, or a refused command.
pub fn dispatch(args: &[String]) -> Result<(), Error> {
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
            let answer = call(&socket, &json!({"command": "status"}))?;
            print_status(&answer["status"]);
            Ok(())
        }
        ["seal"] => {
            let answer = call(&socket, &json!({"command": "seal"}))?;
            println!("Sealed: the root key is dropped from memory and the listeners close.");
            print_status(&answer["status"]);
            Ok(())
        }
        ["init", options @ ..] => init(&socket, options),
        ["unseal", options @ ..] => unseal(&socket, options),
        _ => Err(crate::usage::command_usage("operator")),
    }
}

/// The socket a client reaches: `seal@socket` of this runtime's conf, else the unit's `/run/moqui/moqui.sock` when
/// it is there (`$RUNTIME_DIRECTORY` is the server's, not a shell's), else `db/operator.sock` in the runtime.
fn socket_path() -> PathBuf {
    let (runtime, framework) = crate::runtime_framework_dirs();
    if let Ok(conf) = moqui_core::context::load_merged_conf(&framework, &runtime) {
        let set = moqui_core::context::conf_env::conf_attribute(&conf, "seal@socket");
        if !set.is_empty() {
            return runtime.join(set);
        }
    }
    if let Ok(dirs) = std::env::var("RUNTIME_DIRECTORY") {
        if let Some(first) = dirs.split(':').next().filter(|first| !first.is_empty()) {
            return PathBuf::from(first).join("moqui.sock");
        }
    }
    let unit = Path::new("/run/moqui/moqui.sock");
    if unit.exists() {
        return unit.to_path_buf();
    }
    runtime.join("db").join("operator.sock")
}

/// Send one request to the socket at `socket` and read its answer: the answer when `ok`, else its error.
///
/// # Errors
///
/// No server on the socket, a connection that ends early, or a refused command (its error text).
pub fn call(socket: &Path, request: &Value) -> Result<Value, Error> {
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
            _ => return Err(crate::usage::command_usage("operator")),
        }
    }
    if request["method"] == "passphrase" {
        request["passphrase"] = Value::String(new_passphrase()?.to_string());
    }
    let answer = call(socket, &request)?;
    print_shares(&answer);
    print_status(&answer["status"]);
    Ok(())
}

fn unseal(socket: &Path, options: &[&str]) -> Result<(), Error> {
    let recovery = match options {
        [] => false,
        ["--recovery"] => true,
        _ => return Err(crate::usage::command_usage("operator")),
    };
    let status = call(socket, &json!({"command": "status"}))?;
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
    let answer = call(socket, &request)?;
    print_status(&answer["status"]);
    Ok(())
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
