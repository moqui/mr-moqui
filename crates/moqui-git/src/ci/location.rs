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

//! `ci-artifact://<artifactId>[/<fileName>]`: a run's artifact or report as a resource location (docs/interfaces/
//! Pipelines.md, "Artifacts and reports"), so every way the framework sends a file can send one, streamed from its
//! content blob: `ec.web.sendResourceResponse`, a PASS authority download over gRPC (`pass_authority::serve_download`,
//! which opens the location its authority service answers), a script reading it. The file name after the id is only
//! the name the file is sent under (its type comes from it); the id alone finds the file. It is read only, and read as
//! `get#RunArtifact` reads it ([`super::service::read_artifact`]): the view right on the run's repository, a hidden
//! run's only with authorization disabled or as the user it was made for; a cache, an artifact that is not there and
//! one the caller does not read are all not found. Installed with the `git` scheme.

use std::sync::Arc;

use moqui_core::context::{Error, ExecutionContext, ResourceStat};
use moqui_core::entity::EntityFacade;

use super::service::{read_artifact, ArtifactRead};

/// The scheme of artifact locations.
pub(crate) const SCHEME: &str = "ci-artifact";

/// The location of the artifact or report `artifact_id`, sent as `file_name`.
pub(crate) fn location_of(artifact_id: &str, file_name: &str) -> String {
    let name: String = file_name
        .chars()
        .map(|ch| {
            if ch == '/' || ch == '\\' || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .collect();
    if name.is_empty() {
        format!("{SCHEME}://{artifact_id}")
    } else {
        format!("{SCHEME}://{artifact_id}/{name}")
    }
}

/// The artifact id of a location of this scheme.
fn artifact_id(location: &str) -> Result<&str, Error> {
    let rest = location
        .strip_prefix(SCHEME)
        .and_then(|rest| rest.strip_prefix("://"))
        .ok_or_else(|| Error::new(format!("not a {SCHEME} location: {location}")))?;
    let id = rest.split_once('/').map_or(rest, |(id, _)| id);
    if id.is_empty() {
        return Err(Error::new(format!("a {SCHEME} location names no artifact: {location}")));
    }
    Ok(id)
}

fn entity() -> Result<EntityFacade, Error> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade for a ci-artifact location"))
}

/// The provider of `ci-artifact://` locations.
#[derive(Debug, Default)]
pub(crate) struct ArtifactLocations;

impl ArtifactLocations {
    /// The artifact a location names, when the caller reads it, with its content.
    fn found(
        location: &str,
    ) -> Result<
        Option<(
            moqui_core::resource::blob::BlobStore,
            moqui_core::resource::blob::BlobInfo,
        )>,
        Error,
    > {
        let entity = entity()?;
        // with authorization disabled, privileged code that checked its caller (an application's download service)
        // vouches for a hidden run's file too, as includeHidden does on the services
        let include_hidden = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
        Ok(match read_artifact(&entity, artifact_id(location)?, include_hidden)? {
            ArtifactRead::Found(_, blobs, info) => Some((blobs, info)),
            ArtifactRead::NotFound | ArtifactRead::ContentGone => None,
        })
    }
}

impl moqui_core::native::ResourceProvider for ArtifactLocations {
    fn read_bytes(&self, location: &str) -> Result<Vec<u8>, Error> {
        let (blobs, info) =
            Self::found(location)?.ok_or_else(|| Error::new(format!("resource not found: {location}")))?;
        let size = usize::try_from(info.size).map_err(|_| Error::new(format!("{location} is too large to read")))?;
        let _rows = crate::rows_guard();
        Ok(blobs.read_range(&info, 0, size)?)
    }

    fn stat(&self, location: &str) -> Result<Option<ResourceStat>, Error> {
        Ok(Self::found(location)?.map(|(_, info)| ResourceStat {
            is_file: true,
            size: info.size,
            last_modified: None,
        }))
    }

    /// The content blob, read a window at a time: no copy of a large artifact is made.
    fn open(self: Arc<Self>, location: &str) -> Result<moqui_core::native::OpenedFile, Error> {
        Ok(Self::found(location)?.map(|(blobs, info)| {
            let stat = ResourceStat {
                is_file: true,
                size: info.size,
                last_modified: None,
            };
            (stat, blobs.source(&info))
        }))
    }
}

#[cfg(test)]
#[path = "location_tests.rs"]
mod tests;
