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

//! Outbound SMTP and inbound IMAP/POP3 for the email services, on top of moqui-core (plans/Crate-Split.md). The send
//! path lives in [`implementation`], the receive path in [`inbound`] (the `imap` feature), with the mail client's sync
//! engine beside it: [`sync`] (`sync#EmailAccount` and the job), [`remote`] (local changes onto the server), [`idle`]
//! (IMAP IDLE workers), [`store`] (a received message as a row), [`discover`] (autodiscover), [`import`] (.eml and
//! .mbox), [`export`] (a stored message as .eml), [`oauth`] (sign-in with Google or Microsoft, the token store and
//! XOAUTH2) and [`delivery`] (mail the host's SMTP listener received for hosted and shared accounts), all of them
//! `imap`. The functions below are the registry entries.
//!
//! Core reaches this crate only through the [`moqui_core::extension`] entry below, and moqui-host links it under its
//! `smtp` feature. In a build without the crate, or without `imap`, its services still load, and a call reports that
//! the feature is not built in. The app's contacts hook (`filter#ContactAddresses`) and the mail capture are core's.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

#[cfg(feature = "imap")]
pub mod delivery;
#[cfg(feature = "imap")]
mod discover;
#[cfg(feature = "imap")]
mod export;
#[cfg(feature = "imap")]
mod idle;
mod implementation;
#[cfg(feature = "imap")]
mod import;
#[cfg(feature = "imap")]
mod inbound;
#[cfg(feature = "imap")]
mod oauth;
#[cfg(feature = "imap")]
mod remote;
#[cfg(feature = "imap")]
mod store;
#[cfg(feature = "imap")]
mod sync;

#[cfg(test)]
pub(crate) use implementation::{SharedSinkGuard, SinkGuard};

#[cfg(all(test, feature = "imap"))]
mod client_tests;
#[cfg(all(test, feature = "imap"))]
mod delivery_tests;
#[cfg(all(test, feature = "imap"))]
mod fake_servers;
#[cfg(all(test, feature = "imap"))]
mod oauth_tests;
#[cfg(test)]
mod senders_tests;
#[cfg(all(test, feature = "imap"))]
mod settings_tests;
#[cfg(all(test, feature = "imap"))]
mod sync_tests;

#[cfg(feature = "imap")]
const FEATURES: &[&str] = &["smtp", "imap"];
#[cfg(not(feature = "imap"))]
const FEATURES: &[&str] = &["smtp"];

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: FEATURES,
        register_services: register,
        before_shutdown: stop_idle,
        ..moqui_core::extension::Extension::EMPTY
    }
}

/// An attribute of the conf's `email-client` element, `${}` expanded and trimmed, from the current context's factory;
/// empty when there is no current context or the conf leaves it out.
fn client_setting(name: &str) -> String {
    moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().conf_setting("email-client", name))
        .unwrap_or_default()
}

fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, send_template)?;
    rust_service!(map, send_message)?;
    rust_service!(map, test_server)?;
    rust_service!(map, push_remote_changes)?;
    #[cfg(feature = "imap")]
    {
        rust_service!(map, poll_server)?;
        rust_service!(map, sync_account)?;
        rust_service!(map, sync_due_accounts)?;
        rust_service!(map, discover_server)?;
        rust_service!(map, import_messages)?;
        rust_service!(map, export_message)?;
        rust_service!(map, start_oauth_sign_in)?;
        rust_service!(map, complete_oauth_sign_in)?;
        rust_service!(map, get_oauth_status)?;
        rust_service!(map, get_oauth_providers)?;
        rust_service!(map, find_oauth_provider)?;
        rust_service!(map, revoke_oauth_sign_in)?;
        rust_service!(map, revoke_oauth_token)?;
    }
    Ok(())
}

/// Start the IMAP IDLE workers on the scheduler's tick (`email-client` `idle-enabled`). The host calls it once, after
/// the entity facade is installed.
#[cfg(feature = "imap")]
pub fn start_idle(factory: &std::sync::Arc<ExecutionContextFactory>) {
    idle::start(factory);
}

/// Stop the IMAP IDLE workers, first in the factory's shutdown: each holds the factory and a connection while it waits.
#[cfg(feature = "imap")]
fn stop_idle(factory: &ExecutionContextFactory) {
    idle::stop_all(factory);
}

#[cfg(not(feature = "imap"))]
fn stop_idle(_factory: &ExecutionContextFactory) {}

/// A mail service body, registered under this crate's path.
macro_rules! mail_service {
    ($name:ident, $($body:tt)+) => {
        fn $name(
            parameters: &BTreeMap<String, Value>,
            facade: &EntityFacade,
        ) -> Result<BTreeMap<String, Value>, LibError> {
            $($body)+(parameters, facade)
        }
    };
}

mail_service!(send_template, implementation::send_template);
mail_service!(send_message, implementation::send_message);
mail_service!(test_server, implementation::test_server);
#[cfg(feature = "imap")]
mail_service!(poll_server, inbound::poll_server);
#[cfg(feature = "imap")]
mail_service!(sync_account, sync::sync_account_service);
#[cfg(feature = "imap")]
mail_service!(sync_due_accounts, sync::sync_due_accounts);
#[cfg(feature = "imap")]
mail_service!(push_remote_changes, remote::push_changes);

/// `push#RemoteChanges` without `imap`: no account reaches an IMAP server from this build, so a local change of the mail
/// client has no server side to queue, as for an account with no IMAP server. The filing, draft and send services of
/// internal accounts call it, so it succeeds rather than reporting the feature missing.
#[cfg(not(feature = "imap"))]
fn push_remote_changes(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    Ok(BTreeMap::new())
}
#[cfg(feature = "imap")]
mail_service!(discover_server, discover::discover_server);
#[cfg(feature = "imap")]
mail_service!(import_messages, import::import_messages);
#[cfg(feature = "imap")]
mail_service!(export_message, export::export_message);
#[cfg(feature = "imap")]
mail_service!(start_oauth_sign_in, oauth::start_sign_in);
#[cfg(feature = "imap")]
mail_service!(complete_oauth_sign_in, oauth::complete_sign_in);
#[cfg(feature = "imap")]
mail_service!(get_oauth_status, oauth::sign_in_status);
#[cfg(feature = "imap")]
mail_service!(get_oauth_providers, oauth::providers_service);
#[cfg(feature = "imap")]
mail_service!(find_oauth_provider, oauth::find_provider_service);
#[cfg(feature = "imap")]
mail_service!(revoke_oauth_sign_in, oauth::sign_out_service);
#[cfg(feature = "imap")]
mail_service!(revoke_oauth_token, oauth::revoke_token_service);

#[cfg(all(test, feature = "imap"))]
mod tests {
    /// The registry has a function at each `moqui_mail::` location the framework's service files name, and no other.
    #[test]
    fn the_services_register_at_the_locations_the_service_files_name() {
        let mut map = std::collections::BTreeMap::new();
        super::register(&mut map).unwrap();
        let registered: Vec<&str> = map.keys().copied().collect();
        let dir = moqui_core::context::framework_dir().join("service/org/moqui/impl");
        let mut named = Vec::new();
        for file in ["EmailServices.xml", "EmailClientServices.xml"] {
            let text = std::fs::read_to_string(dir.join(file)).unwrap();
            named.extend(
                text.split("location=\"")
                    .skip(1)
                    .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
                    .filter(|location| location.starts_with("moqui_mail::")),
            );
        }
        named.sort_unstable();
        named.dedup();
        assert_eq!(registered, named);
    }
}
