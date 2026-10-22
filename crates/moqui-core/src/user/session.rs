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

//! Session state owned by the [`ExecutionContext`] it belongs to.
//!
//! A bound web session also lives in the factory map and flushes to `moqui.server.HttpSession`.
//! The authorization cache stays on the context and is not written to the row. With no context
//! entered, reads see an empty session and writes are dropped.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, MutexGuard};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use hmac::{Hmac, Mac};
use moqui_db::sql::{
    delete as sql_delete, insert as sql_insert, select as sql_select, update as sql_update, CompareOp, Condition,
    FieldRef, LimitOffset,
};
use moqui_lib::codec::hex_encode;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::Value as Json;
use sha2::Sha256;

use super::authz::AuthzCheck;
use super::login::now_millis;
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::driver::{DbConnection, DbRow};
use crate::entity::{cache, EntityFacade, EntityValue};

/// What the artifact authorization checks cached for the logged-in user, and when.
#[derive(Clone, Debug, Default)]
pub(crate) struct AuthzCache {
    pub(crate) generation: u64,
    pub(crate) groups: Option<Arc<std::collections::HashSet<String>>>,
    pub(crate) checks: Option<Arc<Vec<Arc<AuthzCheck>>>>,
    pub(crate) tarpit: BTreeMap<String, Arc<Vec<TarpitCheck>>>,
}

/// One tarpit rule for an artifact type, for the user's groups.
#[derive(Clone, Debug)]
pub(crate) struct TarpitCheck {
    pub(crate) artifact_name: String,
    pub(crate) name_is_pattern: bool,
    pub(crate) max_hits_count: i64,
    pub(crate) max_hits_duration: i64,
    pub(crate) tarpit_duration: i64,
}

pub const SESSION_COOKIE_NAME: &str = "moqui.session";
pub const VISITOR_COOKIE_NAME: &str = "moqui.visitor";

const HTTP_SESSION: &str = "moqui.server.HttpSession";
const VISITOR: &str = "moqui.server.Visitor";
const VISIT: &str = "moqui.server.Visit";
const USER_ACCOUNT: &str = "moqui.security.UserAccount";
const SAVED: &str = "moqui.saved.parameters";
const ERROR_PARAMETERS: &str = "moqui.error.parameters";
const MESSAGE_PUBLIC: &str = "moqui.message.public";
const MESSAGE_ERROR: &str = "moqui.message.error";
/// The type of each saved message (`info`, `success`, `warning`, `danger`), in the order of the messages.
const MESSAGE_TYPES: &str = "moqui.message.types";
/// The validation errors a failed submit left, each `{form, field, serviceName, message}`.
const MESSAGE_VALIDATION: &str = "moqui.message.validationErrors";
const SCREEN_HISTORY: &str = "moqui.screen.history";
const LAST_PATH: &str = "moqui.screen.last.path";
const LAST_PARAMETERS: &str = "moqui.screen.last.parameters";
const MAX_FLUSH_ATTEMPTS: usize = 5;
/// Live sessions a factory keeps when `session-config` `max` is unset.
pub(crate) const DEFAULT_MAX_LIVE: usize = 10_000;
const VISITOR_MAX_AGE: u64 = 31_536_000;
/// `sessionId` is dictionary type `id` (`VARCHAR(40)`). Hex encoding uses two characters per byte.
const SESSION_ID_BYTES: usize = 20;
/// `sessionToken` is `text-short` (`VARCHAR(63)`). Twenty bytes is forty hex characters.
const SESSION_TOKEN_BYTES: usize = 20;
const ROW_FIELDS: &[&str] = &[
    "sessionId",
    "userId",
    "visitId",
    "visitorId",
    "locale",
    "timeZone",
    "sessionToken",
    "attributes",
    "sessionRevision",
    "lastUpdatedStamp",
];
const INSERT_COLUMNS: &[&str] = ROW_FIELDS;

type SessionMac = Hmac<Sha256>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlushMode {
    Eventual,
    Guarantee,
}

/// Factory-owned sessions and the flush mode from the `webroot` webapp's `session-config` `flush`.
///
/// The lock is held only over memory. A flush copies what it writes, claims the id in `flushing`
/// and does its SQL without the lock.
pub struct SessionTable {
    pub(crate) mode: FlushMode,
    sessions: BTreeMap<String, LiveSession>,
    /// Millis of the last sweep of expired `HttpSession` rows.
    last_purge: i64,
    /// Most live sessions kept, from `session-config` `max`. `None` keeps every session.
    max_live: Option<usize>,
    /// Ids a flush or a row delete is working on. One at a time per id.
    flushing: BTreeSet<String>,
    /// Signalled when an id leaves `flushing`.
    flushed: Arc<Condvar>,
    /// Sessions that expired since the listeners last heard, for their `sessionDestroyed` ([`take_expired`]).
    expired: Vec<String>,
}

impl SessionTable {
    pub(crate) fn new(mode: FlushMode, max_live: Option<usize>) -> Self {
        Self {
            mode,
            sessions: BTreeMap::new(),
            last_purge: 0,
            max_live,
            flushing: BTreeSet::new(),
            flushed: Arc::new(Condvar::new()),
            expired: Vec::new(),
        }
    }
}

/// An attribute of the `webroot` webapp's `session-config` in the merged conf, `${}` expanded and trimmed.
fn session_config_attr(conf: &MNode, name: &str) -> String {
    conf.first_named("webapp-list")
        .and_then(|list| list.first_named_attrs("webapp", &[("name", "webroot")]))
        .and_then(|webapp| webapp.first_named("session-config"))
        .and_then(|config| config.attribute(name))
        .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw))
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// The flush mode (`session-config` `flush`) and the most live sessions (`max`, 10000 when unset; 0 or a negative
/// value keeps every session) from the merged conf.
pub(crate) fn settings_from_conf(conf: &MNode) -> Result<(FlushMode, Option<usize>), Error> {
    let mode = match session_config_attr(conf, "flush").as_str() {
        "" | "eventual" => FlushMode::Eventual,
        "guarantee" => FlushMode::Guarantee,
        other => {
            return Err(Error::new(format!(
                "session-config flush must be eventual or guarantee, not {other}"
            )))
        }
    };
    let max_live = match session_config_attr(conf, "max").parse::<i64>() {
        Ok(max) if max > 0 => Some(usize::try_from(max).unwrap_or(usize::MAX)),
        Ok(_) => None,
        Err(_) => Some(DEFAULT_MAX_LIVE),
    };
    Ok((mode, max_live))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Column {
    UserId,
    VisitId,
    VisitorId,
    Locale,
    TimeZone,
    SessionToken,
}

#[derive(Clone, Debug, Default)]
struct Columns {
    user_id: Option<String>,
    visit_id: Option<String>,
    visitor_id: Option<String>,
    locale: Option<String>,
    time_zone: Option<String>,
    session_token: Option<String>,
}

#[derive(Clone, Debug)]
struct Snapshot {
    revision: i64,
    revision_value: Value,
    columns: Columns,
    attributes: BTreeMap<String, Json>,
}

#[derive(Clone, Debug)]
struct LiveSession {
    columns: Columns,
    username: Option<String>,
    anonymous: bool,
    attributes: BTreeMap<String, Json>,
    dirty_columns: BTreeSet<Column>,
    dirty_json: BTreeSet<String>,
    snapshot: Option<Snapshot>,
    invalidated: bool,
    /// Previous id after password rotation. Not flushed and not adopted.
    retired: bool,
    /// Millis of the last request that bound this session.
    last_access: i64,
    /// Millis when `lastUpdatedStamp` was last written or queued for writing.
    touch_mark: i64,
    /// Write `lastUpdatedStamp` at the next flush even with no dirty field.
    touch: bool,
    /// Changes whenever something a flush writes changes, so a flush that ran without the lock
    /// can tell whether it wrote the latest state.
    revision: u64,
    /// The client address [`remember_client_ip`] last stored on the visit, so a request from the same address writes
    /// nothing. Not flushed.
    client_ip: Option<String>,
}

/// What [`flush_id`] writes, copied from the live session under the lock.
struct FlushPlan {
    revision: u64,
    columns: Columns,
    attributes: BTreeMap<String, Json>,
    dirty_columns: BTreeSet<Column>,
    dirty_json: BTreeSet<String>,
    snapshot: Option<Snapshot>,
    invalidated: bool,
}

struct Base {
    exists: bool,
    revision: i64,
    revision_value: Value,
    columns: Columns,
    attributes: BTreeMap<String, Json>,
}

struct Merged {
    columns: Columns,
    attributes: BTreeMap<String, Json>,
    revision: i64,
}

#[derive(Clone, Default)]
pub(crate) struct Memory {
    pub(crate) authz: AuthzCache,
    user_id: Option<String>,
    username: Option<String>,
    visit_id: Option<String>,
    visitor_id: Option<String>,
    locale: Option<String>,
    time_zone: Option<String>,
    csrf_token: Option<String>,
    anonymous: bool,
    flash: Vec<String>,
    flash_errors: Vec<String>,
    saved: BTreeMap<String, String>,
    errors: BTreeMap<String, String>,
    screen_history: Vec<String>,
    attributes: BTreeMap<String, Json>,
    bound_id: Option<String>,
    factory: Option<Arc<ExecutionContextFactory>>,
    dirty_columns: BTreeSet<Column>,
    dirty_json: BTreeSet<String>,
}

struct ThreadView {
    bound_id: Option<String>,
    factory: Option<Arc<ExecutionContextFactory>>,
    user_id: Option<String>,
    username: Option<String>,
    visit_id: Option<String>,
    visitor_id: Option<String>,
    locale: Option<String>,
    time_zone: Option<String>,
    csrf_token: Option<String>,
    anonymous: bool,
    attributes: BTreeMap<String, Json>,
    dirty_columns: BTreeSet<Column>,
    dirty_json: BTreeSet<String>,
}

/// Borrow of one web session on this thread. Drop copies dirty fields back to the factory.
#[must_use = "dropping the guard unbinds the web session"]
pub struct SessionGuard {
    https: bool,
}

impl SessionGuard {
    pub fn session_id(&self) -> String {
        with_session(|session| session.bound_id.clone().unwrap_or_default())
    }

    pub fn visitor_id(&self) -> Option<String> {
        with_session(|session| session.visitor_id.clone())
    }

    pub fn session_cookie_header(&self) -> Result<String, Error> {
        format_session_cookie(&self.session_id(), self.https)
    }

    pub fn visitor_cookie_header(&self) -> Option<String> {
        self.visitor_id()
            .map(|visitor_id| format_visitor_cookie(&visitor_id, self.https))
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        unbind_current();
    }
}

/// Bind this thread to a factory session.
///
/// `session_cookie` is the `moqui.session` value. A bad MAC, a missing row, or a malformed value
/// mints a new id. `visitor_cookie` is the `moqui.visitor` value.
pub fn bind(
    factory: &Arc<ExecutionContextFactory>,
    session_cookie: Option<&str>,
    visitor_cookie: Option<&str>,
    https: bool,
) -> Result<SessionGuard, Error> {
    if !ExecutionContext::is_entered() {
        return Err(Error::new("no execution context"));
    }
    if with_session(|session| session.bound_id.is_some()) {
        return Err(Error::new("a web session is already bound"));
    }
    let presented = presented_id(session_cookie)?;
    let (id, created) = resolve_id(factory, presented.as_deref())?;
    touch_session(factory, &id);
    install_thread(factory, &id)?;
    if created {
        let scripts = factory.webapp_scripts();
        if scripts.has_listeners() {
            scripts.fire("sessionCreated", session_event(&id));
        }
    }
    // A request with no moqui cookie may come from a client that keeps none, such as a crawler, which
    // would leave a Visitor and a Visit behind on every request. Its visit starts when the session
    // cookie comes back or a user logs in.
    let keeps_cookies = [session_cookie, visitor_cookie]
        .iter()
        .any(|cookie| cookie.is_some_and(|value| !value.trim().is_empty()));
    let visit = if keeps_cookies {
        ensure_visit(factory, visitor_cookie)
    } else {
        Ok(())
    };
    if let Err(err) = visit.and_then(|_| forget_user_logged_out_elsewhere()) {
        unbind_current();
        if created {
            lock_sessions(factory).sessions.remove(&id);
        }
        return Err(err);
    }
    evict_over_cap(factory, &id);
    Ok(SessionGuard { https })
}

/// The event of a session listener: `{sessionId}`.
pub(crate) fn session_event(id: &str) -> Value {
    let mut map = indexmap::IndexMap::new();
    map.insert("sessionId".to_string(), Value::string(id));
    Value::Map(map)
}

/// Start the visit a bound web session put off, for a login on it.
pub(crate) fn start_deferred_visit() -> Result<(), Error> {
    let factory = with_session(|session| session.bound_id.as_ref().and(session.factory.clone()));
    match factory {
        Some(factory) => ensure_visit(&factory, None),
        None => Ok(()),
    }
}

/// `moqui.session={id}.{mac}; HttpOnly; SameSite=Lax; Path=/` and `; Secure` when `https`.
pub fn format_session_cookie(session_id: &str, https: bool) -> Result<String, Error> {
    let mac = sign_session(session_id)?;
    Ok(format!(
        "{SESSION_COOKIE_NAME}={session_id}.{mac}{}",
        cookie_flags(https, None)
    ))
}

/// `moqui.visitor` cookie. `Max-Age` is one year. `Secure` only when `https` is set.
pub fn format_visitor_cookie(visitor_id: &str, https: bool) -> String {
    format!(
        "{VISITOR_COOKIE_NAME}={visitor_id}{}",
        cookie_flags(https, Some(VISITOR_MAX_AGE))
    )
}

/// Session id when the MAC matches. Malformed values and a bad MAC are `Ok(None)`.
pub fn verify_session_cookie(value: &str) -> Result<Option<String>, Error> {
    let Some((id, tag_b64)) = split_session_cookie(value) else {
        return Ok(None);
    };
    let Ok(tag) = URL_SAFE_NO_PAD.decode(tag_b64) else {
        return Ok(None);
    };
    if mac_matches(id, &tag)? {
        Ok(Some(id.to_string()))
    } else {
        Ok(None)
    }
}

/// The user of a session this node holds live: `None` when the node has no current copy (the
/// caller reads the `HttpSession` row), `Some(None)` when the session has no user or was rotated
/// away. An eventual flush has not written a new login yet, so the live copy comes first. A copy
/// another node changed is current only for the columns written here.
pub fn live_session_user_id(factory: &ExecutionContextFactory, session_id: &str) -> Option<Option<String>> {
    let table = lock_sessions(factory);
    let live = table.sessions.get(session_id)?;
    if live.retired {
        return Some(None);
    }
    if live.invalidated && !live.dirty_columns.contains(&Column::UserId) {
        return None;
    }
    Some(live.columns.user_id.clone().filter(|user_id| !user_id.is_empty()))
}

/// Guarantee mode flushes the bound session. Eventual mode returns without writing.
pub fn end_request() -> Result<(), Error> {
    flush_bound_if_guarantee()
}

/// Drop the clean snapshot for `id` and keep dirty fields. A missing id is left unset.
pub fn invalidate(id: &str) {
    let Some(factory) = bound_factory() else {
        return;
    };
    let mut table = lock_sessions(&factory);
    let Some(live) = table.sessions.get_mut(id) else {
        return;
    };
    live.snapshot = None;
    live.invalidated = true;
    live.revision = next_revision();
}

/// Write every dirty live session on this factory. The bound thread is synced first.
pub(crate) fn flush_dirty(factory: &ExecutionContextFactory) -> Result<(), Error> {
    sync_bound(factory);
    expire_idle(factory)?;
    let ids: Vec<String> = {
        let table = lock_sessions(factory);
        table
            .sessions
            .iter()
            .filter(|(_, live)| needs_flush(live))
            .map(|(id, _)| id.clone())
            .collect()
    };
    for id in ids {
        flush_id(factory, &id)?;
    }
    Ok(())
}

/// Move the bound session onto a new id after a successful password check.
pub(crate) fn rotate_on_password_login() -> Result<(), Error> {
    let (factory, old_id) = with_session(|session| (session.factory.clone(), session.bound_id.clone()));
    let (Some(factory), Some(old_id)) = (factory, old_id) else {
        return Ok(());
    };
    sync_bound(&factory);
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let new_id = random_hex(SESSION_ID_BYTES)?;
    let new_token = random_hex(SESSION_TOKEN_BYTES)?;
    stage_rotated(&factory, &old_id, &new_id, &new_token)?;
    {
        // A flush of the old id that is under way would write the row back after the delete.
        let _claim = FlushClaim::wait(&factory, &old_id);
        if let Err(err) = delete_session_row(&facade, &old_id) {
            lock_sessions(&factory).sessions.remove(&new_id);
            return Err(err);
        }
        retire_old(&factory, &old_id);
    }
    invalidate(&old_id);
    invalidate(&new_id);
    with_session(|session| {
        session.bound_id = Some(new_id.clone());
        session.csrf_token = Some(new_token);
        session.dirty_columns.clear();
        session.dirty_json.clear();
    });
    let guarantee = lock_sessions(&factory).mode == FlushMode::Guarantee;
    if guarantee {
        flush_id(&factory, &new_id)?;
    }
    Ok(())
}

/// End another session of the signed-in user, by its visit: the `Visit` gets its `thruDate`, and each web session of
/// that visit (a live one on this node, and its `HttpSession` row) is ended as a rotated-away session is, so its cookie
/// starts over signed out. Only the user's own visit, and not the current one (a logout ends that). Returns how many
/// sessions were ended (none when the visit's session had already expired).
///
/// # Errors
///
/// Returns an error with nobody signed in, for the current visit, and for a visit that is not the user's (or not there).
pub fn end_user_visit(visit_id: &str) -> Result<usize, Error> {
    let visit_id = visit_id.trim();
    let Some(user_id) = current_user_id()?.filter(|id| !id.is_empty()) else {
        return Err(Error::new("A session is ended only by its signed in user"));
    };
    if visit_id.is_empty() {
        return Err(Error::new("No session (visit) to end"));
    }
    if visit_id_now().as_deref() == Some(visit_id) {
        return Err(Error::new("This is the current session: sign out to end it"));
    }
    let factory = bound_factory()
        .or_else(|| ExecutionContext::with_current(|ec| Arc::clone(ec.factory())))
        .ok_or_else(|| Error::new("no execution context"))?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _authz = without_authz();
    let visit = facade
        .find(VISIT)
        .map_err(Error::from)?
        .eq("visitId", Value::string(visit_id))
        .one()
        .map_err(Error::from)?;
    let Some(mut visit) = visit.filter(|row| row.get("userId").and_then(Value::as_str) == Some(user_id.as_str()))
    else {
        return Err(Error::new(format!("Session {visit_id} is not one of yours")));
    };
    if visit.get("thruDate").is_none_or(Value::is_null) {
        visit.set("thruDate", super::login::now_value());
        visit.update(&facade).map_err(Error::from)?;
    }
    // the sessions of the visit: those this node holds live (an eventual flush may not have written them yet) and the rows
    let mut ids: BTreeSet<String> = lock_sessions(&factory)
        .sessions
        .iter()
        .filter(|(_, live)| !live.retired && live.columns.visit_id.as_deref() == Some(visit_id))
        .map(|(id, _)| id.clone())
        .collect();
    for row in facade
        .find(HTTP_SESSION)
        .map_err(Error::from)?
        .eq("visitId", Value::string(visit_id))
        .list()
        .map_err(Error::from)?
        .iter()
    {
        if let Some(id) = row.get("sessionId").and_then(Value::as_str) {
            ids.insert(id.to_string());
        }
    }
    let bound = with_session(|session| session.bound_id.clone());
    let mut ended = 0;
    for id in ids.iter().filter(|id| Some(id.as_str()) != bound.as_deref()) {
        // a flush of the session under way would write the row back after the delete
        let _claim = FlushClaim::wait(&factory, id);
        delete_session_row(&facade, id)?;
        retire_old(&factory, id);
        ended += 1;
    }
    Ok(ended)
}

/// The visit of this session, if it has one.
fn visit_id_now() -> Option<String> {
    with_session(|session| session.visit_id.clone())
}

pub(crate) fn flush_after_logout() -> Result<(), Error> {
    flush_bound_if_guarantee()
}

/// The logged-in username. `Ok(None)` when nobody is logged in.
pub fn current_username() -> Result<Option<String>, Error> {
    Ok(with_session(|session| session.username.clone()))
}

pub fn current_user_id() -> Result<Option<String>, Error> {
    Ok(with_session(|session| session.user_id.clone()))
}

pub fn is_anonymous() -> Result<bool, Error> {
    Ok(with_session(|session| session.anonymous))
}

/// Store the client address on this session's visit. An empty address is left unchanged, and so is the address the
/// session last stored: only a request from a new address reads and writes the visit.
///
/// # Errors
///
/// Returns an error when the visit row cannot be read or updated.
pub fn remember_client_ip(address: &str) -> Result<(), Error> {
    let address = address.trim();
    if address.is_empty() {
        return Ok(());
    }
    let Some(visit_id) = visit_id()? else {
        return Ok(());
    };
    let Some(factory) = bound_factory() else {
        return Ok(());
    };
    let bound_id = with_session(|session| session.bound_id.clone());
    let known = |table: &SessionTable| {
        bound_id
            .as_ref()
            .and_then(|id| table.sessions.get(id))
            .is_some_and(|live| live.client_ip.as_deref() == Some(address))
    };
    if known(&lock_sessions(&factory)) {
        return Ok(());
    }
    let Some(facade) = factory.entity() else {
        return Ok(());
    };
    let _authz = without_authz();
    let Some(mut visit) = facade
        .find(VISIT)
        .map_err(Error::from)?
        .eq("visitId", Value::string(&visit_id))
        .one()
        .map_err(Error::from)?
    else {
        return Ok(());
    };
    if visit.get("clientIpAddress").and_then(Value::as_str) != Some(address) {
        visit.set("clientIpAddress", Value::string(address));
        visit.update(&facade).map_err(Error::from)?;
    }
    if let Some(id) = bound_id {
        if let Some(live) = lock_sessions(&factory).sessions.get_mut(&id) {
            live.client_ip = Some(address.to_string());
        }
    }
    Ok(())
}

/// Every session attribute as text. Maps and lists are JSON.
pub fn session_attributes() -> BTreeMap<String, String> {
    with_session(|session| {
        session
            .attributes
            .iter()
            .filter_map(|(key, value)| attribute_text(Some(value)).map(|text| (key.clone(), text)))
            .collect()
    })
}

/// One session attribute as text. Maps and lists are returned as JSON.
pub fn session_attribute(name: &str) -> Option<String> {
    with_session(|session| attribute_text(session.attributes.get(name)))
}

/// Replace one session attribute. `None` removes it.
pub fn set_session_attribute(name: &str, value: Option<&str>) {
    with_session(|session| match value {
        Some(text) => put_json(session, name, Json::String(text.to_string())),
        None => {
            session.attributes.remove(name);
            if session.bound_id.is_some() {
                session.dirty_json.insert(name.to_string());
            }
        }
    });
}

/// Change one session attribute from its latest text: `update` gets the current text and returns
/// the new one, `None` to remove it.
///
/// For a bound web session the change is made on the live session, under the table lock, so
/// requests of the same session running at once each build on the other's change. This thread's
/// copy takes the result but is not marked changed, so its write-back at the end of the request
/// does not put an older value back.
pub fn update_session_attribute(name: &str, update: impl FnOnce(Option<String>) -> Option<String>) {
    let (bound, own) = with_session(|session| {
        let bound = session.bound_id.clone().zip(session.factory.clone());
        // A value this thread set and has not written back is the latest one.
        let own = session
            .dirty_json
            .contains(name)
            .then(|| attribute_text(session.attributes.get(name)));
        (bound, own)
    });
    let mut update = Some(update);
    let updated = bound.and_then(|(id, factory)| {
        let mut table = lock_sessions(&factory);
        let live = table.sessions.get_mut(&id).filter(|live| !live.retired)?;
        let current = own.unwrap_or_else(|| attribute_text(live.attributes.get(name)));
        let updated = update.take()?(current);
        match &updated {
            Some(text) => {
                live.attributes.insert(name.to_string(), Json::String(text.clone()));
            }
            None => {
                live.attributes.remove(name);
            }
        }
        live.dirty_json.insert(name.to_string());
        live.revision = next_revision();
        Some(updated)
    });
    match (updated, update) {
        (Some(updated), _) => with_session(|session| {
            match updated {
                Some(text) => {
                    session.attributes.insert(name.to_string(), Json::String(text));
                }
                None => {
                    session.attributes.remove(name);
                }
            }
            session.dirty_json.remove(name);
        }),
        // No live session to change: this thread's copy, written back when the request ends.
        (None, Some(update)) => set_session_attribute(name, update(session_attribute(name)).as_deref()),
        (None, None) => {}
    }
}

/// True when the attribute is a boolean true or the text `true` or `Y`.
pub fn session_attribute_flag(name: &str) -> bool {
    with_session(|session| match session.attributes.get(name) {
        Some(Json::Bool(flag)) => *flag,
        Some(Json::String(text)) => matches!(text.as_str(), "true" | "Y" | "y"),
        _ => false,
    })
}

fn attribute_text(value: Option<&Json>) -> Option<String> {
    match value {
        Some(Json::String(text)) => Some(text.clone()),
        Some(Json::Bool(true)) => Some("true".to_string()),
        Some(Json::Bool(false)) => Some("false".to_string()),
        Some(Json::Number(number)) => Some(number.to_string()),
        Some(Json::Null) | None => None,
        Some(other) => serde_json::to_string(other).ok(),
    }
}

/// The CSRF token when one is already stored. Does not create one.
pub fn peek_csrf_token() -> Option<String> {
    with_session(|session| session.csrf_token.clone())
}

/// CSRF token for this thread. A bound session uses `sessionToken`.
pub fn csrf_token() -> Result<String, Error> {
    let existing = with_session(|session| session.csrf_token.clone());
    if let Some(token) = existing {
        return Ok(token);
    }
    let token = random_hex(SESSION_TOKEN_BYTES)?;
    with_session(|session| {
        if session.csrf_token.is_none() {
            session.csrf_token = Some(token.clone());
            mark_column(session, Column::SessionToken);
        }
    });
    Ok(with_session(|session| session.csrf_token.clone().unwrap_or(token)))
}

pub fn visit_id() -> Result<Option<String>, Error> {
    Ok(with_session(|session| session.visit_id.clone()))
}

pub fn visitor_id() -> Result<Option<String>, Error> {
    Ok(with_session(|session| session.visitor_id.clone()))
}

/// User id and visitor id of the entered context, for `moqui_logs`. `None` when no context is entered.
pub fn current_log_actor() -> Option<(Option<String>, Option<String>)> {
    ExecutionContext::with_current(|ec| {
        let session = ec.session();
        (session.user_id.clone(), session.visitor_id.clone())
    })
}

/// Session id, visitor id, and username of the bound session, for `moqui_http_log`. All three are
/// absent when no context is entered or no session is bound. The username is absent when nobody is logged in.
pub fn http_log_identity() -> (Option<String>, Option<String>, Option<String>) {
    ExecutionContext::with_current(|ec| {
        let session = ec.session();
        if session.bound_id.is_none() {
            return (None, None, None);
        }
        (
            session.bound_id.clone(),
            session.visitor_id.clone(),
            session.username.clone(),
        )
    })
    .unwrap_or((None, None, None))
}

/// The user and visitor [`current_log_actor`] reads. The username stored with the user id is the same text.
#[doc(hidden)]
pub fn set_current_log_actor(user_id: &str, visitor_id: &str) {
    set_user(user_id, user_id);
    set_visit("log-visit", visitor_id);
}

pub fn locale() -> Result<Option<String>, Error> {
    Ok(with_session(|session| session.locale.clone()))
}

pub fn time_zone() -> Result<Option<String>, Error> {
    Ok(with_session(|session| session.time_zone.clone()))
}

pub fn flash_messages() -> Result<Vec<String>, Error> {
    Ok(with_session(|session| session.flash.clone()))
}

pub fn saved_parameters() -> Result<BTreeMap<String, String>, Error> {
    Ok(with_session(|session| session.saved.clone()))
}

pub fn error_parameters() -> Result<BTreeMap<String, String>, Error> {
    Ok(with_session(|session| session.errors.clone()))
}

pub fn screen_history() -> Result<Vec<String>, Error> {
    Ok(with_session(|session| session.screen_history.clone()))
}

pub fn set_locale(locale: Option<&str>) {
    with_session(|session| {
        session.locale = locale.map(str::to_string);
        mark_column(session, Column::Locale);
    });
}

pub fn set_time_zone(time_zone: Option<&str>) {
    with_session(|session| {
        session.time_zone = time_zone.map(str::to_string);
        mark_column(session, Column::TimeZone);
    });
}

#[cfg(test)]
pub fn add_flash_message(message: &str) {
    add_flash_message_typed(message, "info");
}

/// A message the next page shows, with its type as Java's `MessageInfo`.
pub fn add_flash_message_typed(message: &str, kind: &str) {
    with_session(|session| {
        session.flash.push(message.to_string());
        let mut types = attributes_string_list(&session.attributes, MESSAGE_TYPES);
        types.resize(session.flash.len() - 1, "info".to_string());
        types.push(if kind.is_empty() { "info" } else { kind }.to_string());
        let encoded = json_string_list(&session.flash);
        put_json(session, MESSAGE_PUBLIC, encoded);
        put_json(session, MESSAGE_TYPES, json_string_list(&types));
    });
}

/// A validation error the next page shows beside its field.
pub fn add_flash_validation_error(error: &BTreeMap<String, String>) {
    with_session(|session| {
        let mut list = match session.attributes.get(MESSAGE_VALIDATION) {
            Some(Json::Array(items)) => items.clone(),
            _ => Vec::new(),
        };
        list.push(json_string_map(error));
        put_json(session, MESSAGE_VALIDATION, Json::Array(list));
    });
}

/// An error message the next page shows, kept apart from the messages.
pub fn add_flash_error(message: &str) {
    with_session(|session| {
        session.flash_errors.push(message.to_string());
        let encoded = json_string_list(&session.flash_errors);
        put_json(session, MESSAGE_ERROR, encoded);
    });
}

/// What the last response saved for this page: messages, errors and the parameters of a failed submit.
/// The session forgets them, so a page shows them once.
#[derive(Clone, Debug, Default)]
pub struct SavedForPage {
    pub messages: Vec<String>,
    /// The type of each of `messages`.
    pub message_types: Vec<String>,
    pub errors: Vec<String>,
    pub validation_errors: Vec<BTreeMap<String, String>>,
    pub error_parameters: BTreeMap<String, String>,
}

pub fn take_saved_for_page() -> SavedForPage {
    with_session(|session| {
        let messages = std::mem::take(&mut session.flash);
        let mut message_types = attributes_string_list(&session.attributes, MESSAGE_TYPES);
        message_types.resize(messages.len(), "info".to_string());
        let validation_errors = match session.attributes.get(MESSAGE_VALIDATION) {
            Some(Json::Array(items)) => items
                .iter()
                .filter_map(|item| match item {
                    Json::Object(map) => Some(
                        map.iter()
                            .filter_map(|(key, value)| match value {
                                Json::String(text) => Some((key.clone(), text.clone())),
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let taken = SavedForPage {
            messages,
            message_types,
            errors: std::mem::take(&mut session.flash_errors),
            validation_errors,
            error_parameters: std::mem::take(&mut session.errors),
        };
        for key in [
            MESSAGE_PUBLIC,
            MESSAGE_ERROR,
            MESSAGE_TYPES,
            MESSAGE_VALIDATION,
            ERROR_PARAMETERS,
        ] {
            if session.attributes.remove(key).is_some() && session.bound_id.is_some() {
                session.dirty_json.insert(key.to_string());
            }
        }
        taken
    })
}

pub fn save_parameter(name: &str, value: &str) {
    with_session(|session| {
        session.saved.insert(name.to_string(), value.to_string());
        let encoded = json_string_map(&session.saved);
        put_json(session, SAVED, encoded);
    });
}

pub fn set_error_parameter(name: &str, value: &str) {
    with_session(|session| {
        session.errors.insert(name.to_string(), value.to_string());
        let encoded = json_string_map(&session.errors);
        put_json(session, ERROR_PARAMETERS, encoded);
    });
}

pub fn push_screen_history(screen: &str) {
    with_session(|session| {
        // Newest first, without repeats, and no more than 40.
        session.screen_history.retain(|existing| existing != screen);
        session.screen_history.insert(0, screen.to_string());
        session.screen_history.truncate(40);
        let encoded = json_string_list(&session.screen_history);
        put_json(session, SCREEN_HISTORY, encoded);
    });
}

/// The parameters a response saved for the next request, which the session forgets: Java's `WebFacadeImpl` takes
/// `moqui.saved.parameters` when a request starts and puts them under the request's own parameters.
pub fn take_saved_parameters() -> BTreeMap<String, String> {
    with_session(|session| {
        let taken = std::mem::take(&mut session.saved);
        if session.attributes.remove(SAVED).is_some() && session.bound_id.is_some() {
            session.dirty_json.insert(SAVED.to_string());
        }
        taken
    })
}

/// `WebFacade.saveScreenLastInfo`: the path and parameters a `screen-last` response goes back to, such as the page a user
/// asked for before signing in.
pub fn save_screen_last(path: &str, parameters: &BTreeMap<String, String>) {
    with_session(|session| {
        put_json(session, LAST_PATH, Json::String(path.to_string()));
        put_json(session, LAST_PARAMETERS, json_string_map(parameters));
    });
}

/// `getRemoveScreenLastPath`: the saved path, which the session forgets.
pub fn take_screen_last_path() -> Option<String> {
    with_session(|session| {
        let path = attribute_text(session.attributes.get(LAST_PATH));
        if session.attributes.remove(LAST_PATH).is_some() && session.bound_id.is_some() {
            session.dirty_json.insert(LAST_PATH.to_string());
        }
        path.filter(|text| !text.is_empty())
    })
}

/// `removeScreenLastParameters`: drop the saved parameters, first making them the saved parameters a `screen-last`
/// response carries when `to_saved`.
pub fn remove_screen_last_parameters(to_saved: bool) {
    with_session(|session| {
        if to_saved {
            session.saved = attributes_string_map(&session.attributes, LAST_PARAMETERS);
            let encoded = json_string_map(&session.saved);
            put_json(session, SAVED, encoded);
        }
        if session.attributes.remove(LAST_PARAMETERS).is_some() && session.bound_id.is_some() {
            session.dirty_json.insert(LAST_PARAMETERS.to_string());
        }
    });
}

/// A new login starts a new session view: the CSRF token and cached authorization data are dropped.
pub(crate) fn set_user(user_id: &str, username: &str) {
    with_session(|session| {
        session.user_id = Some(user_id.to_string());
        session.username = Some(username.to_string());
        session.anonymous = false;
        session.csrf_token = None;
        session.authz = AuthzCache::default();
        mark_column(session, Column::UserId);
    });
}

pub(crate) fn set_anonymous(anonymous: bool) {
    with_session(|session| session.anonymous = anonymous);
}

pub(crate) fn set_visit(visit_id: &str, visitor_id: &str) {
    with_session(|session| {
        session.visit_id = Some(visit_id.to_string());
        session.visitor_id = Some(visitor_id.to_string());
        mark_column(session, Column::VisitId);
        mark_column(session, Column::VisitorId);
    });
}

/// Clears the user and the anonymous flag. Visit ids and screen state stay. The CSRF token and
/// cached authorization data go, so a later login cannot reuse them.
pub(crate) fn clear_login() {
    with_session(|session| {
        session.user_id = None;
        session.username = None;
        session.anonymous = false;
        session.csrf_token = None;
        session.authz = AuthzCache::default();
        mark_column(session, Column::UserId);
    });
}

/// Who is logged in on this context, with what a login changes beside the user, for [`restore_identity`].
pub(crate) struct Identity {
    user_id: Option<String>,
    username: Option<String>,
    anonymous: bool,
    locale: Option<String>,
    time_zone: Option<String>,
    csrf_token: Option<String>,
    authz: AuthzCache,
    pre_authc: Option<Json>,
    factor_required: Option<Json>,
}

pub(crate) fn identity() -> Identity {
    with_session(|session| Identity {
        user_id: session.user_id.clone(),
        username: session.username.clone(),
        anonymous: session.anonymous,
        locale: session.locale.clone(),
        time_zone: session.time_zone.clone(),
        csrf_token: session.csrf_token.clone(),
        authz: session.authz.clone(),
        pre_authc: session.attributes.get(super::login::PRE_AUTHC_USERNAME).cloned(),
        factor_required: session.attributes.get(super::login::AUTHC_FACTOR_REQUIRED).cloned(),
    })
}

/// Put back an [`identity`], as Java's `logoutLocal` pops the user. The session id and visit stay as they are.
pub(crate) fn restore_identity(identity: &Identity) {
    with_session(|session| {
        session.user_id.clone_from(&identity.user_id);
        session.username.clone_from(&identity.username);
        session.anonymous = identity.anonymous;
        session.locale.clone_from(&identity.locale);
        session.time_zone.clone_from(&identity.time_zone);
        session.csrf_token.clone_from(&identity.csrf_token);
        session.authz = identity.authz.clone();
        mark_column(session, Column::UserId);
        mark_column(session, Column::Locale);
        mark_column(session, Column::TimeZone);
        for (key, value) in [
            (super::login::PRE_AUTHC_USERNAME, &identity.pre_authc),
            (super::login::AUTHC_FACTOR_REQUIRED, &identity.factor_required),
        ] {
            match value {
                Some(value) => put_json(session, key, value.clone()),
                None => {
                    if session.attributes.remove(key).is_some() && session.bound_id.is_some() {
                        session.dirty_json.insert(key.to_string());
                    }
                }
            }
        }
    });
}

#[cfg(test)]
pub(crate) fn reset_session() {
    with_session(|session| *session = Memory::default());
}

pub(crate) fn with_session<T>(body: impl FnOnce(&mut Memory) -> T) -> T {
    let mut pending = Some(body);
    let ran = ExecutionContext::with_current(|ec| pending.take().map(|run| run(&mut ec.session())));
    match ran.flatten() {
        Some(value) => value,
        None => match pending.take() {
            Some(run) => run(&mut Memory::default()),
            None => unreachable!("the session body runs once"),
        },
    }
}

fn lock_sessions(factory: &ExecutionContextFactory) -> MutexGuard<'_, SessionTable> {
    lock(&factory.sessions)
}

/// A new [`LiveSession::revision`]. Unique in the process, so a session put back under the same
/// id never matches a revision an older flush copied.
fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn random_hex(size: usize) -> Result<String, Error> {
    let mut bytes = vec![0_u8; size];
    getrandom::fill(&mut bytes).map_err(|err| Error::new(format!("could not read random bytes: {err}")))?;
    Ok(hex_encode(&bytes))
}

fn presented_id(session_cookie: Option<&str>) -> Result<Option<String>, Error> {
    match session_cookie {
        Some(value) if !value.is_empty() => verify_session_cookie(value),
        _ => Ok(None),
    }
}

fn resolve_id(factory: &Arc<ExecutionContextFactory>, presented: Option<&str>) -> Result<(String, bool), Error> {
    if let Some(id) = presented {
        if adopt_presented(factory, id)? {
            return Ok((id.to_string(), false));
        }
    }
    let id = random_hex(SESSION_ID_BYTES)?;
    let token = random_hex(SESSION_TOKEN_BYTES)?;
    let live = LiveSession {
        columns: Columns {
            session_token: Some(token),
            ..Columns::default()
        },
        username: None,
        anonymous: false,
        attributes: BTreeMap::new(),
        dirty_columns: all_columns(),
        dirty_json: BTreeSet::new(),
        snapshot: None,
        invalidated: false,
        retired: false,
        last_access: now_millis(),
        touch_mark: 0,
        touch: false,
        revision: next_revision(),
        client_ip: None,
    };
    lock_sessions(factory).sessions.insert(id.clone(), live);
    Ok((id, true))
}

fn adopt_presented(factory: &ExecutionContextFactory, id: &str) -> Result<bool, Error> {
    let timeout = session_timeout_millis(factory);
    let now = now_millis();
    let mut expired = false;
    let retired = {
        let mut table = lock_sessions(factory);
        match table.sessions.get(id) {
            Some(live) if live.retired => true,
            Some(live) if is_idle(live.last_access, now, timeout) => {
                expired = true;
                table.sessions.remove(id);
                false
            }
            Some(_) => return Ok(true),
            None => false,
        }
    };
    if retired {
        return Ok(false);
    }
    let Some(facade) = factory.entity() else {
        return Ok(false);
    };
    if expired {
        let _claim = FlushClaim::wait(factory, id);
        delete_session_row(&facade, id)?;
        return Ok(false);
    }
    let Some(row) = read_row(&facade, id)? else {
        return Ok(false);
    };
    let stamp = row.get("lastUpdatedStamp").and_then(super::login::value_millis);
    if stamp.is_some_and(|stamp| is_idle(stamp, now, timeout)) {
        delete_session_row(&facade, id)?;
        return Ok(false);
    }
    install_from_row(factory, id)?;
    Ok(true)
}

/// The `webroot` webapp's `session-config` `timeout` in minutes, 60 when unset. 0 or a negative value never expires.
fn session_timeout_millis(factory: &ExecutionContextFactory) -> Option<i64> {
    let minutes = factory.webapp_child_setting("webroot", "session-config", "timeout").parse::<i64>().unwrap_or(60);
    (minutes > 0).then(|| minutes.saturating_mul(60_000))
}

fn is_idle(last_access: i64, now: i64, timeout: Option<i64>) -> bool {
    timeout.is_some_and(|timeout| now.saturating_sub(last_access) > timeout)
}

/// Record a request on a bound session. `lastUpdatedStamp` is rewritten at most every quarter timeout.
fn touch_session(factory: &ExecutionContextFactory, id: &str) {
    let timeout = session_timeout_millis(factory);
    let now = now_millis();
    let mut table = lock_sessions(factory);
    let Some(live) = table.sessions.get_mut(id) else {
        return;
    };
    live.last_access = now;
    let Some(timeout) = timeout else {
        return;
    };
    if now.saturating_sub(live.touch_mark) > timeout / 4 {
        live.touch_mark = now;
        live.touch = true;
        live.revision = next_revision();
    }
}

/// Keep the table within `session-config` `max`: drop the sessions used longest ago, down to nine
/// tenths of the cap. A dropped session reloads from its row, so a dirty one is written first and
/// dropped on the next pass. A failed write leaves the table over the cap until a later request.
fn evict_over_cap(factory: &ExecutionContextFactory, keep: &str) {
    for _ in 0..2 {
        let dirty = {
            let mut table = lock_sessions(factory);
            let Some(max) = table.max_live else {
                return;
            };
            if table.sessions.len() <= max {
                return;
            }
            let excess = table.sessions.len() - (max - max / 10);
            let mut oldest: Vec<(i64, String)> = table
                .sessions
                .iter()
                .filter(|(id, _)| id.as_str() != keep)
                .map(|(id, live)| (live.last_access, id.clone()))
                .collect();
            oldest.sort_unstable();
            let mut dirty = Vec::new();
            let mut removed = 0;
            for (_, id) in oldest {
                if removed + dirty.len() >= excess {
                    break;
                }
                if table.sessions.get(&id).is_some_and(needs_flush) {
                    dirty.push(id);
                } else {
                    table.sessions.remove(&id);
                    removed += 1;
                }
            }
            dirty
        };
        if dirty.is_empty() {
            return;
        }
        for id in &dirty {
            if let Err(err) = flush_id(factory, id) {
                tracing::warn!("could not write session {id} before evicting it: {err}");
                return;
            }
        }
    }
}

/// Drop idle sessions from memory and, at most once a minute, delete idle `HttpSession` rows.
fn expire_idle(factory: &ExecutionContextFactory) -> Result<(), Error> {
    let Some(timeout) = session_timeout_millis(factory) else {
        return Ok(());
    };
    let now = now_millis();
    let listening = factory.webapp_scripts().has_listeners();
    let purge = {
        let mut table = lock_sessions(factory);
        let mut expired = Vec::new();
        table.sessions.retain(|id, live| {
            let idle = is_idle(live.last_access, now, Some(timeout));
            if idle && listening {
                expired.push(id.clone());
            }
            !idle
        });
        table.expired.extend(expired);
        if now.saturating_sub(table.last_purge) > 60_000 {
            table.last_purge = now;
            true
        } else {
            false
        }
    };
    if !purge {
        return Ok(());
    }
    let Some(facade) = factory.entity() else {
        return Ok(());
    };
    let cutoff = super::login::stamp_value(now.saturating_sub(timeout));
    let condition = Condition::less("lastUpdatedStamp", cutoff.clone());
    let sql = facade
        .with_entity(HTTP_SESSION, |entity, dialect, _| {
            sql_delete(entity, dialect, Some(&condition)).map(|statement| statement.sql)
        })
        .map_err(Error::from)?;
    with_session_sql(&facade, |conn| {
        conn.execute(&sql, &[cutoff])?;
        Ok(())
    })?;
    cache::on_write(&facade, HTTP_SESSION, "delete", &BTreeMap::new()).map_err(Error::from)
}

/// The sessions that expired in memory since the last call, for the listeners' `sessionDestroyed`. A session that
/// was only in the database when it expired is deleted with no event.
pub(crate) fn take_expired(factory: &ExecutionContextFactory) -> Vec<String> {
    std::mem::take(&mut lock_sessions(factory).expired)
}

fn install_from_row(factory: &ExecutionContextFactory, id: &str) -> Result<(), Error> {
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let Some(row) = read_row(&facade, id)? else {
        return Err(Error::new("session row disappeared"));
    };
    let base = base_from_row(&row)?;
    let stamp = row.get("lastUpdatedStamp").and_then(super::login::value_millis);
    let username = base
        .columns
        .user_id
        .as_deref()
        .and_then(|user_id| username_for(&facade, user_id));
    let live = LiveSession {
        columns: base.columns.clone(),
        username,
        anonymous: false,
        attributes: base.attributes.clone(),
        dirty_columns: BTreeSet::new(),
        dirty_json: BTreeSet::new(),
        snapshot: Some(snapshot_from_base(&base)),
        invalidated: false,
        retired: false,
        // The request adopting it is an access, and eviction must not take it before the bind ends.
        last_access: now_millis(),
        touch_mark: stamp.unwrap_or(0),
        touch: false,
        revision: next_revision(),
        client_ip: None,
    };
    let mut table = lock_sessions(factory);
    if table.sessions.get(id).is_some_and(|existing| !existing.retired) {
        return Ok(());
    }
    table.sessions.insert(id.to_string(), live);
    Ok(())
}

fn install_thread(factory: &Arc<ExecutionContextFactory>, id: &str) -> Result<(), Error> {
    let live = lock_sessions(factory)
        .sessions
        .get(id)
        .cloned()
        .ok_or_else(|| Error::new("session disappeared"))?;
    // Leave `authz` alone. A new context starts empty, and a login clears it in `set_user`.
    with_session(|session| {
        session.user_id = live.columns.user_id.clone();
        session.username = live.username.clone();
        session.visit_id = live.columns.visit_id.clone();
        session.visitor_id = live.columns.visitor_id.clone();
        session.locale = live.columns.locale.clone();
        session.time_zone = live.columns.time_zone.clone();
        session.csrf_token = live.columns.session_token.clone();
        session.anonymous = live.anonymous;
        session.attributes = live.attributes.clone();
        project_known(session);
        session.bound_id = Some(id.to_string());
        session.factory = Some(Arc::clone(factory));
        session.dirty_columns.clear();
        session.dirty_json.clear();
    });
    Ok(())
}

fn forget_user_logged_out_elsewhere() -> Result<(), Error> {
    let Some(user_id) = current_user_id()? else {
        return Ok(());
    };
    let Some(factory) = bound_factory() else {
        return Ok(());
    };
    let Some(facade) = factory.entity() else {
        return Ok(());
    };
    // Visit and account reads during bind run before a user is authorized.
    let _authz = without_authz();
    let Some(account) = facade
        .find(USER_ACCOUNT)
        .map_err(Error::from)?
        .eq("userId", Value::string(&user_id))
        .one()
        .map_err(Error::from)?
    else {
        return Ok(());
    };
    let logged_out = account
        .get("hasLoggedOut")
        .and_then(Value::as_str)
        .is_some_and(|flag| flag == "Y");
    if logged_out {
        clear_login();
    }
    Ok(())
}

fn ensure_visit(factory: &Arc<ExecutionContextFactory>, visitor_cookie: Option<&str>) -> Result<(), Error> {
    if visit_id()?.is_some() {
        return Ok(());
    }
    let _authz = without_authz();
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let visitor_id = resolve_visitor(&facade, visitor_cookie)?;
    let visit_id = create_visit(&facade, &visitor_id)?;
    set_visit(&visit_id, &visitor_id);
    // Java defines first-hit-in-visit and never runs it; here it runs when a request starts a visit (Parity-Diffs P9-02)
    factory
        .webapp_scripts()
        .run_actions_logged("first-hit-in-visit", &BTreeMap::new());
    Ok(())
}

fn resolve_visitor(facade: &EntityFacade, cookie: Option<&str>) -> Result<String, Error> {
    if let Some(id) = cookie.map(str::trim).filter(|id| !id.is_empty()) {
        let found = facade
            .find(VISITOR)
            .map_err(Error::from)?
            .eq("visitorId", Value::string(id))
            .one()
            .map_err(Error::from)?;
        if found.is_some() {
            return Ok(id.to_string());
        }
    }
    create_visitor(facade)
}

/// A new `moqui.server.Visitor` row. Returns its id.
pub(crate) fn create_visitor(facade: &EntityFacade) -> Result<String, Error> {
    let mut visitor = facade.make_value(VISITOR).map_err(Error::from)?;
    visitor.set("createdDate", super::login::now_value());
    visitor.set_sequenced_id_primary(facade).map_err(Error::from)?;
    let visitor_id = required_id(&visitor, "visitorId")?;
    visitor.create(facade).map_err(Error::from)?;
    Ok(visitor_id)
}

/// What a new `moqui.server.Visit` records of the web request that starts it, as Java's
/// `UserFacadeImpl.initFromHttpRequest`: the URL and referrer, the browser's user agent and locale, and the client address.
#[derive(Clone, Debug, Default)]
pub struct VisitRequest {
    pub url: String,
    pub referrer: String,
    pub user_agent: String,
    /// The first `Accept-Language` locale, `en_US` style; empty for the session's or the server's.
    pub locale: String,
    pub client_ip: String,
}

thread_local! {
    static VISIT_REQUEST: std::cell::RefCell<Option<VisitRequest>> = const { std::cell::RefCell::new(None) };
}

/// The request a visit started on this thread records, from [`VisitRequestGuard::set`] until the guard drops.
#[must_use = "dropping the guard forgets the request"]
pub struct VisitRequestGuard {
    previous: Option<VisitRequest>,
}

impl VisitRequestGuard {
    pub fn set(request: VisitRequest) -> Self {
        Self {
            previous: VISIT_REQUEST.with(|slot| slot.borrow_mut().replace(request)),
        }
    }
}

impl Drop for VisitRequestGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        VISIT_REQUEST.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// Text cut to `max` characters, as Java's `substring(0, 255)` of the URL and user agent.
fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// A new `moqui.server.Visit` for `visitor_id`, with the current user when one is logged in, the session, and, in a web
/// request, what Java's visit records of it (`initialRequest`, `initialReferrer`, `initialUserAgent`, `initialLocale`,
/// `webappName`, the client and server addresses). Returns its id.
pub(crate) fn create_visit(facade: &EntityFacade, visitor_id: &str) -> Result<String, Error> {
    let mut visit = facade.make_value(VISIT).map_err(Error::from)?;
    visit.set("visitorId", Value::string(visitor_id));
    visit.set("fromDate", super::login::now_value());
    if let Some(user_id) = current_user_id()? {
        visit.set("userId", Value::string(user_id));
    }
    if let Some(session_id) = with_session(|session| session.bound_id.clone()) {
        visit.set("sessionId", Value::string(session_id));
    }
    if let Some(request) = VISIT_REQUEST.with(|slot| slot.borrow().clone()) {
        let locale = if request.locale.is_empty() {
            with_session(|session| session.locale.clone()).unwrap_or_else(crate::l10n::locale)
        } else {
            request.locale.clone()
        };
        let (server_ip, server_host) = crate::context::hits::server_address();
        for (name, value) in [
            ("webappName", "webroot".to_string()),
            ("initialLocale", locale),
            ("initialRequest", cut(&request.url, 255)),
            ("initialReferrer", request.referrer.clone()),
            ("initialUserAgent", cut(&request.user_agent, 255)),
            ("clientIpAddress", request.client_ip.clone()),
            // Java's request.getRemoteHost(): the address, as no name is looked up
            ("clientHostName", request.client_ip.clone()),
            ("serverIpAddress", server_ip.clone()),
            ("serverHostName", server_host.clone()),
        ] {
            if !value.is_empty() {
                visit.set(name, Value::string(value));
            }
        }
    }
    visit.set_sequenced_id_primary(facade).map_err(Error::from)?;
    let visit_id = required_id(&visit, "visitId")?;
    visit.create(facade).map_err(Error::from)?;
    Ok(visit_id)
}

fn required_id(value: &EntityValue, field: &str) -> Result<String, Error> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::new(format!("{field} was not sequenced")))
}

fn without_authz() -> Option<crate::context::AuthzGuard> {
    ExecutionContext::with_current(|ctx| ctx.disable_authz())
}

fn username_for(facade: &EntityFacade, user_id: &str) -> Option<String> {
    let _authz = without_authz();
    let row = facade
        .find(USER_ACCOUNT)
        .ok()?
        .eq("userId", Value::string(user_id))
        .one()
        .ok()??;
    row.get("username")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn unbind_current() {
    let view = with_session(thread_view);
    let Some(id) = view.bound_id.clone() else {
        return;
    };
    let Some(factory) = view.factory.clone() else {
        clear_bind();
        return;
    };
    store_view(&mut lock_sessions(&factory), &id, &view);
    clear_bind();
}

/// Copy the thread's dirty fields to the live session. A session the table dropped meanwhile, by
/// expiry, eviction or a cleared map, comes back whole when the thread has changes to keep.
fn store_view(table: &mut SessionTable, id: &str, view: &ThreadView) {
    if let Some(live) = table.sessions.get_mut(id) {
        if !live.retired {
            apply_thread(live, view);
        }
    } else if view_dirty(view) {
        let mut live = live_from_thread(view);
        live.dirty_columns = all_columns();
        live.dirty_json = live.attributes.keys().cloned().collect();
        table.sessions.insert(id.to_string(), live);
    }
}

fn clear_bind() {
    with_session(|session| {
        session.bound_id = None;
        session.factory = None;
        session.dirty_columns.clear();
        session.dirty_json.clear();
    });
}

fn sync_bound(factory: &ExecutionContextFactory) {
    let view = with_session(thread_view);
    let Some(thread_factory) = &view.factory else {
        return;
    };
    if !std::ptr::eq(Arc::as_ptr(thread_factory), std::ptr::from_ref(factory)) {
        return;
    }
    let Some(id) = view.bound_id.clone() else {
        return;
    };
    store_view(&mut lock_sessions(factory), &id, &view);
    with_session(|session| {
        if session.bound_id.as_deref() == Some(id.as_str()) {
            session.dirty_columns.clear();
            session.dirty_json.clear();
        }
    });
}

fn flush_bound_if_guarantee() -> Result<(), Error> {
    let (factory, id) = with_session(|session| (session.factory.clone(), session.bound_id.clone()));
    let (Some(factory), Some(id)) = (factory, id) else {
        return Ok(());
    };
    let guarantee = lock_sessions(&factory).mode == FlushMode::Guarantee;
    if !guarantee {
        return Ok(());
    }
    sync_bound(&factory);
    flush_id(&factory, &id)
}

/// Write one live session. The SQL runs without the table lock: the dirty state is copied under
/// it, and the result is kept only when nothing changed the session meanwhile. A session written
/// to during the flush stays dirty for the next one.
fn flush_id(factory: &ExecutionContextFactory, id: &str) -> Result<(), Error> {
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _claim = FlushClaim::wait(factory, id);
    let mut force_sql = false;
    for _ in 0..MAX_FLUSH_ATTEMPTS {
        let Some(plan) = flush_plan(&lock_sessions(factory), id) else {
            return Ok(());
        };
        #[cfg(test)]
        tests::flush_hook(id);
        let base = load_base(&facade, id, &plan, force_sql)?;
        let merged = overlay(&plan, &base);
        if !write_merged(&facade, id, &base, &merged, &plan.dirty_columns, &plan.dirty_json)? {
            force_sql = true;
            continue;
        }
        store_success(&mut lock_sessions(factory), id, &merged, plan.revision);
        cache::on_write(&facade, HTTP_SESSION, "update", &session_key(id)).map_err(Error::from)?;
        return Ok(());
    }
    Err(Error::new(format!("HttpSession sessionRevision conflict for {id}")))
}

/// What a flush of `id` writes. `None` when the session is gone or has nothing to write.
fn flush_plan(table: &SessionTable, id: &str) -> Option<FlushPlan> {
    let live = table.sessions.get(id).filter(|live| needs_flush(live))?;
    Some(FlushPlan {
        revision: live.revision,
        columns: live.columns.clone(),
        attributes: live.attributes.clone(),
        dirty_columns: live.dirty_columns.clone(),
        dirty_json: live.dirty_json.clone(),
        snapshot: live.snapshot.clone(),
        invalidated: live.invalidated,
    })
}

/// One flush or row delete at a time for a session id. Waiting releases the table lock, so
/// requests carry on while another thread writes the row.
struct FlushClaim<'a> {
    factory: &'a ExecutionContextFactory,
    id: String,
}

impl<'a> FlushClaim<'a> {
    fn wait(factory: &'a ExecutionContextFactory, id: &str) -> Self {
        let mut table = lock_sessions(factory);
        let flushed = Arc::clone(&table.flushed);
        while table.flushing.contains(id) {
            table = flushed.wait(table).unwrap_or_else(|err| err.into_inner());
        }
        table.flushing.insert(id.to_string());
        Self {
            factory,
            id: id.to_string(),
        }
    }
}

impl Drop for FlushClaim<'_> {
    fn drop(&mut self) {
        let mut table = lock_sessions(self.factory);
        table.flushing.remove(&self.id);
        table.flushed.notify_all();
    }
}

/// Keep what a flush wrote. When the session changed after `revision` was copied, its dirty fields
/// stay for the next flush, which then starts from the row this one wrote.
fn store_success(table: &mut SessionTable, id: &str, merged: &Merged, revision: u64) {
    let Some(live) = table.sessions.get_mut(id) else {
        return;
    };
    let snapshot = Snapshot {
        revision: merged.revision,
        revision_value: Value::I64(merged.revision),
        columns: merged.columns.clone(),
        attributes: merged.attributes.clone(),
    };
    if live.revision != revision {
        if !live.invalidated {
            live.snapshot = Some(snapshot);
        }
        return;
    }
    live.columns = merged.columns.clone();
    live.attributes = merged.attributes.clone();
    live.snapshot = Some(snapshot);
    live.dirty_columns.clear();
    live.dirty_json.clear();
    live.invalidated = false;
    live.touch = false;
}

fn load_base(facade: &EntityFacade, id: &str, plan: &FlushPlan, force_sql: bool) -> Result<Base, Error> {
    if !force_sql && !plan.invalidated {
        if let Some(snapshot) = &plan.snapshot {
            return Ok(base_from_snapshot(snapshot));
        }
    }
    match read_row(facade, id)? {
        Some(row) => base_from_row(&row),
        None => Ok(Base {
            exists: false,
            revision: 0,
            revision_value: Value::I64(0),
            columns: Columns::default(),
            attributes: BTreeMap::new(),
        }),
    }
}

fn overlay(plan: &FlushPlan, base: &Base) -> Merged {
    let mut columns = base.columns.clone();
    copy_dirty_columns(&plan.dirty_columns, &plan.columns, &mut columns);
    let mut attributes = base.attributes.clone();
    for key in &plan.dirty_json {
        match plan.attributes.get(key) {
            Some(value) => {
                attributes.insert(key.clone(), value.clone());
            }
            None => {
                attributes.remove(key);
            }
        }
    }
    Merged {
        columns,
        attributes,
        revision: base.revision.saturating_add(1),
    }
}

fn write_merged(
    facade: &EntityFacade,
    id: &str,
    base: &Base,
    merged: &Merged,
    dirty_columns: &BTreeSet<Column>,
    dirty_json: &BTreeSet<String>,
) -> Result<bool, Error> {
    if !base.exists {
        insert_row(facade, id, merged)?;
        return Ok(true);
    }
    let (names, mut params) = update_assignments(merged, dirty_columns, dirty_json);
    params.push(Value::string(id));
    params.push(base.revision_value.clone());
    let sql = facade
        .with_entity(HTTP_SESSION, |entity, dialect, _| {
            let condition = Condition::and(vec![
                Condition::bound("sessionId", CompareOp::Equals),
                Condition::bound("sessionRevision", CompareOp::Equals),
            ]);
            sql_update(entity, dialect, &names, Some(&condition)).map(|statement| statement.sql)
        })
        .map_err(Error::from)?;
    let count = with_session_sql(facade, |conn| conn.execute(&sql, &params))?;
    Ok(count > 0)
}

fn update_assignments(
    merged: &Merged,
    dirty_columns: &BTreeSet<Column>,
    dirty_json: &BTreeSet<String>,
) -> (Vec<&'static str>, Vec<Value>) {
    let mut names = Vec::new();
    let mut params = Vec::new();
    for column in dirty_columns {
        names.push(column_name(*column));
        params.push(column_value(&merged.columns, *column));
    }
    if !dirty_json.is_empty() {
        names.push("attributes");
        params.push(Value::string(encode_attributes(&merged.attributes)));
    }
    names.push("sessionRevision");
    params.push(Value::I64(merged.revision));
    names.push("lastUpdatedStamp");
    params.push(super::login::now_value());
    (names, params)
}

fn insert_row(facade: &EntityFacade, id: &str, merged: &Merged) -> Result<(), Error> {
    let sql = facade
        .with_entity(HTTP_SESSION, |entity, dialect, _| {
            sql_insert(entity, dialect, INSERT_COLUMNS)
        })
        .map_err(Error::from)?;
    let params = insert_params(id, merged);
    with_session_sql(facade, |conn| {
        conn.execute(&sql, &params)?;
        Ok(())
    })
}

fn insert_params(id: &str, merged: &Merged) -> Vec<Value> {
    vec![
        Value::string(id),
        column_value(&merged.columns, Column::UserId),
        column_value(&merged.columns, Column::VisitId),
        column_value(&merged.columns, Column::VisitorId),
        column_value(&merged.columns, Column::Locale),
        column_value(&merged.columns, Column::TimeZone),
        column_value(&merged.columns, Column::SessionToken),
        Value::string(encode_attributes(&merged.attributes)),
        Value::I64(merged.revision),
        super::login::now_value(),
    ]
}

fn read_row(facade: &EntityFacade, id: &str) -> Result<Option<DbRow>, Error> {
    let fields: Vec<FieldRef> = ROW_FIELDS.iter().copied().map(FieldRef::new).collect();
    let condition = Condition::equals("sessionId", Value::string(id));
    let sql = facade
        .with_entity(HTTP_SESSION, |entity, dialect, _| {
            sql_select(entity, dialect, &fields, Some(&condition), &[], LimitOffset::default())
                .map(|statement| statement.sql)
        })
        .map_err(Error::from)?;
    let params = [Value::string(id)];
    let rows = with_session_sql(facade, |conn| conn.query(&sql, &params))?;
    Ok(rows.into_iter().next())
}

fn delete_session_row(facade: &EntityFacade, id: &str) -> Result<(), Error> {
    let condition = Condition::equals("sessionId", Value::string(id));
    let sql = facade
        .with_entity(HTTP_SESSION, |entity, dialect, _| {
            sql_delete(entity, dialect, Some(&condition)).map(|statement| statement.sql)
        })
        .map_err(Error::from)?;
    with_session_sql(facade, |conn| {
        conn.execute(&sql, &[Value::string(id)])?;
        Ok(())
    })?;
    cache::on_write(facade, HTTP_SESSION, "delete", &session_key(id)).map_err(Error::from)
}

fn session_key(id: &str) -> crate::entity::FieldMap {
    let mut keys = crate::entity::FieldMap::new();
    keys.insert(std::sync::Arc::from("sessionId"), Value::string(id));
    keys
}

fn with_session_sql<T>(
    facade: &EntityFacade,
    body: impl FnOnce(&mut dyn DbConnection) -> Result<T, moqui_lib::error::Error>,
) -> Result<T, Error> {
    let tx = facade.transactions_for(HTTP_SESSION)?;
    crate::entity::exec::with_reserve_tx(&tx, body).map_err(Error::from)
}

fn base_from_row(row: &DbRow) -> Result<Base, Error> {
    let (revision, revision_value) = revision_of(row.get("sessionRevision"));
    Ok(Base {
        exists: true,
        revision,
        revision_value,
        columns: Columns {
            user_id: opt_text(row, "userId"),
            visit_id: opt_text(row, "visitId"),
            visitor_id: opt_text(row, "visitorId"),
            locale: opt_text(row, "locale"),
            time_zone: opt_text(row, "timeZone"),
            session_token: opt_text(row, "sessionToken"),
        },
        attributes: attributes_from_row(row)?,
    })
}

fn attributes_from_row(row: &DbRow) -> Result<BTreeMap<String, Json>, Error> {
    let text = match row.get("attributes") {
        Some(Value::String(text)) if !text.is_empty() => text.as_str(),
        Some(Value::String(_)) | Some(Value::Null) | None => return Ok(BTreeMap::new()),
        Some(_) => return Err(Error::new("HttpSession attributes is not text")),
    };
    parse_attributes(text)
}

fn base_from_snapshot(snapshot: &Snapshot) -> Base {
    Base {
        exists: true,
        revision: snapshot.revision,
        revision_value: snapshot.revision_value.clone(),
        columns: snapshot.columns.clone(),
        attributes: snapshot.attributes.clone(),
    }
}

fn snapshot_from_base(base: &Base) -> Snapshot {
    Snapshot {
        revision: base.revision,
        revision_value: base.revision_value.clone(),
        columns: base.columns.clone(),
        attributes: base.attributes.clone(),
    }
}

fn revision_of(value: Option<&Value>) -> (i64, Value) {
    match value {
        Some(Value::I64(number)) => (*number, Value::I64(*number)),
        Some(Value::I32(number)) => (i64::from(*number), Value::I32(*number)),
        Some(Value::String(text)) => (text.parse().unwrap_or(0), Value::String(text.clone())),
        Some(other) => (0, other.clone()),
        None => (0, Value::Null),
    }
}

fn opt_text(row: &DbRow, name: &str) -> Option<String> {
    match row.get(name) {
        Some(Value::String(text)) if !text.is_empty() => Some(text.clone()),
        _ => None,
    }
}

fn stage_rotated(factory: &ExecutionContextFactory, old_id: &str, new_id: &str, new_token: &str) -> Result<(), Error> {
    let mut table = lock_sessions(factory);
    let Some(old) = table.sessions.get(old_id) else {
        return Err(Error::new("bound session is missing"));
    };
    if old.retired {
        return Err(Error::new("bound session is missing"));
    }
    let mut born = old.clone();
    born.columns.session_token = Some(new_token.to_string());
    born.dirty_columns = all_columns();
    born.dirty_json = born.attributes.keys().cloned().collect();
    born.snapshot = None;
    born.invalidated = false;
    born.retired = false;
    born.revision = next_revision();
    table.sessions.insert(new_id.to_string(), born);
    Ok(())
}

fn retire_old(factory: &ExecutionContextFactory, old_id: &str) {
    let mut table = lock_sessions(factory);
    let Some(old) = table.sessions.get_mut(old_id) else {
        return;
    };
    old.dirty_columns.clear();
    old.dirty_json.clear();
    old.retired = true;
    old.revision = next_revision();
}

fn needs_flush(live: &LiveSession) -> bool {
    !live.retired && (live.touch || !live.dirty_columns.is_empty() || !live.dirty_json.is_empty())
}

fn bound_factory() -> Option<Arc<ExecutionContextFactory>> {
    let bound = with_session(|session| session.factory.clone());
    if bound.is_some() {
        return bound;
    }
    ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()))
}

fn thread_view(session: &mut Memory) -> ThreadView {
    ThreadView {
        bound_id: session.bound_id.clone(),
        factory: session.factory.clone(),
        user_id: session.user_id.clone(),
        username: session.username.clone(),
        visit_id: session.visit_id.clone(),
        visitor_id: session.visitor_id.clone(),
        locale: session.locale.clone(),
        time_zone: session.time_zone.clone(),
        csrf_token: session.csrf_token.clone(),
        anonymous: session.anonymous,
        attributes: session.attributes.clone(),
        dirty_columns: session.dirty_columns.clone(),
        dirty_json: session.dirty_json.clone(),
    }
}

fn apply_thread(live: &mut LiveSession, view: &ThreadView) {
    copy_dirty_columns(&view.dirty_columns, &view_columns(view), &mut live.columns);
    for key in &view.dirty_json {
        match view.attributes.get(key) {
            Some(value) => {
                live.attributes.insert(key.clone(), value.clone());
            }
            None => {
                live.attributes.remove(key);
            }
        }
    }
    live.dirty_columns.extend(&view.dirty_columns);
    live.dirty_json.extend(view.dirty_json.iter().cloned());
    live.username = view.username.clone();
    live.anonymous = view.anonymous;
    if view_dirty(view) {
        live.revision = next_revision();
    }
}

fn view_columns(view: &ThreadView) -> Columns {
    Columns {
        user_id: view.user_id.clone(),
        visit_id: view.visit_id.clone(),
        visitor_id: view.visitor_id.clone(),
        locale: view.locale.clone(),
        time_zone: view.time_zone.clone(),
        session_token: view.csrf_token.clone(),
    }
}

fn copy_dirty_columns(dirty: &BTreeSet<Column>, from: &Columns, into: &mut Columns) {
    if dirty.contains(&Column::UserId) {
        into.user_id = from.user_id.clone();
    }
    if dirty.contains(&Column::VisitId) {
        into.visit_id = from.visit_id.clone();
    }
    if dirty.contains(&Column::VisitorId) {
        into.visitor_id = from.visitor_id.clone();
    }
    if dirty.contains(&Column::Locale) {
        into.locale = from.locale.clone();
    }
    if dirty.contains(&Column::TimeZone) {
        into.time_zone = from.time_zone.clone();
    }
    if dirty.contains(&Column::SessionToken) {
        into.session_token = from.session_token.clone();
    }
}

fn live_from_thread(view: &ThreadView) -> LiveSession {
    LiveSession {
        columns: view_columns(view),
        username: view.username.clone(),
        anonymous: view.anonymous,
        attributes: view.attributes.clone(),
        dirty_columns: view.dirty_columns.clone(),
        dirty_json: view.dirty_json.clone(),
        snapshot: None,
        invalidated: false,
        retired: false,
        last_access: now_millis(),
        touch_mark: 0,
        touch: false,
        revision: next_revision(),
        client_ip: None,
    }
}

fn view_dirty(view: &ThreadView) -> bool {
    !view.dirty_columns.is_empty() || !view.dirty_json.is_empty()
}

fn all_columns() -> BTreeSet<Column> {
    BTreeSet::from([
        Column::UserId,
        Column::VisitId,
        Column::VisitorId,
        Column::Locale,
        Column::TimeZone,
        Column::SessionToken,
    ])
}

fn column_name(column: Column) -> &'static str {
    match column {
        Column::UserId => "userId",
        Column::VisitId => "visitId",
        Column::VisitorId => "visitorId",
        Column::Locale => "locale",
        Column::TimeZone => "timeZone",
        Column::SessionToken => "sessionToken",
    }
}

fn column_value(columns: &Columns, column: Column) -> Value {
    let text = match column {
        Column::UserId => &columns.user_id,
        Column::VisitId => &columns.visit_id,
        Column::VisitorId => &columns.visitor_id,
        Column::Locale => &columns.locale,
        Column::TimeZone => &columns.time_zone,
        Column::SessionToken => &columns.session_token,
    };
    match text {
        Some(value) => Value::string(value),
        None => Value::Null,
    }
}

fn mark_column(session: &mut Memory, column: Column) {
    if session.bound_id.is_some() {
        session.dirty_columns.insert(column);
    }
}

fn put_json(session: &mut Memory, key: &str, value: Json) {
    session.attributes.insert(key.to_string(), value);
    if session.bound_id.is_some() {
        session.dirty_json.insert(key.to_string());
    }
}

fn project_known(session: &mut Memory) {
    session.saved = attributes_string_map(&session.attributes, SAVED);
    session.errors = attributes_string_map(&session.attributes, ERROR_PARAMETERS);
    session.flash = attributes_string_list(&session.attributes, MESSAGE_PUBLIC);
    session.flash_errors = attributes_string_list(&session.attributes, MESSAGE_ERROR);
    session.screen_history = attributes_string_list(&session.attributes, SCREEN_HISTORY);
}

fn attributes_string_map(attributes: &BTreeMap<String, Json>, key: &str) -> BTreeMap<String, String> {
    let Some(Json::Object(map)) = attributes.get(key) else {
        return BTreeMap::new();
    };
    map.iter()
        .filter_map(|(name, value)| match value {
            Json::String(text) => Some((name.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

fn attributes_string_list(attributes: &BTreeMap<String, Json>, key: &str) -> Vec<String> {
    let Some(Json::Array(list)) = attributes.get(key) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|value| match value {
            Json::String(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn json_string_map(map: &BTreeMap<String, String>) -> Json {
    Json::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), Json::String(value.clone())))
            .collect(),
    )
}

fn json_string_list(list: &[String]) -> Json {
    Json::Array(list.iter().map(|text| Json::String(text.clone())).collect())
}

fn cookie_flags(https: bool, max_age: Option<u64>) -> String {
    let mut out = String::from("; HttpOnly; SameSite=Lax; Path=/");
    if let Some(age) = max_age {
        out.push_str(&format!("; Max-Age={age}"));
    }
    if https {
        out.push_str("; Secure");
    }
    out
}

fn split_session_cookie(value: &str) -> Option<(&str, &str)> {
    let (id, tag) = value.split_once('.')?;
    if tag.contains('.') || !is_session_id(id) {
        return None;
    }
    Some((id, tag))
}

fn is_session_id(id: &str) -> bool {
    id.len() == SESSION_ID_BYTES * 2
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sign_session(id: &str) -> Result<String, Error> {
    let key = cookie_key()?;
    let mut mac =
        SessionMac::new_from_slice(&key).map_err(|_| Error::new("session_cookie_key cannot be used as an HMAC key"))?;
    mac.update(id.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

fn mac_matches(id: &str, tag: &[u8]) -> Result<bool, Error> {
    let key = cookie_key()?;
    let mut mac =
        SessionMac::new_from_slice(&key).map_err(|_| Error::new("session_cookie_key cannot be used as an HMAC key"))?;
    mac.update(id.as_bytes());
    // verify_slice compares the tag in constant time.
    Ok(mac.verify_slice(tag).is_ok())
}

/// A key for another signature, derived from `session_cookie_key`: HMAC-SHA256 of `label` under it, so a value signed
/// with it is never a valid session tag (the MCP server's `requestState`, `crate::llm::mcp::state`).
///
/// # Errors
///
/// Returns an error when `session_cookie_key` is not set or not standard base64.
#[cfg(feature = "llm")]
pub(crate) fn derived_cookie_key(label: &str) -> Result<Vec<u8>, Error> {
    let key = cookie_key()?;
    let mut mac =
        SessionMac::new_from_slice(&key).map_err(|_| Error::new("session_cookie_key cannot be used as an HMAC key"))?;
    mac.update(label.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn cookie_key() -> Result<Vec<u8>, Error> {
    let raw = moqui_lib::env::get_prop_or_env("session_cookie_key");
    if raw.is_empty() {
        return Err(Error::new("session_cookie_key is not set"));
    }
    STANDARD
        .decode(raw.trim().as_bytes())
        .map_err(|_| Error::new("session_cookie_key is not standard base64"))
}

/// The `attributes` column, one compact JSON object.
fn encode_attributes(attributes: &BTreeMap<String, Json>) -> String {
    // A map with text keys always serializes.
    serde_json::to_string(attributes).unwrap_or_else(|_| "{}".to_string())
}

fn parse_attributes(text: &str) -> Result<BTreeMap<String, Json>, Error> {
    match serde_json::from_str::<Json>(text) {
        Ok(Json::Object(map)) => Ok(map.into_iter().collect()),
        Ok(_) => Err(Error::new("HttpSession attributes is not a JSON object")),
        Err(err) => Err(Error::new(format!("HttpSession attributes JSON is invalid: {err}"))),
    }
}

/// Process-wide lock for tests that set `session_cookie_key` or other webapp properties.
#[cfg(test)]
pub(crate) fn cookie_property_lock() -> MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    lock(&LOCK)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{mpsc, Arc, MutexGuard};
    use std::time::Duration;

    use base64::Engine;

    use moqui_lib::object::Value;

    use super::super::login;
    use super::super::password::{hash_login_key, simple_hash};
    use super::*;
    use crate::context::ExecutionContextFactory;
    use crate::entity::{EntityFacade, EntityValue};

    #[test]
    fn a_session_belongs_to_its_context_and_never_leaks_to_the_next() {
        let first = ExecutionContextFactory::test_context();
        {
            let _entered = first.enter();
            set_user("U1", "ada");
            assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
        }
        assert!(current_username().unwrap().is_none());
        let second = ExecutionContextFactory::test_context();
        let _entered = second.enter();
        assert!(
            current_username().unwrap().is_none(),
            "a new context starts with no user"
        );
    }

    #[test]
    fn session_attribute_round_trip_stays_on_this_context() {
        let ctx = ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        set_session_attribute("saved", Some("yes"));
        assert_eq!(session_attribute("saved").as_deref(), Some("yes"));
        set_session_attribute("saved", None);
        assert_eq!(session_attribute("saved"), None);
    }

    #[test]
    fn login_and_logout_rotate_the_csrf_token() {
        let ctx = ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        let anonymous = csrf_token().unwrap();
        set_user("U1", "ada");
        let after_login = csrf_token().unwrap();
        assert_ne!(anonymous, after_login);
        clear_login();
        assert_ne!(after_login, csrf_token().unwrap());
    }

    #[test]
    fn current_username_is_none_until_a_user_is_set() {
        let ctx = ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        reset_session();
        assert!(current_username().unwrap().is_none());
        assert!(current_user_id().unwrap().is_none());
        assert!(!is_anonymous().unwrap());
        set_user("U1", "ada");
        set_locale(Some("en"));
        set_time_zone(Some("UTC"));
        set_visit("100000", "visitor-1");
        add_flash_message("saved-flash");
        save_parameter("q", "1");
        set_error_parameter("password", "required");
        push_screen_history("Login");
        assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
        assert_eq!(current_user_id().unwrap().as_deref(), Some("U1"));
        assert_eq!(locale().unwrap().as_deref(), Some("en"));
        assert_eq!(time_zone().unwrap().as_deref(), Some("UTC"));
        assert_eq!(visit_id().unwrap().as_deref(), Some("100000"));
        assert_eq!(visitor_id().unwrap().as_deref(), Some("visitor-1"));
        assert_eq!(flash_messages().unwrap(), vec!["saved-flash".to_string()]);
        assert_eq!(saved_parameters().unwrap().get("q").map(String::as_str), Some("1"));
        assert_eq!(
            error_parameters().unwrap().get("password").map(String::as_str),
            Some("required")
        );
        assert_eq!(screen_history().unwrap(), vec!["Login".to_string()]);
        let token = csrf_token().unwrap();
        assert_eq!(token.len(), SESSION_TOKEN_BYTES * 2);
        assert_eq!(csrf_token().unwrap(), token);
        assert!(!is_anonymous().unwrap());
        clear_login();
        assert!(current_username().unwrap().is_none());
        assert!(!is_anonymous().unwrap());
        assert_eq!(visit_id().unwrap().as_deref(), Some("100000"));
        reset_session();
    }

    #[test]
    fn a_bad_mac_is_an_unknown_session() {
        let (_lock, ctx, _facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let id = "ab".repeat(SESSION_ID_BYTES);
        let header = format_session_cookie(&id, false).unwrap();
        assert!(header.starts_with("moqui.session="));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Lax"));
        assert!(header.contains("Path=/"));
        assert!(!header.contains("Secure"));
        let secure = format_session_cookie(&id, true).unwrap();
        assert!(secure.contains("Secure"));
        let visitor = format_visitor_cookie("visitor-1", false);
        assert!(visitor.starts_with("moqui.visitor=visitor-1;"));
        assert!(visitor.contains("Max-Age=31536000"));
        assert!(visitor.contains("HttpOnly"));
        assert!(!visitor.contains("Secure"));
        assert!(format_visitor_cookie("visitor-1", true).contains("Secure"));

        let value = cookie_value(&header);
        assert_eq!(verify_session_cookie(value).unwrap().as_deref(), Some(id.as_str()));
        let bad = tamper_mac(value);
        assert!(verify_session_cookie(&bad).unwrap().is_none());
        assert!(verify_session_cookie("nope").unwrap().is_none());
        assert!(verify_session_cookie("abc.def.ghi").unwrap().is_none());

        let guard = bind(&factory, Some(&bad), None, false).unwrap();
        assert_ne!(guard.session_id(), id);
        drop(guard);
        let ghost = format_session_cookie(&id, false).unwrap();
        let guard = bind(&factory, Some(cookie_value(&ghost)), None, false).unwrap();
        assert_ne!(guard.session_id(), id);
    }

    /// A visit records the request that starts it, as Java's; a user ends another session of theirs by its visit (its
    /// row goes, its live copy is retired, so its cookie starts over signed out, and the Visit gets its thruDate), but
    /// not the current one and not another user's.
    #[test]
    fn a_user_ends_another_session_of_theirs_by_its_visit() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let other_browser = |user: &'static str| {
            let factory = Arc::clone(&factory);
            std::thread::spawn(move || {
                let ctx = factory.new_context();
                let _entered = ctx.enter();
                let _request = VisitRequestGuard::set(VisitRequest {
                    url: format!("https://erp.example.com/qapps?{}", "x".repeat(300)),
                    referrer: "https://example.com/start".to_string(),
                    user_agent: "Firefox/140.0".to_string(),
                    locale: "de_DE".to_string(),
                    client_ip: "203.0.113.9".to_string(),
                });
                let guard = bind(&factory, None, None, false).unwrap();
                set_user(user, user);
                start_deferred_visit().unwrap();
                flush_dirty(&factory).unwrap();
                (
                    guard.session_cookie_header().unwrap(),
                    guard.session_id(),
                    visit_id().unwrap().unwrap(),
                )
            })
            .join()
            .unwrap()
        };
        let (b_cookie, b_id, b_visit) = other_browser("ada");
        let (_, _, bob_visit) = other_browser("bob");
        let visit = facade
            .find(VISIT)
            .unwrap()
            .eq("visitId", Value::string(&b_visit))
            .one()
            .unwrap()
            .unwrap();
        for (field, expected) in [
            ("userId", "ada"),
            ("sessionId", b_id.as_str()),
            ("webappName", "webroot"),
            ("initialLocale", "de_DE"),
            ("initialReferrer", "https://example.com/start"),
            ("initialUserAgent", "Firefox/140.0"),
            ("clientIpAddress", "203.0.113.9"),
            ("clientHostName", "203.0.113.9"),
        ] {
            assert_eq!(text(&visit, field), expected, "{field}");
        }
        assert_eq!(
            text(&visit, "initialRequest").chars().count(),
            255,
            "cut to 255 as Java"
        );
        assert!(!text(&visit, "serverHostName").is_empty() && !text(&visit, "serverIpAddress").is_empty());

        // this browser: Ada again
        let guard = bind(&factory, None, None, false).unwrap();
        set_user("ada", "ada");
        start_deferred_visit().unwrap();
        let a_visit = visit_id().unwrap().unwrap();
        assert!(end_user_visit(&a_visit)
            .unwrap_err()
            .to_string()
            .contains("current session"));
        assert!(end_user_visit(&bob_visit)
            .unwrap_err()
            .to_string()
            .contains("not one of yours"));
        assert!(end_user_visit("NoSuchVisit").is_err());
        assert_eq!(end_user_visit(&b_visit).unwrap(), 1);
        let ended = facade
            .find(VISIT)
            .unwrap()
            .eq("visitId", Value::string(&b_visit))
            .one()
            .unwrap()
            .unwrap();
        assert!(ended.get("thruDate").is_some_and(|value| !value.is_null()));
        assert!(
            load_session_optional(&facade, &b_id).is_none(),
            "the session row is gone"
        );
        assert!(lock_sessions(&factory)
            .sessions
            .get(&b_id)
            .is_some_and(|live| live.retired));
        assert_eq!(end_user_visit(&b_visit).unwrap(), 0, "nothing left to end");
        drop(guard);
        // the other browser's next request starts over, signed out
        let factory_b = Arc::clone(&factory);
        let (id_after, user_after) = std::thread::spawn(move || {
            let ctx = factory_b.new_context();
            let _entered = ctx.enter();
            let guard = bind(&factory_b, Some(cookie_value(&b_cookie)), None, false).unwrap();
            (guard.session_id(), current_user_id().unwrap())
        })
        .join()
        .unwrap();
        assert_ne!(id_after, b_id);
        assert_eq!(user_after, None);
    }

    #[test]
    fn flush_then_drop_the_map_reloads_the_signed_cookie() {
        let (_lock, ctx, _facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let guard = bind(&factory, None, None, false).unwrap();
        save_parameter("color", "blue");
        flush_dirty(&factory).unwrap();
        let id = guard.session_id();
        let cookie = guard.session_cookie_header().unwrap();
        drop(guard);
        clear_sessions(&factory);
        let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
        assert_eq!(guard.session_id(), id);
        assert_eq!(
            saved_parameters().unwrap().get("color").map(String::as_str),
            Some("blue")
        );
    }

    #[test]
    fn concurrent_requests_build_on_each_others_attribute_updates() {
        let (_lock, ctx, _facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let append = |item: &'static str| {
            move |current: Option<String>| {
                Some(current.map_or_else(|| item.to_string(), |text| format!("{text},{item}")))
            }
        };
        let guard = bind(&factory, None, None, false).unwrap();
        let id = guard.session_id();
        let cookie = guard.session_cookie_header().unwrap();
        update_session_attribute("ids", append("a"));
        // A second request of the session, while this one is still bound.
        std::thread::spawn({
            let factory = Arc::clone(&factory);
            move || {
                let ctx = factory.new_context();
                let _entered = ctx.enter();
                let _guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
                update_session_attribute("ids", append("b"));
            }
        })
        .join()
        .unwrap();
        update_session_attribute("ids", append("c"));
        assert_eq!(session_attribute("ids").as_deref(), Some("a,b,c"));
        drop(guard);
        let live = lock_sessions(&factory)
            .sessions
            .get(&id)
            .unwrap()
            .attributes
            .get("ids")
            .cloned();
        assert_eq!(live, Some(Json::String("a,b,c".into())));
    }

    #[test]
    fn an_idle_session_expires_and_its_row_is_deleted() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let guard = bind(&factory, None, None, false).unwrap();
        save_parameter("color", "blue");
        flush_dirty(&factory).unwrap();
        let id = guard.session_id();
        let cookie = guard.session_cookie_header().unwrap();
        drop(guard);
        assert!(read_row(&facade, &id).unwrap().is_some());

        // Two hours idle against the 60 minute default.
        lock_sessions(&factory).sessions.get_mut(&id).unwrap().last_access = now_millis() - 2 * 60 * 60_000;
        let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
        assert_ne!(guard.session_id(), id);
        assert!(saved_parameters().unwrap().is_empty());
        assert!(read_row(&facade, &id).unwrap().is_none());
    }

    #[test]
    fn the_sweep_drops_idle_sessions_and_keeps_active_ones() {
        let (_lock, ctx, _facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let guard = bind(&factory, None, None, false).unwrap();
        let active = guard.session_id();
        drop(guard);
        let stale = random_hex(SESSION_ID_BYTES).unwrap();
        {
            let mut table = lock_sessions(&factory);
            let mut live = table.sessions.get(&active).unwrap().clone();
            live.last_access = now_millis() - 3 * 60 * 60_000;
            table.sessions.insert(stale.clone(), live);
        }
        expire_idle(&factory).unwrap();
        let table = lock_sessions(&factory);
        assert!(table.sessions.contains_key(&active));
        assert!(!table.sessions.contains_key(&stale));
    }

    #[test]
    fn dirty_merge_keeps_a_clean_json_key() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let guard = bind(&factory, None, None, false).unwrap();
        save_parameter("a", "one");
        set_error_parameter("b", "two");
        flush_dirty(&factory).unwrap();
        let id = guard.session_id();
        invalidate(&id);
        let replacement = attributes_json("from-db", "two");
        exec_update(
            &facade,
            &["attributes"],
            &[Value::string(&replacement), Value::string(&id)],
        );
        set_error_parameter("b", "new");
        flush_dirty(&factory).unwrap();
        let row = load_session(&facade, &id);
        let map = parse_attributes(&text(&row, "attributes")).unwrap();
        assert_eq!(map.get(SAVED), Some(&serde_json::json!({"a": "from-db"})));
        assert_eq!(map.get(ERROR_PARAMETERS), Some(&serde_json::json!({"b": "new"})));
        assert_eq!(map.get("moqui.second.factor"), Some(&Json::String("kept".to_string())));
    }

    #[test]
    fn revision_conflict_retries_and_keeps_the_clean_column() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let guard = bind(&factory, None, None, false).unwrap();
        set_locale(Some("en"));
        save_parameter("q", "1");
        flush_dirty(&factory).unwrap();
        let id = guard.session_id();
        exec_update(
            &facade,
            &["locale", "sessionRevision"],
            &[Value::string("fr"), Value::I64(7), Value::string(&id)],
        );
        save_parameter("q", "2");
        flush_dirty(&factory).unwrap();
        let row = load_session(&facade, &id);
        assert_eq!(text(&row, "locale"), "fr");
        assert_eq!(i64_field(&row, "sessionRevision"), 8);
        let map = parse_attributes(&text(&row, "attributes")).unwrap();
        assert_eq!(map.get(SAVED), Some(&serde_json::json!({"q": "2"})));
    }

    #[test]
    fn password_login_rotates_and_a_failed_password_keeps_the_id() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        create_user(&facade, "ada", "Secret1!");
        let guard = bind(&factory, None, None, false).unwrap();
        flush_dirty(&factory).unwrap();
        let old_id = guard.session_id();
        let old_csrf = csrf_token().unwrap();
        assert!(login::login_user("ada", "wrong").is_err());
        assert_eq!(guard.session_id(), old_id);
        assert!(current_username().unwrap().is_none());
        assert!(load_session_optional(&facade, &old_id).is_some());

        assert!(visit_id().unwrap().is_none());
        login::login_user("ada", "Secret1!").unwrap();
        let new_id = guard.session_id();
        assert_ne!(new_id, old_id);
        assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
        let visit = visit_id()
            .unwrap()
            .expect("a login starts the visit the session put off");
        let visit = facade
            .find(VISIT)
            .unwrap()
            .eq("visitId", Value::string(&visit))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(text(&visit, "userId"), "ada");
        let new_csrf = csrf_token().unwrap();
        assert_ne!(new_csrf, old_csrf);
        assert_eq!(new_csrf.len(), SESSION_TOKEN_BYTES * 2);
        assert!(load_session_optional(&facade, &old_id).is_none());
        flush_dirty(&factory).unwrap();
        let row = load_session(&facade, &new_id);
        assert_eq!(text(&row, "userId"), "ada");

        login::logout().unwrap();
        let stayed = guard.session_id();
        login::internal_login_user("ada").unwrap();
        assert_eq!(guard.session_id(), stayed);
        login::logout().unwrap();
    }

    #[test]
    fn guarantee_flush_survives_clearing_the_factory_map() {
        let (_lock, ctx, _facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());

        let guard = bind(&factory, None, None, false).unwrap();
        save_parameter("k", "v");
        end_request().unwrap();
        let cookie = guard.session_cookie_header().unwrap();
        drop(guard);
        clear_sessions(&factory);
        let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
        assert!(!saved_parameters().unwrap().contains_key("k"));
        drop(guard);

        lock_sessions(&factory).mode = FlushMode::Guarantee;
        let guard = bind(&factory, None, None, false).unwrap();
        save_parameter("k", "v");
        end_request().unwrap();
        let id = guard.session_id();
        let cookie = guard.session_cookie_header().unwrap();
        drop(guard);
        clear_sessions(&factory);
        let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
        assert_eq!(guard.session_id(), id);
        assert_eq!(saved_parameters().unwrap().get("k").map(String::as_str), Some("v"));
    }

    #[test]
    fn login_key_header_logs_in_and_an_expired_key_does_not() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        create_user(&facade, "ada", "Secret1!");
        create_login_key(&facade, "temp-key", "ada", None);
        create_login_key(&facade, "old-key", "ada", Some(1));
        let guard = bind(&factory, None, None, false).unwrap();
        let id = guard.session_id();
        login::apply_header_keys(Some("temp-key"), None).unwrap();
        assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
        assert_eq!(guard.session_id(), id);
        login::logout().unwrap();
        assert!(login::apply_header_keys(None, Some("old-key")).is_err());
        assert!(current_username().unwrap().is_none());
        assert_eq!(guard.session_id(), id);
        login::login_user("ada", "Secret1!").unwrap();
        let rotated = guard.session_id();
        login::apply_header_keys(Some("not-a-key"), None).unwrap();
        assert_eq!(current_username().unwrap().as_deref(), Some("ada"));
        assert_eq!(guard.session_id(), rotated);
    }

    #[test]
    fn a_query_string_api_key_is_not_consulted() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        create_user(&facade, "ada", "Secret1!");
        create_login_key(&facade, "temp-key", "ada", None);
        save_parameter("api_key", "temp-key");
        let mut other = BTreeMap::new();
        other.insert("api_key".to_string(), "temp-key".to_string());
        login::apply_header_keys(None, None).unwrap();
        assert!(current_username().unwrap().is_none());
        assert_eq!(other.get("api_key").map(String::as_str), Some("temp-key"));
    }

    #[test]
    fn one_visit_per_bound_session_reuses_a_known_visitor() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let guard = bind(&factory, None, None, false).unwrap();
        let session_id = guard.session_id();
        assert!(
            visit_id().unwrap().is_none(),
            "a request with no cookie starts no visit"
        );
        assert!(guard.visitor_cookie_header().is_none());
        assert_eq!(count_entity(&facade, VISITOR), 0);
        let session_cookie = guard.session_cookie_header().unwrap();
        drop(guard);

        let guard = bind(&factory, Some(cookie_value(&session_cookie)), None, false).unwrap();
        assert_eq!(guard.session_id(), session_id);
        let visit = visit_id().unwrap().unwrap();
        let visitor = guard.visitor_id().unwrap();
        let visitor_cookie = guard.visitor_cookie_header().unwrap();
        drop(guard);
        let guard = bind(
            &factory,
            Some(cookie_value(&session_cookie)),
            Some(cookie_value(&visitor_cookie)),
            false,
        )
        .unwrap();
        assert_eq!(guard.session_id(), session_id);
        assert_eq!(visit_id().unwrap().as_deref(), Some(visit.as_str()));
        assert_eq!(guard.visitor_id().as_deref(), Some(visitor.as_str()));
        assert_eq!(count_entity(&facade, VISIT), 1);
        drop(guard);

        let guard = bind(&factory, None, Some(&visitor), false).unwrap();
        assert_ne!(guard.session_id(), session_id);
        assert_eq!(guard.visitor_id().as_deref(), Some(visitor.as_str()));
        assert_ne!(visit_id().unwrap().as_deref(), Some(visit.as_str()));
        assert_eq!(count_entity(&facade, VISIT), 2);
        assert_eq!(count_entity(&facade, VISITOR), 1);
        drop(guard);

        let guard = bind(&factory, None, Some("missing-visitor"), false).unwrap();
        assert_ne!(guard.visitor_id().as_deref(), Some("missing-visitor"));
        assert_eq!(count_entity(&facade, VISITOR), 2);
    }

    /// The visit gets the client address once: a later request from the same address of the session writes nothing,
    /// and one from a new address writes it.
    #[test]
    fn the_client_address_is_written_to_the_visit_only_when_it_changes() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let session_cookie = bind(&factory, None, None, false)
            .unwrap()
            .session_cookie_header()
            .unwrap();
        let visit_row = |visit: &str| {
            facade
                .find(VISIT)
                .unwrap()
                .eq("visitId", Value::string(visit))
                .use_cache(false)
                .one()
                .unwrap()
                .unwrap()
        };
        let request = |address: &str| {
            let guard = bind(&factory, Some(cookie_value(&session_cookie)), None, false).unwrap();
            remember_client_ip(address).unwrap();
            let visit = visit_id().unwrap().unwrap();
            drop(guard);
            visit
        };
        let visit = request("10.0.0.1");
        let first = visit_row(&visit);
        assert_eq!(first.get("clientIpAddress"), Some(&Value::string("10.0.0.1")));
        std::thread::sleep(std::time::Duration::from_millis(5));
        request("10.0.0.1");
        assert_eq!(
            visit_row(&visit).get("lastUpdatedStamp"),
            first.get("lastUpdatedStamp"),
            "the same address wrote the visit again"
        );
        request("10.0.0.2");
        assert_eq!(
            visit_row(&visit).get("clientIpAddress"),
            Some(&Value::string("10.0.0.2"))
        );
    }

    #[test]
    fn requests_bind_and_touch_while_a_flush_waits_on_sql() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let guard = bind(&factory, None, None, false).unwrap();
        save_parameter("color", "blue");
        let id = guard.session_id();
        let cookie = guard.session_cookie_header().unwrap();
        drop(guard);

        let blocked = BlockedFlush::new(&id);
        let flusher = {
            let factory = Arc::clone(&factory);
            let id = id.clone();
            std::thread::spawn(move || flush_id(&factory, &id))
        };
        blocked.entered.recv().unwrap();
        let (done, finished) = mpsc::channel();
        let request = {
            let factory = Arc::clone(&factory);
            std::thread::spawn(move || {
                let ctx = factory.new_context();
                let _entered = ctx.enter();
                let touched = bind(&factory, Some(cookie_value(&cookie)), None, false).map(|guard| guard.session_id());
                let fresh = bind(&factory, None, None, false).map(|guard| guard.session_id());
                done.send((touched, fresh)).unwrap();
            })
        };
        // The deadline only turns a regression into a failure instead of a hang.
        let (touched, fresh) = finished
            .recv_timeout(Duration::from_secs(60))
            .expect("bind waited for the flush");
        assert_eq!(touched.unwrap(), id);
        assert_ne!(fresh.unwrap(), id);
        assert!(read_row(&facade, &id).unwrap().is_none(), "the flush is still blocked");
        blocked.release();
        flusher.join().unwrap().unwrap();
        request.join().unwrap();
        let row = load_session(&facade, &id);
        let saved = parse_attributes(&text(&row, "attributes")).unwrap();
        assert_eq!(saved.get(SAVED), Some(&serde_json::json!({"color": "blue"})));
    }

    #[test]
    fn a_write_during_a_flush_stays_dirty_and_is_written_next() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        let guard = bind(&factory, None, None, false).unwrap();
        save_parameter("color", "blue");
        let id = guard.session_id();
        let cookie = guard.session_cookie_header().unwrap();
        drop(guard);

        let blocked = BlockedFlush::new(&id);
        let flusher = {
            let factory = Arc::clone(&factory);
            let id = id.clone();
            std::thread::spawn(move || flush_id(&factory, &id))
        };
        blocked.entered.recv().unwrap();
        let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
        save_parameter("color", "red");
        drop(guard);
        blocked.release();
        flusher.join().unwrap().unwrap();

        let saved = |facade: &EntityFacade| {
            let row = load_session(facade, &id);
            parse_attributes(&text(&row, "attributes")).unwrap().remove(SAVED)
        };
        assert_eq!(saved(&facade), Some(serde_json::json!({"color": "blue"})));
        {
            let table = lock_sessions(&factory);
            let live = table.sessions.get(&id).unwrap();
            assert!(needs_flush(live));
            assert!(live.dirty_json.contains(SAVED));
        }
        flush_dirty(&factory).unwrap();
        assert_eq!(saved(&facade), Some(serde_json::json!({"color": "red"})));
        assert!(!needs_flush(lock_sessions(&factory).sessions.get(&id).unwrap()));
    }

    #[test]
    fn cookieless_requests_keep_the_table_within_its_cap() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        lock_sessions(&factory).max_live = Some(5);
        for _ in 0..20 {
            drop(bind(&factory, None, None, false).unwrap());
            assert!(lock_sessions(&factory).sessions.len() <= 5);
        }
        assert_eq!(count_entity(&facade, VISITOR), 0);
        assert_eq!(count_entity(&facade, VISIT), 0);
    }

    #[test]
    fn eviction_writes_a_dirty_session_before_dropping_it() {
        let (_lock, ctx, facade, _key) = setup();
        let _entered = ctx.enter();
        let factory = Arc::clone(ctx.factory());
        lock_sessions(&factory).max_live = Some(3);
        let guard = bind(&factory, None, None, false).unwrap();
        save_parameter("color", "blue");
        let id = guard.session_id();
        let cookie = guard.session_cookie_header().unwrap();
        drop(guard);
        assert!(read_row(&facade, &id).unwrap().is_none());
        // Sessions made in the same millisecond tie, so this one is made the oldest.
        lock_sessions(&factory).sessions.get_mut(&id).unwrap().last_access -= 60_000;
        for _ in 0..5 {
            drop(bind(&factory, None, None, false).unwrap());
        }
        assert!(!lock_sessions(&factory).sessions.contains_key(&id));
        let guard = bind(&factory, Some(cookie_value(&cookie)), None, false).unwrap();
        assert_eq!(guard.session_id(), id);
        assert_eq!(
            saved_parameters().unwrap().get("color").map(String::as_str),
            Some("blue")
        );
    }

    #[test]
    fn stored_attributes_keep_their_text_and_bad_ones_are_errors() {
        let stored = r#"{"a":{"b":[1.50,true,null],"z":"x\"y"},"emoji":"😀","n":12345678901234567890123}"#;
        let parsed = parse_attributes(stored).unwrap();
        assert_eq!(parsed.get("emoji"), Some(&Json::String("\u{1F600}".to_string())));
        assert_eq!(
            attribute_text(parsed.get("n")).as_deref(),
            Some("12345678901234567890123")
        );
        assert_eq!(
            encode_attributes(&parsed),
            "{\"a\":{\"b\":[1.50,true,null],\"z\":\"x\\\"y\"},\"emoji\":\"\u{1F600}\",\"n\":12345678901234567890123}"
        );
        assert!(parse_attributes("[1]").is_err());
        let deep = format!("{{\"a\":{}{}}}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(parse_attributes(&deep).is_err());
    }

    type FlushHook = Arc<dyn Fn() + Send + Sync>;

    static FLUSH_HOOK: std::sync::Mutex<Option<(String, FlushHook)>> = std::sync::Mutex::new(None);

    /// Called by [`flush_id`] after it copied what it writes and before its SQL.
    pub(super) fn flush_hook(id: &str) {
        let hook = lock(&FLUSH_HOOK)
            .as_ref()
            .filter(|(hook_id, _)| hook_id == id)
            .map(|(_, hook)| Arc::clone(hook));
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Holds the flushes of one session id at [`flush_hook`] until released.
    struct BlockedFlush {
        entered: mpsc::Receiver<()>,
        release: mpsc::Sender<()>,
    }

    impl BlockedFlush {
        fn new(id: &str) -> Self {
            let (entered_tx, entered) = mpsc::channel();
            let (release, release_rx) = mpsc::channel::<()>();
            let release_rx = std::sync::Mutex::new(release_rx);
            let hook: FlushHook = Arc::new(move || {
                let _ = entered_tx.send(());
                let _ = lock(&release_rx).recv();
            });
            *lock(&FLUSH_HOOK) = Some((id.to_string(), hook));
            Self { entered, release }
        }

        fn release(self) {
            *lock(&FLUSH_HOOK) = None;
            let _ = self.release.send(());
        }
    }

    struct KeyGuard;

    impl KeyGuard {
        fn install() -> Self {
            moqui_lib::env::set_property("session_cookie_key", test_cookie_key());
            Self
        }
    }

    impl Drop for KeyGuard {
        fn drop(&mut self) {
            moqui_lib::env::clear_property("session_cookie_key");
            reset_session();
        }
    }

    fn setup() -> (
        MutexGuard<'static, ()>,
        crate::context::ExecutionContext,
        EntityFacade,
        KeyGuard,
    ) {
        let lock = cookie_tests_lock();
        reset_session();
        let key = KeyGuard::install();
        let ctx = ExecutionContextFactory::test_context();
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        (lock, ctx, facade, key)
    }

    fn cookie_tests_lock() -> MutexGuard<'static, ()> {
        super::cookie_property_lock()
    }

    fn test_cookie_key() -> String {
        STANDARD.encode([9_u8; 32])
    }

    fn clear_sessions(factory: &ExecutionContextFactory) {
        lock_sessions(factory).sessions.clear();
    }

    fn cookie_value(header: &str) -> &str {
        header.split(';').next().unwrap().split_once('=').unwrap().1
    }

    fn tamper_mac(value: &str) -> String {
        let (id, mac) = value.split_once('.').unwrap();
        let mut raw = URL_SAFE_NO_PAD.decode(mac).unwrap();
        raw[0] ^= 0x01;
        format!("{id}.{}", URL_SAFE_NO_PAD.encode(raw))
    }

    fn attributes_json(saved: &str, error: &str) -> String {
        let mut saved_map = BTreeMap::new();
        saved_map.insert("a".to_string(), saved.to_string());
        let mut error_map = BTreeMap::new();
        error_map.insert("b".to_string(), error.to_string());
        let mut root = BTreeMap::new();
        root.insert(SAVED.to_string(), json_string_map(&saved_map));
        root.insert(ERROR_PARAMETERS.to_string(), json_string_map(&error_map));
        root.insert("moqui.second.factor".to_string(), Json::String("kept".to_string()));
        encode_attributes(&root)
    }

    fn exec_update(facade: &EntityFacade, columns: &[&str], params: &[Value]) {
        let sql = facade
            .with_entity(HTTP_SESSION, |entity, dialect, _| {
                let condition = moqui_db::sql::Condition::bound("sessionId", moqui_db::sql::CompareOp::Equals);
                moqui_db::sql::update(entity, dialect, columns, Some(&condition)).map(|statement| statement.sql)
            })
            .unwrap();
        let tx = facade.transactions_for(HTTP_SESSION).unwrap();
        crate::entity::exec::with_reserve_tx(&tx, |conn| {
            conn.execute(&sql, params)?;
            Ok(())
        })
        .unwrap();
    }

    fn create_user(facade: &EntityFacade, username: &str, password: &str) {
        let stored = simple_hash(password, "s4lt", "SHA-256", false).unwrap();
        let mut value = facade.make_value(USER_ACCOUNT).unwrap();
        value.set("userId", Value::string(username));
        value.set("username", Value::string(username));
        value.set("currentPassword", Value::string(stored));
        value.set("passwordSalt", Value::string("s4lt"));
        value.set("passwordHashType", Value::string("SHA-256"));
        value.set("passwordBase64", Value::string("N"));
        value.create(facade).unwrap();
    }

    fn create_login_key(facade: &EntityFacade, key: &str, user_id: &str, thru_millis: Option<i64>) {
        let mut row = facade.make_value("moqui.security.UserLoginKey").unwrap();
        row.set("loginKey", Value::string(hash_login_key(key).unwrap()));
        row.set("userId", Value::string(user_id));
        row.set("fromDate", stamp(1));
        if let Some(millis) = thru_millis {
            row.set("thruDate", stamp(millis));
        }
        row.create(facade).unwrap();
    }

    fn load_session(facade: &EntityFacade, id: &str) -> EntityValue {
        load_session_optional(facade, id).unwrap_or_else(|| panic!("missing session {id}"))
    }

    fn load_session_optional(facade: &EntityFacade, id: &str) -> Option<EntityValue> {
        facade
            .find(HTTP_SESSION)
            .unwrap()
            .eq("sessionId", Value::string(id))
            .one()
            .unwrap()
    }

    fn count_entity(facade: &EntityFacade, name: &str) -> usize {
        facade.find(name).unwrap().list().unwrap().len()
    }

    fn text(value: &EntityValue, field: &str) -> String {
        value.get(field).and_then(Value::as_str).unwrap_or("").to_string()
    }

    fn i64_field(value: &EntityValue, field: &str) -> i64 {
        match value.get(field) {
            Some(Value::I64(number)) => *number,
            Some(Value::I32(number)) => i64::from(*number),
            _ => 0,
        }
    }

    fn stamp(millis: i64) -> Value {
        moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
    }
}
