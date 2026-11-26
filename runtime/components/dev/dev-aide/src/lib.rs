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

//! Dev AIDE, the assisted IDE (plans/archive/design-dev-aide.md): the Rust side of the `dev-aide` component. `submit_component!`
//! binds its services, which the component's service files name; the first of them a factory runs turns the framework's
//! reload reports into notifications for the open Workspace pages ([`notify`]). The general pieces it uses (the reload, the checks,
//! the schema digest, the lossless XML layer) are the framework's (`moqui_core::devtools`, `moqui_lib::xml_edit`).
//!
//! Every service checks the instance purpose (dev) and the user's tier ([`tier`]) first; paths are confined to the roots
//! ([`confine`]); the git layer runs the developer's own git command ([`git`]); every write is audited ([`audit`]).

pub mod agent;
mod artifacts;
pub mod audit;
mod browser;
mod buffers;
mod changeset;
mod coedit;
pub mod confine;
mod data_load;
mod dbform;
mod docs;
mod eca_specs;
mod editor;
mod entities;
pub mod git;
mod hosted;
mod hosting;
mod libraries;
mod local_run;
mod notify;
mod patch;
mod rebuild;
mod replace;
mod repo;
mod repos;
pub mod roots;
mod screens;
mod services;
pub mod settings;
mod site;
mod tasks;
mod template;
pub mod tier;
mod try_overlay;
mod types;
mod vcs;
pub(crate) mod workspace;
mod worktree;

use moqui_core::context::Error;
use moqui_core::native::{submit_component, NativeComponent, NativeRegistry};

submit_component! {
    NativeComponent { name: "dev-aide", register }
}

fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    workspace::register(registry)?;
    libraries::register(registry)?;
    buffers::register(registry)?;
    artifacts::register(registry)?;
    browser::register(registry)?;
    changeset::register(registry)?;
    dbform::register(registry)?;
    docs::register(registry)?;
    editor::register(registry)?;
    coedit::register(registry)?;
    types::register(registry)?;
    replace::register(registry)?;
    entities::register(registry)?;
    data_load::register(registry)?;
    screens::register(registry)?;
    services::register(registry)?;
    eca_specs::register(registry)?;
    vcs::register(registry)?;
    repo::register(registry)?;
    repos::register(registry)?;
    hosted::register(registry)?;
    hosting::register(registry)?;
    worktree::register(registry)?;
    local_run::register(registry)?;
    rebuild::register(registry)?;
    tasks::register(registry)
}
