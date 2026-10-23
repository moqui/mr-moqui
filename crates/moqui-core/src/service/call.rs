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

//! Synchronous service call.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use moqui_db::model::{EntityDefinition, KeyMap};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, Value};

use super::define::{Authenticate, ServiceDef, ServiceKind};
use super::util::has_error;
use super::validate;
use crate::context::{Error, ExecutionContext};
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

/// Per-call overrides of the service definition, set by the call builder.
#[derive(Clone, Debug, Default)]
pub struct CallOptions {
    pub disable_authz: bool,
    pub ignore_transaction: Option<bool>,
    pub require_new_transaction: Option<bool>,
    pub use_transaction_cache: Option<bool>,
    pub timeout: Option<std::time::Duration>,
    /// `multi(true)`: one call for each row of `name_N` parameters, as a multi form-list posts them.
    pub multi: bool,
    /// `softValidate(true)`: input that fails validation is reported as danger messages, not errors, and the service
    /// does not run, as Java's.
    pub soft_validate: bool,
}

impl CallOptions {
    fn attribute(&self, base: TransactionAttribute) -> TransactionAttribute {
        let base_new = matches!(
            base,
            TransactionAttribute::ForceNew | TransactionAttribute::ForceCache | TransactionAttribute::IgnoreSuspend
        );
        let new = self.require_new_transaction.unwrap_or(base_new);
        // As Java: ignore stops the begin and force-new the join, independently.
        let ignore = self.ignore_transaction == Some(true)
            || matches!(base, TransactionAttribute::Ignore | TransactionAttribute::IgnoreSuspend);
        if ignore {
            return if new {
                TransactionAttribute::IgnoreSuspend
            } else {
                TransactionAttribute::Ignore
            };
        }
        let base_cache = matches!(base, TransactionAttribute::Cache | TransactionAttribute::ForceCache);
        let cache = self.use_transaction_cache.unwrap_or(base_cache);
        match (new, cache) {
            (false, false) => TransactionAttribute::UseOrBegin,
            (true, false) => TransactionAttribute::ForceNew,
            (false, true) => TransactionAttribute::Cache,
            (true, true) => TransactionAttribute::ForceCache,
        }
    }
}

/// Run `name` on a service worker and return at once. The worker has its own execution context and
/// acts as the current user. A failure is logged there. If the worker queue is full the call is
/// stored as a one-shot service job instead.
///
/// # Errors
///
/// Returns an error when there is no execution context, the pool is not running, or a full queue
/// cannot store the call.
pub fn async_call(name: &str, parameters: &BTreeMap<String, Value>) -> Result<(), Error> {
    let factory = super::pool::capture_factory()?;
    factory.service_pools().submit(&canonical(name), parameters)
}

/// Run `name` on a service worker and wait for its out parameters. [`start_future`] returns without waiting.
///
/// # Errors
///
/// Returns an error when the call fails, or the worker queue is full.
pub fn call_future(name: &str, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
    let factory = super::pool::capture_factory()?;
    factory.service_pools().call_future(&canonical(name), parameters)
}

/// Start `name` on a service worker and return a handle for its out parameters. Like
/// [`call_future`], a full queue is an error at once.
///
/// # Errors
///
/// Returns an error when there is no execution context, the pool is not running, or the queue is full.
pub fn start_future(name: &str, parameters: &BTreeMap<String, Value>) -> Result<ServiceFuture, Error> {
    let factory = super::pool::capture_factory()?;
    let receiver = factory.service_pools().submit_future(&canonical(name), parameters)?;
    Ok(ServiceFuture { receiver, done: None })
}

/// A started service call. Java's `Future<Map>` from `callFuture`.
pub struct ServiceFuture {
    receiver: super::pool::CallReceiver,
    done: Option<Result<BTreeMap<String, Value>, Error>>,
}

impl ServiceFuture {
    fn settle(&mut self, wait: Option<std::time::Duration>) -> Result<bool, Error> {
        if self.done.is_some() {
            return Ok(true);
        }
        let received = match wait {
            None => Some(super::pool::receive(&self.receiver)),
            Some(limit) if limit.is_zero() => match self.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err(Error::new("service worker pool is not running")))
                }
            },
            Some(limit) => match self.receiver.recv_timeout(limit) {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    Some(Err(Error::new("service worker pool is not running")))
                }
            },
        };
        self.done = received;
        Ok(self.done.is_some())
    }

    /// Whether the call has finished, successfully or not.
    pub fn is_done(&mut self) -> bool {
        self.settle(Some(std::time::Duration::ZERO)).unwrap_or(false)
    }

    /// Wait for the out parameters; with a limit, `Ok(None)` means it was not done in time.
    ///
    /// # Errors
    ///
    /// Returns the service's own error when the call failed.
    pub fn get(&mut self, limit: Option<std::time::Duration>) -> Result<Option<BTreeMap<String, Value>>, Error> {
        if !self.settle(limit)? {
            return Ok(None);
        }
        match &self.done {
            Some(Ok(out)) => Ok(Some(out.clone())),
            Some(Err(err)) => Err(Error::new(err.message().to_string())),
            None => Ok(None),
        }
    }
}

/// Run `name` and return its out parameters. `create-entity` is `create#entity`.
pub fn sync(name: &str, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
    sync_with(name, parameters, &CallOptions::default())
}

/// The multi call of Java's `ServiceCallSyncImpl`: for each row `N` (from 0, until a row has none of the in-parameters
/// as `name_N`), the service is called with that row's values, then the plain parameters and the results so far for the
/// rest. With `_useRowSubmit` (or `_useRowSubmit_N`) only rows with `_rowSubmit_N` run. All rows run in one transaction,
/// which an error rolls back; the first error stops the rows. The results of the rows are merged.
///
/// # Errors
///
/// Returns an error when a call fails other than with error messages.
pub fn sync_multi(
    name: &str,
    parameters: &BTreeMap<String, Value>,
    options: &CallOptions,
) -> Result<BTreeMap<String, Value>, Error> {
    let name = canonical(name);
    let in_names: Vec<String> = match service_in_parameter_names(&name) {
        Some(names) => names,
        None => match entity_auto_noun(&name) {
            Some(entity) => current_facade()
                .and_then(|facade| {
                    facade
                        .with_entity(&entity, |def, _, _| {
                            Ok(def.field_names().into_iter().map(str::to_string).collect())
                        })
                        .ok()
                })
                .unwrap_or_default(),
            None => Vec::new(),
        },
    };
    let facade = current_facade().ok_or_else(|| Error::new("no entity facade is installed"))?;
    let entity = entity_auto_noun(&name).unwrap_or_else(|| ServiceDef::semaphore_entity().to_string());
    let tx = facade.transactions_for(&entity)?;
    let single = CallOptions {
        multi: false,
        ..options.clone()
    };
    let flag = |key: &str| parameters.get(key).map(moqui_lib::object::to_plain_string).as_deref() == Some("true");
    let mut result: BTreeMap<String, Value> = BTreeMap::new();
    let outcome = tx.run(TransactionAttribute::UseOrBegin, None, || {
        for index in 0.. {
            let mut row: BTreeMap<String, Value> = BTreeMap::new();
            for parameter in &in_names {
                if let Some(value) = parameters.get(&format!("{parameter}_{index}")) {
                    row.insert(parameter.clone(), value.clone());
                }
            }
            if row.is_empty() {
                break;
            }
            if (flag("_useRowSubmit") || flag(&format!("_useRowSubmit_{index}")))
                && !flag(&format!("_rowSubmit_{index}"))
            {
                continue;
            }
            for parameter in &in_names {
                if row.get(parameter).is_some_and(|value| !is_empty(value)) {
                    continue;
                }
                if let Some(value) = parameters.get(parameter).filter(|value| !is_empty(value)) {
                    row.insert(parameter.clone(), value.clone());
                } else if let Some(value) = result.get(parameter).filter(|value| !is_empty(value)) {
                    row.insert(parameter.clone(), value.clone());
                }
            }
            let produced = sync_with(&name, &row, &single)?;
            result.extend(produced);
            if has_error() {
                break;
            }
        }
        if has_error() && tx.in_transaction() {
            tx.set_rollback_only(format!("Error message found running service {name} in multi mode"))?;
        }
        Ok(())
    });
    match outcome {
        Ok(()) => Ok(result),
        // The rows' errors are messages; the rollback they cause is not a failure of the call.
        Err(_) if has_error() => Ok(result),
        Err(err) => Err(err.into()),
    }
}

/// [`sync`] with per-call overrides of authorization and the transaction attribute.
pub fn sync_with(
    name: &str,
    parameters: &BTreeMap<String, Value>,
    options: &CallOptions,
) -> Result<BTreeMap<String, Value>, Error> {
    let _authz = if options.disable_authz {
        crate::context::ExecutionContext::with_current(|ctx| ctx.disable_authz())
    } else {
        None
    };
    let name = canonical(name);
    let facade = current_facade();
    let def = resolve(&name, facade.as_ref())?;
    super::coverage::record(&def.name);
    super::seca::run_seca_rules(&def.name, "pre-validate", parameters, None)?;
    // Input that fails its checks is validation errors in the message facade and no run, as in Java: the caller sees
    // the errors (a screen shows them by their fields) instead of a failed call.
    let mark = crate::context::ExecutionContext::with_current(|ctx| ctx.message().error_mark());
    let cleaned = match validate::clean(&def, parameters, facade.as_ref()) {
        Ok(cleaned) => cleaned,
        Err(err) if err.kind() == crate::context::ErrorKind::Validation && options.soft_validate => {
            if let Some(mark) = mark {
                crate::context::ExecutionContext::with_current(|ctx| {
                    let mut messages = ctx.message();
                    for text in messages.take_errors_since(mark) {
                        messages.add_message_typed(text, "danger");
                    }
                });
            }
            return Ok(BTreeMap::new());
        }
        Err(err) if err.kind() == crate::context::ErrorKind::Validation => {
            tracing::warn!(service = %def.name, "not run, input failed validation: {}", err.message());
            return Ok(BTreeMap::new());
        }
        Err(err) => return Err(err),
    };
    super::seca::run_seca_rules(&def.name, "pre-auth", &cleaned, None)?;
    // A call with credentials logs in first, as Java does with authUsername and authPassword, and the caller's user
    // is back when the call returns, as Java's logoutLocal in finally.
    let _login = match auth_credentials(parameters) {
        Some((username, password)) => match crate::user::login_user_local(&username, &password) {
            Ok(login) => Some(login),
            Err(err) => {
                add_error(err.message());
                return Ok(BTreeMap::new());
            }
        },
        None => None,
    };
    // Internal work with authorization disabled, a data load for one, needs no logged-in user.
    let authz_off = crate::context::ExecutionContext::with_current(|ctx| ctx.authz_disabled()).unwrap_or(true);
    if def.authenticate == Authenticate::Required
        && !authz_off
        && crate::user::current_username().ok().flatten().is_none()
        && !crate::user::is_anonymous().unwrap_or(false)
    {
        return Err(Error::of_kind(
            crate::context::ErrorKind::AuthenticationRequired,
            format!("User must be logged in to call service {}", def.name),
        ));
    }
    // The service goes on the artifact stack, so the entity operations it makes inherit its
    // authorization. Only a service that requires authentication needs an allow record.
    let _frame = ServiceFrame::push(&def, &cleaned)?;
    let facade = facade.ok_or_else(|| Error::new("no entity facade is installed"))?;
    let guard = crate::service::semaphore::take(&def, &cleaned, &facade)?;
    let entity_name = if def.kind == ServiceKind::EntityAuto && !def.noun.is_empty() {
        def.noun.clone()
    } else {
        ServiceDef::semaphore_entity().to_string()
    };
    let tx = facade.transactions_for(&entity_name)?;
    let attribute = options.attribute(def.transaction);
    // Errors the service leaves in the message facade roll the transaction back but are not a
    // failure of the call: the results come back with the errors alongside, as in Java.
    let mut message_failed: Option<BTreeMap<String, Value>> = None;
    let outcome = tx.run(attribute, options.timeout.or(def.timeout), || {
        let _deadline = crate::script::DeadlineGuard::new(tx.time_remaining());
        if def.no_tx_cache {
            crate::entity::cache::flush_and_disable(&facade, &tx)?;
        }
        // on_rollback is not a commit hook. tx-commit is last so a post-service
        // entity write can still register the feed schedule hook.
        register_tx_rollback(&tx, &def.name, &cleaned)?;
        super::seca::run_seca_rules(&def.name, "pre-service", &cleaned, None).map_err(script_err)?;
        if has_error() {
            tracing::warn!(service = %def.name, "errors before the service ran, so it was not run");
            return Ok(BTreeMap::new());
        }
        let produced = execute(&def, &cleaned, &facade)?;
        super::seca::run_seca_rules(&def.name, "post-service", &cleaned, Some(&produced)).map_err(script_err)?;
        if has_error() {
            message_failed = Some(produced.clone());
            // Under `ignore` with nothing in place there is no transaction to mark.
            if tx.in_transaction() {
                tx.set_rollback_only(format!("Error running service {} (message)", def.name))?;
            }
        } else {
            register_tx_commit(&tx, &def.name, &cleaned, &produced)?;
        }
        Ok(produced)
    });
    let produced = match outcome {
        Ok(produced) => Ok(produced),
        Err(err) => match message_failed.take() {
            Some(produced) => Ok(produced),
            None => {
                // A failure is also an error message, so rules that skip on error skip.
                add_error(&err.to_string());
                Err(err)
            }
        },
    };
    // post-commit rules run whether or not the service worked. Those that skip on error do.
    let post = super::seca::run_seca_rules(&def.name, "post-commit", &cleaned, produced.as_ref().ok());
    drop(guard);
    let produced = produced?;
    post?;
    Ok(project(&def, produced))
}

/// The artifact frame for a running service. It pops when dropped, and ends an anonymous login the
/// call started.
struct ServiceFrame {
    pushed: bool,
    anonymous: bool,
}

impl ServiceFrame {
    fn push(def: &ServiceDef, parameters: &BTreeMap<String, Value>) -> Result<Self, Error> {
        if !crate::user::context_entered() {
            return Ok(Self {
                pushed: false,
                anonymous: false,
            });
        }
        let requires_authz = def.authenticate == Authenticate::Required;
        let outcome = crate::context::ExecutionContext::with_current(|ctx| {
            ctx.push_checked(
                "AT_SERVICE",
                &def.name,
                auth_action(def),
                requires_authz,
                true,
                Some(parameters),
            )?;
            let remembered = (!def.no_remember_parameters).then_some(parameters);
            ctx.set_hit_detail(Some(&def.service_type), remembered);
            Ok(())
        });
        if let Some(Err(err)) = outcome {
            return Err(err);
        }
        let mut frame = Self {
            pushed: outcome.is_some(),
            anonymous: false,
        };
        // After the push, so the frame is the one marked.
        let all = match def.authenticate {
            Authenticate::AnonymousAll => Some(true),
            Authenticate::AnonymousView => Some(false),
            _ => None,
        };
        if let Some(all) = all {
            let user_id = crate::user::current_user_id().ok().flatten().unwrap_or_default();
            crate::context::ExecutionContext::with_current(|ctx| ctx.set_anonymous_authorized(all, &user_id));
            frame.anonymous = crate::user::login_anonymous().unwrap_or(false);
        }
        Ok(frame)
    }
}

impl Drop for ServiceFrame {
    fn drop(&mut self) {
        if self.anonymous {
            crate::user::logout_anonymous_only();
        }
        if self.pushed {
            crate::context::ExecutionContext::with_current(|ctx| {
                let _ = ctx.pop();
            });
        }
    }
}

/// `authUsername` and `authPassword`, or the same inside `authUserAccount`.
fn auth_credentials(parameters: &BTreeMap<String, Value>) -> Option<(String, String)> {
    let text = |value: Option<&Value>| value.map(moqui_lib::object::to_plain_string).unwrap_or_default();
    let (mut username, mut password) = (String::new(), String::new());
    if parameters.contains_key("authUsername") {
        username = text(parameters.get("authUsername"));
        password = text(parameters.get("authPassword"));
    } else if let Some(Value::Map(account)) = parameters.get("authUserAccount") {
        username = text(account.get("username"));
        if username.is_empty() {
            username = text(parameters.get("authUsername"));
        }
        password = text(account.get("currentPassword"));
        if password.is_empty() {
            password = text(parameters.get("authPassword"));
        }
    }
    (!username.is_empty() && !password.is_empty()).then_some((username, password))
}

fn add_error(message: &str) {
    let message = message.to_string();
    crate::context::ExecutionContext::with_current(|ctx| ctx.message().add_error(message));
}

fn register_tx_rollback(
    tx: &crate::transaction::TransactionManager,
    service: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<(), LibError> {
    if !tx.in_transaction() {
        return Ok(());
    }
    let service = service.to_string();
    let parameters = parameters.clone();
    tx.on_rollback(move || {
        if let Err(err) = super::seca::run_seca_rules(&service, "tx-rollback", &parameters, None) {
            tracing::warn!(error = %err, "tx-rollback rules failed");
        }
    })
}

fn register_tx_commit(
    tx: &crate::transaction::TransactionManager,
    service: &str,
    parameters: &BTreeMap<String, Value>,
    results: &BTreeMap<String, Value>,
) -> Result<(), LibError> {
    if !tx.in_transaction() {
        return Ok(());
    }
    let service = service.to_string();
    let parameters = parameters.clone();
    let results = results.clone();
    tx.on_commit(move || {
        if let Err(err) = super::seca::run_seca_rules(&service, "tx-commit", &parameters, Some(&results)) {
            tracing::warn!(error = %err, "tx-commit rules failed");
        }
    })
}

pub fn call(name: &str) -> Result<(), Error> {
    sync(name, &BTreeMap::new()).map(|_| ())
}

fn canonical(name: &str) -> String {
    if name.contains('#') {
        return name.to_string();
    }
    for verb in ["create", "update", "store", "delete"] {
        let prefix = format!("{verb}-");
        if let Some(rest) = name.strip_prefix(&prefix) {
            return format!("{verb}#{rest}");
        }
    }
    name.to_string()
}

fn resolve(name: &str, facade: Option<&EntityFacade>) -> Result<Arc<ServiceDef>, Error> {
    if let Some(def) = super::define::find(name) {
        return Ok(def);
    }
    let (verb, noun) = split_name(name);
    if matches!(verb.as_str(), "create" | "update" | "store" | "delete") && !noun.is_empty() {
        if let Some(facade) = facade {
            if facade.with_entity(&noun, |_, _, _| Ok(())).is_ok() {
                return Ok(Arc::new(ServiceDef::implicit(name, &verb, &noun)));
            }
        }
    }
    Err(Error::new(format!("no service named {name}")))
}

fn split_name(name: &str) -> (String, String) {
    match name.split_once('#') {
        Some((verb, noun)) => (verb.to_string(), noun.to_string()),
        None => (name.to_string(), String::new()),
    }
}

/// The authorization action of the service `name`: its `authz-action`, else by its verb. A name with no definition goes
/// by the verb in the name, as Java's `TransitionItem.resolveAuthzAction`.
pub(crate) fn service_authz_action(name: &str) -> String {
    let name = canonical(name);
    if let Some(def) = super::define::find(&name) {
        return auth_action(&def).to_string();
    }
    let (verb, _) = split_name(&name);
    let verb = verb.rsplit('.').next().unwrap_or(&verb);
    verb_action(verb).to_string()
}

/// The in-parameter names of a defined service, in definition order, with `auto-parameters` expanded. None when no
/// service has that name.
pub(crate) fn service_in_parameter_names(name: &str) -> Option<Vec<String>> {
    let form = service_form_info(name)?;
    Some(form.in_params.into_iter().map(|parameter| parameter.name).collect())
}

/// A defined service as a form builds fields from it (`auto-fields-service`, `auto-widget-service`): its noun, and its
/// parameters with `auto-parameters` expanded. None when no service has that name.
pub(crate) struct FormService {
    pub name: String,
    pub noun: String,
    pub in_params: Vec<super::define::ParamDef>,
    pub out_params: Vec<super::define::ParamDef>,
}

pub(crate) fn service_form_info(name: &str) -> Option<FormService> {
    let def = super::define::find(&canonical(name))?;
    let facade = crate::context::ExecutionContext::with_current(|ec| ec.factory().entity()).flatten();
    let unexpanded = |err: Error, params: &[super::define::ParamDef]| {
        tracing::warn!(service = %def.name, error = %err, "auto-parameters not expanded for a form");
        params.iter().filter(|param| !param.name.is_empty()).cloned().collect()
    };
    let in_params = super::validate::declared_in(&def, facade.as_ref())
        .map_or_else(|err| unexpanded(err, &def.in_params), |params| params.to_vec());
    let out_params = super::validate::expand_params(&def, &def.out_params, facade.as_ref())
        .unwrap_or_else(|err| unexpanded(err, &def.out_params));
    Some(FormService {
        in_params,
        out_params,
        name: def.name.clone(),
        noun: def.noun.clone(),
    })
}

/// The entity of an implicit entity-auto service name such as `create#moqui.basic.Enumeration`, when it has no
/// definition of its own.
pub(crate) fn entity_auto_noun(name: &str) -> Option<String> {
    let name = canonical(name);
    if super::define::find(&name).is_some() {
        return None;
    }
    let (verb, noun) = split_name(&name);
    (matches!(verb.as_str(), "create" | "update" | "store" | "delete") && !noun.is_empty()).then_some(noun)
}

fn auth_action(def: &ServiceDef) -> &str {
    def.authz_action.as_deref().unwrap_or_else(|| verb_action(&def.verb))
}

fn verb_action(verb: &str) -> &'static str {
    match verb {
        "create" => "create",
        "update" | "store" => "update",
        "delete" => "delete",
        "view" | "find" => "view",
        _ => "all",
    }
}

fn execute(
    def: &ServiceDef,
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    match def.kind {
        ServiceKind::Rust => match def.rust_fn {
            Some(run) => run(parameters, facade),
            None => Err(LibError::new(format!("service {} has no rust function", def.name))),
        },
        ServiceKind::Native => {
            let run = def
                .native_fn
                .as_ref()
                .ok_or_else(|| LibError::new(format!("service {} has no native implementation", def.name)))?;
            ExecutionContext::with_current(|ec| {
                run.call(
                    ec,
                    &crate::native::ServiceInfo {
                        name: &def.name,
                        service_type: &def.service_type,
                        location: &def.location,
                        method: &def.method,
                    },
                    parameters,
                )
            })
            .ok_or_else(|| LibError::new("no execution context is entered"))?
        }
        ServiceKind::Interface => Err(LibError::new(format!("interface {} has no runner", def.name))),
        ServiceKind::Unsupported => Err(LibError::new(format!(
            "service {} runs {}, a script language this port does not run",
            def.name, def.location
        ))),
        ServiceKind::Script => run_script(def, parameters),
        ServiceKind::RemoteRest => super::remote::run_remote_rest(
            &def.name,
            &def.location,
            None,
            &def.verb,
            parameters,
            facade.sim_active(),
        )
        .map_err(|err| LibError::new(err.message())),
        ServiceKind::EntityAuto => run_entity(facade, &def.verb, &def.noun, parameters, 0),
        ServiceKind::Inline => run_inline(def, parameters),
    }
}

fn run_inline(def: &ServiceDef, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError> {
    let _frame = crate::script::IsolatedContext::push();
    for (name, value) in parameters {
        crate::script::set_context_name(name, value.clone()).map_err(script_err)?;
    }
    if !def.actions.trim().is_empty() {
        crate::script::run_actions(&def.actions).map_err(|err| in_service(def, &err))?;
    }
    Ok(collect_outputs(def))
}

/// The declared out-parameters, read from the script frame.
fn collect_outputs(def: &ServiceDef) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for param in super::validate::expanded_out(def).iter() {
        if param.name.is_empty() || param.required == super::define::Required::Disabled {
            continue;
        }
        if let Some(value) = crate::script::context_value(&param.name) {
            out.insert(param.name.clone(), value);
        }
    }
    out
}

/// A script service whose body is a JavaScript file. It reads the parameters as bare names and
/// leaves its results the same way, as inline actions do.
fn run_script(def: &ServiceDef, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError> {
    let _frame = crate::script::IsolatedContext::push();
    for (name, value) in parameters {
        crate::script::set_context_name(name, value.clone()).map_err(script_err)?;
    }
    crate::script::run_script_location(&def.location).map_err(|err| in_service(def, &err))?;
    Ok(collect_outputs(def))
}

/// A failure in a service's body, with the service named after it (Java logs "Error running service ..."): a script
/// error alone gives only a line in the generated code.
fn in_service(def: &ServiceDef, err: &crate::context::Error) -> LibError {
    let message = err.message();
    if message.contains("\n    in service ") {
        return LibError::new(message);
    }
    LibError::new(format!("{}\n    in service {}", message.trim_end(), def.name))
}

fn script_err(err: crate::context::Error) -> LibError {
    LibError::new(err.message())
}

fn project(def: &ServiceDef, values: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let out_params = super::validate::expanded_out(def);
    let names: Vec<&str> = out_params
        .iter()
        .filter(|param| !param.name.is_empty() && param.required != super::define::Required::Disabled)
        .map(|param| param.name.as_str())
        .collect();
    if names.is_empty() {
        return values;
    }
    values
        .into_iter()
        .filter(|(name, _)| names.contains(&name.as_str()))
        .collect()
}

fn run_entity(
    facade: &EntityFacade,
    verb: &str,
    noun: &str,
    parameters: &BTreeMap<String, Value>,
    depth: u32,
) -> Result<BTreeMap<String, Value>, LibError> {
    if depth > 8 {
        return Err(LibError::new("nested entity-auto is too deep"));
    }
    let pk_names = facade.with_entity(noun, |entity, _, _| {
        Ok(entity
            .pk_field_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>())
    })?;
    let mut record = facade.make_value(noun)?;
    for (key, value) in parameters {
        if reserved(key) || matches!(value, Value::Map(_) | Value::List(_)) {
            continue;
        }
        // Java's `setFields(parameters, true, ...)` sets an empty string as null: an empty drop-down is no key.
        let value = match value {
            Value::String(text) if text.is_empty() => Value::Null,
            other => other.clone(),
        };
        record.set(key, value);
    }
    // `EntityAutoServiceRunner.checkFromDate`: a `fromDate` primary key left out of a create or store is
    // `ec.user.nowTimestamp`.
    if matches!(verb, "create" | "store")
        && pk_names.iter().any(|name| name == "fromDate")
        && record
            .get("fromDate")
            .is_none_or(|value| value.is_null() || is_empty(value))
    {
        record.set("fromDate", crate::user::now_effective_value());
    }
    // a primary key field with a default (a content date, say) takes it before a missing key is sequenced
    if matches!(verb, "create" | "store") && pk_missing(&record, &pk_names) {
        let with_defaults = facade.apply_field_defaults(noun, record.fields())?;
        for name in &pk_names {
            let missing = record.get(name).is_none_or(|value| value.is_null() || is_empty(value));
            if let Some(value) = with_defaults.get(name).filter(|value| missing && !value.is_null()) {
                record.set(name, value.clone());
            }
        }
    }
    let mut status_out = BTreeMap::new();
    match verb {
        "delete" => record.delete(facade)?,
        "update" => {
            check_status(facade, noun, &record, &pk_names, parameters, &mut status_out)?;
            record.update(facade)?
        }
        "create" => {
            assign_sequence(facade, &mut record, &pk_names)?;
            record.create(facade)?;
        }
        "store" => {
            if pk_missing(&record, &pk_names) {
                assign_sequence(facade, &mut record, &pk_names)?;
                record.create(facade)?;
            } else if parameters.contains_key("statusId")
                && facade.with_entity(noun, |entity, _, _| Ok(entity.has_field("statusId")))?
            {
                // Java's store checks status with one uncached read. A missing row is created
                // directly; other stores choose create/update through EntityValue.refresh.
                if check_status(facade, noun, &record, &pk_names, parameters, &mut status_out)? {
                    record.create_or_update(facade)?;
                } else {
                    record.create(facade)?;
                }
            } else {
                record.create_or_update(facade)?;
            }
        }
        other => return Err(LibError::new(format!("entity-auto verb {other} is not supported"))),
    }
    // Java deletes only the named record. Deleting children after the parent would break their
    // foreign keys, and before it would delete rows the caller never named.
    if verb != "delete" {
        nest(facade, verb, noun, parameters, &record, depth)?;
    }
    let mut out = status_out;
    for pk in &pk_names {
        if let Some(value) = record.get(pk) {
            out.insert(pk.clone(), value.clone());
        }
    }
    Ok(out)
}

/// `EntityAutoServiceRunner.storeEntity` with no service around it: create or update the record
/// and the records nested under it. Returns the primary key values. The ETL loader calls this.
pub(crate) fn store_entity(
    facade: &EntityFacade,
    entity_name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, LibError> {
    run_entity(facade, "store", entity_name, parameters, 0)
}

fn assign_sequence(facade: &EntityFacade, record: &mut EntityValue, pk_names: &[String]) -> Result<(), LibError> {
    if pk_names.is_empty() || !pk_missing(record, pk_names) {
        return Ok(());
    }
    if pk_names.len() == 1 {
        record.set_sequenced_id_primary(facade)?;
        return Ok(());
    }
    if let Some(last) = pk_names.last() {
        if record.get(last).is_none_or(|value| value.is_null() || is_empty(value)) {
            record.set_sequenced_id_secondary(facade)?;
        }
    }
    Ok(())
}

fn pk_missing(record: &EntityValue, pk_names: &[String]) -> bool {
    pk_names
        .iter()
        .any(|name| record.get(name).is_none_or(|value| value.is_null() || is_empty(value)))
}

/// `EntityAutoServiceRunner.checkStatus`: when an update passes `statusId` for an entity with that field, the results
/// get `oldStatusId` and `statusChanged`, and a changed status needs a `StatusFlowTransition` from the old to the new.
fn check_status(
    facade: &EntityFacade,
    noun: &str,
    record: &EntityValue,
    pk_names: &[String],
    parameters: &BTreeMap<String, Value>,
    out: &mut BTreeMap<String, Value>,
) -> Result<bool, LibError> {
    let Some(new_status) = parameters.get("statusId") else {
        return Ok(false);
    };
    if !facade.with_entity(noun, |entity, _, _| Ok(entity.has_field("statusId")))? {
        return Ok(false);
    }
    let mut find = facade.find(noun)?.use_cache(false);
    for name in pk_names {
        find = find.eq(name, record.get(name).cloned().unwrap_or(Value::Null));
    }
    let Some(existing) = find.one()? else {
        return Ok(false);
    };
    let old_status = existing.get("statusId").cloned().unwrap_or(Value::Null);
    out.insert("oldStatusId".to_string(), old_status.clone());
    let same = moqui_lib::object::to_plain_string(&old_status) == moqui_lib::object::to_plain_string(new_status);
    out.insert("statusChanged".to_string(), Value::Bool(!same));
    let (old_id, new_id) = (
        moqui_lib::object::to_plain_string(&old_status),
        moqui_lib::object::to_plain_string(new_status),
    );
    if same || old_id.is_empty() || new_id.is_empty() || old_status.is_null() || new_status.is_null() {
        return Ok(true);
    }
    let transitions = facade
        .find("moqui.basic.StatusFlowTransition")?
        .eq("statusId", Value::string(old_id.clone()))
        .eq("toStatusId", Value::string(new_id.clone()))
        .list()?;
    let mut valid = 0;
    for transition in transitions.values() {
        let permission = transition
            .get("userPermissionId")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        if permission.is_empty() || crate::user::has_permission(&permission)? {
            valid += 1;
        }
    }
    if valid == 0 {
        let name_of = |id: &str| -> Result<String, LibError> {
            let item = facade
                .find("moqui.basic.StatusItem")?
                .eq("statusId", Value::string(id.to_string()))
                .one()?;
            Ok(item
                .and_then(|item| item.get("description").map(moqui_lib::object::to_plain_string))
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| id.to_string()))
        };
        return Err(LibError::new(format!(
            "Status change not allowed from {} to {}",
            name_of(&old_id)?,
            name_of(&new_id)?
        )));
    }
    Ok(true)
}

fn nest(
    facade: &EntityFacade,
    verb: &str,
    noun: &str,
    parameters: &BTreeMap<String, Value>,
    record: &EntityValue,
    depth: u32,
) -> Result<(), LibError> {
    let child_verb = if verb == "delete" { "delete" } else { "store" };
    for (key, value) in parameters {
        if reserved(key) {
            continue;
        }
        let children = nested_maps(value);
        if children.is_empty() {
            continue;
        }
        let Some(link) = link_for(facade, noun, key)? else {
            continue;
        };
        for mut child in children {
            fill_link(&mut child, parameters, record, &link);
            run_entity(facade, child_verb, &link.entity, &child, depth + 1)?;
        }
    }
    Ok(())
}

fn nested_maps(value: &Value) -> Vec<BTreeMap<String, Value>> {
    match value {
        Value::Map(entries) => vec![copy_map(entries)],
        Value::List(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(entries) => Some(copy_map(entries)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn copy_map<'a, I, K>(entries: I) -> BTreeMap<String, Value>
where
    I: IntoIterator<Item = (&'a K, &'a Value)>,
    K: AsRef<str> + 'a,
{
    entries
        .into_iter()
        .map(|(key, value)| (key.as_ref().to_string(), value.clone()))
        .collect()
}

/// Target entity and the `(field, value)` conditions that select `record`'s related rows.
pub(crate) fn related_conditions(
    facade: &EntityFacade,
    parent: &str,
    relationship: &str,
    record: &EntityValue,
) -> Result<(String, Vec<(String, Value)>), LibError> {
    let (entity, pairs, constants) = facade.with_entity(parent, |entity, _, model| {
        let Some(found) = entity.relationship(relationship) else {
            return Err(LibError::new(format!("{parent} has no relationship {relationship}")));
        };
        let target = model.entity(&found.related_entity_name)?;
        let fields = snap_fields(target);
        let pairs = expand_key_map(&found.relationship_type, &found.key_map, &fields);
        let constants: Vec<(String, String)> = found
            .key_values
            .iter()
            .map(|entry| (entry.related.clone(), entry.value.clone()))
            .collect();
        Ok((target.full_name.clone(), pairs, constants))
    })?;
    let mut conditions = Vec::new();
    for (source, target) in pairs {
        conditions.push((target, record.get(&source).cloned().unwrap_or(Value::Null)));
    }
    for (field, value) in constants {
        conditions.push((field, Value::string(&value)));
    }
    Ok((entity, conditions))
}

struct Link {
    entity: String,
    pairs: Vec<(String, String)>,
    constants: Vec<(String, String)>,
    parent_pks: Vec<String>,
    child_fields: BTreeSet<String>,
}

struct FieldSnap {
    name: String,
    is_pk: bool,
}

fn link_for(facade: &EntityFacade, parent: &str, key: &str) -> Result<Option<Link>, LibError> {
    facade.with_entity(parent, |entity, _, model| {
        let parent_pks = entity
            .pk_field_names()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if let Some(relationship) = entity.relationship(key) {
            let key_is_entity = model.entity(key).is_ok();
            if relationship.mutable || key_is_entity {
                let target = model.entity(&relationship.related_entity_name)?;
                let fields = snap_fields(target);
                let pairs = expand_key_map(&relationship.relationship_type, &relationship.key_map, &fields);
                let constants = relationship
                    .key_values
                    .iter()
                    .map(|entry| (entry.related.clone(), entry.value.clone()))
                    .collect();
                return Ok(Some(owned_link(
                    target.full_name.clone(),
                    pairs,
                    constants,
                    parent_pks,
                    &fields,
                )));
            }
        }
        if let Ok(target) = model.entity(key) {
            let fields = snap_fields(target);
            return Ok(Some(owned_link(
                target.full_name.clone(),
                Vec::new(),
                Vec::new(),
                parent_pks,
                &fields,
            )));
        }
        Ok(None)
    })
}

fn owned_link(
    entity: String,
    pairs: Vec<(String, String)>,
    constants: Vec<(String, String)>,
    parent_pks: Vec<String>,
    fields: &[FieldSnap],
) -> Link {
    Link {
        entity,
        pairs,
        constants,
        parent_pks,
        child_fields: fields.iter().map(|field| field.name.clone()).collect(),
    }
}

fn snap_fields(entity: &EntityDefinition) -> Vec<FieldSnap> {
    entity
        .fields
        .iter()
        .map(|field| FieldSnap {
            name: field.name.clone(),
            is_pk: field.is_pk,
        })
        .collect()
}

fn expand_key_map(relationship_type: &str, key_map: &[KeyMap], fields: &[FieldSnap]) -> Vec<(String, String)> {
    let pks: Vec<String> = fields
        .iter()
        .filter(|field| field.is_pk)
        .map(|field| field.name.clone())
        .collect();
    let one = relationship_type.starts_with("one");
    if key_map.is_empty() && one {
        return pks.into_iter().map(|pk| (pk.clone(), pk)).collect();
    }
    if key_map.len() == 1 {
        let map = &key_map[0];
        let related = match map.related.as_deref().filter(|value| !value.is_empty()) {
            Some(related) => related.to_string(),
            None if one && pks.len() == 1 => pks[0].clone(),
            None => map.field_name.clone(),
        };
        return vec![(map.field_name.clone(), related)];
    }
    let names: BTreeSet<&str> = fields.iter().map(|field| field.name.as_str()).collect();
    key_map
        .iter()
        .map(|map| {
            let mut related = map
                .related
                .clone()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| map.field_name.clone());
            if one && pks.len() == 1 && !names.contains(related.as_str()) {
                related = pks[0].clone();
            }
            (map.field_name.clone(), related)
        })
        .collect()
}

fn fill_link(child: &mut BTreeMap<String, Value>, parent: &BTreeMap<String, Value>, record: &EntityValue, link: &Link) {
    for (source, target) in &link.pairs {
        if !blank(child.get(target)) {
            continue;
        }
        let value = parent.get(source).cloned().or_else(|| record.get(source).cloned());
        if let Some(value) = value {
            if !is_empty(&value) {
                child.insert(target.clone(), value);
            }
        }
    }
    for (field, value) in &link.constants {
        if blank(child.get(field)) {
            child.insert(field.clone(), Value::string(value));
        }
    }
    for pk in &link.parent_pks {
        if !link.child_fields.contains(pk) || !blank(child.get(pk)) {
            continue;
        }
        if let Some(value) = record.get(pk).filter(|value| !is_empty(value)).cloned() {
            child.insert(pk.clone(), value);
        }
    }
}

fn blank(value: Option<&Value>) -> bool {
    value.is_none_or(is_empty)
}

fn reserved(name: &str) -> bool {
    matches!(
        name,
        "ec" | "_entity" | "authUsername" | "authPassword" | "authUserAccount"
    )
}

fn current_facade() -> Option<EntityFacade> {
    crate::context::ExecutionContext::with_current(|ctx| ctx.factory().entity()).flatten()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use moqui_lib::object::{basic_convert, Value};

    use super::sync;
    use crate::context::{ExecutionContext, ExecutionContextFactory};

    #[test]
    fn entity_auto_store_inserts_then_updates() {
        let tape = Arc::new(Mutex::new(Vec::new()));
        let facade = crate::testing::open_with_tape(Arc::clone(&tape));
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let mut params = BTreeMap::new();
        params.insert("testId".into(), Value::string("T1"));
        params.insert("testMedium".into(), Value::string("first"));
        let created = sync("store-moqui.test.TestEntity", &params).unwrap();
        assert_eq!(created.get("testId").and_then(Value::as_str), Some("T1"));
        params.insert("testMedium".into(), Value::string("second"));
        sync("store#moqui.test.TestEntity", &params).unwrap();
        let row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("T1"))
            .one()
            .unwrap()
            .expect("stored row");
        assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("second"));
        let sql = tape.lock().unwrap_or_else(|err| err.into_inner()).join("\n");
        assert!(sql.contains("INSERT") && sql.contains("test_entity"), "{sql}");
        assert!(sql.contains("UPDATE") && sql.contains("test_entity"), "{sql}");
        assert!(!sql.contains("ON CONFLICT"), "{sql}");
    }

    #[test]
    fn entity_auto_sets_empty_string_as_null() {
        let facade = crate::testing::open_with_tape(Arc::new(Mutex::new(Vec::new())));
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let mut params = BTreeMap::new();
        params.insert("testId".into(), Value::string("T2"));
        params.insert("testMedium".into(), Value::string(""));
        sync("create#moqui.test.TestEntity", &params).unwrap();
        let row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("T2"))
            .one()
            .unwrap()
            .expect("created row");
        assert!(matches!(row.get("testMedium"), None | Some(Value::Null)), "{row:?}");
    }

    #[test]
    fn missing_required_parameter_is_named() {
        let path = write_fixture(
            "required",
            r#"<services>
                <service verb="need" noun="Value">
                    <in-parameters><parameter name="amount" required="true"/></in-parameters>
                </service>
            </services>"#,
        );
        let ctx = ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        // As Java: the service does not run, the call returns nothing, and the error is a validation error on the
        // field, which a form shows beside it.
        let out = sync("need#Value", &BTreeMap::new()).unwrap();
        assert!(out.is_empty(), "{out:?}");
        let (errors, validation) = ExecutionContext::with_current(|ec| {
            let messages = ec.message();
            (messages.errors().to_vec(), messages.validation_errors().to_vec())
        })
        .unwrap();
        // Java's message, which does not name the field; the validation error does.
        assert!(errors.iter().any(|text| text == "Field cannot be empty"), "{errors:?}");
        assert_eq!(validation.len(), 1);
        assert_eq!(validation[0].message, "Field cannot be empty");
        assert_eq!(
            (validation[0].field.as_str(), validation[0].service.as_str()),
            ("amount", "need#Value")
        );
    }

    #[test]
    fn held_fail_semaphore_errors() {
        let path = write_fixture(
            "semaphore",
            r#"<services>
                <service verb="lock" noun="Row" semaphore="fail">
                    <in-parameters><parameter name="note"/></in-parameters>
                </service>
            </services>"#,
        );
        let tape = Arc::new(Mutex::new(Vec::new()));
        let facade = crate::testing::open_with_tape(tape);
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        let mut row = facade
            .make_value("moqui.service.semaphore.ServiceParameterSemaphore")
            .unwrap();
        row.set("serviceName", Value::string("lock#Row"));
        row.set("parameterValue", Value::string("_NA_"));
        row.set("lockThread", Value::string("other"));
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        row.set("lockTime", basic_convert(&Value::I64(millis), "Timestamp").value);
        row.create(&facade).unwrap();
        let err = sync("lock#Row", &BTreeMap::new()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("semaphore"), "{text}");
        assert!(!text.contains("actions not run"), "{text}");
    }

    #[test]
    fn a_stale_semaphore_is_taken_over_and_its_old_owner_cannot_free_the_new_hold() {
        let path = write_fixture(
            "semaphore-takeover",
            r#"<services><service verb="guard" noun="Row" semaphore="fail"/></services>"#,
        );
        let (facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        let mut quick =
            crate::service::define::ServiceDef::clone(&crate::service::define::find("guard#Row").expect("registered"));
        quick.semaphore_ignore = std::time::Duration::from_millis(1);
        let mut patient = quick.clone();
        patient.semaphore_ignore = std::time::Duration::from_secs(3600);
        let none = BTreeMap::new();

        let first = crate::service::semaphore::take(&quick, &none, &facade)
            .unwrap()
            .expect("first hold");
        assert!(
            crate::service::semaphore::take(&patient, &none, &facade).is_err(),
            "held"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
        let second = crate::service::semaphore::take(&quick, &none, &facade)
            .unwrap()
            .expect("takes over the stale hold");
        // The first holder finishing late must not free the second holder's lock.
        drop(first);
        assert!(
            crate::service::semaphore::take(&patient, &none, &facade).is_err(),
            "still held by the second"
        );
        drop(second);
        crate::service::semaphore::take(&patient, &none, &facade)
            .unwrap()
            .expect("free once the owner is done");
    }

    #[test]
    fn inline_actions_find_and_create_rows() {
        let path = write_fixture(
            "inline",
            r#"<services>
                <service verb="copy" noun="Row">
                    <in-parameters><parameter name="seedId"/></in-parameters>
                    <out-parameters><parameter name="foundId"/></out-parameters>
                    <actions>
                        <entity-find entity-name="moqui.test.TestEntity" list="found">
                            <econdition field-name="testId" from="seedId"/>
                        </entity-find>
                        <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                        <script>made.set("testId", "B"); made.set("testMedium", "made"); foundId = found.get(0).get("testId");</script>
                        <entity-create value-field="made"/>
                    </actions>
                </service>
            </services>"#,
        );
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let _authz = ctx.disable_authz();
        let mut seed = facade.make_value("moqui.test.TestEntity").unwrap();
        seed.set("testId", Value::string("A"));
        seed.set("testMedium", Value::string("seed"));
        seed.create(&facade).unwrap();
        let mut params = BTreeMap::new();
        params.insert("seedId".into(), Value::string("A"));
        let out = sync("copy#Row", &params).unwrap();
        assert_eq!(out.get("foundId").and_then(Value::as_str), Some("A"));
        let seed_row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("A"))
            .one()
            .unwrap()
            .expect("seed row");
        let made_row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("B"))
            .one()
            .unwrap()
            .expect("created row");
        assert_eq!(seed_row.get("testMedium").and_then(Value::as_str), Some("seed"));
        assert_eq!(made_row.get("testMedium").and_then(Value::as_str), Some("made"));
    }

    fn seeded_context(tape_rows: &[(&str, &str)]) -> (crate::entity::EntityFacade, crate::context::ExecutionContext) {
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(facade.clone());
        {
            let _entered = ctx.enter();
            let _authz = ctx.disable_authz();
            for (id, medium) in tape_rows {
                let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
                row.set("testId", Value::string(*id));
                row.set("testMedium", Value::string(*medium));
                row.create(&facade).unwrap();
            }
        }
        (facade, ctx)
    }

    #[test]
    fn action_outputs_reach_out_parameters_without_script_assignment() {
        let path = write_fixture(
            "outputs",
            r#"<services>
                <service verb="probe" noun="Row">
                    <in-parameters><parameter name="testId"/></in-parameters>
                    <out-parameters>
                        <parameter name="foundCount" type="Integer"/><parameter name="oneMedium"/>
                        <parameter name="lastId"/><parameter name="cmp"/>
                    </out-parameters>
                    <actions>
                        <entity-find entity-name="moqui.test.TestEntity" list="found">
                            <econdition field-name="testId" from="testId"/>
                        </entity-find>
                        <entity-find-one entity-name="moqui.test.TestEntity" value-field="oneRow"/>
                        <set field="foundCount" from="found.length"/>
                        <set field="oneMedium" from="oneRow.get('testMedium')"/>
                        <iterate list="found" entry="row"><set field="lastId" from="row.get('testId')"/></iterate>
                        <if><condition><compare field="foundCount" operator="greater" value="0" type="Integer"/></condition>
                            <then><set field="cmp" value="yes"/></then></if>
                    </actions>
                </service>
            </services>"#,
        );
        let (_facade, ctx) = seeded_context(&[("A", "alpha")]);
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let _authz = ctx.disable_authz();
        let mut params = BTreeMap::new();
        params.insert("testId".into(), Value::string("A"));
        let out = sync("probe#Row", &params).unwrap();
        assert_eq!(
            out.get("foundCount").map(moqui_lib::object::to_plain_string).as_deref(),
            Some("1"),
            "{out:?}"
        );
        assert_eq!(out.get("oneMedium").and_then(Value::as_str), Some("alpha"));
        assert_eq!(out.get("lastId").and_then(Value::as_str), Some("A"));
        assert_eq!(out.get("cmp").and_then(Value::as_str), Some("yes"));
        // The callee's names do not leak into the caller's frame.
        assert!(crate::script::context_value("foundCount").is_none());
    }

    #[test]
    fn service_call_action_passes_parameters_and_merges_out_map() {
        let path = write_fixture(
            "nested-call",
            r#"<services>
                <service verb="double" noun="Num">
                    <in-parameters><parameter name="x" type="Integer"/></in-parameters>
                    <out-parameters><parameter name="y" type="Integer"/></out-parameters>
                    <actions><set field="y" from="x + x"/></actions>
                </service>
                <service verb="outer" noun="Num">
                    <in-parameters><parameter name="n" type="Integer"/></in-parameters>
                    <out-parameters><parameter name="answer" type="Integer"/><parameter name="after"/></out-parameters>
                    <actions>
                        <service-call name="double#Num" out-map="res"><field-map field-name="x" from="n"/></service-call>
                        <set field="answer" from="res.y"/>
                        <service-call name="nothing#Here" ignore-error="true"/>
                        <set field="after" value="ran"/>
                    </actions>
                </service>
                <service verb="onto" noun="Context">
                    <in-parameters><parameter name="n" type="Integer"/></in-parameters>
                    <out-parameters><parameter name="answer" type="Integer"/></out-parameters>
                    <actions>
                        <service-call name="double#Num" out-map="context"><field-map field-name="x" from="n"/></service-call>
                        <set field="answer" from="y"/>
                    </actions>
                </service>
                <service verb="stops" noun="Num">
                    <out-parameters><parameter name="after"/></out-parameters>
                    <actions>
                        <service-call name="nothing#Here"/>
                        <set field="after" value="ran"/>
                    </actions>
                </service>
            </services>"#,
        );
        let (_facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let _authz = ctx.disable_authz();
        let mut params = BTreeMap::new();
        params.insert("n".into(), Value::I32(21));
        let out = sync("outer#Num", &params).unwrap();
        assert_eq!(
            out.get("answer").map(moqui_lib::object::to_plain_string).as_deref(),
            Some("42"),
            "{out:?}"
        );
        assert_eq!(out.get("after").and_then(Value::as_str), Some("ran"));
        assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
        // out-map="context", the default of a transition's service-call: the results go on the stack by name
        let out = sync("onto#Context", &params).unwrap();
        assert_eq!(
            out.get("answer").map(moqui_lib::object::to_plain_string).as_deref(),
            Some("42"),
            "{out:?}"
        );
        let out = sync("stops#Num", &BTreeMap::new()).unwrap();
        assert!(!out.contains_key("after"), "an unignored error stops the actions");
        assert!(ctx.message().errors().iter().any(|text| text.contains("nothing#Here")));
    }

    #[test]
    fn message_errors_roll_back_but_return_the_results() {
        let path = write_fixture(
            "message-error",
            r#"<services>
                <service verb="halfway" noun="Row">
                    <out-parameters><parameter name="note"/></out-parameters>
                    <actions>
                        <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                        <script>made.set("testId", "ROLLED"); note = "ran";</script>
                        <entity-create value-field="made"/>
                        <message error="true">boom</message>
                    </actions>
                </service>
            </services>"#,
        );
        let (facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        let out = sync("halfway#Row", &BTreeMap::new()).expect("the call itself did not fail");
        assert_eq!(out.get("note").and_then(Value::as_str), Some("ran"));
        assert_eq!(ctx.message().errors(), ["boom"]);
        let rows = facade.find("moqui.test.TestEntity").unwrap().list().unwrap();
        assert_eq!(rows.len(), 0, "the create was rolled back with the message error");
    }

    #[test]
    fn post_commit_rules_run_after_a_failure_only_when_they_run_on_error() {
        let path = write_fixture(
            "post-commit",
            r#"<services>
                <service verb="shell" noun="Only" type="interface"/>
            </services>"#,
        );
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        super::super::eca_rules::install(
            ctx.factory(),
            super::super::eca_rules::from_xml(
                r#"<secas>
                    <seca service="shell#Only" when="post-commit">
                        <actions><script>ec.message.addMessage("plain")</script></actions>
                    </seca>
                    <seca service="shell#Only" when="post-commit" run-on-error="true">
                        <actions><script>ec.message.addMessage("cleanup")</script></actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        ctx.factory().set_entity(facade);
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        assert!(sync("shell#Only", &BTreeMap::new()).is_err());
        assert_eq!(ctx.message().messages(), ["cleanup"]);
    }

    #[test]
    fn parameters_are_converted_then_checked_and_defaults_apply() {
        let path = write_fixture(
            "validation",
            r#"<services>
                <service verb="typed" noun="Input">
                    <in-parameters>
                        <parameter name="count" type="Integer"><number-range min="1" max="10"/></parameter>
                        <parameter name="label"/>
                        <parameter name="rich" allow-html="any"/>
                        <parameter name="careful" allow-html="safe"/>
                        <parameter name="calc" type="Integer" default="2 + 3"/>
                        <parameter name="tpl" default-value="${label}-suffix"/>
                        <parameter name="later" type="Integer"/>
                        <parameter name="sum" type="Integer" default="calc + later"/>
                    </in-parameters>
                </service>
            </services>"#,
        );
        let (_facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let text = |value: &str| Value::string(value);
        let call = |pairs: &[(&str, Value)]| {
            let params: BTreeMap<String, Value> = pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
            crate::service::validate("typed#Input", &params)
        };

        let ok = call(&[("count", text("7")), ("label", text("hi"))]).unwrap();
        assert_eq!(ok.get("count"), Some(&Value::I32(7)), "converted to the declared type");
        assert_eq!(ok.get("calc"), Some(&Value::I32(5)), "default expression");
        assert_eq!(
            ok.get("tpl").and_then(Value::as_str),
            Some("hi-suffix"),
            "default-value expands"
        );
        // A default sees the parameters before it as cleaned: a default, and a number converted after the first default.
        let summed = call(&[("label", text("hi")), ("later", text("4"))]).unwrap();
        assert_eq!(summed.get("sum"), Some(&Value::I32(9)), "{summed:?}");

        let bad = call(&[("count", text("abc"))]).unwrap_err();
        assert!(bad.to_string().contains("not a valid Integer"), "{bad}");
        let range = call(&[("count", text("11"))]).unwrap_err();
        assert!(
            range.to_string().contains("count"),
            "a range check sees the converted number: {range}"
        );

        let html = call(&[("label", text("<b>x</b>"))]).unwrap_err();
        assert!(html.to_string().contains("HTML not allowed"), "{html}");
        call(&[("rich", text("<b>x</b>"))]).expect("allow-html any");
        let cleaned = call(&[(
            "careful",
            text(r#"<b onclick="x()">hi</b><script>bad()</script> <a href="javascript:evil()">l</a> <img src="http://h/i.png" onerror="x()">"#),
        )])
        .expect("safe cleans instead of rejecting");
        let out = cleaned.get("careful").and_then(Value::as_str).unwrap().to_string();
        assert!(out.contains("<b>hi</b>"), "{out}");
        assert!(
            !out.contains("script") && !out.contains("bad()") && !out.contains("onclick"),
            "{out}"
        );
        assert!(!out.contains("javascript:") && !out.contains("onerror"), "{out}");
        assert!(out.contains(r#"src="http://h/i.png""#), "{out}");
        let link = call(&[("careful", text(r#"<a href="https://example.com/">go</a>"#))]).unwrap();
        let link = link.get("careful").and_then(Value::as_str).unwrap().to_string();
        assert!(link.contains(r#"rel="noopener noreferrer""#), "{link}");
        let plain = call(&[("careful", text("a & b, no markup"))]).unwrap();
        assert_eq!(plain.get("careful").and_then(Value::as_str), Some("a & b, no markup"));
    }

    #[test]
    fn an_update_service_with_auto_parameters_accepts_a_changed_field() {
        let (facade, ctx) = seeded_context(&[("U1", "before")]);
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let mut params = BTreeMap::new();
        params.insert("testId".into(), Value::string("U1"));
        params.insert("testMedium".into(), Value::string("after"));
        sync("update#moqui.test.TestEntity", &params).expect("update by primary key with a changed field");
        let row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("U1"))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("after"));
    }

    #[test]
    fn a_script_service_runs_its_javascript_file_and_optional_chaining_loads() {
        let script = std::env::temp_dir().join(format!("moqui-script-service-{}.js", std::process::id()));
        std::fs::write(&script, "shout = greeting.toUpperCase() + '!';").unwrap();
        let path = write_fixture(
            "script-service",
            &format!(
                r#"<services>
                    <service verb="shout" noun="Greeting" type="script" location="{}">
                        <in-parameters><parameter name="greeting"/></in-parameters>
                        <out-parameters><parameter name="shout"/></out-parameters>
                    </service>
                    <service verb="chain" noun="Safely">
                        <out-parameters><parameter name="seen"/></out-parameters>
                        <actions><set field="seen" from="missing?.name ?? 'none'"/></actions>
                    </service>
                    <service verb="groovy" noun="Elvis">
                        <actions><set field="x" from="a ?: b"/></actions>
                    </service>
                </services>"#,
                script.display()
            ),
        );
        let (_facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert_eq!(failures.len(), 1, "only the Elvis service fails: {failures:?}");
        assert!(failures[0].contains("groovy#Elvis"), "{failures:?}");
        let _authz = ctx.disable_authz();
        let mut params = BTreeMap::new();
        params.insert("greeting".into(), Value::string("hello"));
        let out = sync("shout#Greeting", &params).unwrap();
        assert_eq!(out.get("shout").and_then(Value::as_str), Some("HELLO!"));
        let out = sync("chain#Safely", &BTreeMap::new()).unwrap();
        assert_eq!(out.get("seen").and_then(Value::as_str), Some("none"));
    }

    #[test]
    fn a_runaway_script_ends_at_the_transaction_deadline() {
        let path = write_fixture(
            "runaway",
            r#"<services>
                <service verb="spin" noun="Forever" transaction-timeout="1">
                    <actions><script>while (true) {}</script></actions>
                </service>
            </services>"#,
        );
        let (_facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let _authz = ctx.disable_authz();
        let started = std::time::Instant::now();
        let err = sync("spin#Forever", &BTreeMap::new()).expect_err("interrupted");
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{err}");
    }

    #[test]
    fn post_service_seca_runs_its_action() {
        let path = write_fixture(
            "seca",
            r#"<services>
                <service verb="mark" noun="Row"/>
            </services>"#,
        );
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        super::super::eca_rules::install(
            ctx.factory(),
            super::super::eca_rules::from_xml(
                r#"<secas>
                    <seca service="mark#Row" when="post-service">
                        <actions><script>ec.message.addMessage("seen")</script></actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        ctx.factory().set_entity(facade);
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        sync("mark#Row", &BTreeMap::new()).unwrap();
        // The rule runs in its own context frame, so its effect is seen through the message facade.
        assert!(ctx.message().messages().iter().any(|text| text == "seen"));
    }

    #[test]
    fn remote_rest_in_sim_mode_errors_before_a_socket() {
        let path = write_fixture(
            "remote",
            r#"<services>
                <service verb="ping" noun="Remote" type="remote-rest" authenticate="false" location="http://127.0.0.1:1/no-such-remote"/>
            </services>"#,
        );
        let facade = crate::testing::open();
        facade.set_sim_active(true);
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(facade);
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let _authz = ctx.disable_authz();
        let err = sync("ping#Remote", &BTreeMap::new()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("sim mode is active"), "{text}");
    }

    #[test]
    fn auto_parameters_are_expanded_once_per_entity_facade() {
        let path = write_fixture(
            "auto-cache",
            r#"<services><service verb="take" noun="Auto">
                <in-parameters><auto-parameters entity-name="moqui.test.TestEntity" include="pk"/></in-parameters>
            </service></services>"#,
        );
        let (facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let def = crate::service::define::find("take#Auto").expect("registered");
        let first = super::validate::declared_in(&def, Some(&facade)).unwrap();
        assert!(first.iter().any(|param| param.name == "testId"), "{first:?}");
        let again = super::validate::declared_in(&def, Some(&facade)).unwrap();
        assert!(Arc::ptr_eq(&first, &again), "kept on the definition");
        let other = crate::testing::open();
        let expanded = super::validate::declared_in(&def, Some(&other)).unwrap();
        assert!(!Arc::ptr_eq(&first, &expanded), "another facade expands again");
        assert_eq!(
            expanded.iter().map(|param| &param.name).collect::<Vec<_>>(),
            first.iter().map(|param| &param.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn auto_parameters_in_out_parameters_are_returned() {
        let path = write_fixture(
            "auto-out",
            r#"<services><service verb="give" noun="AutoOut">
                <out-parameters><auto-parameters entity-name="moqui.test.TestEntity" include="nonpk"/></out-parameters>
                <actions><set field="testMedium" value="kept"/><set field="notDeclared" value="dropped"/></actions>
            </service></services>"#,
        );
        let (_facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let out = sync("give#AutoOut", &BTreeMap::new()).unwrap();
        assert_eq!(
            out.get("testMedium").map(moqui_lib::object::to_plain_string).as_deref(),
            Some("kept"),
            "{out:?}"
        );
        assert!(!out.contains_key("notDeclared"), "{out:?}");
    }

    #[test]
    fn auto_parameters_of_a_view_entity_use_its_aliases() {
        let path = write_fixture(
            "auto-view",
            r#"<services><service verb="give" noun="AutoView">
                <in-parameters><auto-parameters entity-name="moqui.test.TestEntityCount"/></in-parameters>
                <out-parameters><auto-parameters entity-name="moqui.test.TestEntityCount"/></out-parameters>
                <actions><set field="entityCount" value="7"/><set field="notDeclared" value="dropped"/></actions>
            </service></services>"#,
        );
        let (_facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let out = sync("give#AutoView", &BTreeMap::new()).unwrap();
        assert_eq!(
            out.get("entityCount")
                .map(moqui_lib::object::to_plain_string)
                .as_deref(),
            Some("7"),
            "{out:?}"
        );
        assert!(!out.contains_key("notDeclared"), "{out:?}");
    }

    fn write_fixture(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("moqui-svc-{name}-{}.xml", std::process::id()));
        std::fs::write(&path, body).unwrap();
        path
    }
}
