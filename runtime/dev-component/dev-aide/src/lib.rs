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

//! Dev AIDE, the assisted IDE (plans/design-dev-aide.md): the Rust side of the `dev-aide` component. `submit_component!`
//! binds its services, which the component's service files name; the first of them a factory runs turns the framework's
//! reload reports into notifications for the open Workspace pages ([`notify`]). The general pieces it uses (the reload, the checks,
//! the schema digest, the lossless XML layer) are the framework's (`moqui_core::devtools`, `moqui_lib::xml_edit`).
//!
//! Every service checks the instance purpose (dev) and the user's tier ([`tier`]) first; paths are confined to the roots
//! ([`confine`]); the git layer runs the developer's own git command ([`git`]); every write is audited ([`audit`]).

pub mod agent;
mod artifacts;
pub mod audit;
mod changeset;
pub mod confine;
mod entities;
pub mod git;
mod notify;
pub mod roots;
pub mod settings;
mod site;
pub mod tier;
mod vcs;
pub(crate) mod workspace;

use moqui_core::context::Error;
use moqui_core::native::{submit_component, NativeComponent, NativeRegistry};

submit_component! {
    NativeComponent { name: "dev-aide", register }
}

fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    workspace::register(registry)?;
    artifacts::register(registry)?;
    changeset::register(registry)?;
    entities::register(registry)?;
    vcs::register(registry)
}
