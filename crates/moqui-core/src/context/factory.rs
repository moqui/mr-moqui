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

//! Execution context factory. One factory is shared. One context is thread-local.

use std::cell::{RefCell, RefMut};
use std::fs::OpenOptions;
use std::io::Write;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use moqui_lib::env;
use moqui_lib::node::MNode;
use moqui_lib::sync::{lock, read, write};

use super::artifact::{ArtifactStack, AuthzGuard};

thread_local! {
    /// The entered contexts, innermost last, one entry per live guard. Each entry is a handle that
    /// shares the context's state, so a leaked guard keeps that state alive and never dangles.
    static ENTERED: RefCell<Vec<ExecutionContext>> = const { RefCell::new(Vec::new()) };
}
use super::cache::CacheFacade;
use super::component::{self, ComponentInfo};
use super::conf;
use super::confcache::ConfCache;
use super::error::Error;
use super::message::MessageFacade;
use super::resource::ResourceFacade;
use crate::entity::EntityFacade;
use crate::user::AuthInfo;

const ENV_HEADER: &str = "# This file holds secrets and must be kept with the instance backup.\n";

/// Tarpit hit times and how long to keep them, by key, swept of idle keys as the map grows.
struct TarpitHits {
    by_key: std::collections::HashMap<String, (Vec<i64>, i64)>,
    /// The size at which the next record sweeps, so a sweep costs O(1) per record over time.
    sweep_at: usize,
}

impl TarpitHits {
    const MIN_SWEEP_AT: usize = 1024;
}

impl Default for TarpitHits {
    fn default() -> Self {
        Self {
            by_key: std::collections::HashMap::new(),
            sweep_at: Self::MIN_SWEEP_AT,
        }
    }
}

pub struct ExecutionContextFactory {
    runtime_dir: PathBuf,
    framework_dir: PathBuf,
    components: Vec<ComponentInfo>,
    resource: ResourceFacade,
    cache: CacheFacade,
    conf_text: String,
    conf_cache: std::sync::OnceLock<ConfCache>,
    /// Hit times per `userId@type:name`, for tarpit counting.
    tarpit_hits: std::sync::Mutex<TarpitHits>,
    property_names: Vec<String>,
    extensions:
        std::sync::Mutex<std::collections::HashMap<std::any::TypeId, std::sync::Arc<dyn std::any::Any + Send + Sync>>>,
    /// Entity facade for `dbresource://`. Empty until the host calls [`Self::set_entity`].
    entity: Arc<RwLock<Option<EntityFacade>>>,
    /// This factory's pool guard. Drops with the factory. Idle has no workers.
    pools: crate::service::ServicePools,
    /// Idle until [`Self::start_request_pool`]. Drop joins the threads.
    requests: crate::service::RequestPool,
    executor: Executor,
    pub(crate) sessions: std::sync::Mutex<crate::user::session::SessionTable>,
    /// True after this factory's cluster heartbeat hook is registered.
    pub(crate) cluster_heartbeat: AtomicBool,
    /// Artifact hit counts and bins, written by the scheduler tick.
    hits: super::hits::ArtifactHits,
    /// When the factory booted, in epoch milliseconds, for the status of the tools dashboard.
    started_millis: i64,
    native: crate::native::NativeRegistry,
    pub(crate) native_tools: crate::native::ToolSet,
}

impl Drop for ExecutionContextFactory {
    fn drop(&mut self) {
        self.native_tools.shutdown();
    }
}

/// How async service calls, `callFuture()`, and scheduled jobs run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Executor {
    /// The service worker pool and the scheduler, on threads. The server.
    Threads,
    /// No threads. Async work runs on the calling thread at a drain point with no transaction in
    /// place, `callFuture()` runs at once, and the scheduler does not run. The browser build
    /// (`plans/archive/Wasm.md`). See [`ExecutionContextFactory::run_pending`].
    Inline,
}

impl Default for Executor {
    /// `Threads`. Test builds read `MOQUI_EXECUTOR=inline`, so the suite can run once inline.
    fn default() -> Self {
        #[cfg(test)]
        if std::env::var("MOQUI_EXECUTOR").is_ok_and(|value| value.eq_ignore_ascii_case("inline")) {
            return Self::Inline;
        }
        Self::Threads
    }
}

/// Options for [`ExecutionContextFactory::boot_with_options`].
#[derive(Clone, Copy, Debug, Default)]
pub struct BootOptions {
    pub executor: Executor,
}

/// The framework component beside `tools` and `webroot`, when `MOQUI_FRAMEWORK` is not set.
#[must_use]
pub fn default_framework_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("base-component/framework")
}

impl ExecutionContextFactory {
    pub fn boot(runtime_dir: impl AsRef<Path>) -> Result<Arc<Self>, Error> {
        let runtime_dir = canonicalize_dir(runtime_dir.as_ref())?;
        let framework_dir = canonicalize_dir(&default_framework_dir(&runtime_dir))?;
        if !framework_dir.join("component.xml").is_file() {
            return Err(Error::new(format!(
                "framework directory {} has no component.xml",
                framework_dir.display()
            )));
        }
        Self::boot_with_framework(runtime_dir, framework_dir)
    }

    pub fn boot_with_framework(
        runtime_dir: impl AsRef<Path>,
        framework_dir: impl AsRef<Path>,
    ) -> Result<Arc<Self>, Error> {
        Self::boot_with_options(runtime_dir, framework_dir, BootOptions::default())
    }

    /// [`Self::boot_with_framework`] with an executor choice. The browser build passes
    /// [`Executor::Inline`], which starts no thread (`plans/archive/Wasm.md`).
    pub fn boot_with_options(
        runtime_dir: impl AsRef<Path>,
        framework_dir: impl AsRef<Path>,
        options: BootOptions,
    ) -> Result<Arc<Self>, Error> {
        assert_shareable::<Self>();
        let boot_started = std::time::Instant::now();
        let runtime_dir = canonicalize_dir(runtime_dir.as_ref())?;
        let framework_dir = canonicalize_dir(framework_dir.as_ref())?;
        tracing::info!(
            "Initializing Moqui Framework version {}\n - runtime directory: {}\n - framework directory: {}",
            env!("CARGO_PKG_VERSION"),
            runtime_dir.display(),
            framework_dir.display()
        );
        if !framework_dir.join("component.xml").is_file() {
            return Err(Error::new(format!(
                "framework directory {} has no component.xml",
                framework_dir.display()
            )));
        }
        let phase = std::time::Instant::now();
        let conf_node = conf::load_merged_conf(&framework_dir, &runtime_dir)?;
        tracing::info!("Merged configuration in {}ms", phase.elapsed().as_millis());
        let phase = std::time::Instant::now();
        let components = component::discover(&runtime_dir, &framework_dir, &conf_node)?;
        tracing::info!(
            "Discovered {} components in {}ms",
            components.len(),
            phase.elapsed().as_millis()
        );
        let component_dirs: Vec<_> = components.iter().map(|info| info.location.clone()).collect();
        let conf_node =
            conf::merge_component_confs(&framework_dir, &runtime_dir, &component_dirs)?.unwrap_or(conf_node);
        let native = crate::native::NativeRegistry::discover(&components, &conf_node)?;
        let env_path = runtime_dir.join(".env");
        let (property_names, mut set_by_boot) = apply_default_properties(&conf_node, &env_path)?;
        let entity = Arc::new(RwLock::new(None));
        let resource = ResourceFacade::new(runtime_dir.clone(), component_roots(&components), Arc::clone(&entity));
        tracing::info!("Resource Facade initialized");
        let cache = CacheFacade::from_conf(&conf_node)?;
        tracing::info!("Cache Facade initialized");
        let conf_text = conf_node.write_string();
        let hits = super::hits::ArtifactHits::from_conf(&conf_text);
        let flush_mode = crate::user::session::flush_mode_from_property()?;
        let pools = match options.executor {
            Executor::Threads => crate::service::start_pools(&conf_text)?,
            Executor::Inline => crate::service::ServicePools::inline(),
        };
        super::locate::remember(&framework_dir, &components);
        tracing::info!(
            runtime = %runtime_dir.display(),
            framework = %framework_dir.display(),
            "Execution Context Factory initialized in {:.3} seconds",
            boot_started.elapsed().as_secs_f64()
        );
        let factory = Arc::new(Self {
            runtime_dir,
            framework_dir,
            components,
            resource,
            cache,
            conf_text,
            conf_cache: std::sync::OnceLock::new(),
            tarpit_hits: std::sync::Mutex::default(),
            property_names,
            extensions: std::sync::Mutex::new(std::collections::HashMap::new()),
            entity,
            pools,
            requests: crate::service::RequestPool::idle(),
            executor: options.executor,
            sessions: std::sync::Mutex::new(crate::user::session::SessionTable::new(flush_mode)),
            cluster_heartbeat: AtomicBool::new(false),
            hits,
            started_millis: jiff::Timestamp::now().as_millisecond(),
            native,
            native_tools: crate::native::ToolSet::default(),
        });
        factory.native.configure(&factory, &conf_node)?;
        set_by_boot.keep();
        factory.pools.attach(&factory);
        // The host calls cluster::start after the entity facade is installed.
        install_session_flush(&factory);
        install_hit_flush(&factory);
        Ok(factory)
    }

    /// Start the request workers from `tools.@request-pool-max` and `tools.@request-queue`.
    ///
    /// # Errors
    ///
    /// Returns an error when a worker thread does not start.
    ///
    /// With [`Executor::Inline`] this does nothing: the pool stays idle and the host runs each
    /// request on the calling thread.
    pub fn start_request_pool(&self) -> Result<(), Error> {
        if self.executor == Executor::Inline {
            return Ok(());
        }
        self.requests.start_from_conf(&self.conf_text)
    }

    /// The executor this factory booted with.
    pub fn executor(&self) -> Executor {
        self.executor
    }

    /// Run queued async work now, on this thread. Only [`Executor::Inline`] queues work; the
    /// threaded executor returns 0. The browser host calls this after each top-level request.
    /// Leaving the outermost execution context on a thread drains too.
    ///
    /// Runs nothing while a transaction is in place on this thread. Each item gets a fresh context
    /// and acts as the user who queued it. Returns how many ran.
    pub fn run_pending(&self) -> usize {
        self.pools.run_pending()
    }

    /// Java's `waitWorkerPoolEmpty(retryLimit)`: wait up to `retry_limit` times 100 ms for the async service work to
    /// finish. Queued inline work runs here. Returns whether the pool is empty.
    pub fn wait_worker_pool_empty(&self, retry_limit: u32) -> bool {
        for attempt in 0..=retry_limit {
            self.pools.run_pending();
            let status = self.pools.status();
            if status.active == 0 && status.queued == 0 && self.pools.pending() == 0 {
                return true;
            }
            if attempt < retry_limit {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        false
    }

    /// Write dirty `moqui.server.HttpSession` rows. The host calls this on shutdown so an
    /// `eventual` session is on disk before the entity facade drops.
    ///
    /// # Errors
    ///
    /// Returns an error when a session row cannot be written.
    pub fn flush_sessions(&self) -> Result<(), Error> {
        crate::user::session::flush_dirty(self)
    }

    /// Write the waiting artifact hits and every hit bin, including the bins still open. The host
    /// calls this on shutdown, as Java's `destroy` writes the bins.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be written.
    pub fn flush_artifact_hits(&self) -> Result<(), Error> {
        super::hits::flush(self, true)
    }

    pub(crate) fn artifact_hits(&self) -> &super::hits::ArtifactHits {
        &self.hits
    }

    /// The factory request pool. Idle until [`Self::start_request_pool`].
    pub fn request_pool(&self) -> &crate::service::RequestPool {
        &self.requests
    }

    /// This factory's service worker pool and scheduler.
    pub fn service_pools(&self) -> &crate::service::ServicePools {
        &self.pools
    }

    pub fn new_context(self: &Arc<Self>) -> ExecutionContext {
        ExecutionContext {
            state: Rc::new(ContextState {
                factory: Arc::clone(self),
                messages: RefCell::new(MessageFacade::new()),
                artifacts: RefCell::new(ArtifactStack::new()),
                session: RefCell::new(crate::user::Memory::default()),
                user_context: RefCell::new(std::collections::BTreeMap::new()),
                shared_map: RefCell::new(indexmap::IndexMap::new()),
                effective_time: std::cell::Cell::new(None),
                script_authz: RefCell::new(None),
            }),
        }
    }

    /// When the factory booted, in epoch milliseconds.
    pub fn started_millis(&self) -> i64 {
        self.started_millis
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    pub fn framework_dir(&self) -> &Path {
        &self.framework_dir
    }

    pub fn components(&self) -> &[ComponentInfo] {
        &self.components
    }

    /// Linked implementations; XML still defines services and bindings.
    pub(crate) fn native(&self) -> &crate::native::NativeRegistry {
        &self.native
    }

    /// Initialize configured tools once, after installing the required facades.
    pub fn init_tools(self: &Arc<Self>) -> Result<(), Error> {
        let context = self.new_context();
        let _entered = context.enter();
        self.native_tools.init(self)
    }

    /// Stop request/service workers before closing tools. The host calls this after its listeners stop.
    pub fn shutdown(&self) {
        self.requests.shutdown();
        self.pools.shutdown();
        self.shutdown_tools();
    }

    /// Stop tools in reverse order. Call only after their callers have stopped.
    pub fn shutdown_tools(&self) {
        self.native_tools.shutdown();
    }

    pub fn get_tool_factory(&self, name: &str) -> Option<Arc<dyn crate::native::ToolFactory>> {
        self.native_tools.get(name)
    }

    pub fn get_tool(
        &self,
        name: &str,
        parameters: &[moqui_lib::object::Value],
    ) -> Result<Arc<dyn crate::native::Tool>, Error> {
        self.get_tool_factory(name)
            .ok_or_else(|| Error::new(format!("no tool factory {name}")))?
            .get_instance(parameters)
    }

    pub fn get_tool_typed<T: crate::native::Tool>(
        &self,
        name: &str,
        parameters: &[moqui_lib::object::Value],
    ) -> Result<Arc<T>, Error> {
        let instance: Arc<dyn std::any::Any + Send + Sync> = self.get_tool(name, parameters)?;
        instance
            .downcast::<T>()
            .map_err(|_| Error::new(format!("tool {name} has a different instance type")))
    }

    /// The tool factory named `name` as its own type, for Rust code that wants more than its instances. None when there
    /// is none or it is another type.
    pub fn get_tool_factory_typed<T: crate::native::ToolFactory>(&self, name: &str) -> Option<Arc<T>> {
        let factory: Arc<dyn std::any::Any + Send + Sync> = self.get_tool_factory(name)?;
        factory.downcast::<T>().ok()
    }

    /// The tool factories that give a TypeScript type for their instance, as (name, type), for `moqui types`.
    pub fn tool_type_declarations(&self) -> Vec<(String, &'static str)> {
        self.native_tools.type_declarations()
    }

    /// Bind a `type="rust"` service location to a closure for this factory, beside the linked catalogue's functions. For
    /// an implementation that keeps state, such as a tool factory's: call it from `ToolFactory::pre_facade_init`.
    /// [`crate::service_location!`] names a function by its Rust path.
    ///
    /// # Errors
    ///
    /// Returns an error when the location is empty, already bound (a built-in or catalogue function included), or when
    /// this factory has already read service definitions, which resolve their location as they are read.
    pub fn register_service(
        &self,
        location: &str,
        run: impl Fn(
                &ExecutionContext,
                &crate::native::ServiceInfo<'_>,
                &crate::native::Parameters,
            ) -> Result<crate::native::Parameters, moqui_lib::error::Error>
            + Send
            + Sync
            + 'static,
    ) -> Result<(), Error> {
        if crate::service::define::definitions_read(self) {
            return Err(Error::new(format!(
                "rust service {location} is registered after the service definitions were read; register it in pre_facade_init"
            )));
        }
        self.native.register_service(location, Arc::new(run))
    }

    pub fn resource(&self) -> &ResourceFacade {
        &self.resource
    }

    pub fn cache(&self) -> &CacheFacade {
        &self.cache
    }

    pub fn property_names(&self) -> &[String] {
        &self.property_names
    }

    /// State another facade keeps per factory, made on first use. `init` may use the factory but
    /// not ask for another extension of its own type.
    pub fn extension<T: std::any::Any + Send + Sync>(&self, init: impl FnOnce(&Self) -> T) -> Arc<T> {
        let key = std::any::TypeId::of::<T>();
        let existing = lock(&self.extensions).get(&key).cloned();
        let value: Arc<dyn std::any::Any + Send + Sync> = match existing {
            Some(value) => value,
            None => {
                // Not under the lock, so init can read other extensions.
                let made: Arc<dyn std::any::Any + Send + Sync> = Arc::new(init(self));
                let mut map = lock(&self.extensions);
                Arc::clone(map.entry(key).or_insert(made))
            }
        };
        match value.downcast::<T>() {
            Ok(typed) => typed,
            Err(_) => unreachable!("an extension is stored under its own type id"),
        }
    }

    /// True when the `artifact-execution` element for `artifact_type` has `flag="true"`.
    pub fn artifact_flag(&self, artifact_type: &str, flag: &str) -> bool {
        self.conf().artifact_flag(artifact_type, flag)
    }

    /// A non-empty `user-facade` child attribute from the conf.
    pub fn user_facade_attr(&self, child: &str, name: &str) -> Option<String> {
        self.conf().user_facade_attr(child, name)
    }

    /// The root URL of a conf `webapp` for a render with no request: Java's `makeWebappHost` with no
    /// web facade. `https://` and the https host and port when `https-enabled` is `true`, else `http://` and
    /// the http ones; an empty host is `localhost`, and the default port is left out. Empty when the
    /// conf has no such webapp.
    pub fn webapp_root_url(&self, webapp_name: &str) -> String {
        let conf = self.conf();
        let attr = |name: &str| {
            conf.webapp_attr(webapp_name, name)
                .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        if conf.webapp_attr(webapp_name, "name").is_none() {
            return String::new();
        }
        let https = attr("https-enabled") == "true";
        let (scheme, host, port, default_port) = if https {
            ("https", attr("https-host"), attr("https-port"), "443")
        } else {
            ("http", attr("http-host"), attr("http-port"), "80")
        };
        let host = if host.is_empty() { "localhost".to_string() } else { host };
        if port.is_empty() || port == default_port {
            format!("{scheme}://{host}")
        } else {
            format!("{scheme}://{host}:{port}")
        }
    }

    /// The https host and port of a conf `webapp` whose `https-enabled` is `true`, either empty when the conf leaves
    /// it out. None when https is not enabled.
    pub fn webapp_https(&self, webapp_name: &str) -> Option<(String, String)> {
        let conf = self.conf();
        let attr = |name: &str| {
            conf.webapp_attr(webapp_name, name)
                .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        (attr("https-enabled") == "true").then(|| (attr("https-host"), attr("https-port")))
    }

    fn conf(&self) -> &ConfCache {
        self.conf_cache.get_or_init(|| ConfCache::parse(&self.conf_text))
    }

    /// Hit times recorded for a tarpit key, oldest first.
    pub fn tarpit_times(&self, key: &str) -> Vec<i64> {
        lock(&self.tarpit_hits)
            .by_key
            .get(key)
            .map(|(times, _)| times.clone())
            .unwrap_or_default()
    }

    /// Record a hit. Times older than `keep_ms` are dropped so the list stays small. Once the map has
    /// doubled since the last sweep, keys with no hit within their own `keep_ms` are dropped too.
    pub fn record_tarpit_hit(&self, key: &str, now_ms: i64, keep_ms: i64) {
        let keep_ms = keep_ms.max(1);
        let mut hits = lock(&self.tarpit_hits);
        let (list, keep) = hits.by_key.entry(key.to_string()).or_default();
        list.retain(|time| now_ms - *time < keep_ms);
        list.push(now_ms);
        *keep = keep_ms;
        if hits.by_key.len() >= hits.sweep_at {
            hits.by_key
                .retain(|_, (times, keep)| times.last().is_some_and(|last| now_ms - *last < *keep));
            hits.sweep_at = hits.by_key.len().saturating_mul(2).max(TarpitHits::MIN_SWEEP_AT);
        }
    }

    pub fn clear_tarpit_hits(&self, key: &str) {
        lock(&self.tarpit_hits).by_key.remove(key);
    }

    /// The number of keys with recorded tarpit hits, for tests.
    #[cfg(test)]
    pub(crate) fn tarpit_key_count(&self) -> usize {
        lock(&self.tarpit_hits).by_key.len()
    }

    pub fn conf_text(&self) -> &str {
        &self.conf_text
    }

    /// Install the entity facade. `dbresource://` reads and writes through it.
    pub fn set_entity(&self, facade: EntityFacade) {
        *write(&self.entity) = Some(facade);
    }

    /// The installed entity facade, when the host has registered one.
    pub fn entity(&self) -> Option<EntityFacade> {
        read(&self.entity).clone()
    }
}

/// One thread's execution context. It is not `Send`: it stays on the thread that made it.
pub struct ExecutionContext {
    /// Shared with the thread-local slot while entered, see [`ExecutionContext::enter`].
    state: Rc<ContextState>,
}

struct ContextState {
    factory: Arc<ExecutionContextFactory>,
    messages: RefCell<MessageFacade>,
    artifacts: RefCell<ArtifactStack>,
    session: RefCell<crate::user::Memory>,
    /// `ec.user.context`: values for this execution context's user, such as the organizations `setup#UserOrganizationInfo`
    /// finds, that entity filter maps read.
    user_context: RefCell<std::collections::BTreeMap<String, moqui_lib::object::Value>>,
    /// `ec.context.sharedMap`: one map for the execution context, shared by every service call and script in it.
    shared_map: RefCell<indexmap::IndexMap<String, moqui_lib::object::Value>>,
    /// Java's `UserFacade.setEffectiveTime`: epoch millis that `ec.user.nowTimestamp` returns instead of the clock.
    effective_time: std::cell::Cell<Option<i64>>,
    /// The guard of a script's `ec.artifactExecution.disableAuthz()`, held until `enableAuthz()` or the context ends.
    script_authz: RefCell<Option<AuthzGuard>>,
}

impl ExecutionContext {
    /// Java's `ec.user.setEffectiveTime`: from now on `ec.user.nowTimestamp`, date filters with no date, and the
    /// defaults Java takes from it (entity-auto `fromDate`, audit log `changedDate`) use `millis`. `None` goes back to
    /// the clock. Timing, login and job scheduling keep the clock.
    pub fn set_effective_time(&self, millis: Option<i64>) {
        self.state.effective_time.set(millis);
    }

    /// The effective time set on this context, if any.
    pub fn effective_time(&self) -> Option<i64> {
        self.state.effective_time.get()
    }

    pub fn factory(&self) -> &Arc<ExecutionContextFactory> {
        &self.state.factory
    }

    pub(crate) fn session(&self) -> RefMut<'_, crate::user::Memory> {
        self.state.session.borrow_mut()
    }

    /// The `ec.user.context` map.
    pub fn user_context(&self) -> RefMut<'_, std::collections::BTreeMap<String, moqui_lib::object::Value>> {
        self.state.user_context.borrow_mut()
    }

    /// Java's `ExecutionContext.getContext().getSharedMap()`: a map that lives as long as this execution context, whatever
    /// the context stack does, so a script can keep a re-entrancy guard in it.
    pub fn shared_map(&self) -> RefMut<'_, indexmap::IndexMap<String, moqui_lib::object::Value>> {
        self.state.shared_map.borrow_mut()
    }

    pub fn message(&self) -> RefMut<'_, MessageFacade> {
        self.state.messages.borrow_mut()
    }

    /// Push an artifact frame without any check, for a frame that is already authorized.
    pub fn push(&self, artifact_type: &str, name: &str, action: &str) {
        self.state
            .artifacts
            .borrow_mut()
            .push_with(artifact_type, name, action, AuthInfo::default());
    }

    /// Decide an artifact against the frame below it, then push it. A denied artifact is not pushed.
    ///
    /// `requires_authz` false pushes the frame whatever the records say, so that it can still
    /// inherit. `parameters` feed the `filterMap` of an authorization record.
    ///
    /// # Errors
    ///
    /// Returns an error when the user is not authorized, or a tarpit lock applies.
    pub fn push_checked(
        &self,
        artifact_type: &str,
        name: &str,
        action: &str,
        requires_authz: bool,
        count_tarpit: bool,
        parameters: Option<&std::collections::BTreeMap<String, moqui_lib::object::Value>>,
    ) -> Result<(), Error> {
        let (last, ancestor_deny, authz_disabled) = self.stack_authz_state();
        // The decision reads entities, which push their own frames, so no borrow is held here.
        let (permitted, info) = crate::user::is_permitted(
            artifact_type,
            name,
            action,
            &crate::user::Permit {
                requires_authz,
                authz_disabled,
                count_tarpit,
                parameters,
            },
            last.as_ref(),
            ancestor_deny,
        )?;
        if !permitted {
            tracing::warn!(stack = %self.state.artifacts.borrow().names(), "artifact authorization failed");
            return Err(crate::user::denied(artifact_type, name, action, &info));
        }
        self.state
            .artifacts
            .borrow_mut()
            .push_with(artifact_type, name, action, info);
        Ok(())
    }

    /// The frame below, any deny on the stack, and whether authorization is disabled.
    pub(crate) fn stack_authz_state(&self) -> (Option<AuthInfo>, bool, bool) {
        let artifacts = self.state.artifacts.borrow();
        let (last, deny) = artifacts.authz_state();
        (last, deny, artifacts.authz_disabled())
    }

    /// Auth info of the top frame.
    pub(crate) fn top_authz_info(&self) -> Option<AuthInfo> {
        self.state.artifacts.borrow().authz_state().0
    }

    /// Mark the top frame inheritable for an anonymous user: all actions, or view only.
    pub fn set_anonymous_authorized(&self, all_actions: bool, user_id: &str) {
        if let Some(info) = self.state.artifacts.borrow_mut().top_info_mut() {
            if all_actions {
                info.anonymous_all(user_id);
            } else {
                info.anonymous_view(user_id);
            }
        }
    }

    /// Pop the top frame. A tracked frame is counted as an artifact hit, in memory.
    pub fn pop(&self) -> Result<(), Error> {
        let popped = self.state.artifacts.borrow_mut().pop()?;
        if popped.hit.track {
            self.count_artifact_hit(&super::hits::Hit {
                artifact_type: &popped.artifact_type,
                sub_type: popped.hit.sub_type.as_deref(),
                name: &popped.name,
                parameters: popped.hit.parameters.as_ref(),
                start_ms: popped.started_ms,
                running_ms: popped.running_ms,
                output_size: None,
            });
        }
        Ok(())
    }

    /// Set the hit sub-type of the top frame, Java's `actionDetail`, and the parameters it records.
    /// The parameters are kept only when the type is counted.
    pub(crate) fn set_hit_detail(
        &self,
        sub_type: Option<&str>,
        parameters: Option<&std::collections::BTreeMap<String, moqui_lib::object::Value>>,
    ) {
        let artifact_type = self.state.artifacts.borrow().top_type();
        let keep = artifact_type.is_some_and(|name| self.state.factory.hits.counts(&name));
        if let Some(hit) = self.state.artifacts.borrow_mut().top_hit_mut() {
            hit.sub_type = sub_type.map(str::to_string);
            hit.parameters = parameters.filter(|_| keep).cloned();
        }
    }

    /// Do not count the top frame as a hit, Java's `setTrackArtifactHit(false)`.
    pub(crate) fn untrack_hit(&self) {
        if let Some(hit) = self.state.artifacts.borrow_mut().top_hit_mut() {
            hit.track = false;
        }
    }

    /// Java's `countArtifactHit`, for a hit that is not a frame of the stack, such as a transition.
    pub(crate) fn count_artifact_hit(&self, hit: &super::hits::Hit<'_>) {
        let flush_now = self.state.factory.hits.count(hit, || super::hits::HitContext {
            visit_id: crate::user::visit_id().ok().flatten(),
            user_id: crate::user::current_user_id().ok().flatten(),
            errors: self.state.messages.borrow().errors().to_vec(),
            request_url: crate::web::request_url(),
            referrer_url: crate::web::referrer_url(),
        });
        // The tick flush does not run during one long call, so a full queue is flushed here, on a thread.
        if flush_now {
            super::hits::spawn_flush(&self.state.factory);
        }
    }

    pub fn disable_authz(&self) -> AuthzGuard {
        self.state.artifacts.borrow().disable_authz()
    }

    /// Java's `ArtifactExecutionFacade.disableAuthz()` for a script, which has no guard to hold: authorization stays
    /// disabled until [`Self::script_enable_authz`] or the context ends. Returns whether it was already disabled.
    pub fn script_disable_authz(&self) -> bool {
        let already = self.authz_disabled();
        let mut held = self.state.script_authz.borrow_mut();
        if held.is_none() {
            *held = Some(self.disable_authz());
        }
        already
    }

    /// Java's `enableAuthz()` for a script: drops the guard of [`Self::script_disable_authz`]. A guard Rust code
    /// holds stays in force.
    pub fn script_enable_authz(&self) {
        let guard = self.state.script_authz.borrow_mut().take();
        drop(guard);
    }

    /// Java's `disableEntityEca`: entity ECA rules do not run for writes of this context until
    /// [`Self::enable_entity_eca`]. Returns whether they were already disabled; a caller that found them disabled
    /// leaves them so.
    pub fn disable_entity_eca(&self) -> bool {
        self.state.artifacts.borrow().disable_entity_eca()
    }

    pub fn enable_entity_eca(&self) {
        self.state.artifacts.borrow().enable_entity_eca();
    }

    pub fn entity_eca_disabled(&self) -> bool {
        self.state.artifacts.borrow().entity_eca_disabled()
    }

    /// Java's `getStackNameString`: the names on the artifact stack, top first.
    pub fn artifact_stack_names(&self) -> String {
        self.state.artifacts.borrow().name_string()
    }

    pub fn authz_disabled(&self) -> bool {
        self.state.artifacts.borrow().authz_disabled()
    }

    /// Make this context current on this thread until the guard drops.
    ///
    /// Entering while another context is current makes this one current, and dropping its guard
    /// brings the other back. [`Self::is_entered`] is true while any guard is alive. The slot holds
    /// a handle to this context's state, so a leaked guard keeps the state alive instead of
    /// leaving the slot dangling, and a guard dropped out of order removes only its own entry.
    pub fn enter(&self) -> ContextGuard<'_> {
        let handle = self.handle();
        let outermost = ENTERED.with(|slot| {
            let mut stack = slot.borrow_mut();
            stack.push(handle);
            stack.len() == 1
        });
        // Leaving the outermost context is an inline drain point.
        let drain = (outermost && self.factory().executor() == Executor::Inline).then(|| Arc::clone(self.factory()));
        ContextGuard {
            entered: Rc::clone(&self.state),
            drain,
            _scope: PhantomData,
        }
    }

    /// True while a context is entered on this thread.
    pub fn is_entered() -> bool {
        ENTERED.try_with(|slot| !slot.borrow().is_empty()).unwrap_or(false)
    }

    /// Run `body` with the entered context. The reference cannot outlive the callback.
    ///
    /// The body gets its own handle, so the context stays alive for the call even if the body
    /// enters or leaves other contexts.
    pub fn with_current<R>(body: impl FnOnce(&ExecutionContext) -> R) -> Option<R> {
        let current = ENTERED
            .try_with(|slot| slot.borrow().last().map(ExecutionContext::handle))
            .ok()
            .flatten()?;
        Some(body(&current))
    }

    /// Another handle to this context's state, for the thread-local slot.
    fn handle(&self) -> ExecutionContext {
        ExecutionContext {
            state: Rc::clone(&self.state),
        }
    }
}

/// Removes its context's entry from this thread's entered stack when dropped, bringing back the
/// context entered before it, or none.
///
/// A guard is not `Send`: it belongs to the thread whose stack it pushed.
#[must_use = "dropping the guard clears the thread-local execution context"]
pub struct ContextGuard<'a> {
    /// The state this guard entered: its entry is the topmost one sharing this state.
    entered: Rc<ContextState>,
    /// Set on an outermost guard of an inline factory: queued async work runs when it drops.
    drain: Option<Arc<ExecutionContextFactory>>,
    _scope: PhantomData<&'a ExecutionContext>,
}

impl Drop for ContextGuard<'_> {
    fn drop(&mut self) {
        // The removed handle drops after the slot borrow ends: dropping the last handle runs the
        // state's own drops, which may read the slot.
        let removed = ENTERED
            .try_with(|slot| {
                let mut stack = slot.borrow_mut();
                let index = stack
                    .iter()
                    .rposition(|context| Rc::ptr_eq(&context.state, &self.entered))?;
                if index + 1 != stack.len() {
                    tracing::warn!("execution context guard dropped out of order: a later guard is still alive");
                }
                Some(stack.remove(index))
            })
            .ok()
            .flatten();
        drop(removed);
        if let Some(factory) = self.drain.take() {
            if !std::thread::panicking() {
                factory.run_pending();
            }
        }
    }
}

fn assert_shareable<T: Send + Sync>() {}

fn install_session_flush(factory: &Arc<ExecutionContextFactory>) {
    let weak = Arc::downgrade(factory);
    factory.pools.add_tick_hook(move || {
        let Some(factory) = weak.upgrade() else {
            return;
        };
        if let Err(err) = crate::user::session::flush_dirty(&factory) {
            tracing::warn!("session flush failed: {err}");
        }
    });
}

/// The artifact hit flush is reserve-connection work on the scheduler tick, as the session flush.
fn install_hit_flush(factory: &Arc<ExecutionContextFactory>) {
    let weak = Arc::downgrade(factory);
    factory.pools.add_tick_hook(move || {
        let Some(factory) = weak.upgrade() else {
            return;
        };
        if let Err(err) = super::hits::flush(&factory, false) {
            tracing::warn!("artifact hit flush failed: {err}");
        }
    });
}

fn canonicalize_dir(path: &Path) -> Result<PathBuf, Error> {
    std::fs::canonicalize(path)
        .map_err(|err| Error::new(format!("could not resolve directory {}", path.display())).with_source(err))
}

fn component_roots(components: &[ComponentInfo]) -> std::collections::HashMap<String, PathBuf> {
    components
        .iter()
        .map(|info| (info.name.clone(), info.location.clone()))
        .collect()
}

fn apply_default_properties(conf_node: &MNode, env_path: &Path) -> Result<(Vec<String>, PropertyRollback), Error> {
    ensure_env_readable(env_path)?;
    if env_path.is_file() {
        env::load_dotenv_file(env_path)?;
    }
    let mut names = Vec::new();
    let mut set_by_boot = PropertyRollback::default();
    let mut generated = Vec::new();
    let mut cert_slot = None;
    let mut key_slot = None;
    for node in conf_node.children_named("default-property") {
        let Some(name) = node.attribute("name").filter(|name| !name.is_empty()) else {
            continue;
        };
        names.push(name.clone());
        if name == "cluster_tls_cert" || name == "cluster_tls_key" {
            let slot = read_tls_slot(&node, &name)?;
            if name == "cluster_tls_cert" {
                cert_slot = Some(slot);
            } else {
                key_slot = Some(slot);
            }
            continue;
        }
        let secret = is_secret(&name, node.attribute("is-secret").as_deref());
        let resolved = env::get_prop_or_env(&name);
        if !resolved.is_empty() {
            log_property(&name, &resolved, secret, false);
            continue;
        }
        let raw = node.attribute("value").unwrap_or_default();
        let mut value = if raw.is_empty() {
            String::new()
        } else {
            env::expand(&raw)?
        };
        if secret && value.ends_with("CHANGEME") && !leave_sentinel(&name) {
            value = generate_secret()?;
            env::set_property(&name, value.clone());
            set_by_boot.remember(&name);
            generated.push((name.clone(), value));
            tracing::info!("Generated secret property {name}");
            continue;
        }
        if value.is_empty() {
            continue;
        }
        env::set_property(&name, value.clone());
        set_by_boot.remember(&name);
        log_property(&name, &value, secret, true);
    }
    install_tls_pair(&mut generated, &mut set_by_boot, cert_slot, key_slot)?;
    append_env_secrets(env_path, &generated)?;
    Ok((names, set_by_boot))
}

struct TlsSlot {
    from_env: bool,
    value: String,
    secret: bool,
}

fn read_tls_slot(node: &MNode, name: &str) -> Result<TlsSlot, Error> {
    let secret = is_secret(name, node.attribute("is-secret").as_deref());
    let resolved = env::get_prop_or_env(name);
    if !resolved.is_empty() {
        return Ok(TlsSlot {
            from_env: true,
            value: resolved,
            secret,
        });
    }
    let raw = node.attribute("value").unwrap_or_default();
    let value = if raw.is_empty() {
        String::new()
    } else {
        env::expand(&raw)?
    };
    Ok(TlsSlot {
        from_env: false,
        value,
        secret,
    })
}

fn install_tls_pair(
    generated: &mut Vec<(String, String)>,
    set_by_boot: &mut PropertyRollback,
    cert: Option<TlsSlot>,
    key: Option<TlsSlot>,
) -> Result<(), Error> {
    let (cert, key) = match (cert, key) {
        (Some(cert), Some(key)) => (cert, key),
        (None, None) => return Ok(()),
        _ => return Err(Error::new("cluster_tls_cert and cluster_tls_key must be set together")),
    };
    let cert_placeholder = cert.value.ends_with("CHANGEME");
    let key_placeholder = key.value.ends_with("CHANGEME");
    // Without `cluster-tls` (the browser build) the pair stays unset, as if empty. Cluster start
    // then fails on the missing cert; nothing else reads it.
    #[cfg(not(feature = "cluster-tls"))]
    if cert_placeholder && key_placeholder {
        let _ = generated;
        return Ok(());
    }
    #[cfg(feature = "cluster-tls")]
    if cert_placeholder && key_placeholder {
        let (cert_b64, key_b64) = crate::cluster::generate_cert_pair()?;
        remember_generated(generated, set_by_boot, "cluster_tls_cert", cert_b64);
        remember_generated(generated, set_by_boot, "cluster_tls_key", key_b64);
        return Ok(());
    }
    if cert.value.is_empty() && key.value.is_empty() {
        return Ok(());
    }
    if cert_placeholder || key_placeholder || cert.value.is_empty() || key.value.is_empty() {
        return Err(Error::new("cluster_tls_cert and cluster_tls_key must be set together"));
    }
    apply_configured(set_by_boot, "cluster_tls_cert", &cert);
    apply_configured(set_by_boot, "cluster_tls_key", &key);
    Ok(())
}

#[cfg(feature = "cluster-tls")]
fn remember_generated(
    generated: &mut Vec<(String, String)>,
    set_by_boot: &mut PropertyRollback,
    name: &str,
    value: String,
) {
    env::set_property(name, value.clone());
    set_by_boot.remember(name);
    generated.push((name.to_string(), value));
    tracing::info!("Generated secret property {name}");
}

fn apply_configured(set_by_boot: &mut PropertyRollback, name: &str, slot: &TlsSlot) {
    log_property(name, &slot.value, slot.secret, !slot.from_env);
    if slot.from_env {
        return;
    }
    env::set_property(name, slot.value.clone());
    set_by_boot.remember(name);
}

fn log_property(name: &str, value: &str, secret: bool, from_default: bool) {
    if secret {
        if from_default {
            tracing::info!("Setting secret property {name} from default");
        } else {
            tracing::info!("Found secret property {name}");
        }
        return;
    }
    if from_default {
        tracing::info!("Setting property {name} from default with value [{value}]");
    } else {
        tracing::info!("Found property {name} with value [{value}]");
    }
}

fn is_secret(name: &str, flag: Option<&str>) -> bool {
    let flag = flag.unwrap_or("");
    flag != "false" && (flag == "true" || name.contains("pass") || name.contains("pw") || name.contains("key"))
}

fn leave_sentinel(name: &str) -> bool {
    name == "entity_ds_crypt_pass_old"
}

fn ensure_env_readable(path: &Path) -> Result<(), Error> {
    if path.exists() && !path.is_file() {
        Err(Error::new(format!(
            "could not read runtime environment file {}: path is a directory",
            path.display()
        )))
    } else {
        Ok(())
    }
}

fn generate_secret() -> Result<String, Error> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|err| Error::new(format!("could not read random bytes for a secret property: {err}")))?;
    Ok(STANDARD.encode(bytes))
}

fn append_env_secrets(path: &Path, pairs: &[(String, String)]) -> Result<(), Error> {
    if pairs.is_empty() {
        return Ok(());
    }
    if path.exists() && !path.is_file() {
        return Err(Error::new(format!(
            "could not append runtime environment file {}: path is a directory",
            path.display()
        )));
    }
    for (name, value) in pairs {
        if name.contains(['=', '\n', '\r']) || value.contains(['\n', '\r']) {
            return Err(Error::new(format!(
                "could not append runtime environment file {}: invalid secret name",
                path.display()
            )));
        }
    }
    let existing = if path.is_file() {
        std::fs::read_to_string(path).map_err(|err| {
            Error::new(format!("could not read runtime environment file {}", path.display())).with_source(err)
        })?
    } else {
        String::new()
    };
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|err| {
        Error::new(format!("could not append runtime environment file {}", path.display())).with_source(err)
    })?;
    if existing.is_empty() {
        file.write_all(ENV_HEADER.as_bytes()).map_err(|err| {
            Error::new(format!("could not append runtime environment file {}", path.display())).with_source(err)
        })?;
    }
    let mut body = String::new();
    for (name, value) in pairs {
        body.push_str(name);
        body.push('=');
        body.push_str(value);
        body.push('\n');
    }
    file.write_all(body.as_bytes()).map_err(|err| {
        Error::new(format!("could not append runtime environment file {}", path.display())).with_source(err)
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|err| {
            Error::new(format!(
                "could not set permissions on runtime environment file {}",
                path.display()
            ))
            .with_source(err)
        })?;
    }
    Ok(())
}

#[derive(Default)]
struct PropertyRollback {
    names: Vec<String>,
    rollback: bool,
}

impl PropertyRollback {
    fn remember(&mut self, name: &str) {
        self.names.push(name.to_string());
        self.rollback = true;
    }

    fn keep(&mut self) {
        self.rollback = false;
    }

    fn clear(&self) {
        for name in &self.names {
            env::clear_property(name);
        }
    }
}

impl Drop for PropertyRollback {
    fn drop(&mut self) {
        if self.rollback {
            self.clear();
        }
    }
}

#[cfg(test)]
impl ExecutionContextFactory {
    pub(crate) fn test_context() -> ExecutionContext {
        Self::test_context_with_conf("")
    }

    /// A test context whose conf text is `conf_text`, for example an `artifact-execution-facade`.
    pub(crate) fn test_context_with_conf(conf_text: &str) -> ExecutionContext {
        Self::test_context_with(conf_text, crate::service::ServicePools::idle())
    }

    /// A test context whose factory owns `pools`, attached so the scheduler can reach the factory.
    pub(crate) fn test_context_with(conf_text: &str, pools: crate::service::ServicePools) -> ExecutionContext {
        Self::test_context_in(conf_text, pools, PathBuf::from("."))
    }

    /// A test context whose framework directory is `framework_dir`, so framework files resolve.
    pub(crate) fn test_context_in(
        conf_text: &str,
        pools: crate::service::ServicePools,
        framework_dir: PathBuf,
    ) -> ExecutionContext {
        Self::test_context_with_components(conf_text, pools, framework_dir, Vec::new())
    }

    /// [`Self::test_context_in`] with `components`, in their load order.
    pub(crate) fn test_context_with_components(
        conf_text: &str,
        pools: crate::service::ServicePools,
        framework_dir: PathBuf,
        components: Vec<ComponentInfo>,
    ) -> ExecutionContext {
        // The conf's cache-list, when the test gives one, makes the named caches.
        let conf = MNode::parse_text("test", conf_text)
            .ok()
            .filter(|root| root.name() == "moqui-conf")
            .unwrap_or_else(|| MNode::parse_text("test", "<moqui-conf/>").expect("conf"));
        let entity = Arc::new(RwLock::new(None));
        let runtime_dir = PathBuf::from(".");
        let executor = if pools.is_inline() {
            Executor::Inline
        } else {
            Executor::Threads
        };
        // `component://framework/...` names the framework directory, as boot registers it before the component scan.
        let mut component_dirs = std::collections::HashMap::new();
        if framework_dir != Path::new(".") {
            component_dirs.insert("framework".to_string(), framework_dir.clone());
        }
        for component in &components {
            component_dirs.insert(component.name.clone(), component.location.clone());
        }
        let factory = Arc::new(Self {
            runtime_dir: runtime_dir.clone(),
            framework_dir,
            components,
            resource: ResourceFacade::new(runtime_dir, component_dirs, entity.clone()),
            cache: CacheFacade::from_conf(&conf).expect("cache"),
            conf_text: conf_text.to_string(),
            conf_cache: std::sync::OnceLock::new(),
            tarpit_hits: std::sync::Mutex::default(),
            property_names: Vec::new(),
            extensions: std::sync::Mutex::new(std::collections::HashMap::new()),
            entity,
            pools,
            requests: crate::service::RequestPool::idle(),
            executor,
            sessions: std::sync::Mutex::new(crate::user::session::SessionTable::new(
                crate::user::session::FlushMode::Eventual,
            )),
            cluster_heartbeat: AtomicBool::new(false),
            hits: super::hits::ArtifactHits::from_conf(conf_text),
            started_millis: jiff::Timestamp::now().as_millisecond(),
            native: crate::native::NativeRegistry::default(),
            native_tools: crate::native::ToolSet::default(),
        });
        factory.pools.attach(&factory);
        install_hit_flush(&factory);
        factory.new_context()
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use super::{ExecutionContext, ExecutionContextFactory, ENTERED};

    /// The entered context's factory is `factory`, read through the context's own state.
    fn current_is(factory: &Arc<ExecutionContextFactory>) -> Option<bool> {
        ExecutionContext::with_current(|ec| {
            ec.message().add_error("still alive");
            Arc::ptr_eq(ec.factory(), factory)
        })
    }

    #[test]
    fn forgotten_guard_keeps_the_context_alive() {
        let ctx = ExecutionContextFactory::test_context();
        let factory = Arc::clone(ctx.factory());
        let state = Rc::downgrade(&ctx.state);
        std::mem::forget(ctx.enter());
        drop(ctx);
        // The slot and the leaked guard own handles, so the state was not freed with `ctx`.
        assert!(state.upgrade().is_some());
        assert_eq!(current_is(&factory), Some(true));
        assert!(ExecutionContext::is_entered());
        // Clear this thread's slot. The forgotten guard's own handle stays leaked.
        drop(ENTERED.with(|slot| std::mem::take(&mut *slot.borrow_mut())));
        assert!(!ExecutionContext::is_entered());
        assert_eq!(current_is(&factory), None);
    }

    #[test]
    fn out_of_order_guards_remove_their_own_entries() {
        let a = ExecutionContextFactory::test_context();
        let b = ExecutionContextFactory::test_context();
        let (factory_a, factory_b) = (Arc::clone(a.factory()), Arc::clone(b.factory()));
        let state_a = Rc::downgrade(&a.state);
        let ga = a.enter();
        let gb = b.enter();
        drop(ga);
        drop(a);
        // `a` left the stack with its guard, so its state is gone and `b` is still current.
        assert!(state_a.upgrade().is_none());
        assert_eq!(current_is(&factory_b), Some(true));
        assert_eq!(current_is(&factory_a), Some(false));
        drop(gb);
        assert!(!ExecutionContext::is_entered());
        assert_eq!(current_is(&factory_b), None);
    }

    #[test]
    fn nested_guards_restore_the_outer_context() {
        let a = ExecutionContextFactory::test_context();
        let b = ExecutionContextFactory::test_context();
        let (factory_a, factory_b) = (Arc::clone(a.factory()), Arc::clone(b.factory()));
        {
            let _outer = a.enter();
            {
                let _same = a.enter();
                let _inner = b.enter();
                assert_eq!(current_is(&factory_b), Some(true));
            }
            assert_eq!(current_is(&factory_a), Some(true));
            // A body that enters and leaves another context keeps its own handle.
            let kept = ExecutionContext::with_current(|ec| {
                let _inner = b.enter();
                Arc::ptr_eq(ec.factory(), &factory_a)
            });
            assert_eq!(kept, Some(true));
        }
        assert!(!ExecutionContext::is_entered());
    }

    #[test]
    fn idle_tarpit_keys_are_swept_as_the_map_grows() {
        let context = super::ExecutionContextFactory::test_context();
        let factory = context.factory();
        // Many users each hit once and go away.
        for user in 0..5_000 {
            factory.record_tarpit_hit(&format!("user{user}@AT_SERVICE:svc"), 1_000, 60_000);
        }
        // All still within their window, so all kept; the next sweep comes at twice this many.
        assert_eq!(factory.tarpit_key_count(), 5_000);
        // An hour later every one of them is idle: the sweep at 8192 keys leaves only the new ones.
        let later = 1_000 + 3_600_000;
        for user in 0..3_200 {
            factory.record_tarpit_hit(&format!("late{user}@AT_SERVICE:svc"), later, 60_000);
        }
        assert_eq!(factory.tarpit_key_count(), 3_200);
        assert_eq!(factory.tarpit_times("late3199@AT_SERVICE:svc"), vec![later]);
        assert!(factory.tarpit_times("user0@AT_SERVICE:svc").is_empty());
    }
}
