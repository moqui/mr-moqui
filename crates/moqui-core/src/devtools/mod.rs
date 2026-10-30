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

//! What the framework itself needs of the development tools (plans/design-dev-aide.md 17): whether this is a dev
//! instance and who may use the tools there, restart from a running server, the log capture of a dev request, and the
//! calls observed on the artifact stack ([`calls`], on the hot path of every push). The tools themselves (reload, the
//! file watcher, `moqui check`, the artifact index, typegen and the spec runner) are moqui-devkit's
//! (plans/Crate-Split.md).

pub mod calls;
pub mod log_capture;
pub mod restart;

/// Whether this instance's purpose is `dev`, where the development tools run.
#[must_use]
pub fn is_dev_instance() -> bool {
    moqui_lib::env::get_prop_or_env("instance_purpose") == "dev"
}

/// The permissions of the dev-aide's tiers (plans/design-dev-aide.md 3.4); `ADMIN` holds `DEV_AIDE` by its seed.
pub const DEV_AIDE_PERMISSIONS: [&str; 3] = ["DEV_AIDE_VIEW", "DEV_AIDE_EXTEND", "DEV_AIDE"];

/// Whether `user_id` may use the development tools here: a dev instance with the dev-aide component loaded, and one of
/// its permissions. The agents' dev tools and the `screen-xml` dialect are offered on this (plans/design-dev-aide.md 13.2).
#[must_use]
pub fn dev_aide_user(factory: &crate::context::ExecutionContextFactory, user_id: &str) -> bool {
    is_dev_instance()
        && !user_id.is_empty()
        && factory
            .components()
            .iter()
            .any(|component| component.name == "dev-aide")
        && DEV_AIDE_PERMISSIONS
            .iter()
            .any(|permission| crate::user::user_has_permission(user_id, permission).unwrap_or(false))
}
