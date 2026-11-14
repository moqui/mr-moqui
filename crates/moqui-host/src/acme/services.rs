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

//! `org.moqui.admin.CertificateServices` (plans/Edge-Node2.md §4.8): what System › Certificates lists and does, and
//! the renewal job. A refusal adds an error and answers a `reasonCode`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::json::from_json;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::json;

use super::store::{self, NameRow};
use super::{check, normal_name, order, Acme};

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        register_services,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, get_certificates)?;
    moqui_core::rust_service!(map, setup_acme_name)?;
    moqui_core::rust_service!(map, renew_acme_name)?;
    moqui_core::rust_service!(map, update_acme_name)?;
    moqui_core::rust_service!(map, remove_acme_name)?;
    moqui_core::rust_service!(map, renew_acme_certificates)?;
    Ok(())
}

type Out = Result<BTreeMap<String, Value>, LibError>;

/// Let's Encrypt's staging directory, which the screen offers for a first try.
pub(crate) const STAGING_URL: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

fn factory() -> Result<Arc<ExecutionContextFactory>, LibError> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("a certificate service needs an execution context"))
}

fn refuse(reason_code: &str, message: &str) -> Out {
    let message = message.to_string();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
    Ok(BTreeMap::from([("reasonCode".to_string(), Value::string(reason_code))]))
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> String {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn flag(parameters: &BTreeMap<String, Value>, key: &str) -> bool {
    matches!(
        text(parameters, key).to_ascii_lowercase().as_str(),
        "y" | "true" | "on" | "1"
    )
}

/// A DNS name an authority could issue for: letters, digits and hyphens in dotted labels, no wildcard, not an address.
pub(crate) fn valid_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        && labels
            .last()
            .is_some_and(|top| !top.bytes().all(|byte| byte.is_ascii_digit()))
}

fn property(name: &str) -> String {
    normal_name(&moqui_lib::env::get_prop_or_env(name))
}

/// The names this node answers, each with what makes it one (its own host, a terminate host, a site, the SMTP host, the
/// XMPP domain, a name set up here, a file in `runtime/cert`).
fn answered_names(factory: &ExecutionContextFactory, set_up: &[NameRow]) -> BTreeMap<String, BTreeSet<String>> {
    let mut names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut add = |name: String, why: String| {
        if !name.is_empty() && name != "localhost" && name != "default" {
            names.entry(name).or_default().insert(why);
        }
    };
    add(property("webapp_http_host"), "This node's host".to_string());
    for host in moqui_core::screen::public_site_hosts(factory) {
        add(normal_name(&host), "A site's host".to_string());
    }
    add(property("smtp_hostname"), "The SMTP host".to_string());
    add(property("xmpp_domain"), "The XMPP domain".to_string());
    match terminate_hosts(factory) {
        Ok(hosts) => {
            for (host, node) in hosts {
                add(host, format!("A terminate host of {node}"));
            }
        }
        Err(err) => tracing::error!("System › Certificates: the edge node's host names could not be read: {err}"),
    }
    for name in set_up {
        add(name.host.clone(), "Set up with ACME".to_string());
    }
    for pair in crate::tls::pairs_in(&store::cert_dir(factory)) {
        let shown = pair
            .strip_prefix("_wildcard.")
            .map_or_else(|| pair.clone(), |parent| format!("*.{parent}"));
        add(normal_name(&shown), "A file in runtime/cert".to_string());
    }
    names
}

/// On an edge node, its terminate hosts and the node each goes to: the names it ends TLS for itself.
fn terminate_hosts(factory: &ExecutionContextFactory) -> Result<Vec<(String, String)>, LibError> {
    const HOST: &str = "moqui.edge.EdgeHost";
    let facade = store::facade(factory)?;
    let rows = store::committed(&facade, HOST, || {
        facade
            .find(HOST)?
            .eq("modeEnumId", Value::string("EdgeHostTerminate"))
            .list()
    })?;
    Ok(rows
        .iter()
        .map(|row| {
            let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
            (normal_name(&field("hostName")), field("pairedNodeId"))
        })
        .collect())
}

/// The layout a pair's files are in (`crate::tls`), from their paths.
fn layout(served: &crate::tls::Served) -> &'static str {
    let file = |path: &std::path::Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string()
    };
    if ["fullchain.pem", "cert.pem"].contains(&file(&served.cert).as_str()) {
        "certbot"
    } else if file(&served.key).ends_with("-key.pem") {
        "mkcert"
    } else {
        "nginx-proxy"
    }
}

/// `get#Certificates`: the names this node answers and the certificate each is served, with the names set up here.
fn get_certificates(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let factory = factory()?;
    let set_up = store::names(&factory)?;
    let locked = store::live_locks(&factory)?;
    let dir = store::cert_dir(&factory);
    let now = moqui_core::user::now_millis();
    let mut list = Vec::new();
    for (name, reasons) in answered_names(&factory, &set_up) {
        let acme = set_up.iter().find(|row| row.host == name);
        let served = crate::tls::served_for(&dir, &name);
        let mut entry = json!({
            "hostName": name,
            "reasons": reasons.into_iter().collect::<Vec<_>>(),
            "found": served.is_some(),
        });
        if let Some(served) = served {
            let relative = served
                .cert
                .strip_prefix(factory.runtime_dir())
                .map_or_else(|_| served.cert.display().to_string(), |path| path.display().to_string());
            let not_after = served
                .leaf
                .as_ref()
                .and_then(|leaf| moqui_lib::certificate::not_after(leaf))
                .map(|seconds| seconds.saturating_mul(1000));
            entry["certificate"] = json!({
                "pair": served.pair,
                "matchKind": if served.pair == name { "exact" } else if served.pair == "default" { "default" } else { "wildcard" },
                "file": relative,
                "layout": layout(&served),
                "acmeWritten": served.cert.parent().is_some_and(|parent| parent.join(store::MARKER).is_file()),
                "issuer": served.leaf.as_ref().and_then(|leaf| moqui_lib::certificate::issuer(leaf)),
                "notAfter": not_after,
                "daysLeft": not_after.map(|until| (until - now) / 86_400_000),
                "problem": served.problem,
            });
        }
        if let Some(acme) = acme {
            // an order whose lock lapsed was cut off by a stop: it shows as failed, and can be renewed
            let cut_off = acme.status == store::ORDERING && order::failed_now(acme, &locked);
            let (status, last_error) = if cut_off {
                (
                    store::FAILED,
                    "The order was cut off before it finished: the server stopped during it".to_string(),
                )
            } else {
                (acme.status.as_str(), acme.last_error.clone())
            };
            entry["acme"] = json!({
                "directoryUrl": acme.directory_url,
                "contactEmail": acme.contact_email,
                "renewing": acme.renewing,
                "statusId": status,
                "lastAttemptDate": acme.last_attempt,
                "lastError": last_error,
                "notAfter": acme.not_after,
                "setUpDate": acme.set_up,
                "ordering": Acme::of(&factory).is_ordering(&name) || order::ordering_now(acme, &locked),
                // another tool's files for the name are served before this module's
                "shadowed": crate::tls::find_files(&dir, &name)
                    .is_some_and(|(cert, _)| !cert.starts_with(dir.join(&name))),
            });
        }
        list.push(entry);
    }
    let listening = Acme::of(&factory).listening().map(|address| address.port());
    Ok(BTreeMap::from([
        (
            "certificateList".to_string(),
            from_json(&serde_json::Value::Array(list)),
        ),
        (
            "listenPort".to_string(),
            listening.map_or(Value::Null, |port| Value::I64(i64::from(port))),
        ),
        (
            "directoryUrl".to_string(),
            Value::string(moqui_lib::env::get_prop_or_env("acme_directory_url").trim()),
        ),
        ("stagingUrl".to_string(), Value::string(STAGING_URL)),
        (
            "challengePort".to_string(),
            Value::I64(i64::from(check::challenge_port())),
        ),
    ]))
}

/// `setup#AcmeName`: check the name reaches this node (unless `skipCheck`), save it, and start its order.
fn setup_acme_name(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let factory = factory()?;
    if !Acme::of(&factory).has_https() {
        return refuse(
            "httpsOff",
            "This node has no HTTPS listener (webapp_https_listen_port is off), so it cannot answer the authority's \
             challenge",
        );
    }
    let host = normal_name(&text(parameters, "hostName"));
    if !valid_host(&host) {
        return refuse(
            "badName",
            "Give a DNS name such as www.example.com: an authority issues no certificate for an address, a wildcard or a single label",
        );
    }
    let mut directory_url = text(parameters, "directoryUrl");
    if directory_url.is_empty() {
        directory_url = moqui_lib::env::get_prop_or_env("acme_directory_url").trim().to_string();
    }
    if !directory_url.starts_with("https://") {
        return refuse("badDirectory", "The authority's directory URL starts with https://");
    }
    if !flag(parameters, "termsAgreed") {
        return refuse(
            "termsNotAgreed",
            "Agree to the authority's terms of service to set the name up",
        );
    }
    let contact = text(parameters, "contactEmail");
    if !contact.is_empty() && (!contact.contains('@') || contact.contains(char::is_whitespace)) {
        return refuse("badContact", "The contact is an email address, or empty");
    }
    let dir = store::cert_dir(&factory);
    if store::foreign_dir(&dir, &host) {
        return refuse(
            "otherTool",
            &format!(
                "runtime/cert/{host}/ holds files another tool wrote, such as certbot: this node does not write over \
                 them. Remove that directory first, or keep using that tool"
            ),
        );
    }
    let check_text = if flag(parameters, "skipCheck") {
        String::new()
    } else {
        match check::check(&factory, &host) {
            Ok(found) => found,
            Err(err) => {
                return refuse(
                    "checkFailed",
                    &format!(
                        "{}. Point the name's DNS at this node (or at its edge node, as a passthrough or terminate \
                         host) and make port {} answer, or order without the check",
                        err.message(),
                        check::challenge_port()
                    ),
                )
            }
        }
    };
    let user_id = moqui_core::user::current_user_id().ok().flatten();
    store::save_name(&factory, &host, &directory_url, &contact, user_id.as_deref())?;
    Acme::of(&factory).want();
    if let Err(err) = store::renewal_job(&factory, true) {
        tracing::error!("ACME: the renewal job could not be started: {err}");
    }
    order::start(&factory, &host)?;
    Ok(BTreeMap::from([
        ("hostName".to_string(), Value::string(&host)),
        ("checkText".to_string(), Value::string(check_text)),
    ]))
}

/// `renew#AcmeName`: order a name set up here again now.
fn renew_acme_name(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let factory = factory()?;
    let host = normal_name(&text(parameters, "hostName"));
    if store::name(&factory, &host)?.is_none() {
        return refuse("notSetUp", &format!("{host} is not set up with ACME here"));
    }
    if Acme::of(&factory).is_ordering(&host) || store::live_locks(&factory)?.contains(&host) {
        return refuse("ordering", &format!("{host} is being ordered now"));
    }
    order::start(&factory, &host)?;
    Ok(BTreeMap::from([("hostName".to_string(), Value::string(&host))]))
}

/// `update#AcmeName`: stop renewing a name, or renew it again.
fn update_acme_name(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let factory = factory()?;
    let host = normal_name(&text(parameters, "hostName"));
    let renewing = flag(parameters, "renewing");
    if !store::set_renewing(&factory, &host, renewing)? {
        return refuse("notSetUp", &format!("{host} is not set up with ACME here"));
    }
    Ok(BTreeMap::from([("renewing".to_string(), Value::Bool(renewing))]))
}

/// `remove#AcmeName`: forget a name set up here, with its certificate and files; the job pauses with the last one.
fn remove_acme_name(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let factory = factory()?;
    let host = normal_name(&text(parameters, "hostName"));
    let removed = store::remove(&factory, &host)?;
    if removed && store::names(&factory)?.is_empty() {
        if let Err(err) = store::renewal_job(&factory, false) {
            tracing::error!("ACME: the renewal job could not be paused: {err}");
        }
    }
    Ok(BTreeMap::from([("removed".to_string(), Value::Bool(removed))]))
}

/// `renew#AcmeCertificates`, the job: renew each name due.
fn renew_acme_certificates(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let factory = factory()?;
    let (renewed, failed) = order::renew_due(&factory)?;
    let count = |n: usize| Value::I64(i64::try_from(n).unwrap_or(i64::MAX));
    Ok(BTreeMap::from([
        ("renewed".to_string(), count(renewed)),
        ("failed".to_string(), count(failed)),
    ]))
}
