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

//! The process entry of the desktop app. CEF starts its renderer, GPU and utility processes from this binary too;
//! `cef_entry_point` sends those to CEF before anything else runs, so only the browser process boots the node.

// No console window on Windows in a release build.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[tauri_runtime_cef::cef_entry_point]
fn main() {
    // the browser terminal's helper: its node in process starts this executable for a host session's program
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(status) = moqui_host::terminal_helper(&args) {
        std::process::exit(status);
    }
    moqui_app::link();
    moqui_desktop::run();
}
