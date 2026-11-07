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

//! The Constitution (plans/LLM-A2A-Assist.md 7.10, docs/ai/Constitution.md): one localized JSON document of shared
//! guidance for people and for every Moqui agent run.
//!
//! - [`document`]: the format, its checks, hash and written text; [`locale`]: field-by-field locale fallback;
//!   [`edit`]: the editor's operations on a whole document; [`render`]: the system-prompt section.
//! - [`resolve`]: which document applies (organization, instance, shipped); [`origins`]: the instance's trusted
//!   reference origins.
//! - [`services`]: `org.moqui.admin.ConstitutionServices` (System › Constitution, the Assist Rules view and the
//!   `get#ConstitutionReferences` agent tool); [`snapshot`]: the document each agent run used.

pub mod document;
pub mod edit;
pub mod locale;
pub mod origins;
pub mod render;
pub mod resolve;
pub(crate) mod services;
pub mod snapshot;

use std::path::Path;

use moqui_lib::resource::{FileResource, Resource as _};

/// Write `text` to `path` whole: a temporary file beside it, then a rename, so a reader sees the old file or the new
/// one. The directory is made when missing.
///
/// # Errors
///
/// Returns a message when the file cannot be written.
pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("could not make {}: {error}", parent.display()))?;
    }
    FileResource::new(path.to_string_lossy().to_string())
        .write_bytes(text.as_bytes())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
