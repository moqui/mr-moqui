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

//! Chromium's process sandbox in a Linux AppImage (plans/archive/Security-Review.md SEC-CLI-103). An AppImage cannot carry the
//! setuid `chrome-sandbox` helper (its payload is mounted `nosuid`), so where the system restricts unprivileged user
//! namespaces (AppArmor, Ubuntu 23.10 and later) or turns them off, `tauri-runtime-cef` starts Chromium with
//! `--no-sandbox` (`SandboxPolicy::Auto`). The owner's decision: the app runs as it does, and tells the user once, with
//! the `.deb`, which keeps the sandbox, as the way to have it. The decision here is the runtime's
//! (`sandbox.rs` `sandbox_decision` and `sandbox_helper_available`), read again, as the runtime does not export it.

use std::path::Path;

use tauri::{AppHandle, Manager};

/// The file in the app's config directory that says the user was told.
const TOLD_FILE: &str = "sandbox-warned";

/// Whether Chromium runs without its sandbox: from an AppImage, with no usable helper named by `CHROME_DEVEL_SANDBOX`,
/// where `/proc/sys/kernel/apparmor_restrict_unprivileged_userns` is 1 or `/proc/sys/user/max_user_namespaces` is 0
/// (none, a file that cannot be read).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn unsandboxed(appimage: bool, helper_usable: bool, restrict: Option<u64>, max_userns: Option<u64>) -> bool {
    appimage && !helper_usable && (restrict == Some(1) || max_userns == Some(0))
}

/// [`unsandboxed`] for this run.
#[cfg(target_os = "linux")]
fn this_run() -> bool {
    use std::os::unix::fs::MetadataExt;
    let sysctl = |path: &str| -> Option<u64> { std::fs::read_to_string(path).ok()?.trim().parse().ok() };
    let appimage = std::env::var_os("APPIMAGE").is_some_and(|path| !path.is_empty());
    let helper_usable = std::env::var_os("CHROME_DEVEL_SANDBOX")
        .filter(|path| !path.is_empty())
        .and_then(|path| std::fs::metadata(path).ok())
        .is_some_and(|helper| helper.uid() == 0 && helper.mode() & 0o4000 != 0 && helper.mode() & 0o0001 != 0);
    unsandboxed(
        appimage,
        helper_usable,
        sysctl("/proc/sys/kernel/apparmor_restrict_unprivileged_userns"),
        sysctl("/proc/sys/user/max_user_namespaces"),
    )
}

#[cfg(not(target_os = "linux"))]
fn this_run() -> bool {
    false
}

/// Whether the user is yet to be told, in `dir` (the app's config directory); it then notes that they are.
pub(crate) fn first_time(dir: &Path) -> bool {
    let told = dir.join(TOLD_FILE);
    if told.exists() {
        return false;
    }
    if let Err(err) = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&told, "1\n")) {
        tracing::warn!("{}: not written, so the sandbox note shows again: {err}", told.display());
    }
    true
}

/// What the user is told.
pub(crate) const NOTE: &str = "This system restricts the user namespaces Chromium's sandbox needs, and an AppImage cannot \
     carry the sandbox's helper, so MR Moqui's window runs here without Chromium's process sandbox: a flaw in a page it \
     shows would reach your files. The .deb package keeps the sandbox. This note shows once.";

/// Tell the user once, when this run's Chromium has no sandbox, in a dialog of the OS; logged every run.
pub(crate) fn warn_once(app: &AppHandle) {
    if !this_run() {
        return;
    }
    tracing::warn!("Chromium runs without its process sandbox (an AppImage where user namespaces are restricted)");
    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    if !first_time(&dir) {
        return;
    }
    std::thread::spawn(|| {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("MR Moqui runs without Chromium's sandbox here")
            .set_description(NOTE)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    });
}

#[cfg(test)]
mod tests {
    use super::{first_time, unsandboxed};

    /// The runtime's decision for an AppImage: no sandbox only with no usable helper and user namespaces restricted or
    /// off; a package that is not an AppImage keeps it.
    #[test]
    fn only_an_appimage_where_user_namespaces_are_restricted_runs_unsandboxed() {
        assert!(unsandboxed(true, false, Some(1), None));
        assert!(unsandboxed(true, false, None, Some(0)));
        assert!(unsandboxed(true, false, Some(1), Some(65536)));
        assert!(!unsandboxed(true, false, Some(0), Some(65536)));
        assert!(!unsandboxed(true, false, None, None), "no evidence of a restriction is none");
        assert!(!unsandboxed(true, true, Some(1), None), "a helper named by CHROME_DEVEL_SANDBOX keeps it");
        assert!(!unsandboxed(false, false, Some(1), Some(0)), "a .deb has the setuid helper");
    }

    /// The note shows once a config directory.
    #[test]
    fn the_note_shows_once() {
        let dir = std::env::temp_dir().join(format!("moqui-desktop-sandbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(first_time(&dir));
        assert!(!first_time(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
