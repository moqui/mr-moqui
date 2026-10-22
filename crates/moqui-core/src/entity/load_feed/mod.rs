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

//! Entity ECA match, the entity data loader, and the `DataFeedPending` insert.
//!
//! Matching ECA rules run their conditions and actions (`service::run_matched_eca`), and data-file
//! service elements are called through `service::run_data_file_service` and also recorded here.
//! Feed receive and delete service names are recorded, and `service::schedule_feed_receive` hands
//! the pending rows to a service worker after commit. Documents are indexed only when search is attached.

mod check;
mod csv;
mod eca;
mod feed;
mod json;
mod loader;
mod pull;
mod xml;
mod zip;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;

use super::facade::EntityFacade;

pub use check::{check_entity_data, check_entity_xml_text, check_info_with, check_with, list_with, CheckResult};
#[cfg(test)]
pub(crate) use eca::add_eca_rules;
pub use eca::{eca_rule_count, eca_rules, match_rules, EcaRule};
pub(crate) use eca::{match_rules_with, EcaIndex};
pub(super) use feed::clear_frame_touches;
pub use feed::register_feed_insert;
pub(crate) use feed::{feed_wants_old, note_touch};
pub use loader::{
    is_restricted_entity, load_entity_data, load_entity_text_file, load_entity_xml_text, location_in_components,
};

/// A data-file service element. It is called as it is read, and kept here for [`data_file_services`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataFileService {
    pub name: String,
    pub location: String,
    pub parameters: BTreeMap<String, String>,
}

/// A real-time feed's receive and delete service names, recorded as feed rows are inserted. A service worker calls them after commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedServiceNames {
    pub data_feed_id: String,
    pub receive_service: String,
    pub delete_service: String,
}

/// Filters and write mode for [`load_entity_data`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadOptions {
    pub data_types: Vec<String>,
    pub components: Vec<String>,
    pub only_create: bool,
    pub dummy_fks: bool,
    pub disable_fk_create: bool,
    /// Transaction timeout for each record. `None` uses the transaction manager default.
    pub timeout: Option<std::time::Duration>,
    /// Java `restrictSensitiveEntities`: refuse every record, root or nested, whose entity is a
    /// restricted entity ([`is_restricted_entity`]). Here service elements are refused too, a hardening beyond the Java, which ran
    /// them and checked only root and CSV entity names.
    pub restrict_sensitive_entities: bool,
    /// Java `csvEntityName`: the entity or service of a CSV text with no entity line.
    pub csv_entity_name: Option<String>,
    /// Java `csvFieldNames`: the fields of a CSV text with no header line.
    pub csv_field_names: Vec<String>,
    /// Java `defaultValues`: fields set on each top-level record (XML, CSV and JSON) and each data-file service
    /// call before its own values, as text.
    pub default_values: Vec<(String, String)>,
    /// Java `csvDelimiter`, `,` when `None`.
    pub csv_delimiter: Option<char>,
    /// Java `csvCommentStart`, `#` when `None`.
    pub csv_comment_start: Option<char>,
    /// Java `csvQuoteChar`, `"` when `None`.
    pub csv_quote_char: Option<char>,
    /// Java `messageNoActionFiles(false)`: a check names only the files that gave messages.
    pub hide_no_action_files: bool,
}

/// How many entity records were written. Data-file service calls are not included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadResult {
    pub records: u64,
    /// Files of a directory location that failed, were logged and skipped.
    pub failed_files: u64,
}

/// The error and its sources, as Java logs `t` and `t.getCause()`.
pub fn error_chain(err: &Error) -> String {
    let mut text = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

#[derive(Default)]
struct LoadSlot {
    services: Vec<DataFileService>,
    /// By `data_feed_id`, so each read of the feed definitions replaces the names of the feeds it saw.
    feed_services: BTreeMap<String, FeedServiceNames>,
    /// The feed definitions as last read. See `feed::feed_config`.
    feed_config: Option<std::sync::Arc<feed::FeedConfig>>,
}

fn with_load_slot<R>(facade: &EntityFacade, body: impl FnOnce(&mut LoadSlot) -> R) -> R {
    facade.with_slot(LoadSlot::default, body)
}

fn with_load_slot_read<R>(facade: &EntityFacade, body: impl FnOnce(&LoadSlot) -> R) -> R {
    facade.with_slot_read(LoadSlot::default, body)
}

/// Service elements the loader has read, in order. They were called as they were read.
pub fn data_file_services(facade: &EntityFacade) -> Vec<DataFileService> {
    with_load_slot(facade, |slot| slot.services.clone())
}

/// Receive and delete service names seen while inserting feed rows. A service worker calls them after commit, not the loader.
pub fn feed_service_names(facade: &EntityFacade) -> Vec<FeedServiceNames> {
    with_load_slot(facade, |slot| slot.feed_services.values().cloned().collect())
}

/// `load-data` locations from the framework conf. This does not load those files.
pub fn conf_data_locations(facade: &EntityFacade) -> Result<Vec<String>, Error> {
    let conf = MNode::parse_file(facade.framework_dir().join("conf/MoquiDefaultConf.xml"))?;
    let facade = conf
        .first_named("entity-facade")
        .ok_or_else(|| Error::new("conf has no entity-facade"))?;
    let mut locations = Vec::new();
    for node in facade.children_named("load-data") {
        if let Some(location) = node.attribute("location") {
            locations.push(location);
        }
    }
    Ok(locations)
}
