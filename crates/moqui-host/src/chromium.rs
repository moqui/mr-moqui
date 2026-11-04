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

//! Finding and starting Chromium, for the PDF engine ([`crate::pdf`]) and the browser of `moqui test-client`
//! (`test_client/browser.rs`): the places a binary may be, the launch arguments both use, and the DevTools address the
//! browser writes once it listens. Not vendored: `./moq chromium-download` puts one in `runtime/chromium`.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;

/// The property (or environment variable, or `runtime/.env` entry) that names the Chromium for PDF and the browser
/// tests.
pub const CHROMIUM_PATH: &str = "moqui_chromium_path";

/// The browser `moqui_chromium_path` names, or empty.
#[must_use]
pub fn configured_chromium() -> String {
    moqui_lib::env::get_prop_or_env(CHROMIUM_PATH).trim().to_string()
}

/// A Chromium or Chrome binary found, and where it was found.
struct Found {
    place: String,
    path: PathBuf,
}

/// The names looked for on the `PATH`, Chromium first, then Google Chrome and Microsoft Edge.
const PATH_NAMES: [&str; 6] = [
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
    "microsoft-edge",
    "microsoft-edge-stable",
];

/// The places a package or installer puts the browser on this OS, Chromium first, then Google Chrome and Microsoft
/// Edge: the distro and snap paths on Linux (which a service's short `PATH` may not reach), `/Applications` and
/// `~/Applications` on macOS, and Program Files and the per-user `LOCALAPPDATA` on Windows.
fn common_locations() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        let apps = [
            "Chromium.app/Contents/MacOS/Chromium",
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ];
        let mut dirs = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Applications"));
        }
        apps.iter()
            .flat_map(|app| dirs.iter().map(move |dir| dir.join(app)))
            .collect()
    } else if cfg!(windows) {
        let apps = [
            r"Chromium\Application\chrome.exe",
            r"Google\Chrome\Application\chrome.exe",
            r"Microsoft\Edge\Application\msedge.exe",
        ];
        let dirs: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|name| std::env::var_os(name).map(PathBuf::from))
            .collect();
        apps.iter()
            .flat_map(|app| dirs.iter().map(move |dir| dir.join(app)))
            .collect()
    } else {
        [
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
            "/usr/lib/chromium/chromium",
            "/usr/lib64/chromium-browser/chromium-browser",
            "/usr/local/bin/chromium",
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/opt/google/chrome/chrome",
            "/usr/bin/microsoft-edge",
            "/opt/microsoft/msedge/msedge",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }
}

/// Every Chromium and Chrome there is, in order: `configured` when it names a file (`configured_name` says where it
/// came from, for the warning when it does not), the one of [`downloaded_chromium`], then [`PATH_NAMES`] on the
/// `PATH` and the [`common_locations`] of this OS, Chromium before Google Chrome and Microsoft Edge. One binary under
/// several names (chromium-browser a link to chromium, /usr/bin/google-chrome to /opt/google/chrome) is there once.
fn found_binaries(runtime_dir: Option<&Path>, configured: &str, configured_name: &str, what: &str) -> Vec<Found> {
    let mut found = Vec::new();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        if path.is_file() {
            found.push(Found {
                place: configured_name.to_string(),
                path,
            });
        } else {
            tracing::warn!("Chromium for {what}: {configured_name} names {configured}, which is not a file");
        }
    }
    if let Some(runtime_dir) = runtime_dir {
        if let Some(path) = downloaded_chromium(runtime_dir) {
            let place = match std::fs::read_to_string(runtime_dir.join("chromium/REVISION")) {
                Ok(text) => format!(
                    "runtime/chromium (snapshot {})",
                    text.split_whitespace().next().unwrap_or("?")
                ),
                Err(_) => "runtime/chromium".to_string(),
            };
            found.push(Found { place, path });
        }
    }
    let search = std::env::var_os("PATH").unwrap_or_default();
    let mut installed = Vec::new();
    for name in PATH_NAMES {
        let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
        if let Some(path) = std::env::split_paths(&search)
            .map(|dir| dir.join(&file))
            .find(|path| path.is_file())
        {
            installed.push((is_chromium(&path), format!("{name} on the PATH"), path));
        }
    }
    for path in common_locations() {
        if path.is_file() {
            installed.push((is_chromium(&path), "a common install location".to_string(), path));
        }
    }
    // Chromium before Google Chrome and Edge; a stable sort keeps the PATH before the common locations.
    installed.sort_by_key(|(chromium, _, _)| !chromium);
    found.extend(installed.into_iter().map(|(_, place, path)| Found { place, path }));
    let mut seen = Vec::new();
    found.retain(|one| {
        let real = std::fs::canonicalize(&one.path).unwrap_or_else(|_| one.path.clone());
        let new = !seen.contains(&real);
        seen.push(real);
        new
    });
    found
}

/// Whether a binary is Chromium itself rather than Google Chrome or Microsoft Edge, from its path.
fn is_chromium(path: &Path) -> bool {
    let text = path.to_string_lossy().to_ascii_lowercase();
    text.contains("chromium")
}

/// Look at every place a Chromium may be, log each one found, and choose one for PDF: `configured` (the `pdf`
/// `chromium-path`, `moqui_chromium_path`) when it names a file, else the first of [`found_binaries`]: the one of
/// [`downloaded_chromium`], then an installed Chromium, then an installed Google Chrome or Microsoft Edge.
///
/// # Errors
///
/// Returns the reason PDF is not available, already logged, when no Chromium is chosen.
pub fn find_chromium(runtime_dir: &Path, configured: &str) -> Result<PathBuf, String> {
    let configured = configured.trim();
    let found = found_binaries(Some(runtime_dir), configured, CHROMIUM_PATH, "PDF");
    for one in &found {
        tracing::info!("Chromium for PDF: found {} at {}", one.place, one.path.display());
    }
    if let Some(chosen) = found.first() {
        tracing::info!(
            "Chromium for PDF: using {} from {}",
            chosen.path.display(),
            chosen.place
        );
        if chosen.place.starts_with("runtime/chromium") {
            check_apparmor_profile(&chosen.path);
        }
        return Ok(chosen.path.clone());
    }
    let reason = if configured.is_empty() {
        moqui_core::screen::PDF_UNAVAILABLE.to_string()
    } else {
        format!("PDF is not available: {CHROMIUM_PATH} names {configured}, which is not a file, and no other Chromium was found")
    };
    tracing::warn!("Chromium for PDF: {reason}");
    Err(reason)
}

/// The browser of `moqui test-client`: `configured` (`--chromium`) when it names a file, else `moqui_chromium_path`
/// when that does, else the first of [`found_binaries`], as for PDF.
///
/// # Errors
///
/// Returns why there is none, naming the places looked at.
pub fn find_test_browser(runtime_dir: Option<&Path>, configured: &str) -> Result<PathBuf, String> {
    let mut configured = configured.trim().to_string();
    let mut name = "--chromium";
    if configured.is_empty() {
        configured = configured_chromium();
        name = CHROMIUM_PATH;
    }
    if !configured.is_empty() && !Path::new(&configured).is_file() {
        return Err(format!("{name} names {configured}, which is not a file"));
    }
    let found = found_binaries(runtime_dir, &configured, name, "the browser tests");
    match found.first() {
        Some(one) => {
            tracing::debug!(
                "Chromium for the browser tests: {} from {}",
                one.path.display(),
                one.place
            );
            if one.place.starts_with("runtime/chromium") {
                check_apparmor_profile(&one.path);
            }
            Ok(one.path.clone())
        }
        None => Err(format!(
            "no Chromium: pass --chromium or set {CHROMIUM_PATH}, run ./moq chromium-download, or install Chromium, \
             Google Chrome or Microsoft Edge"
        )),
    }
}

/// The Chromium that `./moq chromium-download` put in `runtime/chromium`.
#[must_use]
pub fn downloaded_chromium(runtime_dir: &Path) -> Option<PathBuf> {
    ["chrome", "Chromium.app/Contents/MacOS/Chromium"]
        .iter()
        .map(|name| runtime_dir.join("chromium").join(name))
        .find(|candidate| candidate.is_file())
}

/// Whether AppArmor limits unprivileged user namespaces (Ubuntu 23.10 and later), which keeps a Chromium that has no
/// AppArmor profile from starting its sandbox.
#[must_use]
pub fn userns_restricted() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        .is_ok_and(|text| text.trim() == "1")
}

/// Warn when the downloaded Chromium needs the `moqui-chromium` AppArmor profile that `./moq chromium-download`
/// describes and it is not there. Distro and Chrome packages install their own profiles.
fn check_apparmor_profile(binary: &Path) {
    if !userns_restricted() {
        return;
    }
    let real = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    let named = std::fs::read_to_string("/etc/apparmor.d/moqui-chromium")
        .is_ok_and(|text| text.contains(&format!("\"{}\"", real.display())));
    if !named {
        tracing::warn!(
            "Chromium: AppArmor limits unprivileged user namespaces here and no moqui-chromium profile names {}, \
             so it cannot start its sandbox and PDF and the browser tests will fail; ./moq help chromium-download shows \
             how to add one",
            real.display()
        );
    }
}

/// The browser's arguments, apart from the profile and the window: headless, DevTools on an ephemeral loopback port,
/// and nothing that phones home. The sandbox stays on. The PDF engine adds its proxy arguments, which keep a document
/// off the network.
#[must_use]
pub fn launch_args() -> [&'static str; 13] {
    [
        "--headless=new",
        "--remote-debugging-address=127.0.0.1",
        "--remote-debugging-port=0",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-gpu",
        "--disable-extensions",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-sync",
        "--metrics-recording-only",
        "--password-store=basic",
        "--use-mock-keychain",
    ]
}

/// The browser socket from `DevToolsActivePort` in the profile, which Chromium writes once it listens.
///
/// # Errors
///
/// Returns an error when the browser exits first or does not listen within `wait`.
pub fn devtools_url(profile: &Path, child: &mut Child, wait: Duration) -> Result<String, Error> {
    let file = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + wait;
    loop {
        if let Ok(text) = std::fs::read_to_string(&file) {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                return Ok(format!("ws://127.0.0.1:{}{}", port.trim(), path.trim()));
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(Error::new(format!("Chromium exited before it listened: {status}")));
        }
        if Instant::now() >= deadline {
            return Err(Error::new(format!("Chromium did not write {}", file.display())));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_browser_that_is_no_file_is_an_error_and_one_that_is_comes_first() {
        let err = find_test_browser(None, "/nowhere/chromium").unwrap_err();
        assert!(err.contains("names /nowhere/chromium, which is not a file"), "{err}");
        let dir = std::env::temp_dir().join(format!("moqui-chromium-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("my-chromium");
        std::fs::write(&fake, "").unwrap();
        assert_eq!(find_test_browser(Some(&dir), fake.to_str().unwrap()).unwrap(), fake);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
