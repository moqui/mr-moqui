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

//! Start at login, a tray toggle that is off until the user turns it on (plans/Mobile-Desktop.md §4 configuration 1):
//! a LaunchAgent in `~/Library/LaunchAgents` on macOS, an XDG autostart entry in `~/.config/autostart` on Linux. The
//! file is the setting: it is there or not. tauri-plugin-autostart is not used: its alpha is unverified with the CEF
//! runtime, and these two files are all it would write here.

use std::path::PathBuf;

use super::entries;

/// Whether this OS has a start at login the app can set.
pub(crate) const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// The file that starts the app at login on this OS.
fn path() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").filter(|home| !home.is_empty())?);
    if cfg!(target_os = "macos") {
        Some(entries::launch_agent_path(&home))
    } else if cfg!(target_os = "linux") {
        let xdg = std::env::var("XDG_CONFIG_HOME").ok();
        Some(entries::autostart_path(&entries::config_home(xdg.as_deref(), &home)))
    } else {
        None
    }
}

/// The file's contents for this app as it runs now.
fn contents() -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    let program = entries::program(&exe, std::env::var("APPIMAGE").ok().as_deref());
    Ok(if cfg!(target_os = "macos") {
        entries::launch_agent(&program)
    } else {
        entries::autostart_entry(&program)
    })
}

/// Whether the app starts at login.
pub(crate) fn enabled() -> bool {
    path().is_some_and(|path| path.exists())
}

/// Turn start at login on or off.
pub(crate) fn set_enabled(on: bool) -> std::io::Result<()> {
    let Some(path) = path() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no start at login on this OS",
        ));
    };
    if on {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, contents()?)?;
        tracing::info!("Start at login on: {}", path.display());
    } else {
        match std::fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => tracing::info!("Start at login off: removed {}", path.display()),
        }
    }
    Ok(())
}

/// When start at login is on, point it at this app as it runs now: the app or the AppImage may have moved, or been
/// replaced by an upgrade at another path.
pub(crate) fn refresh() {
    let Some(path) = path().filter(|path| path.exists()) else {
        return;
    };
    let Ok(now) = contents() else {
        return;
    };
    if std::fs::read_to_string(&path).ok().as_deref() != Some(now.as_str()) {
        match std::fs::write(&path, now) {
            Ok(()) => tracing::info!("Start at login: {} now runs this app", path.display()),
            Err(err) => tracing::warn!("Start at login: could not update {}: {err}", path.display()),
        }
    }
}
