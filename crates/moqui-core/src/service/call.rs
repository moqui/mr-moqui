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
    /// `ignorePreviousError(true)`: errors already in the message facade are set aside while the service runs, so it
    /// runs anyway, and come back after it.
    pub ignore_previous_error: bool,
    /// `noRememberParameters()`: the artifact hit does not keep the parameters.
    pub no_remember_parameters: bool,
    /// Opt-in public/protocol boundary: retain typed failures instead of converting them to result messages.
    pub throw_on_error: bool,
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
/// Returns the errors of [`sync`], or an error when the worker queue is full.
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
///
/// As Java's `call()`, a failure of the service's body or of its commit is not an error of the call: its message and
/// causes are recorded in the message facade, the transaction rolls back, and the results come back as they were, empty
/// when the body failed. Callers read the message facade, as Java's read `hasError()`; [`sync_checked`] does that for
/// Rust code that needs a `Result`.
///
/// # Errors
///
/// Returns an error for what Java throws: an unknown or interface service, a call that needs a logged-in user, a held
/// semaphore, a failing pre-validate or pre-auth rule, and an authorization failure, in the body too.
pub fn sync(name: &str, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
    sync_with(name, parameters, &CallOptions::default())
}

/// [`sync_with`] for Rust code that needs a `Result`: the errors the call recorded in the message facade are an error
/// here too, with their text. They stay in the message facade.
///
/// # Errors
///
/// Returns the errors of [`sync`], and the errors the call recorded.
pub fn sync_checked(
    name: &str,
    parameters: &BTreeMap<String, Value>,
    options: &CallOptions,
) -> Result<BTreeMap<String, Value>, Error> {
    let mark = ExecutionContext::with_current(|ctx| ctx.message().error_mark());
    let results = sync_with(name, parameters, options)?;
    let errors = mark
        .and_then(|mark| ExecutionContext::with_current(|ctx| ctx.message().errors_since(mark)))
        .unwrap_or_default();
    if errors.is_empty() {
        Ok(results)
    } else {
        Err(if options.throw_on_error {
            Error::of_kind(crate::context::ErrorKind::Unprocessable, errors.join("\n"))
        } else {
            Error::new(errors.join("\n"))
        })
    }
}

/// The multi call of Java's `ServiceCallSyncImpl`: for each row `N` (from 0, until a row has none of the in-parameters
/// as `name_N`), the service is called with that row's values, then the plain parameters and the results so far for the
/// rest. With `_useRowSubmit` (or `_useRowSubmit_N`) only rows with `_rowSubmit_N` run. All rows run in one transaction,
/// which an error rolls back; the first error stops the rows. The results of the rows are merged.
///
/// # Errors
///
/// Returns the errors of [`sync`] for a row, and an error of the transaction around the rows.
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

/// What [`prepare_async`] decided for an async call.
pub(super) enum AsyncCall {
    /// Queue the call with these parameters.
    Run(BTreeMap<String, Value>),
    /// Queue nothing; a future gets this result at once.
    Skip(BTreeMap<String, Value>),
}

/// Java's `validateCall`, run as the caller before an async call is queued: an unknown or interface service is an error
/// at once, the parameters are converted and validated, the caller must be authorized for the service, and
/// `authUsername` is added. The worker then runs the call with authorization disabled, as Java's.
///
/// Nothing is queued when the parameters failed validation: the errors are in the message facade and a future gets an
/// empty result. Java queues the call anyway, without the authorization check, and its run fails the same validation.
/// In a sim session nothing is queued either, as Java's `ServiceCallAsyncImpl`, since a worker would not run in the
/// sim: a future gets `simSkipped` and `serviceName`.
pub(super) fn prepare_async(name: &str, parameters: &BTreeMap<String, Value>) -> Result<AsyncCall, Error> {
    let name = canonical(name);
    // Under the read fence a worker would run the call unfenced, so it is refused, not queued.
    crate::entity::fence::check_effect(crate::entity::fence::Effect::AsyncCall, &name)
        .map_err(|err| Error::new(err.message()))?;
    let facade = current_facade();
    if facade.as_ref().is_some_and(EntityFacade::sim_active) {
        tracing::info!("Skipping async service {name} in LLM sim session");
        let mut skipped = BTreeMap::new();
        skipped.insert("simSkipped".to_string(), Value::Bool(true));
        skipped.insert("serviceName".to_string(), Value::string(&name));
        return Ok(AsyncCall::Skip(skipped));
    }
    let def = resolve(&name, facade.as_ref())?;
    if def.kind == ServiceKind::Interface {
        return Err(Error::new(format!("Cannot run interface service [{}]", def.name)));
    }
    let mut cleaned = match validate::clean(&def, parameters, facade.as_ref()) {
        Ok(cleaned) => cleaned,
        Err(err) if err.kind() == crate::context::ErrorKind::Validation => {
            tracing::warn!(service = %def.name, "async call not queued, input failed validation: {}", err.message());
            return Ok(AsyncCall::Skip(BTreeMap::new()));
        }
        Err(err) => return Err(err),
    };
    // As Java: the service is pushed, which checks its authorization, and popped at once, with no artifact hit.
    let checked = ExecutionContext::with_current(|ctx| {
        ctx.push_checked(
            "AT_SERVICE",
            &def.name,
            verb_action(&def.verb),
            def.authenticate == Authenticate::Required,
            true,
            None,
        )?;
        ctx.untrack_hit();
        ctx.pop()
    });
    if let Some(Err(err)) = checked {
        return Err(err);
    }
    let username = crate::user::current_username().ok().flatten();
    cleaned.insert("authUsername".to_string(), username.map_or(Value::Null, Value::String));
    Ok(AsyncCall::Run(cleaned))
}

/// [`sync`] with per-call overrides of authorization and the transaction attribute.
///
/// # Errors
///
/// Returns the errors of [`sync`].
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
    // An unknown service is an error first, as Java's `call` looks it up before `callSingle`.
    let def = resolve(&name, facade.as_ref())?;
    // As Java's `callSingle`: with `ignorePreviousError` the errors so far are set aside until the call returns.
    let _set_aside = options.ignore_previous_error.then(ErrorsAside::push);
    if has_error() {
        tracing::warn!(service = %def.name, "Found error(s) before service {}, so not running service", def.name);
        return Ok(BTreeMap::new());
    }
    // A transaction marked for rollback runs no more services, unless this one runs in a new transaction.
    if options.require_new_transaction != Some(true) {
        if let Some(tx) = facade.as_ref().and_then(|facade| facade.default_transactions().ok()) {
            if tx.is_rollback_only() {
                tracing::warn!(service = %def.name, "Transaction marked for rollback, not running service {}", def.name);
                if !options.ignore_previous_error && !has_error() {
                    add_error(&format!(
                        "Transaction marked for rollback, not running service {}",
                        def.name
                    ));
                }
                return Ok(BTreeMap::new());
            }
        }
    }
    super::coverage::record(&def.name);
    // Java's `currentParameters`: what a pre-validate rule sets is what is validated.
    let mut input = std::borrow::Cow::Borrowed(parameters);
    if super::seca::has_rules(&def.name, "pre-validate") {
        super::seca::run_seca_rules(&def.name, "pre-validate", input.to_mut(), None)?;
    }
    // Input that fails its checks is validation errors in the message facade and no run, as in Java: the caller sees
    // the errors (a screen shows them by their fields) instead of a failed call.
    let mark = crate::context::ExecutionContext::with_current(|ctx| ctx.message().error_mark());
    let mut cleaned = match validate::clean(&def, &input, facade.as_ref()) {
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
    // As Java, an interface is an error before the run, not a failure of its body.
    if def.kind == ServiceKind::Interface {
        return Err(Error::new(format!(
            "Service {} is an interface and cannot be run",
            def.name
        )));
    }
    if def.service_type == "entity-implicit" {
        // As Java's `runImplicitEntityAuto`, which runs the pre-validate rules a second time and returns before the
        // pre-auth rules.
        super::seca::run_seca_rules(&def.name, "pre-validate", &mut cleaned, None)?;
    } else {
        // After the login and the authentication check, as Java's.
        super::seca::run_seca_rules(&def.name, "pre-auth", &mut cleaned, None)?;
    }
    // The service goes on the artifact stack, so the entity operations it makes inherit its
    // authorization. Only a service that requires authentication needs an allow record.
    let _frame = ServiceFrame::push(&def, &cleaned, options.no_remember_parameters)?;
    let facade = facade.ok_or_else(|| Error::new("no entity facade is installed"))?;
    let guard = crate::service::semaphore::take(&def, &cleaned, &facade)?;
    let entity_name = if def.kind == ServiceKind::EntityAuto && !def.noun.is_empty() {
        def.noun.clone()
    } else {
        ServiceDef::semaphore_entity().to_string()
    };
    let tx = facade.transactions_for(&entity_name)?;
    let attribute = options.attribute(def.transaction);
    // Java's `result`: empty until the runner returns, then its results, which the post-service rules may change.
    let mut produced: BTreeMap<String, Value> = BTreeMap::new();
    // Errors the service leaves in the message facade roll the transaction back but are not a
    // failure of the call: the results come back with the errors alongside, as in Java.
    let mut message_failed = false;
    // The body and its post-service rules ran; a failure after that is the commit's.
    let mut finished = false;
    let outcome = tx.run(attribute, options.timeout.or(def.timeout), || {
        let _deadline = crate::script::DeadlineGuard::new(tx.time_remaining());
        if def.no_tx_cache {
            crate::entity::cache::flush_and_disable(&facade, &tx)?;
        }
        super::seca::run_seca_rules(&def.name, "pre-service", &mut cleaned, None).map_err(script_err)?;
        if has_error() {
            tracing::warn!(service = %def.name, "errors before the service ran, so it was not run");
            return Ok(());
        }
        let ran = execute(&def, &cleaned, &facade);
        // As Java's `finally` after the runner: tx-rollback rules are registered once the service ran, with its results,
        // empty when it failed. tx-commit is last so a post-service entity write can still register the feed schedule
        // hook.
        let registered = register_tx_rollback(&tx, &def.name, &cleaned, ran.as_ref().unwrap_or(&BTreeMap::new()));
        produced = ran?;
        registered?;
        // Java registers the tx-commit rules here too, with copies, so they do not see what the post-service rules set.
        let for_commit = super::seca::has_rules(&def.name, "tx-commit").then(|| (cleaned.clone(), produced.clone()));
        super::seca::run_seca_rules(&def.name, "post-service", &mut cleaned, Some(&mut produced))
            .map_err(script_err)?;
        finished = true;
        if has_error() {
            message_failed = true;
            // Under `ignore` with nothing in place there is no transaction to mark.
            if tx.in_transaction() {
                tx.set_rollback_only(format!("Error running service {} (message)", def.name))?;
            }
        } else if let Some((parameters, results)) = &for_commit {
            register_tx_commit(&tx, &def.name, parameters, results)?;
        }
        Ok(())
    });
    // As Java's `callSingle`, a failure of the body or of the commit is recorded in the message facade, the transaction
    // is rolled back, and the call returns the results as they were: empty when the runner failed. Only an authorization
    // failure goes on as an error, as Java rethrows `ArtifactAuthorizationException`, and so does a failure with no
    // message facade to record it in.
    let mut failure = match outcome {
        Ok(()) => None,
        // The rollback the message errors asked for.
        Err(_) if message_failed => None,
        Err(err) => Some(err),
    };
    if let Some(err) = &failure {
        // An authorization failure has already rolled back the transaction the call began, where Java's `finally`
        // commits what the body wrote before the refusal: safer, so a refused call leaves nothing (accepted, P6b-15).
        let authz = crate::context::ErrorKind::of(err) == crate::context::ErrorKind::Unauthorized;
        if !authz {
            // Java tells the callbacks of a body failure; a commit failure comes after they heard the results.
            if finished {
                call_callbacks(&def.name, &cleaned, Ok(&produced));
            } else {
                call_callbacks(&def.name, &cleaned, Err(err));
            }
            if record_failure(&def.name, err) && !options.throw_on_error {
                failure = None;
            }
        }
    } else {
        call_callbacks(&def.name, &cleaned, Ok(&produced));
    }
    // post-commit rules run whether or not the service worked. Those that skip on error do. As Java's, a failed call
    // gives them its results as they were, an empty map when the runner failed.
    let post = super::seca::run_seca_rules(&def.name, "post-commit", &mut cleaned, Some(&mut produced));
    drop(guard);
    if let Some(err) = failure {
        return Err(err.into());
    }
    post?;
    Ok(project(&def, produced))
}

/// Java's `catch (Throwable t)`: the failure's message and each cause's are added to the message facade, for the
/// client to show: the message text only ([`crate::context::client_text`]), without the script's stack, the generated
/// line or the Rust source location, which the server log has (the diagnostic, logged here). Display already writes the
/// causes after the message, so a cause is added on its own only when no error so far shows its text. False with no
/// execution context, where the error would be lost.
fn record_failure(service: &str, err: &LibError) -> bool {
    if err.to_string().contains("is not built into this host") {
        // a tool this package leaves out (git, imap): its callers are told each time, the log once per service
        static TOLD: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
            std::sync::OnceLock::new();
        let first = TOLD
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(service.to_string());
        if first {
            tracing::info!(service, "Service {service} is not available: {err}");
        }
    } else {
        tracing::error!(service, "Service {service} failed: {}", err.diagnostic());
    }
    let mut texts = vec![crate::context::client_text(&err.to_string())];
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        let text = crate::context::client_text(&cause.to_string());
        if !text.is_empty() && !texts.iter().any(|known| known.contains(&text)) {
            texts.push(text);
        }
        source = cause.source();
    }
    ExecutionContext::with_current(|ctx| {
        let mut messages = ctx.message();
        for text in texts {
            messages.add_error(text);
        }
    })
    .is_some()
}

/// The message errors set aside by `ignorePreviousError`, put back when dropped (Java's `pushErrors`/`popErrors`).
struct ErrorsAside;

impl ErrorsAside {
    fn push() -> Self {
        ExecutionContext::with_current(|ctx| ctx.message().push_errors());
        Self
    }
}

impl Drop for ErrorsAside {
    fn drop(&mut self) {
        ExecutionContext::with_current(|ctx| ctx.message().pop_errors());
    }
}

/// The artifact frame for a running service. It pops when dropped, and ends an anonymous login the
/// call started.
struct ServiceFrame {
    pushed: bool,
    anonymous: bool,
}

impl ServiceFrame {
    fn push(def: &ServiceDef, parameters: &BTreeMap<String, Value>, no_remember: bool) -> Result<Self, Error> {
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
            let remembered = (!def.no_remember_parameters && !no_remember).then_some(parameters);
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
    results: &BTreeMap<String, Value>,
) -> Result<(), LibError> {
    register_tx_rules(tx, service, "tx-rollback", parameters, Some(results))
}

fn register_tx_commit(
    tx: &crate::transaction::TransactionManager,
    service: &str,
    parameters: &BTreeMap<String, Value>,
    results: &BTreeMap<String, Value>,
) -> Result<(), LibError> {
    register_tx_rules(tx, service, "tx-commit", parameters, Some(results))
}

/// Java's `SecaSynchronization`: when the transaction commits (or rolls back), the `tx-commit` (or `tx-rollback`)
/// rules run on a service worker, in a context and transaction of their own. A sim session never really commits, so
/// its rules do not run.
///
/// A pattern rule runs here as at any other `when`. Java's `registerTx` compares the rule's service name, the pattern
/// text, to the called name, so its pattern tx rules never run (an accepted difference, P7a-15).
fn register_tx_rules(
    tx: &crate::transaction::TransactionManager,
    service: &str,
    when: &'static str,
    parameters: &BTreeMap<String, Value>,
    results: Option<&BTreeMap<String, Value>>,
) -> Result<(), LibError> {
    if !tx.in_transaction() || !super::seca::has_rules(service, when) {
        return Ok(());
    }
    let Some(factory) = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory())) else {
        return Ok(());
    };
    let sim = current_facade().is_some_and(|facade| facade.sim_active());
    // The rules would run on the service pool, outside the fence: a read that has them is refused.
    if !sim {
        crate::entity::fence::check_effect(
            crate::entity::fence::Effect::AsyncCall,
            &format!("the {when} rules of {service}"),
        )?;
    }
    let service = service.to_string();
    let mut parameters = parameters.clone();
    let mut results = results.cloned();
    let hook = move || {
        if sim {
            tracing::info!("Skipping {when} rules of {service} in a sim session");
            return;
        }
        let pools_factory = Arc::clone(&factory);
        pools_factory.service_pools().spawn("service tx ECA rules", move || {
            let facade = factory
                .entity()
                .ok_or_else(|| Error::new("no entity facade is installed"))?;
            let tx = facade.default_transactions()?;
            tx.run(TransactionAttribute::UseOrBegin, None, || {
                super::seca::run_seca_rules(&service, when, &mut parameters, results.as_mut()).map_err(Into::into)
            })
            .map_err(Into::into)
        });
    };
    if when == "tx-commit" {
        tx.on_commit(hook)
    } else {
        tx.on_rollback(hook)
    }
}

/// Java's `ServiceCallSpecial.registerOnCommit` and `registerOnRollback`: run `name` asynchronously once the
/// transaction in place commits (`on_commit`) or rolls back.
///
/// # Errors
///
/// Returns an error for an unknown service or when no transaction is in place.
pub fn register_on_end(name: &str, parameters: &BTreeMap<String, Value>, on_commit: bool) -> Result<(), Error> {
    let name = canonical(name);
    let facade = current_facade().ok_or_else(|| Error::new("no entity facade is installed"))?;
    resolve(&name, Some(&facade))?;
    let tx = facade.default_transactions()?;
    if !tx.in_transaction() {
        return Err(Error::new(format!(
            "Cannot register service {name} to run at the end of a transaction: no transaction is in place"
        )));
    }
    let parameters = parameters.clone();
    let hook = move || {
        if let Err(err) = async_call(&name, &parameters) {
            tracing::error!(error = %err, "Could not run service {name} registered for the end of the transaction");
        }
    };
    if on_commit {
        tx.on_commit(hook)?;
    } else {
        tx.on_rollback(hook)?;
    }
    Ok(())
}

/// Java's `ServiceCallback`: told the in-parameters and the results of each call of a service, or its error.
pub trait ServiceCallback: Send + Sync {
    fn is_enabled(&self) -> bool {
        true
    }
    fn receive_result(&self, context: &BTreeMap<String, Value>, result: &BTreeMap<String, Value>);
    fn receive_error(&self, context: &BTreeMap<String, Value>, error: &LibError);
}

type Callbacks = std::sync::RwLock<std::collections::HashMap<String, Vec<Arc<dyn ServiceCallback>>>>;

fn callbacks() -> &'static Callbacks {
    static CALLBACKS: std::sync::OnceLock<Callbacks> = std::sync::OnceLock::new();
    CALLBACKS.get_or_init(Default::default)
}

/// Java's `ServiceFacade.registerCallback`: `callback` hears of every later call of `name`.
pub fn register_callback(name: &str, callback: Arc<dyn ServiceCallback>) {
    callbacks()
        .write()
        .unwrap_or_else(|err| err.into_inner())
        .entry(canonical(name))
        .or_default()
        .push(callback);
}

fn call_callbacks(name: &str, context: &BTreeMap<String, Value>, outcome: Result<&BTreeMap<String, Value>, &LibError>) {
    let list = callbacks()
        .read()
        .unwrap_or_else(|err| err.into_inner())
        .get(name)
        .cloned()
        .unwrap_or_default();
    for callback in list.iter().filter(|callback| callback.is_enabled()) {
        match outcome {
            Ok(result) => callback.receive_result(context, result),
            Err(err) => callback.receive_error(context, err),
        }
    }
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

/// The implicit entity-auto definitions made, by service name, kept with the facade whose entities they name. One
/// definition per name keeps its expanded parameters (see `validate::declared_in`) from one call to the next.
type ImplicitDefs = crate::entity::ByDefinition<Arc<ServiceDef>>;

pub(crate) fn resolve(name: &str, facade: Option<&EntityFacade>) -> Result<Arc<ServiceDef>, Error> {
    if let Some(def) = super::define::find(name) {
        return Ok(def);
    }
    let (verb, noun) = split_name(name);
    if matches!(verb.as_str(), "create" | "update" | "store" | "delete") && !noun.is_empty() {
        if let Some(facade) = facade {
            let generation = crate::entity::definition_generation(facade);
            if let Some(def) = facade.with_slot_read(ImplicitDefs::default, |defs| defs.get(name, generation)) {
                return Ok(def);
            }
            if facade.with_entity(&noun, |_, _, _| Ok(())).is_ok() {
                let def = Arc::new(ServiceDef::implicit(name, &verb, &noun));
                facade.with_slot(ImplicitDefs::default, |defs| {
                    defs.insert(name, generation, Arc::clone(&def))
                });
                return Ok(def);
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

/// The `authenticate` of the service `name`. None when no service has that name, as for an implicit entity-auto one.
pub(crate) fn service_authenticate(name: &str) -> Option<super::define::Authenticate> {
    super::define::find(&canonical(name)).map(|def| def.authenticate)
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
/// Java's `isServiceDefined`: a registered service, or an entity-auto name (`create`, `update`, `store`, `delete`)
/// of a defined entity. With no entity facade an entity-auto name counts as defined.
pub fn is_service_defined(name: &str, facade: Option<&EntityFacade>) -> bool {
    let name = canonical(name);
    if super::define::find(&name).is_some() {
        return true;
    }
    match (entity_auto_noun(&name), facade) {
        (Some(noun), Some(facade)) => crate::entity::info::is_entity_defined(facade, &noun),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

pub(crate) fn entity_auto_noun(name: &str) -> Option<String> {
    let name = canonical(name);
    if super::define::find(&name).is_some() {
        return None;
    }
    let (verb, noun) = split_name(&name);
    (matches!(verb.as_str(), "create" | "update" | "store" | "delete") && !noun.is_empty()).then_some(noun)
}

/// A service as an agent's tool takes it (plans/LLM-A2A-Assist.md 4.6): its definition (an implicit entity-auto one
/// too), its authorization action, its input checked and converted as a call checks it, and the JSON schemas of its
/// parameters and results as the OpenAPI document writes them.
pub struct ToolService {
    pub(crate) def: Arc<ServiceDef>,
}

impl ToolService {
    /// The service `name`, defined or entity-auto.
    ///
    /// # Errors
    ///
    /// Returns an error when no service has that name, or it is an interface.
    pub fn of(name: &str) -> Result<Self, Error> {
        let def = resolve(&canonical(name), current_facade().as_ref())?;
        if def.kind == ServiceKind::Interface {
            return Err(Error::new(format!(
                "{} is an interface service, which cannot run",
                def.name
            )));
        }
        Ok(Self { def })
    }

    pub fn name(&self) -> &str {
        &self.def.name
    }

    /// view, create, update, delete or all: its `authz-action`, else by its verb.
    pub fn authz_action(&self) -> &str {
        auth_action(&self.def)
    }

    /// It is declared `read-only="true"`.
    pub fn read_only(&self) -> bool {
        self.def.read_only
    }

    /// Whether the user must be authorized for it (`authenticate` is not false or anonymous).
    pub fn needs_authz(&self) -> bool {
        self.def.authenticate == Authenticate::Required
    }

    /// The entity of an entity-auto service.
    pub fn entity_noun(&self) -> Option<String> {
        (self.def.kind == ServiceKind::EntityAuto && !self.def.noun.is_empty()).then(|| self.def.noun.clone())
    }

    pub fn verb(&self) -> &str {
        &self.def.verb
    }

    /// The input as a call checks it: converted, defaulted, unknown names dropped when the service validates its
    /// input. The validation errors the check adds to the message facade are taken out again.
    ///
    /// # Errors
    ///
    /// Returns the validation errors, each with its field.
    pub fn clean(&self, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
        let mark = ExecutionContext::with_current(|ctx| ctx.message().error_mark());
        let cleaned = validate::clean(&self.def, parameters, current_facade().as_ref());
        if let Some(mark) = mark {
            ExecutionContext::with_current(|ctx| ctx.message().take_errors_since(mark));
        }
        cleaned
    }

    /// The JSON schema of the in-parameters.
    ///
    /// # Errors
    ///
    /// Returns an error when the `auto-parameters` cannot be expanded.
    pub fn input_schema(&self, facade: Option<&EntityFacade>) -> Result<Value, Error> {
        let params = validate::declared_in(&self.def, facade)?;
        // an implicit entity-auto service declares none: its entity's fields are its parameters
        if params.is_empty() && self.def.kind == ServiceKind::EntityAuto {
            if let Some(schema) =
                facade.and_then(|facade| super::rest::entity_auto_schema(facade, &self.def.verb, &self.def.noun))
            {
                return Ok(schema);
            }
        }
        let params: Vec<&super::define::ParamDef> = params
            .iter()
            .filter(|param| !param.name.is_empty() && param.required != super::define::Required::Disabled)
            .collect();
        Ok(super::rest::params_schema(
            &params,
            &mut super::rest::Schemas::new(facade),
        ))
    }

    /// The JSON schema of the out-parameters.
    pub fn output_schema(&self, facade: Option<&EntityFacade>) -> Value {
        let expanded = validate::expanded_out(&self.def);
        let params: Vec<&super::define::ParamDef> = expanded
            .iter()
            .filter(|param| !param.name.is_empty() && param.required != super::define::Required::Disabled)
            .collect();
        super::rest::params_schema(&params, &mut super::rest::Schemas::new(facade))
    }
}

fn auth_action(def: &ServiceDef) -> &str {
    def.authz_action.as_deref().unwrap_or_else(|| verb_action(&def.verb))
}

fn verb_action(verb: &str) -> &'static str {
    match verb {
        "create" => "create",
        "update" | "store" => "update",
        "delete" => "delete",
        // As Java's `ServiceDefinition.verbAuthzActionEnumMap`.
        "view" | "find" | "get" | "search" => "view",
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
            None => Err(LibError::new(
                match super::rust_services::missing_feature(&def.location) {
                    Some(feature) => format!("service {}: {feature} is not built into this host", def.name),
                    None => format!("service {} has no rust function", def.name),
                },
            )),
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
        ServiceKind::RemoteRest => {
            crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, &def.name).and_then(|()| {
                super::remote::run_remote_rest(
                    &def.name,
                    &def.location,
                    // As Java: the service's `method` attribute, else the verb when it is an HTTP method, else POST.
                    Some(def.method.as_str()),
                    &def.verb,
                    parameters,
                    facade.sim_active(),
                )
                .map_err(|err| LibError::new(err.message()))
            })
        }
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
        crate::script::run_actions_at(&def.actions, def.action_source.as_ref()).map_err(|err| in_service(def, err))?;
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
    crate::script::run_script_location(&def.location).map_err(|err| in_service(def, err))?;
    Ok(collect_outputs(def))
}

/// A failure in a service's body, with the service named after it (Java logs "Error running service ..."): a script
/// error alone gives only a line in the generated code.
fn in_service(def: &ServiceDef, err: crate::context::Error) -> LibError {
    if err.message().contains("\n    in service ") {
        return err.into();
    }
    let message = format!("{}\n    in service {}", err.message().trim_end(), def.name);
    err.with_message(message).into()
}

fn script_err(err: crate::context::Error) -> LibError {
    err.into()
}

/// The results a caller gets: only the declared out-parameters, or all when none are declared. Java returns the
/// runner's map as it is; keeping to the declared ones means a body's other values do not reach a REST or remote
/// caller (accepted, P6b-14). Inline and script bodies have no `result` map to fill, as a script here may use that name.
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
    // As Java's `setFields`, only the entity's fields are set: an implicit call passes its parameters as they came.
    let settable = |entity: &moqui_db::model::EntityDefinition, key: &str, value: &Value| {
        !reserved(key) && entity.has_field(key) && !matches!(value, Value::Map(_) | Value::List(_))
    };
    let pk_names = crate::entity::cache::pk_field_names(facade, noun)?;
    let pk_names: &[String] = &pk_names;
    if verb == "delete" {
        delete_entity(facade, noun, parameters, pk_names)?;
        return Ok(BTreeMap::new());
    }
    // A blank form field posts "": as Java's `setFields` with setIfEmpty (`EntityJavaUtil.setFieldsEv`), an empty text is
    // null for every field (a drop-down left empty clears a foreign key, which "" would fail).
    let fields = facade.with_entity(noun, |entity, _, _| {
        Ok(parameters
            .iter()
            .filter(|(key, value)| settable(entity, key, value))
            .map(|(key, value)| match value {
                Value::String(text) if text.is_empty() => (key, Value::Null),
                _ => (key, value.clone()),
            })
            .collect::<Vec<_>>())
    })?;
    let mut record = facade.make_value(noun)?;
    for (key, value) in fields {
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
    if matches!(verb, "create" | "store") && pk_missing(&record, pk_names) {
        let with_defaults = facade.apply_field_defaults(noun, record.fields())?;
        for name in pk_names {
            let missing = record.get(name).is_none_or(|value| value.is_null() || is_empty(value));
            if let Some(value) = with_defaults
                .get(name.as_str())
                .filter(|value| missing && !value.is_null())
            {
                record.set(name, value.clone());
            }
        }
    }
    let mut status_out = BTreeMap::new();
    match verb {
        "update" => {
            check_status(facade, noun, &record, pk_names, parameters, &mut status_out)?;
            record.update(facade)?
        }
        "create" => {
            assign_sequence(facade, &mut record, pk_names)?;
            record.create(facade)?;
        }
        "store" => {
            if pk_missing(&record, pk_names) {
                assign_sequence(facade, &mut record, pk_names)?;
                record.create(facade)?;
            } else if parameters.contains_key("statusId")
                && facade.with_entity(noun, |entity, _, _| Ok(entity.has_field("statusId")))?
            {
                // Java's store checks status with one uncached read. A missing row is created
                // directly; other stores choose create/update through EntityValue.refresh.
                if check_status(facade, noun, &record, pk_names, parameters, &mut status_out)? {
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
    nest(facade, verb, noun, parameters, &record, depth)?;
    let mut out = status_out;
    for pk in pk_names {
        if let Some(value) = record.get(pk) {
            out.insert(pk.clone(), value.clone());
        }
    }
    Ok(out)
}

/// `EntityAutoServiceRunner.deleteEntity`: every primary key field is needed, and `*` in one or more of them deletes
/// each record that matches the other fields given. Java deletes only the named records, not nested ones: deleting
/// children after the parent would break their foreign keys, and before it would delete rows the caller never named.
fn delete_entity(
    facade: &EntityFacade,
    noun: &str,
    parameters: &BTreeMap<String, Value>,
    pk_names: &[String],
) -> Result<(), LibError> {
    let given = |name: &String| {
        parameters
            .get(name)
            .is_some_and(|value| !value.is_null() && !is_empty(value))
    };
    if pk_names.is_empty() || !pk_names.iter().all(given) {
        return Err(LibError::new(
            "Must specify all primary key fields to delete, can use wildcard of '*' in one or more PK fields to delete multiple records",
        ));
    }
    let wildcard = |name: &str| parameters.get(name).and_then(Value::as_str) == Some("*");
    if pk_names.iter().any(|name| wildcard(name)) {
        let fields = facade.with_entity(noun, |entity, _, _| {
            Ok(entity.fields.iter().map(|field| field.name.clone()).collect::<Vec<_>>())
        })?;
        let mut find = facade.find(noun)?;
        for field in fields {
            if wildcard(&field) {
                continue;
            }
            if let Some(value) = parameters.get(&field).filter(|value| !is_empty(value)) {
                find = find.eq(&field, value.clone());
            }
        }
        for row in find.list()?.iter() {
            row.delete(facade)?;
        }
        return Ok(());
    }
    let mut record = facade.make_value(noun)?;
    for name in pk_names {
        if let Some(value) = parameters.get(name) {
            record.set(name, value.clone());
        }
    }
    record.delete(facade)?;
    Ok(())
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
        let name_of = |id: &str| -> Result<Value, LibError> {
            let item = facade
                .find("moqui.basic.StatusItem")?
                .eq("statusId", Value::string(id.to_string()))
                .use_cache(true)
                .one()?;
            Ok(item
                .and_then(|item| item.get("description").cloned())
                .unwrap_or(Value::Null))
        };
        let values = [
            (
                "fullEntityName",
                Value::string(localize_or(&format!("{noun}##EntityName"))),
            ),
            ("lookedUpStatusId", Value::string(old_id.clone())),
            ("parameterStatusId", Value::string(new_id.clone())),
            ("lookedUpStatusName", name_of(&old_id)?),
            ("parameterStatusName", name_of(&new_id)?),
        ];
        return Err(LibError::new(status_transition_message(&values)));
    }
    Ok(true)
}

/// `text` in the session locale, or as it is.
fn localize_or(text: &str) -> String {
    crate::l10n::localize(text).unwrap_or_else(|_| text.to_string())
}

/// Java's `StatusFlowTransitionNotFoundTemplate` expanded with `values`. Without its localized message (the seed
/// data's `CommonL10nData.xml` is not loaded) the default locale's text is used.
fn status_transition_message(values: &[(&str, Value)]) -> String {
    const TEMPLATE: &str = "StatusFlowTransitionNotFoundTemplate";
    const DEFAULT: &str =
        "Status change not allowed from ${lookedUpStatusName || lookedUpStatusId} to ${parameterStatusName || parameterStatusId}";
    let localized = localize_or(TEMPLATE);
    let template = if localized == TEMPLATE {
        DEFAULT
    } else {
        localized.as_str()
    };
    let _frame = crate::script::ContextFrame::push();
    for (name, value) in values {
        let _ = crate::script::set_context_name(name, value.clone());
    }
    crate::script::expand(template).unwrap_or_else(|_| template.to_string())
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
#[path = "call_tests.rs"]
mod tests;
