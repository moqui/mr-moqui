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

//! Bringing a Geo data file to `runtime/tmp/geo-import` (plans/archive/Maps.md §6.4, step 2): a registered data set's URL
//! downloaded through `moqui_core::outbound` with its internal-address guard, conditionally (the ETag and
//! Last-Modified of the edition in use, so an unchanged source ends the run as current), written as it arrives and
//! stopped at `maps@import-max-bytes`; or an operator's file copied from an allowed resource location, a window at a
//! time. Either way its SHA-256 and size are taken as it is written.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use moqui_lib::error::Error as LibError;
use sha2::{Digest, Sha256};

use moqui_core::context::ExecutionContext;
use moqui_core::outbound::{self, Method, OpenRequest};

/// The whole download's limit.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// How often, in bytes, progress is reported while a file is written.
pub const PROGRESS_BYTES: u64 = 8 << 20;
/// Where an operator's file may come from: the caller's own folder of the Maps import folder (where Geo data's Import
/// keeps an upload, `<IMPORT_ROOT><userId>/`), the runtime's import folder (files an operator put on the server), and
/// the caller's own uploads.
pub const IMPORT_ROOT: &str = "dbresource://moqui/maps/import/";
pub const RUNTIME_IMPORT_ROOT: &str = "runtime://import/";

/// A file brought in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    pub path: PathBuf,
    pub bytes: u64,
    /// SHA-256, lower-case hex.
    pub checksum: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// What a download came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    Fetched(Fetched),
    /// The source answered 304: the edition in use is current.
    Unchanged,
}

/// The edition in use's validators, sent so an unchanged source answers 304.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conditional {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// The longest ETag kept (`sourceEtag`, text-medium) and Last-Modified (`sourceLastModified`, text-short); a longer
/// validator is dropped, so the run fetches whole next time rather than failing to record it.
pub const MAX_ETAG: usize = 255;
pub const MAX_LAST_MODIFIED: usize = 63;

/// A data set's `urlTemplate` or `archiveMember` with `{country}` (an ISO alpha-2 code, as the registered sources take
/// it) and `{year}` (its `sourceYear`, four digits) filled in.
///
/// # Errors
///
/// The template names a value that is missing or not of that shape: nothing else may reach a URL's path.
pub fn expand_template(template: &str, country: &str, year: &str) -> Result<String, String> {
    if template.contains("{country}") && !(country.len() == 2 && country.chars().all(|c| c.is_ascii_uppercase())) {
        return Err(format!(
            "{{country}} of {template} needs a two-letter country code, not {country:?}"
        ));
    }
    if template.contains("{year}") && !(year.len() == 4 && year.chars().all(|c| c.is_ascii_digit())) {
        return Err(format!(
            "{{year}} of {template} needs a four-digit sourceYear, not {year:?}"
        ));
    }
    Ok(template.replace("{country}", country).replace("{year}", year))
}

/// The value of response header `name`, any case.
#[must_use]
pub fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Copy `reader` to `writer`, refusing more than `max` bytes; `progress` is told the bytes so far every
/// [`PROGRESS_BYTES`]. Returns the bytes copied and their SHA-256 in hex.
///
/// # Errors
///
/// A read or write error, or more than `max` bytes.
pub fn copy_capped(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    max: u64,
    progress: &mut dyn FnMut(u64),
) -> Result<(u64, String), LibError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    let mut reported = 0_u64;
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(LibError::new(format!("the file was cut off after {total} bytes")).with_source(err)),
        };
        total += count as u64;
        if total > max {
            return Err(LibError::new(format!(
                "the file is larger than maps@import-max-bytes ({max} bytes)"
            )));
        }
        let chunk = buffer.get(..count).unwrap_or_default();
        hasher.update(chunk);
        writer
            .write_all(chunk)
            .map_err(|err| LibError::new("could not write the file under runtime/tmp").with_source(err))?;
        if total - reported >= PROGRESS_BYTES {
            reported = total;
            progress(total);
        }
    }
    writer
        .flush()
        .map_err(|err| LibError::new("could not write the file under runtime/tmp").with_source(err))?;
    Ok((total, hex(&hasher.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// The file a run brings its source to: `runtime/tmp/geo-import/<geoDataImportId>.data`, its folder made.
///
/// # Errors
///
/// No execution context, or the folder cannot be made.
pub fn tmp_path(run_id: &str) -> Result<PathBuf, LibError> {
    let runtime = ExecutionContext::with_current(|ec| ec.factory().runtime_dir().to_path_buf())
        .ok_or_else(|| LibError::new("no execution context for the runtime folder"))?;
    let folder = runtime.join("tmp").join("geo-import");
    std::fs::create_dir_all(&folder)
        .map_err(|err| LibError::new(format!("could not make {}", folder.display())).with_source(err))?;
    Ok(folder.join(format!("{}.data", run_id.replace(['/', '\\', '.'], "_"))))
}

/// Download `url` to `path`, conditionally on `conditional`, stopping past `max` bytes.
///
/// # Errors
///
/// The request fails, the source answers other than 200 or 304, says or sends more than `max` bytes, or the file
/// cannot be written. A part-written file is removed.
pub fn download(
    url: &str,
    conditional: &Conditional,
    max: u64,
    path: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<Fetch, LibError> {
    let mut headers = Vec::new();
    if let Some(etag) = &conditional.etag {
        headers.push(("If-None-Match".to_string(), etag.clone()));
    }
    if let Some(last_modified) = &conditional.last_modified {
        headers.push(("If-Modified-Since".to_string(), last_modified.clone()));
    }
    let response = outbound::open(OpenRequest {
        method: Method::Get,
        url: url.to_string(),
        headers,
        body: None,
        timeout: Some(DOWNLOAD_TIMEOUT),
        follow_redirects: true,
        // a registered source is a public host: never an internal address (a data set row has no private-network
        // switch; an installation's own mirror is an operator's file)
        deny_internal: true,
    })
    .map_err(|err| {
        LibError::new(format!(
            "could not download {}: {}",
            outbound::redacted(url),
            err.message()
        ))
    })?;
    match response.status {
        304 => return Ok(Fetch::Unchanged),
        200 => {}
        status => {
            return Err(LibError::new(format!(
                "{} answered HTTP {status}",
                outbound::redacted(url)
            )))
        }
    }
    if response.length.is_some_and(|length| length > max) {
        return Err(LibError::new(format!(
            "{} is {} bytes, more than maps@import-max-bytes ({max})",
            outbound::redacted(url),
            response.length.unwrap_or_default()
        )));
    }
    let etag = header(&response.headers, "ETag").filter(|etag| etag.len() <= MAX_ETAG);
    let last_modified = header(&response.headers, "Last-Modified").filter(|text| text.len() <= MAX_LAST_MODIFIED);
    let mut reader = response.reader;
    let written = write_file(path, |file| copy_capped(&mut reader, file, max, progress))?;
    Ok(Fetch::Fetched(Fetched {
        path: path.to_path_buf(),
        bytes: written.0,
        checksum: written.1,
        etag,
        last_modified,
    }))
}

/// Create `path` and fill it with `body`; on an error the file is removed.
fn write_file(
    path: &Path,
    body: impl FnOnce(&mut std::fs::File) -> Result<(u64, String), LibError>,
) -> Result<(u64, String), LibError> {
    let mut file = std::fs::File::create(path)
        .map_err(|err| LibError::new(format!("could not create {}", path.display())).with_source(err))?;
    let written = body(&mut file);
    if written.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    written
}

/// Whether the current user may import an operator's file from `location` ([`location_allowed_for`]).
#[must_use]
pub fn location_allowed(location: &str) -> bool {
    let user = moqui_core::user::current_user_id().ok().flatten();
    location_allowed_for(location, user.as_deref())
}

/// Whether `user` may import an operator's file from `location`: under their own folder of [`IMPORT_ROOT`]
/// (`<IMPORT_ROOT><userId>/`), under [`RUNTIME_IMPORT_ROOT`], or among their own uploads
/// (`dbresource://moqui/database/upload/<userId>/`); no `..`, `.`, `\` or `%`. Another user's folder is refused.
#[must_use]
pub fn location_allowed_for(location: &str, user: Option<&str>) -> bool {
    let location = location.trim();
    if location.is_empty()
        || location.contains('\\')
        || location.contains('%')
        || location.split('/').any(|part| part == ".." || part == ".")
    {
        return false;
    }
    let under = |prefix: &str| location.starts_with(prefix) && location.len() > prefix.len();
    if under(RUNTIME_IMPORT_ROOT) {
        return true;
    }
    user.filter(|user| !user.is_empty() && !user.contains('/'))
        .is_some_and(|user| {
            under(&format!("{IMPORT_ROOT}{user}/")) || under(&format!("{}/{user}/", crate::etl::access::UPLOAD_ROOT))
        })
}

/// Copy the operator's file at `location` (allowed by [`location_allowed`], checked by the caller) to `path`, read as
/// the framework a window at a time.
///
/// # Errors
///
/// No file there, larger than `max`, or a read or write error.
pub fn copy_location(
    location: &str,
    max: u64,
    path: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<Fetched, LibError> {
    let opened = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().open(location)
    })
    .ok_or_else(|| LibError::new("no execution context to read the file"))?
    .map_err(|err| LibError::new(format!("could not read {location}: {}", err.message())))?
    .ok_or_else(|| LibError::new(format!("there is no file at {location}")))?;
    if opened.stat.size > max {
        return Err(LibError::new(format!(
            "{location} is {} bytes, more than maps@import-max-bytes ({max})",
            opened.stat.size
        )));
    }
    let source = opened.source;
    let size = source.len();
    let mut reader = SourceReader {
        source: source.as_ref(),
        offset: 0,
        size,
    };
    let written = write_file(path, |file| copy_capped(&mut reader, file, max, progress))?;
    Ok(Fetched {
        path: path.to_path_buf(),
        bytes: written.0,
        checksum: written.1,
        etag: None,
        last_modified: None,
    })
}

/// A resource's bytes as a reader, a window at a time.
struct SourceReader<'a> {
    source: &'a dyn moqui_core::resource::source::ByteSource,
    offset: u64,
    size: u64,
}

impl Read for SourceReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.offset >= self.size || buffer.is_empty() {
            return Ok(0);
        }
        let want = buffer.len().min(1 << 20);
        let bytes = self
            .source
            .read_at(self.offset, want)
            .map_err(|err| std::io::Error::other(err.message().to_string()))?;
        let count = bytes.len().min(buffer.len());
        if count == 0 {
            return Ok(0);
        }
        buffer
            .get_mut(..count)
            .unwrap_or_default()
            .copy_from_slice(bytes.get(..count).unwrap_or_default());
        self.offset += count as u64;
        Ok(count)
    }
}
