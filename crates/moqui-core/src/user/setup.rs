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

//! Who may create the first administrator while no account exists (`create#InitialAdminAccount`, SEC-WEB-08,
//! SEC-CLI-01): a request from this machine (a loopback peer, straight to the listener, through no proxy), or one that
//! gives the one-time setup secret this server printed in its startup log. The secret is made once per server and
//! held in memory only, so a restart makes a new one. None of it is offered unless `initial_admin_enabled` is true
//! (`get#InitialAdminNeeded`; false in the default conf, true in the dev conf).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::context::ExecutionContextFactory;
use crate::web::RequestVia;

const INITIAL_ADMIN_NEEDED: &str = "org.moqui.impl.UserServices.get#InitialAdminNeeded";

/// The setup secret of one server: an app password's form, about 99 bits.
struct SetupSecret(String);

fn secret(factory: &ExecutionContextFactory) -> Arc<SetupSecret> {
    factory.extension(|_| {
        SetupSecret(super::login::new_app_password().unwrap_or_else(|err| {
            // no random source: no secret works, so only a request from this machine creates the administrator
            tracing::error!("No setup secret for the first administrator: {err}");
            String::new()
        }))
    })
}

/// Whether the current caller is on this machine. A request: a loopback peer that reached the listener itself (or the
/// Desktop app's node in process), with no proxy's forwarding header, so a reverse proxy on the same host does not make
/// every client local. With no request (the terminal's dialog over SSH, server code): the address
/// [`super::with_client_ip`] gave is loopback, or there is none (work in this process).
pub fn initial_admin_local() -> bool {
    match crate::web::current_request() {
        Some(request) => request_is_local(&request, &crate::web::client_addressing().header().to_ascii_lowercase()),
        None => {
            let address = super::login::client_ip();
            address.trim().is_empty() || loopback(&address)
        }
    }
}

fn loopback(text: &str) -> bool {
    text.trim()
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.to_canonical().is_loopback())
}

fn request_is_local(request: &crate::web::WebRequest, client_ip_header: &str) -> bool {
    matches!(request.via, RequestVia::Direct | RequestVia::InProcess)
        && loopback(&request.peer_ip)
        && loopback(&crate::web::client_ip(request))
        && ["forwarded", "x-forwarded-for", "x-real-ip", client_ip_header]
            .iter()
            .all(|name| name.is_empty() || request.header(name).is_none())
}

/// Whether the current request may create the first administrator: it is [`initial_admin_local`], or `given` is this
/// server's setup secret (compared in constant time; dashes, spaces and case do not matter).
pub fn initial_admin_allowed(given: &str) -> bool {
    if initial_admin_local() {
        return true;
    }
    let Some(factory) = crate::context::ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) else {
        return false;
    };
    let expected = super::login::app_password_key(&secret(&factory).0);
    let given = super::login::app_password_key(given);
    !expected.is_empty() && moqui_lib::codec::constant_time_eq(given.as_bytes(), expected.as_bytes())
}

/// At startup, while no account exists, write the setup secret to the log with where it is used, so the operator (who
/// reads the log) can create the first administrator from another machine. With `initial_admin_enabled` off it writes
/// how to turn it on instead, and no secret. Nothing is written once an account exists.
pub fn log_initial_admin_setup(factory: &Arc<ExecutionContextFactory>) {
    let ctx = factory.new_context();
    let _entered = ctx.enter();
    let out = match crate::service::call::sync(INITIAL_ADMIN_NEEDED, &BTreeMap::new()) {
        Ok(out) => out,
        Err(err) => {
            tracing::error!("Could not tell whether the first administrator is needed: {err}");
            return;
        }
    };
    let flag = |name: &str| {
        out.get(name)
            .is_some_and(|value| moqui_lib::object::to_plain_string(value) == "true")
    };
    if flag("disabled") {
        tracing::warn!(
            "No user account exists yet. To create the first administrator at /Login, set initial_admin_enabled=true \
             (in .env or the environment) and restart; take it out again once the administrator is made."
        );
        return;
    }
    if !flag("needed") {
        return;
    }
    let secret = secret(factory);
    if secret.0.is_empty() {
        return;
    }
    tracing::warn!(
        "No user account exists yet. Create the first administrator at /Login: from this machine as is, or from \
         another address with this setup secret: {}. The secret is good until the server stops.",
        secret.0
    );
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
