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

//! Where the entity facade reaches the execution context (plans/Crate-Split.md). The framework above submits one
//! [`EntityHooks`]; with none linked, each function here answers as code outside an entered context sees it: no user,
//! no authorization, no scripts, ECA rules not run, the clock for the effective time.

use std::any::Any;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use jiff::tz::TimeZone;
use moqui_db::model::EntityDefinition;
use moqui_db::sql::Condition;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Converted, Value};

use crate::entity::load_feed::{DataFileService, EcaRule};
use crate::entity::{EntityFacade, EntityValue, FieldMap};
use crate::error::Error;

/// Submit the framework's [`EntityHooks`]; moqui-core does, once.
pub use inventory::submit;

/// A guard the framework hands back; dropping it undoes what it did (a disabled authorization check).
pub type Guard = Box<dyn Any>;

/// One SQL statement's span and duration, from [`trace_statement`] until [`StatementTrace::finish`].
pub trait StatementTrace {
    /// The statement ended, with `error` when it failed.
    fn finish(self: Box<Self>, error: Option<&str>);
}

/// What the entity facade calls in the framework. Every entry answers for the context entered on this thread.
// The entries spell out their signatures, which aliases would only hide.
#[allow(clippy::type_complexity)]
pub struct EntityHooks {
    // ambient
    /// An execution context is entered on this thread.
    pub entered: fn() -> bool,
    /// The facade of the entered context's factory.
    pub current_facade: fn() -> Option<EntityFacade>,
    /// The entered context's effective time, epoch millis, when it has one.
    pub effective_millis: fn() -> Option<i64>,
    pub user_id: fn() -> Option<String>,
    pub visit_id: fn() -> Option<String>,
    /// The agent the entered context works for.
    pub agent_id: fn() -> Option<String>,
    /// The user is in the group.
    pub in_group: fn(&str) -> Result<bool, Error>,
    /// The user's locale (`ec.user.locale`).
    pub locale: fn() -> String,
    /// The user's time zone, else the server's.
    pub time_zone: fn() -> TimeZone,
    pub entity_eca_disabled: fn() -> bool,
    /// An attribute of an element of the factory's conf, `${}` expanded.
    pub conf_setting: fn(&str, &str) -> String,
    /// The file of a component or runtime location.
    pub locate: fn(&str) -> Result<PathBuf, Error>,
    pub add_message: fn(String),
    pub add_error: fn(String),
    pub message_errors: fn() -> Vec<String>,
    /// Take the message errors as one text, clearing them.
    pub take_errors: fn() -> Option<String>,
    /// A find that requires search parameters had none (`_entityListNoSearchParms`).
    pub note_no_search_params: fn(),
    // authorization
    pub authz_disabled: fn() -> bool,
    pub disable_authz: fn() -> Option<Guard>,
    /// Push an `AT_ENTITY` frame (name, action, whether it needs an allow, hit sub type); `None` with no context.
    pub push_entity: fn(&str, &str, bool, &str) -> Option<Result<(), Error>>,
    /// Whether an `AT_ENTITY` frame (name, action, whether it needs an allow) would be allowed, without pushing it or
    /// counting a hit; `None` with no context.
    pub entity_permitted: fn(&str, &str, bool) -> Option<Result<bool, Error>>,
    pub pop_artifact: fn(),
    /// Set the hit sub type of the frame on top.
    pub set_hit_detail: fn(&str),
    pub stack_names: fn() -> String,
    pub entity_filters: fn(&str, Option<Condition>) -> Option<Condition>,
    pub view_filters: fn(&EntityDefinition, Option<Condition>) -> Option<Condition>,
    /// An entity was written, for the authorization cache.
    pub note_write: fn(&str),
    // scripts
    pub push_frame: fn(),
    pub pop_frame: fn(),
    pub set_name: fn(&str, Value) -> Result<(), Error>,
    pub eval: fn(&str) -> Result<Value, Error>,
    pub expand: fn(&str) -> Result<String, Error>,
    pub context_value: fn(&str) -> Option<Value>,
    /// The fields of a map or a script's entity value.
    pub value_map: fn(&Value) -> Option<IndexMap<String, Value>>,
    pub value_items: fn(&Value) -> Option<Vec<Value>>,
    // services
    /// Call a service, with authorization off when asked.
    pub call: fn(&str, &BTreeMap<String, Value>, bool) -> Result<BTreeMap<String, Value>, Error>,
    pub store_entity: fn(&EntityFacade, &str, &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError>,
    /// Run the matched entity ECA rules of one phase.
    pub run_eca:
        fn(&EntityFacade, &str, &str, bool, &[EcaRule], &mut FieldMap, Option<&EntityValue>) -> Result<(), LibError>,
    pub run_data_file_service: fn(&EntityFacade, &DataFileService) -> Result<(), LibError>,
    pub schedule_feed_receive: fn(&EntityFacade, Vec<String>) -> Result<(), LibError>,
    /// The service a data file names, as Java's `isServiceDefined` takes it.
    pub defined_service_name: fn(&str) -> Option<String>,
    pub registration_failures: fn(&str) -> Vec<String>,
    // observers
    /// Publish a write to the cluster's caches.
    pub cluster_note_write: fn(&EntityFacade, &str, &[String], &FieldMap) -> Result<(), Error>,
    pub trace_statement: fn(&str) -> Option<Box<dyn StatementTrace>>,
    /// No other thread returns a connection or fires a deadline (the inline executor).
    pub inline_executor: fn() -> bool,
}
inventory::collect!(EntityHooks);

impl EntityHooks {
    /// No context entered.
    pub const NONE: Self = Self {
        entered: || false,
        current_facade: || None,
        effective_millis: || None,
        user_id: || None,
        visit_id: || None,
        agent_id: || None,
        in_group: |_| Ok(false),
        locale: || "en_US".to_string(),
        time_zone: moqui_lib::object::text_time_zone,
        entity_eca_disabled: || false,
        conf_setting: |_, _| String::new(),
        // a file path or `file:` location; the component and classpath roots are the framework's
        locate: |location| {
            let scheme = ["component:", "classpath:", "content:"]
                .iter()
                .any(|scheme| location.starts_with(scheme));
            if scheme || (location.contains("://") && !location.starts_with("file:")) {
                return Err(Error::new(format!("no execution context to locate {location}")));
            }
            crate::resource::file_path(location)
        },
        add_message: |_| {},
        add_error: |_| {},
        message_errors: Vec::new,
        take_errors: || None,
        note_no_search_params: || {},
        authz_disabled: || false,
        disable_authz: || None,
        push_entity: |_, _, _, _| None,
        entity_permitted: |_, _, _| None,
        pop_artifact: || {},
        set_hit_detail: |_| {},
        stack_names: String::new,
        entity_filters: |_, condition| condition,
        view_filters: |_, condition| condition,
        note_write: |_| {},
        push_frame: || {},
        pop_frame: || {},
        set_name: |_, _| Ok(()),
        eval: |expression| Err(Error::new(format!("no script runtime to evaluate {expression}"))),
        expand: |text| Err(Error::new(format!("no script runtime to expand {text}"))),
        context_value: |_| None,
        value_map: |value| match value {
            Value::Map(entries) => Some(entries.clone()),
            _ => None,
        },
        value_items: |value| match value {
            Value::List(items) => Some(items.clone()),
            _ => None,
        },
        call: |name, _, _| Err(Error::new(format!("no service facade to call {name}"))),
        store_entity: |_, name, _| Err(LibError::new(format!("no service facade to store {name}"))),
        run_eca: |_, _, _, _, _, _, _| Ok(()),
        run_data_file_service: |_, service| Err(LibError::new(format!("no service facade to run {}", service.name))),
        schedule_feed_receive: |_, _| Ok(()),
        defined_service_name: |_| None,
        registration_failures: |_| Vec::new(),
        cluster_note_write: |_, _, _, _| Ok(()),
        trace_statement: |_| None,
        inline_executor: || false,
    };
}

/// The framework's hooks, else [`EntityHooks::NONE`].
#[must_use]
pub fn get() -> &'static EntityHooks {
    static HOOKS: OnceLock<&'static EntityHooks> = OnceLock::new();
    HOOKS.get_or_init(|| {
        let mut entries = inventory::iter::<EntityHooks>.into_iter();
        let first = entries.next();
        if entries.next().is_some() {
            tracing::error!("more than one framework submitted EntityHooks; using the first");
        }
        first.unwrap_or(&EntityHooks::NONE)
    })
}

pub fn entered() -> bool {
    (get().entered)()
}

/// The clock, epoch millis.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// Java's `ec.user.nowTimestamp` as epoch millis: the entered context's effective time, or the clock.
pub fn now_effective_millis() -> i64 {
    (get().effective_millis)().unwrap_or_else(now_millis)
}

/// A `java.sql.Timestamp` value of epoch `millis`.
pub fn stamp_value(millis: i64) -> Value {
    moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

/// Java's `ec.user.nowTimestamp`.
pub fn now_effective_value() -> Value {
    stamp_value(now_effective_millis())
}

/// `moqui_lib::object::basic_convert`, with timestamp text in the user's time zone.
pub fn basic_convert(value: &Value, java_type: &str) -> Converted {
    if is_timestamp_text(value, java_type) {
        moqui_lib::object::basic_convert_in(value, java_type, &(get().time_zone)())
    } else {
        moqui_lib::object::basic_convert(value, java_type)
    }
}

/// `moqui_lib::object::convert_checked`, with timestamp text in the user's time zone, as [`basic_convert`].
pub fn convert_checked(value: &Value, java_type: &str) -> Option<Value> {
    if is_timestamp_text(value, java_type) {
        moqui_lib::object::convert_checked_in(value, java_type, &(get().time_zone)())
    } else {
        moqui_lib::object::convert_checked(value, java_type)
    }
}

fn is_timestamp_text(value: &Value, java_type: &str) -> bool {
    matches!(value, Value::String(_)) && java_type.rsplit('.').next() == Some("Timestamp")
}

/// The user's time zone, else the server's.
pub fn time_zone() -> TimeZone {
    (get().time_zone)()
}

/// A `resource-facade` attribute of the conf, trimmed; empty with no context or when the conf leaves it out.
pub fn resource_setting(name: &str) -> String {
    (get().conf_setting)("resource-facade", name)
}

/// The scripts' context for one evaluation: names set after [`ScriptFrame::push`] are gone when it drops.
#[must_use = "the frame is popped when the guard drops"]
pub struct ScriptFrame;

impl ScriptFrame {
    pub fn push() -> Self {
        (get().push_frame)();
        Self
    }
}

impl Drop for ScriptFrame {
    fn drop(&mut self) {
        (get().pop_frame)();
    }
}

pub fn set_name(name: &str, value: Value) -> Result<(), Error> {
    (get().set_name)(name, value)
}

pub fn eval(expression: &str) -> Result<Value, Error> {
    (get().eval)(expression)
}

pub fn expand(text: &str) -> Result<String, Error> {
    (get().expand)(text)
}

/// Call a service with its own options: authorization off when `disable_authz`.
pub fn call(
    name: &str,
    parameters: &BTreeMap<String, Value>,
    disable_authz: bool,
) -> Result<BTreeMap<String, Value>, Error> {
    (get().call)(name, parameters, disable_authz)
}

/// [`StatementTrace`] of one statement, inside a trace.
pub fn trace_statement(sql: &str) -> Option<Box<dyn StatementTrace>> {
    (get().trace_statement)(sql)
}
