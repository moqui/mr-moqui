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

//! Downloads the host saves itself (plans/Mobile-Desktop.md §4, "One transport seam"). An app scheme's answer is
//! whole (Tauri's responder takes a body, which CEF reads from memory), and a download Chromium takes from a scheme is
//! asked for again, so an attachment or a large file the window opens would be held in memory twice. Instead the
//! scheme answers the window 204, which leaves its page as it is, and the host asks where to save with the save dialog,
//! streams the body to a `.part` file beside it, renames it when done, and says so in a notification.
//!
//! A page's own fetch of a file (one it saves as a blob) is answered whole as before; so is a small file a page links
//! to without an attachment disposition.

use std::io::Write;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// A document answer with no attachment disposition is saved by the host past this size (`Content-Length`).
pub(crate) const STREAM_OVER: u64 = 16 * 1024 * 1024;

/// The name to save an answer as when the host saves it: a window's (or frame's) request whose answer is an
/// attachment, or larger than [`STREAM_OVER`] and not a page. `None` for an answer the window takes itself.
pub(crate) fn saved_by_host(document: bool, status: u16, headers: &[(String, String)], path: &str) -> Option<String> {
    if !document || status != 200 {
        return None;
    }
    let header = |name: &str| moqui_client::proxy::header(headers, name);
    if let Some(name) = header("content-disposition").and_then(moqui_client::proxy::attachment_name) {
        return Some(name);
    }
    let length = header("content-length").and_then(|text| text.trim().parse::<u64>().ok())?;
    let page = header("content-type").is_some_and(|kind| kind.to_ascii_lowercase().starts_with("text/html"));
    if length <= STREAM_OVER || page {
        return None;
    }
    let bare = path.split_once('?').map_or(path, |(bare, _)| bare);
    let last = bare.rsplit('/').next().unwrap_or("");
    let last = last.replace('"', "");
    moqui_client::proxy::attachment_name(&format!("attachment; filename=\"{last}\""))
}

/// The file a download is written to until it is whole: `<name>.part` beside it.
pub(crate) fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// Write a download to `path` through its `.part` file: `copy` writes the body. The `.part` file is renamed when the
/// body is whole and removed when it is not.
///
/// # Errors
///
/// Returns the writer's, `copy`'s or the rename's error.
pub(crate) fn write_to(path: &Path, copy: impl FnOnce(&mut dyn Write) -> Result<u64, String>) -> Result<u64, String> {
    let part = part_path(path);
    let written = std::fs::File::create(&part)
        .map_err(|err| format!("{}: {err}", part.display()))
        .and_then(|file| {
            let mut out = std::io::BufWriter::new(file);
            let written = copy(&mut out)?;
            out.flush().map_err(|err| format!("{}: {err}", part.display()))?;
            Ok(written)
        })
        .and_then(|written| {
            std::fs::rename(&part, path)
                .map(|()| written)
                .map_err(|err| format!("{}: {err}", path.display()))
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// Ask where to save `name` (the save dialog, from the Downloads folder), then stream it there with `copy` and say
/// when it is done, on a thread of its own: the scheme's worker answers the window at once. Cancelling the dialog
/// drops the body unread.
pub(crate) fn save(
    app: &AppHandle,
    name: String,
    copy: impl FnOnce(&mut dyn Write) -> Result<u64, String> + Send + 'static,
) {
    let app = app.clone();
    let started = std::thread::Builder::new().name("download".to_string()).spawn(move || {
        let Some(path) = choose(&app, &name) else {
            return;
        };
        match write_to(&path, copy) {
            Ok(_) => notify(&app, &format!("Saved {name}"), &path.display().to_string()),
            Err(err) => {
                tracing::warn!("The download of {name} failed: {err}");
                notify(&app, &format!("{name} was not saved"), &err);
            }
        }
    });
    if let Err(err) = started {
        tracing::error!("The download of a file did not start: {err}");
    }
}

/// The save dialog for `name`, shown on the main thread; waits for the answer.
fn choose(app: &AppHandle, name: &str) -> Option<PathBuf> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let name = name.to_string();
    let shown = app.run_on_main_thread(move || {
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title("Save")
            .set_file_name(&name)
            .set_can_create_directories(true);
        if let Some(folder) = dirs_downloads() {
            dialog = dialog.set_directory(folder);
        }
        let answer = dialog.save_file();
        std::thread::spawn(move || {
            let _ = sender.send(tauri::async_runtime::block_on(answer).map(|file| file.path().to_path_buf()));
        });
    });
    if let Err(err) = shown {
        tracing::error!("Could not show the save dialog: {err}");
        return None;
    }
    receiver.recv().ok().flatten()
}

/// The user's Downloads folder, when there is one.
fn dirs_downloads() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").filter(|home| !home.is_empty())?);
    Some(home.join("Downloads")).filter(|folder| folder.is_dir())
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    let shown = app
        .notification()
        .builder()
        .title(crate::bridge::clip(title, 200))
        .body(crate::bridge::clip(body, 1000))
        .show();
    if let Err(err) = shown {
        tracing::warn!("{title}, no notification: {err}");
    }
}

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;
