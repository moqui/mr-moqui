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

//! SSH on `russh` and SFTP on `russh-sftp` (`plans/archive/Ssh.md`): the SFTP client behind `moqui_core::sftp`, and the
//! built-in server with its SFTP subsystem and admin shell.

#[cfg(feature = "tui")]
mod agent;
pub mod client;
pub mod cluster;
#[cfg(feature = "git")]
mod git;
pub mod server;
mod sftp_fs;
mod shell;

use std::path::Path;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;

/// Install the SFTP client on `factory`. First-use host keys go in its `runtime/conf/ssh_known_hosts`.
pub fn install_client(factory: &ExecutionContextFactory, runtime_dir: &Path) {
    let known_hosts = runtime_dir.join("conf").join("ssh_known_hosts");
    moqui_core::sftp::install_sftp_connector(factory, Arc::new(client::RusshConnector::new(known_hosts)));
}
