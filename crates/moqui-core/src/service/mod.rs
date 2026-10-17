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

//! Service facade: definitions, synchronous calls, ECAs, pools, jobs, and REST. JSON-RPC is not part of this port.

#[macro_use]
pub(crate) mod rust_services;
mod backup;
pub(crate) mod call;
pub mod coverage;
pub(crate) mod define;
mod eca;
mod eca_rules;
mod elfinder;
mod email_eca;
mod images;
pub mod info;
#[cfg(test)]
mod inline_tests;
mod jobs;
mod mail;
mod pool;
mod remote;
mod request_pool;
mod rest;
pub(crate) mod rule_actions;
mod schedule;
#[cfg(test)]
mod script_ports_tests;
mod seca;
mod security_checks;
pub(crate) mod semaphore;
mod spreadsheet;
mod sso;
mod system_sync;
mod tools;
pub(crate) mod util;
mod validate;

pub use call::{
    async_call, call, call_future, register_callback, register_on_end, start_future, sync, ServiceCallback,
    ServiceFuture,
};
pub use define::load_definitions;
pub use define::{check, configured_service_files, lint};
pub(crate) use define::{input_hints, input_js_rules, merge_param, VALIDATE_NUMBER, VALIDATE_NUMBER_INT};
pub use eca::{run_data_file_service, run_matched_eca, schedule_feed_receive, search_not_up};
pub use email_eca::run_email_rules;
#[cfg(test)]
#[cfg(test)]
pub(crate) use jobs::send_job_error_notification;
pub use jobs::{run_job_now, PassReport};
#[cfg(all(test, feature = "mail"))]
pub(crate) use mail::{SharedSinkGuard, SinkGuard};
pub use pool::{start_pools, PoolStatus, RunnerStats, ServicePools};
#[cfg(feature = "http")]
pub use remote::shared_http_client;
pub use request_pool::{caps, check_serve_caps, RequestPool};
#[cfg(test)]
pub(crate) use rest::install;
pub(crate) use rest::openapi_document;
pub use rest::{resolve_service as resolve_rest_service, root_resource_infos, run};
pub use seca::run_seca_rules;
pub use security_checks::security_checks_run;
pub(crate) use security_checks::{assert_privileged_group_membership, assert_sealed_permission_grant};
pub(crate) use system_sync::receive_rest_message;
pub(crate) use validate::clean_safe_html;
pub use validate::validate;
pub(crate) use validate::{auto_entity_name, auto_fields};

/// A JSON body as a [`moqui_lib::object::Value`].
pub(crate) fn parse_json(text: &str) -> Result<moqui_lib::object::Value, crate::context::Error> {
    let json = serde_json::from_str::<serde_json::Value>(text)
        .map_err(|err| crate::context::Error::new(format!("JSON: {err}")))?;
    Ok(moqui_lib::json::from_json(&json))
}

/// An entity in the default group, to pick the transaction manager for work that has no entity of
/// its own, such as a screen transition.
pub(crate) fn semaphore_entity_name() -> &'static str {
    define::ServiceDef::semaphore_entity()
}
