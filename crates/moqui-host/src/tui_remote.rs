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

//! `moqui tui --server <url>` (plans/archive/Screen-TUI-Impl.md 4.3): the terminal client of a running server, over HTTP. A
//! level is `GET <path>?renderMode=tui`, a transition asks for JSON, and every request carries the `login_key` header.
//! The login posts the password once to the REST `login` with `createLoginKey=true` and a `deviceName`, so the key is in
//! the user's device list where it can be revoked (`moqui_lib::remote_login`, shared with the desktop client), and
//! keeps only the key, in `~/.config/moqui/tui/<host>.toml` (mode 0600); a 401 later, an expired key above all, opens
//! the login dialog again.
//! Plain `http://` is refused except to this machine.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use moqui_core::screen::RenderOutput;
use moqui_lib::error::Error;
use moqui_lib::reconnect::Backoff;
use moqui_lib::remote_login::{form_body, SignIn};
use moqui_lib::sync::lock;
use moqui_tui::{LinkError, PushSink, Subscription};

use crate::tui::{Ask, Link, Transport};

/// The client over HTTP.
pub type HttpLink = Link<Http>;

/// Seconds a request may take before the client says the server did not answer.
const TIMEOUT_SECONDS: u64 = 60;

/// Requests to one server, as the user whose login key it holds.
#[derive(Clone)]
pub struct Http {
    /// `https://host:port`, no trailing slash.
    base: String,
    client: reqwest::blocking::Client,
    login: Arc<Mutex<Option<Stored>>>,
    /// The key file, or none to keep the key in memory only.
    store: Option<PathBuf>,
}

/// What the key file holds.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stored {
    user: String,
    key: String,
}

impl HttpLink {
    /// A link to `server`, with the key stored for it when there is one.
    ///
    /// # Errors
    ///
    /// Returns an error for a url that does not parse, a scheme other than http(s), or `http://` to another machine.
    pub fn connect(server: &str, store: Option<PathBuf>) -> Result<Self, Error> {
        Ok(Link::over(Http::new(base_url(server)?, store)?))
    }
}

impl Http {
    /// Requests to `base` (checked by [`base_url`]), with the key in `store` when there is one.
    fn new(base: String, store: Option<PathBuf>) -> Result<Self, Error> {
        let client = reqwest::blocking::Client::builder()
            // Session attributes (including one-time values across redirects) belong to this live connection.
            // The jar stays in memory; only the existing device login key is written to the key file.
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECONDS))
            .user_agent(concat!("moqui-tui/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| Error::new(format!("HTTP client: {err}")))?;
        let login = store.as_deref().and_then(read_stored);
        Ok(Http {
            base,
            client,
            login: Arc::new(Mutex::new(login)),
            store,
        })
    }

    fn stored(&self) -> Option<Stored> {
        lock(&self.login).clone()
    }
}

/// `https://host:port` from what the user typed, refusing plain http to anything but loopback.
fn base_url(server: &str) -> Result<String, Error> {
    let url = reqwest::Url::parse(server.trim()).map_err(|err| Error::new(format!("--server {server}: {err}")))?;
    let host = url.host_str().unwrap_or("");
    match url.scheme() {
        "https" => {}
        "http" if matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1") => {}
        "http" => {
            return Err(Error::new(format!(
                "--server {server}: plain http sends the login over the network; use https, or http to localhost"
            )))
        }
        other => return Err(Error::new(format!("--server {server}: {other} is not http or https"))),
    }
    let mut base = format!("{}://{host}", url.scheme());
    if let Some(port) = url.port() {
        base.push_str(&format!(":{port}"));
    }
    base.push_str(url.path().trim_end_matches('/'));
    Ok(base)
}

/// The key file of `server`: `~/.config/moqui/tui/<host>[_<port>].toml` (or under `XDG_CONFIG_HOME`).
///
/// # Errors
///
/// Returns an error when the url does not parse or no home directory is known.
pub fn key_file(server: &str) -> Result<PathBuf, Error> {
    let url = reqwest::Url::parse(server.trim()).map_err(|err| Error::new(format!("--server {server}: {err}")))?;
    let mut name: String = url
        .host_str()
        .unwrap_or("server")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if let Some(port) = url.port() {
        name.push_str(&format!("_{port}"));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| Error::new("no HOME to keep the login key in"))?;
    Ok(config.join("moqui/tui").join(format!("{name}.toml")))
}

fn read_stored(path: &std::path::Path) -> Option<Stored> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = |name: &str| {
        text.lines()
            .filter_map(|line| line.split_once('='))
            .find(|(key, _)| key.trim() == name)
            .map(|(_, value)| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    };
    Some(Stored {
        user: value("user")?,
        key: value("loginKey")?,
    })
}

/// The login key stored in a key file, for the dev tools' `--server` (crate::devtools_remote).
pub(crate) fn stored_key(path: &std::path::Path) -> Option<String> {
    read_stored(path).map(|stored| stored.key)
}

/// Write the key file, readable by its owner only. A failure is logged: the key still serves this run.
fn write_stored(path: &std::path::Path, stored: &Stored) {
    let text = format!(
        "# The login key of moqui tui --server; delete this file, or run with --forget, to log in again.\nuser = \"{}\"\nloginKey = \"{}\"\n",
        stored.user.replace(['"', '\\', '\n'], ""),
        stored.key
    );
    let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        std::io::Write::write_all(&mut file, text.as_bytes())
    });
    if let Err(err) = written {
        tracing::error!("TUI login key not stored in {}: {err}", path.display());
    }
}

impl Transport for Http {
    fn send(&self, ask: Ask) -> Result<RenderOutput, LinkError> {
        let url = format!("{}{}", self.base, ask.path);
        let mut parameters: Vec<(String, String)> = ask.parameters.into_iter().collect();
        if ask.tui {
            parameters.push(("renderMode".to_string(), "tui".to_string()));
        }
        let mut request = if ask.method.eq_ignore_ascii_case("GET") {
            if parameters.is_empty() {
                self.client.get(&url)
            } else {
                self.client.get(format!("{url}?{}", form_body(&parameters)))
            }
        } else {
            let method = reqwest::Method::from_bytes(ask.method.to_ascii_uppercase().as_bytes())
                .map_err(|err| LinkError::Remote(format!("{}: {err}", ask.method)))?;
            self.client
                .request(method, &url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(form_body(&parameters))
        };
        if ask.json {
            request = request.header("Accept", "application/json");
        }
        if let Some(stored) = self.stored() {
            request = request.header("login_key", stored.key);
        }
        let response = request
            .send()
            .map_err(|err| LinkError::Remote(format!("{url}: {err}")))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_string())))
            .collect();
        let body = response
            .bytes()
            .map_err(|err| LinkError::Remote(format!("{url}: {err}")))?
            .to_vec();
        Ok(RenderOutput { status, headers, body })
    }

    fn login(&self, username: &str, password: &str, code: &str) -> Result<(), LinkError> {
        let device = device_name();
        let signed =
            moqui_lib::remote_login::sign_in(&self.client, &self.base, username, password, code, Some(&device))
                .map_err(|err| LinkError::Remote(err.message().to_string()))?;
        match signed {
            SignIn::Key(key) => {
                let stored = Stored {
                    user: username.to_string(),
                    key,
                };
                if let Some(path) = &self.store {
                    write_stored(path, &stored);
                }
                *lock(&self.login) = Some(stored);
                Ok(())
            }
            SignIn::Code(ask) => Err(LinkError::Code(ask)),
        }
    }

    /// `/rest/login` to pre-authenticate, then `sendOtp` in that session: its cookie and its CSRF token.
    fn send_code(&self, username: &str, password: &str, factor: &str) -> Result<String, LinkError> {
        moqui_lib::remote_login::send_code(&self.client, &self.base, username, password, factor)
            .map_err(|err| LinkError::Remote(err.message().to_string()))?;
        Ok(factor.to_string())
    }

    fn user(&self) -> Option<String> {
        self.stored().map(|stored| stored.user)
    }

    /// `/notws` with the login key, on a thread of its own that connects again after a drop, waiting longer each time
    /// up to a minute, with jitter (`moqui_lib::reconnect`). Before a login it only waits.
    fn subscribe(&self, sink: PushSink) -> Option<Subscription> {
        let stop = Arc::new(AtomicBool::new(false));
        let http = self.clone();
        let stopped = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("tui-push".to_string())
            .spawn(move || http.push_loop(&sink, &stopped))
            .map_err(|err| tracing::warn!("TUI notifications not pushed: {err}"))
            .ok()?;
        Some(Box::new(StopOnDrop(stop)))
    }
}

/// Ends the push thread when the client lets go of its subscription.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// How often a socket read gives up waiting so the push thread can see it was stopped.
const PUSH_POLL: Duration = Duration::from_secs(1);

/// A stream the notification socket reads: plain, or TLS for https.
trait Duplex: std::io::Read + std::io::Write {}

impl<T: std::io::Read + std::io::Write> Duplex for T {}

impl Http {
    fn push_loop(&self, sink: &PushSink, stop: &AtomicBool) {
        let mut backoff = Backoff::new();
        while !stop.load(Ordering::Relaxed) {
            if self.stored().is_some() {
                match self.listen(sink, stop) {
                    Ok(()) => backoff.reset(),
                    Err(err) => tracing::debug!("TUI notification socket: {err}"),
                }
            }
            let until = std::time::Instant::now() + backoff.next_wait(moqui_lib::reconnect::jitter());
            while !stop.load(Ordering::Relaxed) && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    /// One socket until it closes or the subscription drops.
    fn listen(&self, sink: &PushSink, stop: &AtomicBool) -> Result<(), String> {
        let key = self.stored().map(|stored| stored.key).unwrap_or_default();
        let url = reqwest::Url::parse(&self.base).map_err(|err| err.to_string())?;
        let host = url.host_str().unwrap_or("localhost").to_string();
        let https = url.scheme() == "https";
        let port = url.port().unwrap_or(if https { 443 } else { 80 });
        let tcp = std::net::TcpStream::connect((host.as_str(), port)).map_err(|err| err.to_string())?;
        tcp.set_read_timeout(Some(PUSH_POLL)).map_err(|err| err.to_string())?;
        let stream: Box<dyn Duplex + Send> = if https {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let config =
                rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
                    .with_safe_default_protocol_versions()
                    .map_err(|err| err.to_string())?
                    .with_root_certificates(roots)
                    .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|err| err.to_string())?;
            let connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(|err| err.to_string())?;
            Box::new(rustls::StreamOwned::new(connection, tcp))
        } else {
            Box::new(tcp)
        };
        let scheme = if https { "wss" } else { "ws" };
        let uri: tungstenite::http::Uri = format!("{scheme}://{host}:{port}{}/notws", url.path().trim_end_matches('/'))
            .parse()
            .map_err(|err: tungstenite::http::uri::InvalidUri| err.to_string())?;
        let request = tungstenite::ClientRequestBuilder::new(uri).with_header("login_key", key);
        let (mut socket, _) = tungstenite::client(request, stream).map_err(|err| err.to_string())?;
        socket
            .send(tungstenite::Message::Text("subscribe:ALL".into()))
            .map_err(|err| err.to_string())?;
        loop {
            if stop.load(Ordering::Relaxed) {
                let _ = socket.close(None);
                return Ok(());
            }
            match socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    if let Ok(body) = serde_json::from_str::<serde_json::Value>(text.as_str()) {
                        sink(crate::tui::push_of(&body));
                    }
                }
                Ok(tungstenite::Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(err) => return Err(err.to_string()),
            }
        }
    }
}

/// The name of this terminal's key in the user's device list.
fn device_name() -> String {
    format!("moqui tui on {}", crate::logging::host_name())
}

/// Remove the stored key of `server`, for `--forget`. Answers whether there was one.
///
/// # Errors
///
/// Returns an error when the url does not parse or the file cannot be removed.
pub fn forget(server: &str) -> Result<bool, Error> {
    let path = key_file(server)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(Error::new(format!("{}: {err}", path.display()))),
    }
}

#[cfg(test)]
#[path = "tui_remote_tests.rs"]
mod tests;
