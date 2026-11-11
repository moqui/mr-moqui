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

//! Stands in for `backup_store.rs` in a build without the `backup-s3` feature.

use moqui_lib::error::Error as LibError;

use crate::backup::BackupOutcome;
use crate::sftp::SftpClient;

const OFF: &str = "this build has no S3 or encrypted backup mirror (the backup-s3 feature is off)";

// the same fields as backup_store.rs, which reads them
#[allow(dead_code)]
pub(super) struct S3Remote<'a> {
    pub send_url: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub prefix: &'a str,
}

pub(super) fn mirror_s3(
    _remote: &S3Remote<'_>,
    _recipient: Option<&str>,
    _outcome: &BackupOutcome,
) -> Result<String, LibError> {
    Err(LibError::new(OFF))
}

pub(super) fn mirror_sftp_encrypted(
    _client: &mut SftpClient,
    _dir: &str,
    _recipient: &str,
    _outcome: &BackupOutcome,
) -> Result<String, LibError> {
    Err(LibError::new(OFF))
}
