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
        Err(Error::new(errors.join("\n")))
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
    crate::entity::fence::check_effect(crate::entity::fence::Effect::AsyncCall, &name).map_err(|err| Error::new(err.message()))?;
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
            if record_failure(&def.name, err) {
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
    tracing::error!(service, "Service {service} failed: {}", err.diagnostic());
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
        crate::entity::fence::check_effect(crate::entity::fence::Effect::AsyncCall, &format!("the {when} rules of {service}"))?;
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

fn resolve(name: &str, facade: Option<&EntityFacade>) -> Result<Arc<ServiceDef>, Error> {
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
                facade.with_slot(ImplicitDefs::default, |defs| defs.insert(name, generation, Arc::clone(&def)));
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
pub(crate) fn is_service_defined(name: &str, facade: Option<&EntityFacade>) -> bool {
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
#[cfg(feature = "llm")]
pub(crate) struct ToolService {
    pub def: Arc<ServiceDef>,
}

#[cfg(feature = "llm")]
impl ToolService {
    /// The service `name`, defined or entity-auto.
    ///
    /// # Errors
    ///
    /// Returns an error when no service has that name, or it is an interface.
    pub(crate) fn of(name: &str) -> Result<Self, Error> {
        let def = resolve(&canonical(name), current_facade().as_ref())?;
        if def.kind == ServiceKind::Interface {
            return Err(Error::new(format!("{} is an interface service, which cannot run", def.name)));
        }
        Ok(Self { def })
    }

    pub(crate) fn name(&self) -> &str {
        &self.def.name
    }

    /// view, create, update, delete or all: its `authz-action`, else by its verb.
    pub(crate) fn authz_action(&self) -> &str {
        auth_action(&self.def)
    }

    /// It is declared `read-only="true"`.
    pub(crate) fn read_only(&self) -> bool {
        self.def.read_only
    }

    /// Whether the user must be authorized for it (`authenticate` is not false or anonymous).
    pub(crate) fn needs_authz(&self) -> bool {
        self.def.authenticate == Authenticate::Required
    }

    /// The entity of an entity-auto service.
    pub(crate) fn entity_noun(&self) -> Option<String> {
        (self.def.kind == ServiceKind::EntityAuto && !self.def.noun.is_empty()).then(|| self.def.noun.clone())
    }

    pub(crate) fn verb(&self) -> &str {
        &self.def.verb
    }

    /// The input as a call checks it: converted, defaulted, unknown names dropped when the service validates its
    /// input. The validation errors the check adds to the message facade are taken out again.
    ///
    /// # Errors
    ///
    /// Returns the validation errors, each with its field.
    pub(crate) fn clean(&self, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
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
    pub(crate) fn input_schema(&self, facade: Option<&EntityFacade>) -> Result<Value, Error> {
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
        Ok(super::rest::params_schema(&params, &mut super::rest::Schemas::new(facade)))
    }

    /// The JSON schema of the out-parameters.
    pub(crate) fn output_schema(&self, facade: Option<&EntityFacade>) -> Value {
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
        ServiceKind::RemoteRest => crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, &def.name)
            .and_then(|()| super::remote::run_remote_rest(
            &def.name,
            &def.location,
            // As Java: the service's `method` attribute, else the verb when it is an HTTP method, else POST.
            Some(def.method.as_str()),
            &def.verb,
            parameters,
            facade.sim_active(),
        )
        .map_err(|err| LibError::new(err.message()))),
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
    let fields = facade.with_entity(noun, |entity, _, _| {
        Ok(parameters
            .iter()
            .filter(|(key, value)| settable(entity, key, value))
            .collect::<Vec<_>>())
    })?;
    let mut record = facade.make_value(noun)?;
    for (key, value) in fields {
        record.set(key, value.clone());
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
            if let Some(value) = with_defaults.get(name.as_str()).filter(|value| missing && !value.is_null()) {
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
        // Java's text, which names no field: the validation error carries it, and the errors string writes it with the
        // field and service, as Java's `toStringPretty`.
        assert_eq!(errors, ["Field cannot be empty"]);
        let text = ExecutionContext::with_current(|ec| ec.message().errors_string()).unwrap();
        assert_eq!(
            text,
            format!(
                "Field cannot be empty(for field Amount of service {}",
                moqui_lib::string::camel_case_to_pretty("need#Value")
            )
        );
        assert_eq!(validation.len(), 1);
        assert_eq!(
            (
                validation[0].field.as_str(),
                validation[0].service.as_str(),
                validation[0].message.as_str()
            ),
            ("amount", "need#Value", "Field cannot be empty")
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
        // Java's text, with the holder's thread
        assert!(
            text.contains("An instance of service semaphore lock#Row with parameter value [_NA_] is already running (thread [other], locked at "),
            "{text}"
        );
        assert!(
            text.contains("and it is setup to fail on semaphore conflict."),
            "{text}"
        );
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
    fn service_failures_identify_owning_actions_and_rust_origin() {
        let path = write_fixture(
            "source-locations",
            "<services>\n<service verb=\"locate\" noun=\"Failure\">\n<actions><script><![CDATA[\nconst location = 'component://MissingDiagnostics/screen/A.xml';\nec.resource.getLocationText(location);\n]]></script></actions>\n</service>\n</services>",
        );
        let (_facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        // As Java's, the failure is recorded in the message facade and the call returns its empty results. The facade
        // has the message for the client; the server log has where it came from.
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let out =
            tracing::subscriber::with_default(crate::testing::LogSubscriber(std::sync::Arc::clone(&lines)), || {
                sync("locate#Failure", &BTreeMap::new())
            })
            .expect("a body failure is recorded, not returned");
        assert!(out.is_empty(), "{out:?}");
        let errors = ctx.message().errors().join("\n");
        assert_eq!(
            errors,
            "Error: component MissingDiagnostics is not registered for location component://MissingDiagnostics/screen/A.xml"
        );
        let diagnostic = lines.lock().unwrap().join("\n");
        assert!(
            diagnostic.contains(&format!("{}#actions-3-1.generated.js:", path.display())),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("Rust: crates/moqui-core/src/context/resource.rs:"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("in service locate#Failure"), "{diagnostic}");
        std::fs::remove_file(path).unwrap();
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

    /// Java's `ServiceCallback`, recording what it hears.
    struct Heard(Mutex<Vec<String>>);

    impl super::ServiceCallback for Heard {
        fn receive_result(&self, _context: &BTreeMap<String, Value>, result: &BTreeMap<String, Value>) {
            self.0.lock().unwrap().push(format!("result {}", result.len()));
        }
        fn receive_error(&self, _context: &BTreeMap<String, Value>, error: &moqui_lib::error::Error) {
            self.0.lock().unwrap().push(format!("error {}", error.message()));
        }
    }

    /// As Java's `callSingle`: a failing body is recorded in the message facade, the callbacks hear of the error, the
    /// transaction rolls back (and its tx-rollback rules run), the post-commit rules that run on error get an empty
    /// result map, and the call returns that empty map.
    #[test]
    fn a_body_failure_is_recorded_rolled_back_and_returns_empty_results() {
        let path = write_fixture(
            "body-failure",
            r#"<services>
                <service verb="break" noun="Late">
                    <out-parameters><parameter name="note"/></out-parameters>
                    <actions>
                        <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                        <script>made.set("testId", "ROLLED"); note = "ran";</script>
                        <entity-create value-field="made"/>
                        <script>throw new Error("late boom")</script>
                    </actions>
                </service>
            </services>"#,
        );
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        super::super::eca_rules::install(
            ctx.factory(),
            super::super::eca_rules::from_xml(
                r#"<secas>
                    <seca service="break#Late" when="post-commit">
                        <actions><script>ec.message.addMessage("plain")</script></actions>
                    </seca>
                    <seca service="break#Late" when="post-commit" run-on-error="true">
                        <actions><script>ec.message.addMessage("cleanup " + (results == null ? "none" : JSON.stringify(results)))</script></actions>
                    </seca>
                    <seca service="break#Late" when="tx-rollback">
                        <actions>
                            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                            <script>made.set("testId", "ROLLBACK_RULE")</script>
                            <entity-create value-field="made"/>
                        </actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        ctx.factory().set_entity(facade.clone());
        let heard = Arc::new(Heard(Mutex::new(Vec::new())));
        super::register_callback("break#Late", heard.clone());
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        let out = sync("break#Late", &BTreeMap::new()).expect("a body failure is recorded, not returned");
        assert!(out.is_empty(), "{out:?}");
        // the message only: the generated line and the service it ran in are in the log (service_failures_identify_...)
        let errors = ctx.message().errors().to_vec();
        assert_eq!(errors, ["Error: late boom"]);
        assert_eq!(ctx.message().messages(), ["cleanup {}"]);
        let heard = heard.0.lock().unwrap().clone();
        assert_eq!(heard.len(), 1, "{heard:?}");
        assert!(
            heard[0].starts_with("error ") && heard[0].contains("late boom"),
            "{heard:?}"
        );
        let find = |id: &str| {
            facade
                .find("moqui.test.TestEntity")
                .unwrap()
                .eq("testId", Value::string(id))
                .one()
                .unwrap()
        };
        assert!(find("ROLLED").is_none(), "the create was rolled back");
        assert!(find("ROLLBACK_RULE").is_some(), "the tx-rollback rule ran");
    }

    /// As Java's `finally`: a commit that fails is recorded in the message facade, after the callbacks heard the
    /// results, and the call returns those results; the post-commit rules that run on error get them too.
    #[test]
    fn a_commit_failure_is_recorded_and_returns_the_results() {
        let path = write_fixture(
            "commit-failure",
            r#"<services>
                <service verb="refuse" noun="Commit" type="rust" location="test::refuse_commit">
                    <out-parameters><parameter name="note"/></out-parameters>
                </service>
            </services>"#,
        );
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory()
            .register_service("test::refuse_commit", |ec, _info, _parameters| {
                let facade = ec.factory().entity().expect("an entity facade");
                let mut row = facade.make_value("moqui.test.TestEntity")?;
                row.set("testId", Value::string("UNCOMMITTED"));
                row.create(&facade)?;
                // A hook that runs before the commit, as the data feed's insert does, and fails it.
                facade
                    .transactions_for("moqui.test.TestEntity")?
                    .on_feed_insert(|_| Err(moqui_lib::error::Error::new("feed insert refused")))?;
                Ok(BTreeMap::from([("note".to_string(), Value::string("ran"))]))
            })
            .unwrap();
        super::super::eca_rules::install(
            ctx.factory(),
            super::super::eca_rules::from_xml(
                r#"<secas>
                    <seca service="refuse#Commit" when="post-commit" run-on-error="true">
                        <actions><script>ec.message.addMessage("cleanup " + JSON.stringify(results))</script></actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        ctx.factory().set_entity(facade.clone());
        let heard = Arc::new(Heard(Mutex::new(Vec::new())));
        super::register_callback("refuse#Commit", heard.clone());
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let _authz = ctx.disable_authz();
        let out = sync("refuse#Commit", &BTreeMap::new()).expect("a commit failure is recorded, not returned");
        assert_eq!(out.get("note").and_then(Value::as_str), Some("ran"));
        let errors = ctx.message().errors().join("\n");
        assert!(errors.contains("feed insert refused"), "{errors}");
        assert_eq!(ctx.message().messages(), [r#"cleanup {"note":"ran"}"#]);
        assert_eq!(*heard.0.lock().unwrap(), ["result 1"]);
        let row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("UNCOMMITTED"))
            .one()
            .unwrap();
        assert!(row.is_none(), "the write was rolled back");
    }

    /// An interface cannot be run: as Java, an error before the call's transaction, so no post-commit rule runs.
    #[test]
    fn an_interface_service_is_an_error_before_it_runs() {
        let path = write_fixture(
            "interface",
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
        let err = sync("shell#Only", &BTreeMap::new()).unwrap_err();
        assert!(err.message().contains("is an interface and cannot be run"), "{err}");
        assert!(ctx.message().messages().is_empty(), "{:?}", ctx.message().messages());
        assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
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
        assert!(
            bad.to_string()
                .contains("Value entered (abc) could not be converted to a Integer"),
            "{bad}"
        );
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

    /// Java's entity-auto `update`: only the fields passed are written, and a `statusId` is checked against the stored
    /// status (read when the update runs, so after any change since a caller read the row), which the results give as
    /// `oldStatusId`.
    #[test]
    fn an_entity_auto_update_writes_the_fields_passed_and_checks_the_stored_status() {
        let (facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let entity = "moqui.service.message.SystemMessage";
        let mut row = facade.make_value(entity).unwrap();
        row.set("systemMessageId", Value::string("M1"));
        row.set("statusId", Value::string("SmsgReceived"));
        row.set("messageText", Value::string("keep"));
        row.set("failCount", Value::I64(2));
        row.create(&facade).unwrap();
        for (from, to) in [("SmsgReceived", "SmsgConsumed"), ("SmsgConsumed", "SmsgConfirmed")] {
            let mut transition = facade.make_value("moqui.basic.StatusFlowTransition").unwrap();
            transition.set("statusFlowId", Value::string("Default"));
            transition.set("statusId", Value::string(from));
            transition.set("toStatusId", Value::string(to));
            transition.create(&facade).unwrap();
        }
        let read = || {
            facade
                .find(entity)
                .unwrap()
                .eq("systemMessageId", Value::string("M1"))
                .use_cache(false)
                .one()
                .unwrap()
                .unwrap()
        };
        // A caller read the row, then another changed its status.
        let mut stale = read();
        let mut other = read();
        other.set("statusId", Value::string("SmsgConsumed"));
        other.update(&facade).unwrap();
        let params = BTreeMap::from([
            ("systemMessageId".to_string(), Value::string("M1")),
            ("statusId".to_string(), Value::string("SmsgConfirmed")),
        ]);
        let out = super::sync_checked(&format!("update#{entity}"), &params, &super::CallOptions::default()).unwrap();
        assert_eq!(out.get("oldStatusId"), Some(&Value::string("SmsgConsumed")), "{out:?}");
        assert_eq!(out.get("statusChanged"), Some(&Value::Bool(true)), "{out:?}");
        let stored = read();
        assert_eq!(stored.get("statusId"), Some(&Value::string("SmsgConfirmed")));
        assert_eq!(stored.get("messageText"), Some(&Value::string("keep")));
        // The stale value changes another field; the status it read is not written back.
        stale.set("messageText", Value::string("stale"));
        stale.update(&facade).unwrap();
        assert_eq!(read().get("statusId"), Some(&Value::string("SmsgConfirmed")));
        assert_eq!(read().get("messageText"), Some(&Value::string("stale")));
        let back = BTreeMap::from([
            ("systemMessageId".to_string(), Value::string("M1")),
            ("statusId".to_string(), Value::string("SmsgReceived")),
        ]);
        let err = super::sync_checked(&format!("update#{entity}"), &back, &super::CallOptions::default()).unwrap_err();
        assert!(err.to_string().contains("Status change not allowed"), "{err}");
        assert_eq!(read().get("statusId"), Some(&Value::string("SmsgConfirmed")));
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
        let out = sync("spin#Forever", &BTreeMap::new()).expect("the interruption is recorded");
        assert!(out.is_empty(), "{out:?}");
        assert!(
            !ctx.message().errors().is_empty(),
            "the interruption is an error message"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
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

    /// As Java's, `parameters` and `results` in a rule are the call's own maps: a pre-* rule changes what is validated
    /// and run, and a post-service rule what is returned.
    #[test]
    fn seca_rules_change_the_call_parameters_and_results() {
        let path = write_fixture(
            "seca-live",
            r#"<services>
                <service verb="echo" noun="Live">
                    <in-parameters><parameter name="note" required="true"/></in-parameters>
                    <out-parameters><parameter name="seen"/></out-parameters>
                    <actions><set field="seen" from="note"/></actions>
                </service>
            </services>"#,
        );
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        super::super::eca_rules::install(
            ctx.factory(),
            super::super::eca_rules::from_xml(
                r#"<secas>
                    <seca service="echo#Live" when="pre-validate">
                        <actions><set field="parameters.note" value="pv"/></actions>
                    </seca>
                    <seca service="echo#Live" when="pre-auth">
                        <actions><script>parameters.note = parameters.note + "-pa"</script></actions>
                    </seca>
                    <seca service="echo#Live" when="pre-service">
                        <actions><script>parameters.note = parameters.note + "-ps"</script></actions>
                    </seca>
                    <seca service="echo#Live" when="post-service">
                        <actions><script>results.seen = results.seen + "-post"</script></actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        ctx.factory().set_entity(facade);
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        // `note` is required and not passed: the pre-validate rule sets it.
        let out = sync("echo#Live", &BTreeMap::new()).unwrap();
        assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
        assert_eq!(
            out.get("seen").and_then(Value::as_str),
            Some("pv-pa-ps-post"),
            "{out:?}"
        );
    }

    /// As Java's `runImplicitEntityAuto`: an implicit entity-auto call runs its pre-validate rules twice and no pre-auth
    /// rules.
    #[test]
    fn an_implicit_entity_auto_call_runs_pre_validate_twice_and_no_pre_auth() {
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        super::super::eca_rules::install(
            ctx.factory(),
            super::super::eca_rules::from_xml(
                r#"<secas>
                    <seca service="create#moqui.test.TestEntity" when="pre-validate">
                        <actions><script>ec.message.addMessage("pre-validate")</script></actions>
                    </seca>
                    <seca service="create#moqui.test.TestEntity" when="pre-auth">
                        <actions><script>ec.message.addMessage("pre-auth")</script></actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let mut params = BTreeMap::new();
        params.insert("testId".to_string(), Value::string("IMPLICIT"));
        sync("create#moqui.test.TestEntity", &params).unwrap();
        assert_eq!(ctx.message().messages(), ["pre-validate", "pre-validate"]);
    }

    /// As Java's, the tx-commit rules get the parameters and results from before the post-service rules.
    #[test]
    fn tx_commit_rules_get_the_results_from_before_the_post_service_rules() {
        let path = write_fixture(
            "tx-commit",
            r#"<services>
                <service verb="note" noun="Commit">
                    <out-parameters><parameter name="note"/></out-parameters>
                    <actions><set field="note" value="from-service"/></actions>
                </service>
            </services>"#,
        );
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        super::super::eca_rules::install(
            ctx.factory(),
            super::super::eca_rules::from_xml(
                r#"<secas>
                    <seca service="note#Commit" when="post-service">
                        <actions><script>results.note = "from-post-service"</script></actions>
                    </seca>
                    <seca service="note#Commit" when="tx-commit">
                        <actions>
                            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                            <script>made.set("testId", "COMMIT_RULE"); made.set("testMedium", results.note)</script>
                            <entity-create value-field="made"/>
                        </actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        let out = sync("note#Commit", &BTreeMap::new()).unwrap();
        assert_eq!(out.get("note").and_then(Value::as_str), Some("from-post-service"));
        let row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("COMMIT_RULE"))
            .one()
            .unwrap()
            .expect("the tx-commit rule ran");
        assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("from-service"));
    }

    /// As Java's, the tx-rollback rules are registered after the service ran, with its results.
    #[test]
    fn tx_rollback_rules_get_the_results() {
        let path = write_fixture(
            "tx-rollback",
            r#"<services>
                <service verb="fail" noun="Late">
                    <out-parameters><parameter name="note"/></out-parameters>
                    <actions>
                        <set field="note" value="before-error"/>
                        <message error="true">late failure</message>
                    </actions>
                </service>
            </services>"#,
        );
        let facade = crate::testing::open();
        let ctx = ExecutionContextFactory::test_context();
        super::super::eca_rules::install(
            ctx.factory(),
            super::super::eca_rules::from_xml(
                r#"<secas>
                    <seca service="fail#Late" when="tx-rollback">
                        <actions>
                            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                            <script>made.set("testId", "ROLLBACK_RULE"); made.set("testMedium", results.note)</script>
                            <entity-create value-field="made"/>
                        </actions>
                    </seca>
                </secas>"#,
                "",
            ),
        );
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let _authz = ctx.disable_authz();
        sync("fail#Late", &BTreeMap::new()).unwrap();
        assert_eq!(ctx.message().errors(), ["late failure"]);
        let row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("ROLLBACK_RULE"))
            .one()
            .unwrap()
            .expect("the tx-rollback rule ran");
        assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("before-error"));
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
        assert!(sync("ping#Remote", &BTreeMap::new()).unwrap().is_empty());
        let text = ctx.message().errors().join("\n");
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

    /// Java's `ServiceDefinition` merge: the implemented service's parameters first, then the service's own merged
    /// into them by name, and an `auto-parameters` field and a parameter of the same name are one parameter, whose
    /// attributes the later one sets. An empty `default-value` is no default. A `<description>` of the later one
    /// replaces the earlier one's, and none keeps it.
    #[test]
    fn implemented_and_auto_parameters_merge_by_name() {
        let path = write_fixture(
            "merge",
            r#"<services>
                <service verb="define" noun="Row" type="interface">
                    <in-parameters>
                        <parameter name="testId" required="true"><description>Inherited id</description></parameter>
                        <parameter name="note" default-value="n"><description>Inherited note</description></parameter>
                        <parameter name="testMedium"><description>Inherited medium</description></parameter>
                    </in-parameters>
                </service>
                <service verb="save" noun="Row">
                    <implements service="define#Row"/>
                    <in-parameters>
                        <auto-parameters entity-name="moqui.test.TestEntity" include="nonpk"/>
                        <parameter name="testDateTime" required="true"><description>Own stamp</description></parameter>
                        <parameter name="testId"><text-length max="5"/></parameter>
                        <parameter name="note" default-value=""><description> Own note </description></parameter>
                    </in-parameters>
                    <actions><set field="x" value="1"/></actions>
                </service>
            </services>"#,
        );
        let (facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let def = crate::service::define::find("save#Row").expect("registered");
        let declared = super::validate::declared_in(&def, Some(&facade)).unwrap();
        let names: Vec<&str> = declared.iter().map(|param| param.name.as_str()).collect();
        assert_eq!(&names[..3], ["testId", "note", "testMedium"], "{names:?}");
        assert_eq!(
            names.iter().filter(|name| **name == "testDateTime").count(),
            1,
            "{names:?}"
        );
        let stamp = declared.iter().find(|param| param.name == "testDateTime").unwrap();
        assert_eq!(stamp.java_type, "java.sql.Timestamp");
        assert_eq!(stamp.required, super::super::define::Required::True);
        assert_eq!(stamp.field_name.as_deref(), Some("testDateTime"));
        let id = declared.iter().find(|param| param.name == "testId").unwrap();
        assert_eq!(
            (id.required, id.checks.len()),
            (super::super::define::Required::True, 1)
        );
        assert_eq!(
            declared
                .iter()
                .find(|param| param.name == "note")
                .unwrap()
                .default_value,
            None
        );
        let description = |name: &str| {
            declared
                .iter()
                .find(|param| param.name == name)
                .and_then(|param| param.description.as_deref())
        };
        assert_eq!(description("testId"), Some("Inherited id"), "none of its own keeps it");
        assert_eq!(description("note"), Some("Own note"), "its own replaces it");
        assert_eq!(
            description("testMedium"),
            Some("Inherited medium"),
            "an auto field has none"
        );
        assert_eq!(description("testDateTime"), Some("Own stamp"), "over an auto field");
        assert_eq!(description("testLong"), None);

        let params = |pairs: &[(&str, &str)]| -> BTreeMap<String, Value> {
            pairs
                .iter()
                .map(|(name, value)| (name.to_string(), Value::string(*value)))
                .collect()
        };
        let cleaned = crate::service::validate(
            "save#Row",
            &params(&[("testId", "R1"), ("testDateTime", "2024-01-02 03:04:05")]),
        )
        .unwrap();
        assert!(
            matches!(cleaned.get("testDateTime"), Some(Value::Timestamp(_))),
            "{cleaned:?}"
        );
        assert!(!cleaned.contains_key("note"), "{cleaned:?}");
        let err = crate::service::validate("save#Row", &params(&[("testId", "ABCDEFG")])).unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("Value entered (ABCDEFG), length 7, is longer than 5 characters."),
            "{text}"
        );
        assert!(
            text.contains("Field cannot be empty (for field testDateTime)"),
            "{text}"
        );
    }

    /// A declared entity-auto service gets only its declared parameters, as any service; an implicit one gets them
    /// as they came, and the runner sets only the entity's fields.
    #[test]
    fn entity_auto_parameters_are_the_declared_ones_or_all_for_an_implicit_call() {
        let path = write_fixture(
            "entity-auto-declared",
            r#"<services>
                <service verb="create" noun="moqui.test.TestEntity" type="entity-auto">
                    <in-parameters><auto-parameters include="nonpk"/></in-parameters>
                    <out-parameters><parameter name="testId"/></out-parameters>
                </service>
            </services>"#,
        );
        let (facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        crate::service::lint(std::slice::from_ref(&path)).unwrap();
        let mut params = BTreeMap::new();
        params.insert("testId".to_string(), Value::string("MINE"));
        params.insert("testMedium".to_string(), Value::string("declared"));
        let created = sync("create#moqui.test.TestEntity", &params).unwrap();
        let id = created
            .get("testId")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        assert!(
            !id.is_empty() && id != "MINE",
            "the primary key is not a declared parameter: {created:?}"
        );

        let implicit =
            super::super::define::ServiceDef::implicit("store#moqui.test.TestEntity", "store", "moqui.test.TestEntity");
        params.insert("notAField".to_string(), Value::string("x"));
        let cleaned = super::validate::clean(&implicit, &params, Some(&facade)).unwrap();
        assert_eq!(cleaned, params);
        sync("store#moqui.test.TestEntity", &params).unwrap();
        let row = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("MINE"))
            .one()
            .unwrap()
            .expect("stored");
        assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("declared"));
    }

    /// Java's `StatusFlowTransitionNotFoundTemplate`, its default text when the seed data is not loaded.
    #[test]
    fn a_status_change_not_allowed_names_the_statuses() {
        let values = |old_name: Value| {
            [
                ("fullEntityName", Value::string("moqui.test.TestEntity")),
                ("lookedUpStatusId", Value::string("TsOpen")),
                ("parameterStatusId", Value::string("TsDone")),
                ("lookedUpStatusName", old_name),
                ("parameterStatusName", Value::string("Done")),
            ]
        };
        assert_eq!(
            super::status_transition_message(&values(Value::string("Open"))),
            "Status change not allowed from Open to Done"
        );
        assert_eq!(
            super::status_transition_message(&values(Value::Null)),
            "Status change not allowed from TsOpen to Done"
        );
    }

    fn write_fixture(name: &str, body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("moqui-svc-{name}-{}.xml", std::process::id()));
        std::fs::write(&path, body).unwrap();
        path
    }
}
