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

//! The files the app writes for the OS, and where: the macOS LaunchAgent and the Linux autostart entry that start it
//! at login, and the Linux `.desktop` entry that makes it a `mailto:`, `tel:` and `sip:` handler where no package
//! installed one (an AppImage, a development run). Pure, so both OSes' files are built and tested on each; the
//! unused half on one OS is allowed dead.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use super::links::SCHEMES;
use super::IDENTIFIER;

/// The name the entries show, as `productName` in tauri.conf.json.
pub(crate) const PRODUCT_NAME: &str = "MR-Moqui";

/// The program to start: the AppImage file when the app runs from one (`APPIMAGE`; the executable is inside its
/// mount, which is gone after the app quits), else the executable.
pub(crate) fn program(exe: &Path, appimage: Option<&str>) -> PathBuf {
    match appimage.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => exe.to_path_buf(),
    }
}

/// `~/Library/LaunchAgents/org.moqui.desktop.plist`.
pub(crate) fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents").join(format!("{IDENTIFIER}.plist"))
}

/// A LaunchAgent that runs `program` at login (launchd reads `~/Library/LaunchAgents` then; nothing is loaded now).
pub(crate) fn launch_agent(program: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{IDENTIFIER}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(&program.to_string_lossy())
    )
}

/// `$XDG_CONFIG_HOME`, else `~/.config` (an XDG variable that is not an absolute path is ignored, as the spec says).
pub(crate) fn config_home(xdg: Option<&str>, home: &Path) -> PathBuf {
    xdg_dir(xdg).unwrap_or_else(|| home.join(".config"))
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub(crate) fn data_home(xdg: Option<&str>, home: &Path) -> PathBuf {
    xdg_dir(xdg).unwrap_or_else(|| home.join(".local/share"))
}

fn xdg_dir(xdg: Option<&str>) -> Option<PathBuf> {
    xdg.map(PathBuf::from).filter(|path| path.is_absolute())
}

/// `<config home>/autostart/org.moqui.desktop.desktop`.
pub(crate) fn autostart_path(config_home: &Path) -> PathBuf {
    config_home.join("autostart").join(format!("{IDENTIFIER}.desktop"))
}

/// The autostart entry that runs `program` at login (XDG Autostart).
pub(crate) fn autostart_entry(program: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={PRODUCT_NAME}\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec_value(&[&program.to_string_lossy()])
    )
}

/// The file name of the handler entry the app writes, `org.moqui.desktop.desktop`, as `xdg-mime default` takes it.
pub(crate) fn handler_file_name() -> String {
    format!("{IDENTIFIER}.desktop")
}

/// `<data home>/applications/org.moqui.desktop.desktop`.
pub(crate) fn handler_path(data_home: &Path) -> PathBuf {
    data_home.join("applications").join(handler_file_name())
}

/// The MIME types of the schemes, `x-scheme-handler/mailto` and the others.
pub(crate) fn scheme_mime_types() -> Vec<String> {
    SCHEMES
        .iter()
        .map(|scheme| format!("x-scheme-handler/{scheme}"))
        .collect()
}

/// A `.desktop` entry that opens the schemes' links with `program` (`%u`: the link is its argument). Not shown in
/// the menus: a package's own entry is the one there.
pub(crate) fn handler_entry(program: &Path) -> String {
    let mut mime = scheme_mime_types().join(";");
    mime.push(';');
    format!(
        "[Desktop Entry]\nType=Application\nName={PRODUCT_NAME}\nExec={} %u\nTerminal=false\nNoDisplay=true\nMimeType={mime}\n",
        exec_value(&[&program.to_string_lossy()])
    )
}

/// An `Exec` value of `args` (Desktop Entry spec, "The Exec key"): an argument with a reserved character is quoted,
/// with `"`, `` ` ``, `$` and `\` escaped inside the quotes; `%` is doubled; then the value's own `\` escapes.
pub(crate) fn exec_value(args: &[&str]) -> String {
    let quoted: Vec<String> = args
        .iter()
        .map(|arg| {
            let arg = arg.replace('%', "%%");
            if arg.is_empty() || arg.chars().any(|ch| " \t\n\"'\\><~|&;$*?#()`".contains(ch)) {
                let mut inner = String::with_capacity(arg.len() + 2);
                for ch in arg.chars() {
                    if matches!(ch, '"' | '`' | '$' | '\\') {
                        inner.push('\\');
                    }
                    inner.push(ch);
                }
                format!("\"{inner}\"")
            } else {
                arg
            }
        })
        .collect();
    quoted.join(" ").replace('\\', "\\\\")
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
#[path = "entries_tests.rs"]
mod tests;
