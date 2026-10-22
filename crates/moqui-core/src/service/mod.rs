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
mod chat;
mod chat_link;
#[cfg(test)]
mod chat_tests;
mod collab;
mod content;
pub mod coverage;
mod dav;
pub(crate) mod define;
mod eca;
mod eca_rules;
mod elfinder;
mod email_eca;
#[cfg(all(test, feature = "git"))]
mod files_tests;
#[cfg(feature = "git")]
mod git;
pub(crate) mod git_eca;
#[cfg(feature = "git")]
mod git_hosting;
#[cfg(feature = "image")]
mod images;
pub mod info;
#[cfg(test)]
mod fence_tests;
#[cfg(test)]
mod inline_tests;
mod jobs;
mod mail;
pub mod mail_capture;
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
pub(crate) mod shell;
#[cfg(feature = "xlsx")]
pub(crate) mod spreadsheet;
mod sso;
mod system_sync;
mod tools;
mod user_keys;
pub(crate) mod util;
mod validate;
#[cfg(all(test, feature = "git"))]
mod wiki_tests;

pub use call::{
    async_call, call, call_future, register_callback, register_on_end, start_future, sync, ServiceCallback,
    ServiceFuture,
};
pub use define::load_definitions;
pub use define::{check, configured_service_files, lint};
pub(crate) use define::{input_hints, input_rules, merge_param, ClientRule};
pub use eca::{run_data_file_service, run_matched_eca, schedule_feed_receive, search_not_up};
pub use email_eca::{email_rule_count, run_email_rule, run_email_rules};
pub(crate) use jobs::job_time_zone;
#[cfg(test)]
pub(crate) use jobs::send_job_error_notification;
pub use jobs::{run_job_now, PassReport};
/// Mail the host's SMTP listener (moqui-host `inbound-smtp`) received: the recipient lookup and the delivery.
#[cfg(feature = "imap")]
pub use mail::delivery as mail_inbound;
#[cfg(all(test, feature = "smtp"))]
pub(crate) use mail::{SharedSinkGuard, SinkGuard};
pub use pool::{start_pools, PoolStatus, RunnerStats, ServicePools};
#[cfg(feature = "http")]
pub use remote::shared_http_client;
pub use request_pool::{caps, check_serve_caps, spare_connections, RequestPool};
#[cfg(test)]
pub(crate) use rest::install;
pub(crate) use rest::openapi_document;
pub use rest::{resolve_service as resolve_rest_service, root_resource_infos, run};
#[cfg(test)]
pub(crate) use rest::install as install_rest;
#[cfg(all(test, feature = "git"))]
pub(crate) use git_hosting::signature as git_hosting_signature;
pub use seca::run_seca_rules;
pub use security_checks::security_checks_run;
pub(crate) use security_checks::{assert_privileged_group_membership, assert_sealed_permission_grant};
pub(crate) use system_sync::receive_rest_message;
pub(crate) use validate::clean_safe_html;
pub use validate::validate;
pub(crate) use validate::{auto_entity_name, auto_fields};

/// Start the mail client's IMAP IDLE workers (`email-client` `idle-enabled`, on the scheduler's tick). The host calls it once,
/// after the entity facade is installed; a host built without mail does nothing.
pub fn start_mail_idle(factory: &std::sync::Arc<crate::context::ExecutionContextFactory>) {
    mail::start_idle(factory);
}

/// Stop the IMAP IDLE workers, for the factory's shutdown.
pub(crate) fn stop_mail_idle(factory: &crate::context::ExecutionContextFactory) {
    mail::stop_idle(factory);
}

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
