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

//! Per-user shared runtime selection. Reading the location never creates it.
use std::io;
use std::path::PathBuf;

pub fn directory() -> io::Result<Option<PathBuf>> {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return Ok(None);
    }
    if crate::env::get_property("MOQUI_SHARED_RUNTIME").as_deref() == Some("off") {
        return Ok(None);
    }
    match std::env::var_os("MOQUI_SHARED_RUNTIME").filter(|dir| !dir.is_empty()) {
        Some(dir) if dir == "off" => Ok(None),
        Some(dir) => {
            let dir = PathBuf::from(dir);
            if dir.is_absolute() {
                Ok(Some(dir))
            } else {
                Err(io::Error::other("MOQUI_SHARED_RUNTIME needs an absolute path or off"))
            }
        }
        None => Ok(std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .map(|p| p.join(".mr-moqui"))),
    }
}
