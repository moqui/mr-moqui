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

//! The entity facade: moqui-entity's, at its old path, with what needs the ExecutionContext.

pub use moqui_entity::entity::*;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;

use crate::context::ExecutionContextFactory;

/// Open the factory's component model and merged configuration over host-owned transaction groups.
/// The host configures the groups' drivers and transaction settings; naming, load-entity files,
/// extensions and encryption use the same merged configuration on every host.
pub fn open_for_factory(
    factory: &ExecutionContextFactory,
    default_group: &str,
    groups: Vec<EntityGroup>,
) -> Result<EntityFacade, Error> {
    let conf = MNode::parse_shared("moqui-conf", factory.conf_text())?;
    let facade = EntityFacade::open_components_with_conf(
        factory.framework_dir(),
        &crate::context::component_entity_dirs(factory),
        default_group,
        groups,
        &conf,
    )?;
    facade.crypt_from_conf(&conf)?;
    Ok(facade)
}

// moqui-entity's tests that need an entered ExecutionContext, a service or a script
#[cfg(test)]
mod dbresource_tests;
#[cfg(test)]
mod document_tests;
#[cfg(test)]
mod field_rw_tests;
#[cfg(test)]
mod load_feed_tests;
#[cfg(test)]
mod search_form_tests;
#[cfg(test)]
mod tests;
