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

//! Resource revision support behind the resource facade (`crate::context::resource`): line diffs of versions, and
//! the `git://` location scheme. Guide: `docs/Revision-Management.md`.

pub mod blob;
#[cfg(feature = "git")]
pub mod git;
pub mod source;
pub mod textdiff;

/// An attribute of the conf's `resource-facade` in the current context's factory, `${}` expanded and trimmed; empty
/// when there is no current context or the conf leaves it out.
pub(crate) fn setting(name: &str) -> String {
    crate::context::ExecutionContext::with_current(|ec| ec.factory().conf_setting("resource-facade", name))
        .unwrap_or_default()
}
