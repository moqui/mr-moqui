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

//! `moqui term` (plans/Terminal-Fancy2.md §4.3): list, read, type into, wait on and read the events of the user's
//! terminal sessions on a running server, over its REST (`/rest/s1/moqui/terminal`). Inside one of the user's shells it
//! takes that session's `MOQUI_TERMINAL_URL` and `MOQUI_TERMINAL_TOKEN`; with `--server URL` the login key
//! `moqui tui --server` keeps for that server, or `MOQUI_LOGIN_KEY`, and `--ticket` for the recent sign-in a command
//! line or a shell asks from another machine.

use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use serde_json::Value as Json;

/// The longest one wait call holds the connection: proxies let it through, and the command waits again.
const WAIT_SLICE: u64 = 25;

/// How a call is signed in.
enum Auth {
    Token { session: String, token: String },
    Key(String),
}

struct Client {
    base: String,
    auth: Auth,
    ticket: String,
    http: reqwest::blocking::Client,
}

impl Client {
    fn connect(server: Option<&str>, ticket: &str) -> Result<Self, Error> {
        let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let (base, auth) = match server {
            Some(url) => {
                let base = url.trim().trim_end_matches('/').to_string();
                if !(base.starts_with("https://") || base.starts_with("http://")) {
                    return Err(Error::new(format!("--server {url}: an http or https URL")));
                }
                let key = env("MOQUI_LOGIN_KEY")
                    .or_else(|| {
                        crate::tui_remote::key_file(&base)
                            .ok()
                            .and_then(|path| crate::tui_remote::stored_key(&path))
                    })
                    .ok_or_else(|| {
                        Error::new(format!(
                            "no login key for {base}: log in once with moqui tui --server {base}, or set MOQUI_LOGIN_KEY"
                        ))
                    })?;
                (base, Auth::Key(key))
            }
            None => match (
                env("MOQUI_TERMINAL_URL"),
                env("MOQUI_TERMINAL_SESSION"),
                env("MOQUI_TERMINAL_TOKEN"),
            ) {
                (Some(url), Some(session), Some(token)) => {
                    (url.trim_end_matches('/').to_string(), Auth::Token { session, token })
                }
                _ => return Err(Error::new(
                    "moqui term runs in a terminal session of the server's (it reads MOQUI_TERMINAL_TOKEN), or with \
                         --server URL",
                )),
            },
        };
        let http = moqui_lib::dev_ca::builder(false)?
            .timeout(Duration::from_secs(WAIT_SLICE + 30))
            .user_agent(concat!("moqui-term/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| Error::new(format!("HTTP client: {err}")))?;
        Ok(Self {
            base,
            auth,
            ticket: ticket.to_string(),
            http,
        })
    }

    /// A call of the terminal resource: `path` below it, a GET with `query` or a POST of `body`.
    fn call(&self, path: &str, query: &[(&str, String)], body: Option<Json>) -> Result<Json, Error> {
        let url = format!("{}/rest/s1/moqui/terminal{path}", self.base);
        let mut request = match &body {
            Some(_) => self.http.post(&url),
            None => self.http.get(&url),
        };
        let mut query: Vec<(&str, String)> = query.to_vec();
        if !self.ticket.is_empty() {
            query.push(("ticket", self.ticket.clone()));
        }
        request = request.query(&query).header("Accept", "application/json");
        request = match &self.auth {
            Auth::Token { session, token } => request
                .header(crate::term::token::SESSION_HEADER, session)
                .header(crate::term::token::TOKEN_HEADER, token),
            Auth::Key(key) => request.header("login_key", key),
        };
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().map_err(|err| Error::new(format!("{url}: {err}")))?;
        let status = response.status();
        let json: Json = response.json().unwrap_or(Json::Null);
        let errors = json
            .get("errors")
            .and_then(Json::as_str)
            .map(str::to_string)
            .or_else(|| json.get("errorCode").map(|_| json.to_string()));
        match errors {
            Some(errors) if !errors.is_empty() => Err(Error::new(errors.trim().to_string())),
            _ if !status.is_success() => Err(Error::new(format!("{url}: {status}"))),
            _ => Ok(json),
        }
    }
}

/// The value of `--name value` in `args`, and `args` without both.
fn take(args: &mut Vec<String>, name: &str) -> Option<String> {
    let at = args.iter().position(|arg| arg == name)?;
    args.remove(at);
    (at < args.len()).then(|| args.remove(at))
}

/// Whether `--name` is in `args`, taken out.
fn flag(args: &mut Vec<String>, name: &str) -> bool {
    let found = args.iter().position(|arg| arg == name);
    if let Some(at) = found {
        args.remove(at);
    }
    found.is_some()
}

fn print_lines(json: &Json) {
    for line in json.get("lines").and_then(Json::as_array).into_iter().flatten() {
        println!("{}", line.as_str().unwrap_or_default());
    }
}

fn id_of(args: &mut Vec<String>, command: &str) -> Result<String, Error> {
    if args.is_empty() || args[0].starts_with("--") {
        return Err(Error::new(format!("moqui term {command} <sessionId>")));
    }
    Ok(args.remove(0))
}

/// `moqui term <command> ...`: exits 1 on an error, and `wait` 2 on a timeout.
///
/// # Errors
///
/// Returns an error when the server cannot be reached, refuses the call, or the arguments are wrong.
pub fn dispatch(rest: &[String]) -> Result<i32, Error> {
    let mut args: Vec<String> = rest.to_vec();
    let server = take(&mut args, "--server");
    let ticket = take(&mut args, "--ticket").unwrap_or_default();
    let Some(command) = (!args.is_empty()).then(|| args.remove(0)) else {
        println!("{}", crate::usage::command_help("term").unwrap_or_default());
        return Ok(0);
    };
    let client = Client::connect(server.as_deref(), &ticket)?;
    match command.as_str() {
        "list" => {
            let json = client.call("", &[], None)?;
            for session in json.get("sessions").and_then(Json::as_array).into_iter().flatten() {
                let text = |key: &str| session.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    text("id"),
                    text("kind"),
                    text("state"),
                    if text("status").is_empty() {
                        "-".to_string()
                    } else {
                        text("status")
                    },
                    text("title")
                );
            }
            Ok(0)
        }
        "capture" => {
            let id = id_of(&mut args, "capture")?;
            let scope = take(&mut args, "--scope").unwrap_or_else(|| "visible".to_string());
            let lines = take(&mut args, "--lines").unwrap_or_else(|| "24".to_string());
            let ansi = flag(&mut args, "--ansi");
            let mut query = vec![("scope", scope), ("lines", lines)];
            if ansi {
                query.push(("format", "ansi".to_string()));
            }
            let json = client.call(&format!("/{id}/screen"), &query, None)?;
            match json.get("ansi").and_then(Json::as_str).filter(|_| ansi) {
                Some(screen) => print!("{screen}"),
                None => print_lines(&json),
            }
            Ok(0)
        }
        "send" => {
            let id = id_of(&mut args, "send")?;
            let keys = take(&mut args, "--keys").unwrap_or_default();
            let text = args.join(" ");
            let json = client.call(
                &format!("/{id}/input"),
                &[],
                Some(serde_json::json!({"text": text, "keys": keys})),
            )?;
            println!("{}", json.get("count").and_then(Json::as_u64).unwrap_or(0));
            Ok(0)
        }
        "wait" => wait(&client, &mut args),
        "events" => {
            let id = id_of(&mut args, "events")?;
            let after = take(&mut args, "--after").unwrap_or_else(|| "0".to_string());
            let json = client.call(&format!("/{id}/events"), &[("after", after)], None)?;
            for event in json.get("events").and_then(Json::as_array).into_iter().flatten() {
                let field = |key: &str| {
                    event.get(key).map_or_else(String::new, |value| match value {
                        Json::String(text) => text.clone(),
                        Json::Null => String::new(),
                        other => other.to_string(),
                    })
                };
                println!(
                    "{}\t{}\t{}\t{}",
                    field("number"),
                    field("kind"),
                    field("status"),
                    field("text")
                );
            }
            Ok(0)
        }
        other => Err(Error::new(format!(
            "moqui term {other}: the commands are list, capture, send, wait and events (moqui help term)"
        ))),
    }
}

/// `moqui term wait <id> ...`: waits of at most [`WAIT_SLICE`] each until one matches or `--timeout` (300) passes.
fn wait(client: &Client, args: &mut Vec<String>) -> Result<i32, Error> {
    let id = id_of(args, "wait")?;
    let mut body = serde_json::Map::new();
    for (option, field) in [
        ("--status", "status"),
        ("--pattern", "pattern"),
        ("--quiet-ms", "quietMs"),
    ] {
        if let Some(value) = take(args, option) {
            body.insert(field.to_string(), Json::String(value));
        }
    }
    for (option, field) in [("--prompt", "prompt"), ("--ended", "ended")] {
        if flag(args, option) {
            body.insert(field.to_string(), Json::Bool(true));
        }
    }
    let total = take(args, "--timeout")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(300)
        .min(300);
    let deadline = Instant::now() + Duration::from_secs(total);
    loop {
        let left = deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .clamp(1, WAIT_SLICE);
        body.insert("timeoutSeconds".to_string(), Json::from(left));
        let json = client.call(&format!("/{id}/wait"), &[], Some(Json::Object(body.clone())))?;
        let matched = json.get("matched").and_then(Json::as_str).unwrap_or_default();
        if !matched.is_empty() || Instant::now() >= deadline {
            print_lines(&json);
            if matched.is_empty() {
                eprintln!("moqui term wait: timed out after {total} s");
                return Ok(2);
            }
            eprintln!("moqui term wait: {matched}");
            return Ok(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{flag, take};

    #[test]
    fn options_are_taken_out_of_the_arguments() {
        let mut args: Vec<String> = ["s1", "--keys", "Enter", "make", "--prompt"]
            .iter()
            .map(|arg| (*arg).to_string())
            .collect();
        assert_eq!(take(&mut args, "--keys").as_deref(), Some("Enter"));
        assert!(flag(&mut args, "--prompt"));
        assert!(!flag(&mut args, "--prompt"));
        assert_eq!(args, ["s1", "make"]);
        assert_eq!(take(&mut args, "--keys"), None);
    }
}
