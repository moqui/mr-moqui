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

use super::*;
use std::path::{Path, PathBuf};

#[test]
fn an_appimage_is_the_program_else_the_executable() {
    let exe = Path::new("/tmp/.mount_MR/usr/bin/moqui-desktop");
    assert_eq!(
        program(exe, Some("/home/jo/Apps/MR.AppImage")),
        PathBuf::from("/home/jo/Apps/MR.AppImage")
    );
    assert_eq!(program(exe, Some(" ")), exe);
    assert_eq!(program(exe, None), exe);
}

#[test]
fn the_launch_agent_runs_the_program_at_login() {
    let home = Path::new("/Users/jo");
    assert_eq!(
        launch_agent_path(home),
        PathBuf::from("/Users/jo/Library/LaunchAgents/org.moqui.desktop.plist")
    );
    let plist = launch_agent(Path::new("/Applications/R&D <x>.app/Contents/MacOS/moqui-desktop"));
    assert!(plist.contains("<string>org.moqui.desktop</string>"));
    assert!(plist.contains("<string>/Applications/R&amp;D &lt;x&gt;.app/Contents/MacOS/moqui-desktop</string>"));
    assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
}

#[test]
fn xdg_directories_fall_back_to_home() {
    let home = Path::new("/home/jo");
    assert_eq!(config_home(None, home), PathBuf::from("/home/jo/.config"));
    assert_eq!(config_home(Some("relative"), home), PathBuf::from("/home/jo/.config"));
    assert_eq!(config_home(Some("/cfg"), home), PathBuf::from("/cfg"));
    assert_eq!(data_home(None, home), PathBuf::from("/home/jo/.local/share"));
    assert_eq!(
        autostart_path(&config_home(None, home)),
        PathBuf::from("/home/jo/.config/autostart/org.moqui.desktop.desktop")
    );
    assert_eq!(
        handler_path(&data_home(None, home)),
        PathBuf::from("/home/jo/.local/share/applications/org.moqui.desktop.desktop")
    );
}

#[test]
fn the_entries_exec_the_program() {
    let autostart = autostart_entry(Path::new("/home/jo/My Apps/MR.AppImage"));
    assert!(autostart.starts_with("[Desktop Entry]\nType=Application\n"));
    assert!(autostart.contains("\nExec=\"/home/jo/My Apps/MR.AppImage\"\n"));
    let handler = handler_entry(Path::new("/usr/bin/moqui-desktop"));
    assert!(handler.contains("\nExec=/usr/bin/moqui-desktop %u\n"));
    assert!(handler.contains("\nMimeType=x-scheme-handler/mailto;x-scheme-handler/tel;x-scheme-handler/sip;\n"));
    assert_eq!(handler_file_name(), "org.moqui.desktop.desktop");
}

#[test]
fn exec_arguments_are_quoted_and_escaped_as_the_spec_says() {
    assert_eq!(exec_value(&["/usr/bin/app"]), "/usr/bin/app");
    assert_eq!(exec_value(&["/opt/100%/app"]), "/opt/100%%/app");
    assert_eq!(exec_value(&["/a b/app", "x"]), "\"/a b/app\" x");
    // inside quotes `$` and `"` get a backslash; then the value's escapes double each backslash
    assert_eq!(exec_value(&["/a$b\"c"]), "\"/a\\\\$b\\\\\"c\"");
    assert_eq!(exec_value(&["/a\\b"]), "\"/a\\\\\\\\b\"");
}
