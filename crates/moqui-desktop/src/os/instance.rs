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

//! One instance per user (plans/Mobile-Desktop.md §4 configuration 1). Chromium's process singleton already keeps a
//! second app from running, but tauri-runtime-cef passes only a deep link on to the first and drops a plain second
//! launch (`on_already_running_app_relaunch`, a TODO there), so a second launch would do nothing. So the first
//! instance listens on a socket in the app config directory, before Tauri and CEF start; a later launch sends it its
//! links there and exits, and the first shows its window and opens them.
//!
//! Only the browser process gets here: `cef_entry_point` in main.rs sends CEF's subprocesses to CEF first. Two
//! launches at the same instant can both become first; Chromium's singleton then takes the second.

#[cfg(unix)]
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::time::Duration;

/// The first line of a launch's message, so a stray connection is not taken for one.
const HEADER: &str = "moqui-desktop launch 1";
/// The most a message may be: a few links.
#[cfg(unix)]
const MAX_MESSAGE: u64 = 64 * 1024;
#[cfg(unix)]
const TIMEOUT: Duration = Duration::from_secs(3);

/// The message a later launch sends: the header, then its links, one a line (a serialized URL has no line break).
pub(crate) fn encode(links: &[tauri::Url]) -> String {
    let mut text = format!("{HEADER}\n");
    for link in links {
        text.push_str(link.as_str());
        text.push('\n');
    }
    text
}

/// The links of a launch's message, or `None` when it is not one.
pub(crate) fn decode(text: &str) -> Option<Vec<tauri::Url>> {
    let mut lines = text.lines();
    if lines.next()? != HEADER {
        return None;
    }
    Some(super::links::from_args(lines.map(String::from)))
}

/// What became of this launch.
pub(crate) enum Claim {
    /// This is the first instance; it listens for later ones.
    First(Listener),
    /// Another instance took this launch: this one exits.
    Forwarded,
    /// No socket (another OS, or it could not be made): run, and leave a second launch to Chromium's singleton.
    Alone,
}

/// The socket of the first instance.
pub(crate) struct Listener {
    path: PathBuf,
    #[cfg(unix)]
    listener: std::os::unix::net::UnixListener,
}

impl Listener {
    /// Take later launches on a thread of its own: each gets an answer at once, then `on_launch` with its links.
    pub(crate) fn serve(self, on_launch: impl Fn(Vec<tauri::Url>) + Send + 'static) -> PathBuf {
        #[cfg(unix)]
        {
            let listener = self.listener;
            let spawned = std::thread::Builder::new()
                .name("single-instance".into())
                .spawn(move || {
                    for stream in listener.incoming() {
                        match stream {
                            Ok(stream) => {
                                if let Some(links) = answer(stream) {
                                    on_launch(links);
                                }
                            }
                            Err(err) => tracing::warn!("Single instance: a later launch did not connect: {err}"),
                        }
                    }
                });
            if let Err(err) = spawned {
                tracing::error!("Single instance: could not start its thread: {err}");
            }
        }
        #[cfg(not(unix))]
        let _ = on_launch;
        self.path
    }
}

#[cfg(unix)]
fn answer(mut stream: std::os::unix::net::UnixStream) -> Option<Vec<tauri::Url>> {
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let mut text = String::new();
    if let Err(err) = (&mut stream).take(MAX_MESSAGE).read_to_string(&mut text) {
        tracing::warn!("Single instance: could not read a later launch: {err}");
        return None;
    }
    let links = decode(&text)?;
    let _ = stream.write_all(b"ok\n");
    Some(links)
}

/// Claim the socket at `path`, or hand `links` to the instance that has it.
#[cfg(unix)]
pub(crate) fn claim(path: &Path, links: &[tauri::Url]) -> Claim {
    use std::os::unix::net::UnixListener;
    if path.exists() {
        match forward(path, links) {
            Ok(true) => return Claim::Forwarded,
            Ok(false) => tracing::warn!(
                "Single instance: {} did not answer as the app; taking it",
                path.display()
            ),
            // ECONNREFUSED: the instance that made it is gone
            Err(err) => tracing::info!("Single instance: {} is stale ({err}); taking it", path.display()),
        }
        let _ = std::fs::remove_file(path);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match UnixListener::bind(path) {
        Ok(listener) => {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            Claim::First(Listener {
                path: path.to_path_buf(),
                listener,
            })
        }
        Err(err) => {
            tracing::warn!("Single instance: could not listen on {}: {err}", path.display());
            Claim::Alone
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn claim(_path: &Path, _links: &[tauri::Url]) -> Claim {
    Claim::Alone
}

/// Send `links` to the instance on `path`; whether it answered.
#[cfg(unix)]
fn forward(path: &Path, links: &[tauri::Url]) -> std::io::Result<bool> {
    let mut stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(encode(links).as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    stream.take(64).read_to_string(&mut reply)?;
    Ok(reply.trim() == "ok")
}

/// Remove the socket as the app quits, so a restart's new process, started before this one ends, is the first.
pub(crate) fn release(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
#[path = "instance_tests.rs"]
mod tests;
