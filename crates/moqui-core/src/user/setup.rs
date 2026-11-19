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
//! held in memory only, so a restart makes a new one. None of it is offered unless `user-facade/login@initial-admin-enabled` is true
//! (`get#InitialAdminNeeded`; false in the default conf, true in the dev conf). On a machine other people sign in to,
//! `user-facade/login@initial-admin-local` `in-process` narrows "this machine" to the server's own process: the Desktop
//! app's node sets it, so another account of the computer that reaches the node's ports needs the secret too
//! (SEC-CLI-101). A starter that hands the secret to its own client sets it (`initial-admin-secret`, a secret of the
//! conf, through the environment): the app does for its sidecar, whose window it gives the secret, and the log then
//! names where it is set, not the secret.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::context::ExecutionContextFactory;
use crate::web::RequestVia;

const INITIAL_ADMIN_NEEDED: &str = "org.moqui.impl.UserServices.get#InitialAdminNeeded";

/// A setup secret the server's starter chose: the Desktop app sets it for its sidecar, whose window it hands it.
const SECRET_SETTING: &str = "user-facade/login@initial-admin-secret";

/// The setup secret of one server: the conf's (`initial-admin-secret`), else one made at the first use, an app
/// password's form, about 99 bits; and whether the conf gave it.
struct SetupSecret(String, bool);

fn secret(factory: &ExecutionContextFactory) -> Arc<SetupSecret> {
    factory.extension(|factory| {
        let given = crate::context::conf_env::setting_on(factory, SECRET_SETTING);
        if !given.is_empty() {
            return SetupSecret(given, true);
        }
        let made = super::login::new_app_password().unwrap_or_else(|err| {
            // no random source: no secret works, so only a request from this machine creates the administrator
            tracing::error!("No setup secret for the first administrator: {err}");
            String::new()
        });
        SetupSecret(made, false)
    })
}

/// Who counts as this machine: `loopback` (the default) or `in-process`.
const LOCAL_SETTING: &str = "user-facade/login@initial-admin-local";

/// Whether only the server's own process counts as this machine (`initial-admin-local` `in-process`).
fn in_process_only() -> bool {
    crate::context::conf_env::setting(LOCAL_SETTING) == "in-process"
}

/// Whether the current caller is on this machine. A request: a loopback peer that reached the listener itself (or the
/// Desktop app's node in process), with no proxy's forwarding header, so a reverse proxy on the same host does not make
/// every client local. With no request (the terminal's dialog over SSH, server code): the address
/// [`super::with_client_ip`] gave is loopback, or there is none (work in this process). With `initial-admin-local`
/// `in-process`, only a request made in the server's process and work with no address count.
pub fn initial_admin_local() -> bool {
    let in_process = in_process_only();
    match crate::web::current_request() {
        Some(request) => request_is_local(
            &request,
            &crate::web::client_addressing().header().to_ascii_lowercase(),
            in_process,
        ),
        None => no_request_is_local(&super::login::client_ip(), in_process),
    }
}

/// Work with no request: local when it has no client address (this process), or a loopback one unless only the
/// process counts (another account of the computer reaches an SSH listener from loopback too).
fn no_request_is_local(address: &str, in_process_only: bool) -> bool {
    address.trim().is_empty() || (!in_process_only && loopback(address))
}

fn loopback(text: &str) -> bool {
    text.trim()
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.to_canonical().is_loopback())
}

fn request_is_local(request: &crate::web::WebRequest, client_ip_header: &str, in_process_only: bool) -> bool {
    let via = if in_process_only {
        matches!(request.via, RequestVia::InProcess)
    } else {
        matches!(request.via, RequestVia::Direct | RequestVia::InProcess)
    };
    via && loopback(&request.peer_ip)
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
/// reads the log) can create the first administrator from another machine. With `user-facade/login@initial-admin-enabled` off it writes
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
            "No user account exists yet. To create the first administrator at /Login, set \
             moqui_conf__user_facade__login__initial_admin_enabled=true (in .env or the environment) and restart; take it \
             out again once the administrator is made."
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
    let local = if in_process_only() {
        "in the app's own window as is"
    } else {
        "from this machine as is"
    };
    if secret.1 {
        // the starter set it and keeps it: the log (which a sidecar's app writes to files of its own) never holds it
        tracing::warn!(
            "No user account exists yet. Create the first administrator at /Login: {local}, or with the setup secret \
             {SECRET_SETTING} sets, which its starter has."
        );
        return;
    }
    tracing::warn!(
        "No user account exists yet. Create the first administrator at /Login: {local}, or from another address with \
         this setup secret: {}. The secret is good until the server stops.",
        secret.0
    );
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
