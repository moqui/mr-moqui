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

//! Entity facade. Field encryption stays in [`crypto`] and is not called from this spine.

pub mod cache;
pub mod crypto;
pub mod datasource;
pub mod document;
pub mod field_rw;
pub mod load_feed;
pub mod query_rest;
pub mod sensitive;

mod data_writer;
mod dbresource;
mod derived;
mod etl;
pub(crate) mod exec;
mod facade;
mod find;
mod fk;
pub mod info;
mod meta;
mod params;
mod schema;
mod search_form;
mod sequence;
mod share;
mod sim_view;
mod value;

pub(crate) use cache::screen_config_generation;
pub use data_writer::{DataFileType, EntityDataWriter};
pub use dbresource::{DbResourceChild, DbResourceInfo, DbResourceStat, DbResourceVersion};
pub use etl::EtlLoader;
pub use exec::statement_stats;
pub use facade::{EntityFacade, EntityFlags, EntityGroup, SimStarter};
pub use find::{EntityFind, EntityListIterator};
pub use meta::{DdlConf, EntityDbMeta};
/// The condition tree `EntityFind::condition` takes, so callers need not depend on `moqui-db`.
pub use moqui_db::sql::{CompareOp, Condition};
pub(crate) use schema::{document as schema_document, FieldEnums};
pub use search_form::SearchFormResult;
pub use sequence::{fill_bank, stagger_step, take_id, SequenceBank};
pub(crate) use sequence::{sequenced_id_primary, uuid_v4};
pub(crate) use value::is_rust_only_sub_type;
pub use value::{EntityList, EntityValue, RustOnlyEntityHits};

/// Forget what the entity layer keeps on this thread once a unit of work is over. With `sim`, ends every sim
/// session still open and clears the sim flag, so a missed `exit_sim` does not send the thread's later writes to a
/// throwaway schema. With no transaction frame left on the thread, also drops the per-frame held rows, written
/// entities and data-feed touches. The request and service pools call this after rolling back what the work left.
pub(crate) fn clear_thread_state(facade: &EntityFacade, sim: bool) {
    if sim {
        cache::clear_sim(facade);
    }
    if !crate::transaction::thread_in_transaction() {
        cache::clear_frame_state();
        load_feed::clear_frame_touches();
    }
}

#[cfg(test)]
mod tests;
