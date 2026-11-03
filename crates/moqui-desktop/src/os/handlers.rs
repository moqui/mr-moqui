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

//! "Make default for email and calls": the app as the OS's handler of `mailto:`, `tel:` and `sip:` links
//! (plans/Mobile-Desktop.md §4 configuration 1). The package registers the app as one handler of them
//! (`plugins.deep-link.desktop.schemes` in tauri.conf.json); this makes it the default. On macOS that is
//! LaunchServices' `LSSetDefaultHandlerForURLScheme`, which works only for the installed app (a development run has
//! no bundle for LaunchServices to find) and may ask the user to confirm. On Linux it is `xdg-mime default`, with the
//! package's `.desktop` file, or, from an AppImage or a development run, one the app writes in
//! `~/.local/share/applications`.

/// Whether this OS has a default handler the app can set.
pub(crate) const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// Make the app the default handler of the schemes. The error says what did not work.
#[cfg(target_os = "macos")]
pub(crate) fn make_default() -> Result<(), String> {
    use std::ffi::c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithBytes(
            alloc: *const c_void,
            bytes: *const u8,
            length: isize,
            encoding: u32,
            is_external: u8,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }
    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn LSSetDefaultHandlerForURLScheme(scheme: *const c_void, bundle_id: *const c_void) -> i32;
    }
    const UTF8: u32 = 0x0800_0100;

    let string = |text: &str| -> *const c_void {
        // SAFETY: the bytes are valid for `len` and copied into the new CFString, which the caller releases.
        unsafe { CFStringCreateWithBytes(std::ptr::null(), text.as_ptr(), text.len() as isize, UTF8, 0) }
    };
    let bundle = string(super::IDENTIFIER);
    if bundle.is_null() {
        return Err("could not make a CFString".into());
    }
    let mut failed = Vec::new();
    for scheme in super::links::SCHEMES {
        let name = string(scheme);
        if name.is_null() {
            failed.push(format!("{scheme}: could not make a CFString"));
            continue;
        }
        // SAFETY: both are live CFStrings; LaunchServices does not keep them past the call.
        let status = unsafe { LSSetDefaultHandlerForURLScheme(name, bundle) };
        // SAFETY: made above by CFStringCreateWithBytes and not used after.
        unsafe { CFRelease(name) };
        if status != 0 {
            failed.push(format!("{scheme}: LaunchServices error {status}"));
        }
    }
    // SAFETY: made above and not used after.
    unsafe { CFRelease(bundle) };
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn make_default() -> Result<(), String> {
    use super::entries;
    use std::process::Command;

    let file = desktop_entry()?;
    let status = Command::new("xdg-mime")
        .arg("default")
        .arg(&file)
        .args(entries::scheme_mime_types())
        .status()
        .map_err(|err| format!("xdg-mime did not run: {err}"))?;
    if status.success() {
        tracing::info!("Default handler of mailto:, tel: and sip: is now {file}");
        Ok(())
    } else {
        Err(format!("xdg-mime default {file} ended with {status}"))
    }
}

/// The app's own scheme (`org.moqui.desktop:`, an SSO sign-in's hand-back) handled by this app: the installed package's
/// entry says so already; an AppImage or a development run writes the user's handler entry and makes it the scheme's
/// handler, which no other app claims. macOS has it from the app's bundle.
#[cfg(target_os = "linux")]
pub(crate) fn claim_app_scheme() -> Result<(), String> {
    use std::process::Command;

    let file = desktop_entry()?;
    if file != super::entries::handler_file_name() {
        return Ok(());
    }
    let mime = format!("x-scheme-handler/{}", super::links::APP_SCHEME);
    let status = Command::new("xdg-mime")
        .arg("default")
        .arg(&file)
        .arg(&mime)
        .status()
        .map_err(|err| format!("xdg-mime did not run: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("xdg-mime default {file} {mime} ended with {status}"))
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn claim_app_scheme() -> Result<(), String> {
    Ok(())
}

/// The desktop entry that runs this app with a link: the installed package's, else the handler entry it writes for
/// the user (an AppImage, a development run).
#[cfg(target_os = "linux")]
fn desktop_entry() -> Result<String, String> {
    use super::entries;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    let exe = std::env::current_exe().map_err(|err| format!("no executable path: {err}"))?;
    let appimage = std::env::var("APPIMAGE").ok().filter(|path| !path.trim().is_empty());
    // the .deb's own entry, `<productName>.desktop`, which runs the installed binary with `%u`
    let package = format!("{}.desktop", entries::PRODUCT_NAME);
    let installed =
        appimage.is_none() && exe.starts_with("/usr") && Path::new("/usr/share/applications").join(&package).exists();
    let file = if installed {
        package
    } else {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or("HOME is not set")?;
        let xdg = std::env::var("XDG_DATA_HOME").ok();
        let path = entries::handler_path(&entries::data_home(xdg.as_deref(), &PathBuf::from(home)));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        }
        let program = entries::program(&exe, appimage.as_deref());
        std::fs::write(&path, entries::handler_entry(&program)).map_err(|err| format!("{}: {err}", path.display()))?;
        if let Some(dir) = path.parent() {
            // the MIME cache of the user's entries; xdg-mime does not need it, some desktops do
            let _ = Command::new("update-desktop-database").arg(dir).status();
        }
        entries::handler_file_name()
    };
    Ok(file)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn make_default() -> Result<(), String> {
    Err("not available on this OS".into())
}
