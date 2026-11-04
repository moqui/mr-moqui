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

//! Components and conf on a running server (plans/Component-Deploy.md): the conf reload's services, and the loading
//! of a component without a restart. The conf reload itself is `moqui_core::context::conf_reload`; this crate holds
//! what uses the reload pipeline ([`crate::reload`]). Guide: docs/tools/Component-Deploy.md.

#[cfg(feature = "git")]
pub mod git;
#[cfg(not(feature = "git"))]
#[path = "git_off.rs"]
pub mod git;
pub mod load;
pub mod services;

pub(crate) use services::register;
