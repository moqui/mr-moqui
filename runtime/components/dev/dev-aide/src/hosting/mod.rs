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

//! `org.moqui.devaide.HostingServices`: the Dev AIDE with a group server (plans/design-dev-aide.md 21, P11). A group
//! server is a connection ([`connect`]: what it offers, the user's token, and what Connect sets from it); a working
//! tree's remote resolves to its hosted repository there, whose merge requests are listed, opened, proposed and checked
//! out ([`review`]). Every call goes to the server's REST as the connected user ([`client`]), so the server's rules
//! decide; a repository an application keeps goes through that application's REST, which the server offers.

pub(crate) mod client;
pub(crate) mod connect;
pub(crate) mod review;
#[cfg(test)]
mod tests;

use moqui_core::context::Error;
use moqui_core::native::NativeRegistry;

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    connect::register(registry)?;
    review::register(registry)
}
