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

//! What `dbresource://` storage needs of the resource facade: content blobs, byte sources, line diffs of versions and
//! the version types. `moqui_core::resource` re-exports them.

use std::path::PathBuf;

use crate::error::Error;

pub mod blob;
pub mod source;
pub mod textdiff;
pub mod version;

/// Resolve a local file location without turning a relative directory into an authority.
pub fn file_path(location: &str) -> Result<PathBuf, Error> {
    let Some(rest) = location.strip_prefix("file:") else {
        return Ok(PathBuf::from(location));
    };
    let path = match rest.strip_prefix("//") {
        Some(rest) => {
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return Err(Error::new(format!(
                    "file resource has a non-local authority: {location}"
                )));
            }
            if path.is_empty() {
                "/".to_string()
            } else {
                format!("/{path}")
            }
        }
        None => rest.to_string(),
    };
    Ok(PathBuf::from(path))
}
