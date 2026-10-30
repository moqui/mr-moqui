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

//! Factory, conf, components, resources, cache, messages, and the artifact stack.

mod artifact;
mod cache;
mod component;
mod conf;
mod confcache;
mod factory;
pub(crate) mod identity;
mod locate;
mod message;
mod resource;
pub mod status;

pub use artifact::AuthzGuard;
pub use cache::CacheFacade;
pub use component::{component_entity_dirs, discover_components, ComponentInfo};
pub use conf::{load_merged_conf, merge_component_confs, merge_conf};
pub use factory::{
    default_framework_dir, AgentOrigin, BeforeInit, BootOptions, ContextGuard, ExecutionContext,
    ExecutionContextFactory, Executor,
};
pub use identity::secret_parameter;
pub use locate::{framework_dir, locate};
pub use message::{ErrorMark, MessageFacade, ValidationError};
pub use moqui_entity::error::{client_text, DeniedArtifact, Error, ErrorKind};
pub use resource::millis_of;
pub(crate) use resource::split_version;
pub use resource::version_author;
pub(crate) use resource::{apply_changes_each, last_change_by_stat, tree_size_by_walk};
pub use resource::{
    with_version_author, ChangeLogEntry, ChangeSetOutcome, Opened, ResourceChange, ResourceChangeInfo, ResourceEntry,
    ResourceFacade, ResourceRef, ResourceStat, ResourceVersion, VersionAuthor,
};

#[cfg(test)]
mod tests;
