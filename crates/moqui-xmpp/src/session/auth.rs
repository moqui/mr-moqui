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

//! Sign-in (docs/interfaces/XMPP.md "Sign-in"): STARTTLS, then SASL (RFC 6120 §6) or SASL2 (XEP-0388).
//!
//! - **SCRAM-SHA-256-PLUS** (tls-exporter, XEP-0440) where the connection has its own TLS session, and **SCRAM-SHA-256**,
//!   both against an app password's stored verifiers (`XmppServices.get#ScramCredentials`).
//! - **PLAIN** only over TLS, as a fallback for a client without SCRAM, and only with an app password
//!   (`check#PlainPassword`): the account password is never taken here. It would count toward the account's lockout,
//!   and cannot carry a second factor; an account with a second factor signs in with an app password, made after it.
//! - **FAST** (XEP-0484) inside SASL2: a token asked for at a sign-in, then one round trip.
//! - **Bind 2** (XEP-0386) binds a resource from the client's tag and turns on carbons, stream management and CSI in
//!   the same round trip; stream management resumes inside SASL2 too.
//!
//! A failed exchange is counted against the source address ([`crate::conn::note_failure`]); enough of them block the
//! address for a while. The account is never locked by them.
//!
//! The session remembers the app password it signed in with (a FAST token keeps the one it was issued under), and
//! [`Session::tick`] checks it now and then (`check#Credential`): revoking that app password, or disabling the
//! account, ends the session and refuses its FAST tokens.

use std::sync::OnceLock;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use minidom::Element;
use serde_json::{json, Value as Json};

use super::xml::{attr, child_text, text_el, E};
use super::{Account, Out, Session, State};
use crate::backend::{self, services};
use crate::scram::{self, ClientFirst, Credentials, ServerExchange, Verifier};
use crate::{fast, jids, ns};

/// The exchange under way.
#[derive(Default)]
pub(crate) struct AuthState {
    pending: Option<Pending>,
    /// A detached session this connection resumed; the connection carries on with it.
    pub(crate) swap: Option<Box<Session>>,
}

struct Pending {
    sasl2: bool,
    scram: ServerExchange,
    request: Option<Sasl2Request>,
    /// The app password of each verifier, by index.
    credential_ids: Vec<String>,
}

/// The parts of a SASL2 `<authenticate>` that act once it succeeds.
#[derive(Clone, Default)]
struct Sasl2Request {
    user_agent_id: String,
    bind: Option<Element>,
    request_token: Option<String>,
    invalidate_token: bool,
    resume: Option<(String, u32)>,
}

/// The key decoy salts are derived from when the server has none of its own ([`crate::Config::decoy_key`]), per
/// process.
fn process_decoy_key() -> &'static [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut key = [0_u8; 32];
        getrandom::fill(&mut key).expect("the system random source works");
        key
    })
}

fn decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.is_empty() || text == "=" {
        return Some(Vec::new());
    }
    STANDARD.decode(text).ok()
}

impl Session {
    pub(super) fn auth_features(&self, features: E) -> E {
        let mut mechanisms = E::new("mechanisms", ns::SASL);
        let mut sasl2 = E::new("authentication", ns::SASL2);
        let names = self.mechanisms();
        for name in &names {
            mechanisms = mechanisms.c(text_el("mechanism", ns::SASL, *name));
            sasl2 = sasl2.c(text_el("mechanism", ns::SASL2, *name));
        }
        let mut fast_mechanisms = E::new("fast", ns::FAST);
        if self.binding.is_some() {
            fast_mechanisms = fast_mechanisms.c(text_el("mechanism", ns::FAST, fast::HT_EXPR));
        }
        fast_mechanisms = fast_mechanisms.c(text_el("mechanism", ns::FAST, fast::HT_NONE));
        let mut bind_inline = E::new("inline", ns::BIND2);
        for var in crate::features::BIND2_INLINE {
            bind_inline = bind_inline.c(E::new("feature", ns::BIND2).a("var", *var));
        }
        sasl2 = sasl2.c(E::new("inline", ns::SASL2)
            .c(E::new("bind", ns::BIND2).c(bind_inline))
            .c(fast_mechanisms)
            .c(E::new("sm", ns::SM)));
        let mut features = features.c(mechanisms);
        if self.binding.is_some() {
            features =
                features.c(E::new("sasl-channel-binding", ns::SASL_CB)
                    .c(E::new("channel-binding", ns::SASL_CB).a("type", "tls-exporter")));
        }
        features.c(sasl2)
    }

    fn mechanisms(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.binding.is_some() {
            names.push("SCRAM-SHA-256-PLUS");
        }
        names.push("SCRAM-SHA-256");
        if self.secure() {
            names.push("PLAIN");
        }
        names
    }

    pub(super) async fn pre_auth(&mut self, element: Element) {
        let namespace = element.ns();
        // TLS is required on the plain port: nothing but STARTTLS is taken before it, so a client whose STARTTLS offer
        // was stripped on the way cannot be led to sign in in the clear
        if matches!(self.transport, super::Transport::Tcp { tls: false }) && !element.is("starttls", ns::TLS) {
            return self.fail("policy-violation", Some("STARTTLS is required first"));
        }
        match (element.name(), namespace.as_str()) {
            ("starttls", ns::TLS) => {
                if matches!(self.transport, super::Transport::Tcp { tls: false }) {
                    self.send(E::new("proceed", ns::TLS).build());
                    self.out.push(Out::StartTls);
                    self.out.push(Out::Reset);
                    self.state = State::Opening;
                } else {
                    self.fail("policy-violation", Some("TLS is already on"));
                }
            }
            ("auth", ns::SASL) => {
                let mechanism = attr(&element, "mechanism");
                let Some(initial) = decode(&element.text()) else {
                    return self.sasl_failure(false, "incorrect-encoding").await;
                };
                self.start(false, &mechanism, initial, None).await;
            }
            ("authenticate", ns::SASL2) => {
                let mechanism = attr(&element, "mechanism");
                let initial = match element.get_child("initial-response", ns::SASL2) {
                    Some(response) => match decode(&response.text()) {
                        Some(initial) => initial,
                        None => return self.sasl_failure(true, "incorrect-encoding").await,
                    },
                    None => Vec::new(),
                };
                let request = Sasl2Request {
                    user_agent_id: element
                        .get_child("user-agent", ns::SASL2)
                        .map(|agent| attr(agent, "id"))
                        .unwrap_or_default(),
                    bind: element.get_child("bind", ns::BIND2).cloned(),
                    request_token: element
                        .get_child("request-token", ns::FAST)
                        .map(|request| attr(request, "mechanism")),
                    invalidate_token: element
                        .get_child("fast", ns::FAST)
                        .is_some_and(|fast| matches!(attr(fast, "invalidate").as_str(), "true" | "1")),
                    resume: element
                        .get_child("resume", ns::SM)
                        .map(|resume| (attr(resume, "previd"), attr(resume, "h").parse().unwrap_or(0))),
                };
                if let Some(binds) = fast::binds(&mechanism) {
                    let count = element
                        .get_child("fast", ns::FAST)
                        .and_then(|fast| fast.attr("count"))
                        .and_then(|count| count.parse::<i64>().ok());
                    self.fast(binds, &initial, count, request).await;
                } else {
                    self.start(true, &mechanism, initial, Some(request)).await;
                }
            }
            ("response", ns::SASL) | ("response", ns::SASL2) => {
                let Some(data) = decode(&element.text()) else {
                    return self.sasl_failure(namespace == ns::SASL2, "incorrect-encoding").await;
                };
                self.respond(&data).await;
            }
            ("abort", ns::SASL) | ("abort", ns::SASL2) => {
                self.auth.pending = None;
                self.sasl_failure(namespace == ns::SASL2, "aborted").await;
            }
            _ => self.fail("not-authorized", None),
        }
    }

    async fn start(&mut self, sasl2: bool, mechanism: &str, initial: Vec<u8>, request: Option<Sasl2Request>) {
        if !self.mechanisms().contains(&mechanism) {
            return self.sasl_failure(sasl2, "invalid-mechanism").await;
        }
        if mechanism == "PLAIN" {
            return self.plain(sasl2, &initial, request).await;
        }
        let plus = mechanism.ends_with("-PLUS");
        let first = match scram::client_first(&initial, plus, self.binding.is_some()) {
            Ok(first) => first,
            Err(_) => return self.sasl_failure(sasl2, "not-authorized").await,
        };
        let (credentials, credential_ids) = match self.credentials(&first).await {
            Some((credentials, ids)) => (Some(credentials), ids),
            None => (None, Vec::new()),
        };
        let binding = if plus { self.binding.clone() } else { None };
        let decoy: &[u8] = if self.server.config.decoy_key.is_empty() {
            process_decoy_key()
        } else {
            &self.server.config.decoy_key
        };
        let (exchange, challenge) = ServerExchange::start(first, credentials, binding, decoy);
        self.auth.pending = Some(Pending {
            sasl2,
            scram: exchange,
            request,
            credential_ids,
        });
        let ns = if sasl2 { ns::SASL2 } else { ns::SASL };
        self.send(text_el("challenge", ns, STANDARD.encode(challenge)));
    }

    /// The user's verifiers, and the app password of each.
    async fn credentials(&self, first: &ClientFirst) -> Option<(Credentials, Vec<String>)> {
        // the account's own username: a JID's localpart is in lower case, the username may not be
        let username = self.account_of(&jids::unescape_node(&first.username)).await?.username;
        let found = self
            .call_internal(services::SCRAM, json!({"username": username}))
            .await
            .ok()?;
        let salt = STANDARD.decode(backend::text(&found, "salt")).ok()?;
        let iterations = u32::try_from(backend::number(&found, "iterations")?).ok()?;
        let decode_list = |field: &str| -> Vec<Vec<u8>> {
            backend::list(&found, field)
                .iter()
                .filter_map(Json::as_str)
                .filter_map(|key| STANDARD.decode(key).ok())
                .collect()
        };
        let verifiers: Vec<Verifier> = decode_list("storedKeyList")
            .into_iter()
            .zip(decode_list("serverKeyList"))
            .map(|(stored_key, server_key)| Verifier { stored_key, server_key })
            .collect();
        let ids: Vec<String> = backend::list(&found, "appPasswordIdList")
            .iter()
            .map(|id| id.as_str().unwrap_or_default().to_string())
            .collect();
        (!verifiers.is_empty()).then_some((
            Credentials {
                salt,
                iterations,
                verifiers,
            },
            ids,
        ))
    }

    async fn respond(&mut self, data: &[u8]) {
        let Some(pending) = self.auth.pending.take() else {
            return self.sasl_failure(false, "not-authorized").await;
        };
        match pending.scram.finish(data) {
            Ok((index, server_final)) => {
                let username = jids::unescape_node(pending.scram.username());
                let credential = pending.credential_ids.get(index).filter(|id| !id.is_empty()).cloned();
                match self.account_of(&username).await {
                    Some(account) => {
                        self.succeed(pending.sasl2, account, credential, server_final, pending.request)
                            .await
                    }
                    None => self.sasl_failure(pending.sasl2, "not-authorized").await,
                }
            }
            Err(_) => self.sasl_failure(pending.sasl2, "not-authorized").await,
        }
    }

    async fn account_of(&self, username: &str) -> Option<Account> {
        let found = self
            .call_internal(services::ACCOUNT, json!({"username": username}))
            .await
            .ok()?;
        if !backend::flag(&found, "canLogin") {
            return None;
        }
        Account::from_json(&found)
    }

    async fn plain(&mut self, sasl2: bool, initial: &[u8], request: Option<Sasl2Request>) {
        let mut parts = initial.split(|b| *b == 0);
        let (Some(authzid), Some(authcid), Some(password), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return self.sasl_failure(sasl2, "malformed-request").await;
        };
        let (Ok(authzid), Ok(authcid), Ok(password)) = (
            std::str::from_utf8(authzid),
            std::str::from_utf8(authcid),
            std::str::from_utf8(password),
        ) else {
            return self.sasl_failure(sasl2, "malformed-request").await;
        };
        let username = jids::unescape_node(authcid);
        if !authzid.is_empty() && jids::parse(authzid).and_then(|jid| jid.node).as_deref() != Some(authcid) {
            return self.sasl_failure(sasl2, "invalid-authzid").await;
        }
        let checked = self
            .call_internal(
                services::CHECK_PLAIN,
                json!({"username": username, "password": password}),
            )
            .await;
        let credential = checked
            .as_ref()
            .ok()
            .map(|found| backend::text(found, "appPasswordId"))
            .filter(|id| !id.is_empty());
        match checked.ok().as_ref().and_then(Account::from_json) {
            Some(account) => self.succeed(sasl2, account, credential, Vec::new(), request).await,
            None => self.sasl_failure(sasl2, "not-authorized").await,
        }
    }

    async fn fast(&mut self, binds: bool, initial: &[u8], count: Option<i64>, request: Sasl2Request) {
        let binding = if binds {
            match &self.binding {
                Some(binding) => binding.clone(),
                None => return self.sasl_failure(true, "invalid-mechanism").await,
            }
        } else {
            Vec::new()
        };
        let Some((name, proof)) = fast::parse_initial(initial) else {
            return self.sasl_failure(true, "malformed-request").await;
        };
        let username = jids::unescape_node(&name);
        let found = self
            .call_internal(
                services::FAST_TOKEN,
                json!({"username": username, "userAgentId": request.user_agent_id}),
            )
            .await
            .unwrap_or(Json::Null);
        let token_list = backend::list(&found, "tokenList");
        let matched = token_list
            .iter()
            .find(|token| fast::verify(&backend::text(token, "token"), &binding, &proof));
        let Some(token) = matched.cloned() else {
            return self.sasl_failure(true, "not-authorized").await;
        };
        // without channel binding a token's count must grow, so a captured exchange is not replayed
        let used = self
            .call_internal(
                services::USE_FAST_TOKEN,
                json!({"fastTokenId": backend::text(&token, "fastTokenId"), "count": count, "bound": binds}),
            )
            .await;
        if used.is_err() {
            return self.sasl_failure(true, "not-authorized").await;
        }
        let Some(account) = self.account_of(&username).await else {
            return self.sasl_failure(true, "not-authorized").await;
        };
        let credential = Some(backend::text(&token, "appPasswordId")).filter(|id| !id.is_empty());
        let responder = fast::responder(&backend::text(&token, "token"), &binding);
        if request.invalidate_token {
            let _ = self
                .call_internal(
                    services::INVALIDATE_FAST_TOKEN,
                    json!({"fastTokenId": backend::text(&token, "fastTokenId")}),
                )
                .await;
        }
        self.succeed(true, account, credential, responder, Some(request)).await;
    }

    async fn sasl_failure(&mut self, sasl2: bool, condition: &str) {
        self.auth.pending = None;
        let blocked = crate::conn::note_failure(&self.server, self.peer);
        tracing::warn!("XMPP sign-in from {} failed: {condition}", self.peer);
        let failure = if sasl2 {
            E::new("failure", ns::SASL2).c(E::new(condition, ns::SASL)).build()
        } else {
            E::new("failure", ns::SASL).c(E::new(condition, ns::SASL)).build()
        };
        self.send(failure);
        if blocked {
            self.fail("policy-violation", Some("too many failed sign-ins from this address"));
        }
    }

    async fn succeed(
        &mut self,
        sasl2: bool,
        account: Account,
        credential: Option<String>,
        additional: Vec<u8>,
        request: Option<Sasl2Request>,
    ) {
        tracing::info!("XMPP sign-in of {} from {}", account.username, self.peer);
        self.account = Some(account);
        self.credential = credential;
        if !sasl2 {
            let data = if additional.is_empty() {
                String::new()
            } else {
                STANDARD.encode(&additional)
            };
            self.send(text_el("success", ns::SASL, data));
            self.out.push(Out::Reset);
            self.state = State::Opening;
            return;
        }
        let request = request.unwrap_or_default();
        let mut success = E::new("success", ns::SASL2);
        if !additional.is_empty() {
            success = success.c(text_el("additional-data", ns::SASL2, STANDARD.encode(&additional)));
        }
        let user_id = self.user_id();
        // resumption inside SASL2: the detached session carries on over this connection
        if let Some((previd, h)) = request.resume.clone() {
            if let Some(mut old) = self.server.registry.take_detached(&previd, &user_id).await {
                let resumed = old.resumed_element(h);
                let jid = old.my_full();
                success = success
                    .c(text_el("authorization-identifier", ns::SASL2, jid))
                    .c(resumed);
                old.adopt_connection(self);
                old.send(success.build());
                old.replay_unacked();
                self.auth.swap = Some(old);
                self.state = State::Closed;
                return;
            }
            success = success.c(E::new("failed", ns::SM)
                .a("h", "0")
                .c(E::new("item-not-found", ns::STANZAS)));
        }
        let mut token = None;
        if let Some(mechanism) = request.request_token.as_deref() {
            // a token stands for the app password signed in with, and goes with it
            if let (Some(_), Some(credential)) = (fast::binds(mechanism), self.credential.clone()) {
                let secret = fast::new_token();
                let lifetime = self.server.config.fast_token_lifetime.as_secs();
                let issued = self
                    .call_internal(
                        services::ISSUE_FAST_TOKEN,
                        json!({"userId": user_id, "userAgentId": request.user_agent_id, "mechanism": mechanism,
                            "token": secret, "lifetimeSeconds": lifetime, "appPasswordId": credential}),
                    )
                    .await;
                if let Ok(issued) = issued {
                    token = Some(
                        E::new("token", ns::FAST)
                            .a("expiry", backend::text(&issued, "expiry"))
                            .a("token", secret)
                            .build(),
                    );
                }
            }
        }
        match &request.bind {
            Some(bind) => {
                let tag = child_text(bind, "tag", ns::BIND2);
                self.bind_resource(&tag, true);
                self.register();
                let mut bound = E::new("bound", ns::BIND2);
                if bind.has_child("enable", ns::CARBONS) {
                    self.carbons.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if bind.has_child("inactive", ns::CSI) {
                    self.csi_inactive = true;
                }
                if let Some(enable) = bind.get_child("enable", ns::SM) {
                    bound = bound.c(self.enable_sm(enable));
                }
                success = success
                    .c(text_el("authorization-identifier", ns::SASL2, self.my_full()))
                    .c(bound);
            }
            None => {
                success = success.c(text_el("authorization-identifier", ns::SASL2, self.my_bare()));
                self.state = State::Authenticated;
            }
        }
        if let Some(token) = token {
            success = success.c(token);
        }
        self.send(success.build());
        if request.bind.is_none() {
            let features = E::new("features", ns::STREAM)
                .c(E::new("bind", ns::BIND))
                .c(E::new("sm", ns::SM))
                .c(E::new("csi", ns::CSI))
                .build();
            self.send(features);
        }
    }

    /// The resource from the client's wish (a tag, with a random part after it for Bind 2), unique for the user.
    pub(super) fn bind_resource(&mut self, wish: &str, tagged: bool) {
        let wish: String = wish
            .trim()
            .chars()
            .filter(|ch| !ch.is_control() && *ch != '/')
            .take(64)
            .collect();
        let user_id = self.user_id();
        let mut resource = match (wish.is_empty(), tagged) {
            (true, _) => super::xml::random_id(),
            (false, true) => format!("{wish}.{}", &super::xml::random_id()[..6]),
            (false, false) => wish,
        };
        if self.server.registry.session(&user_id, &resource).is_some() {
            resource = format!("{resource}.{}", &super::xml::random_id()[..6]);
        }
        self.resource = Some(resource);
    }

    /// RFC 6120 resource binding, after SASL.
    pub(super) async fn pre_bind(&mut self, element: Element) {
        if element.ns() == ns::SM && element.name() == "resume" {
            return self.resume(&element).await;
        }
        if element.is("iq", ns::CLIENT) && attr(&element, "type") == "set" {
            if let Some(bind) = element.get_child("bind", ns::BIND) {
                let wish = child_text(bind, "resource", ns::BIND);
                self.bind_resource(&wish, false);
                self.register();
                let result = super::xml::iq_result(&element)
                    .a("to", self.my_full())
                    .c(E::new("bind", ns::BIND).c(text_el("jid", ns::BIND, self.my_full())))
                    .build();
                self.send(result);
                return;
            }
        }
        if element.ns() == ns::SM && element.name() == "enable" {
            self.send(
                E::new("failed", ns::SM)
                    .c(E::new("unexpected-request", ns::STANZAS))
                    .build(),
            );
            return;
        }
        self.fail("not-authorized", Some("bind a resource first"));
    }
}
