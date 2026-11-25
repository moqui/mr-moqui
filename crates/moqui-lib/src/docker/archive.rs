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

//! Secret files on Docker (plans/archive/Vault-Instances.md §4.2): a tar of `run/secrets/<name>`, each file with its owner and
//! mode, put into a created container with `PUT /containers/{id}/archive?path=/` before it starts. The file is in the
//! container's own layer, so it is on the host's disk only while the container exists. Docker keeps the owner of each
//! entry as the tar says it (it sets the container user's only with `copyUIDGID`, which this does not send).

use zeroize::Zeroizing;

use super::spec::{SecretFile, SECRETS_DIR};
use crate::error::Error;

/// A tar block.
const BLOCK: usize = 512;

/// The ustar header of one entry.
fn header(path: &str, kind: tar::EntryType, size: usize, mode: u32, uid: u32, gid: u32) -> Result<tar::Header, Error> {
    let mut header = tar::Header::new_ustar();
    header.set_entry_type(kind);
    header
        .set_path(path)
        .map_err(|err| Error::new(format!("{path} does not fit a tar header")).with_source(err))?;
    header.set_size(u64::try_from(size).unwrap_or(u64::MAX));
    header.set_mode(mode);
    header.set_uid(u64::from(uid));
    header.set_gid(u64::from(gid));
    header.set_mtime(0);
    header.set_cksum();
    Ok(header)
}

/// The tar of `secrets`: the directory `run/secrets/` (0755, root's), then each file at its mode and owner. Written
/// straight into one buffer sized first, so building it leaves no copy behind from a reallocation, and that buffer is
/// zeroized when dropped. Only this buffer: the HTTP client copies the body into buffers of its own as it sends it.
///
/// # Errors
///
/// When a name does not fit a tar header (names are checked by [`super::ContainerSpec::check`] first).
pub fn secrets_tar(secrets: &[SecretFile]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let dir = SECRETS_DIR.trim_start_matches('/');
    let size = BLOCK
        + secrets
            .iter()
            .map(|secret| BLOCK + secret.bytes.len().div_ceil(BLOCK) * BLOCK)
            .sum::<usize>()
        + 2 * BLOCK;
    let mut out = Zeroizing::new(Vec::with_capacity(size));
    out.extend_from_slice(header(&format!("{dir}/"), tar::EntryType::Directory, 0, 0o755, 0, 0)?.as_bytes());
    for secret in secrets {
        let path = format!("{dir}/{}", secret.name);
        let entry = header(
            &path,
            tar::EntryType::Regular,
            secret.bytes.len(),
            secret.mode,
            secret.uid,
            secret.gid,
        )?;
        out.extend_from_slice(entry.as_bytes());
        out.extend_from_slice(&secret.bytes);
        let padded = out.len().div_ceil(BLOCK) * BLOCK;
        out.resize(padded, 0);
    }
    // the end of the archive: two empty blocks
    let end = out.len() + 2 * BLOCK;
    out.resize(end, 0);
    Ok(out)
}
