//! Linked component implementations. XML still defines services and selects tools and handlers.
//!
//! A component crate submits a [`NativeComponent`] and the application links that crate
//! (`use my_component as _;`). Only components discovered in the runtime contribute code.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use moqui_lib::error::Error as LibError;
use moqui_lib::node::MNode;
use moqui_lib::object::{HostObject, Value};
use moqui_lib::sync::{lock, read, write};

use crate::context::{
    ChangeLogEntry, ChangeSetOutcome, ComponentInfo, Error, ExecutionContext, ExecutionContextFactory, ResourceChange,
    ResourceChangeInfo, ResourceEntry, ResourceStat, ResourceVersion,
};
use crate::script::template::TemplateRenderer;

/// Submit a `NativeComponent { name, register }` from a linked component crate.
pub use inventory::submit as submit_component;

/// The `type="rust"` location of a function by its Rust path, `module_path!() + "::" + name`, for
/// [`NativeRegistry::service_function`] or [`ExecutionContextFactory::register_service`]:
///
/// ```ignore
/// registry.service_function(moqui_core::service_location!(echo), echo)?;
/// ```
#[macro_export]
macro_rules! service_location {
    ($function:ident) => {
        concat!(module_path!(), "::", stringify!($function))
    };
}

pub struct NativeComponent {
    pub name: &'static str,
    pub register: fn(&mut NativeRegistry) -> Result<(), Error>,
}
inventory::collect!(NativeComponent);

pub type Parameters = BTreeMap<String, Value>;

/// XML metadata passed to a linked function or custom service runner.
pub struct ServiceInfo<'a> {
    pub name: &'a str,
    pub service_type: &'a str,
    pub location: &'a str,
    pub method: &'a str,
}

pub type NativeService = fn(&ExecutionContext, &ServiceInfo<'_>, &Parameters) -> Result<Parameters, LibError>;
pub(crate) type NativeBody =
    dyn Fn(&ExecutionContext, &ServiceInfo<'_>, &Parameters) -> Result<Parameters, LibError> + Send + Sync;

/// A service body as the definition holds it: a catalogue function, or a closure bound with
/// [`ExecutionContextFactory::register_service`].
#[derive(Clone)]
pub struct NativeServiceFn(Arc<NativeBody>);

impl NativeServiceFn {
    pub(crate) fn new(body: Arc<NativeBody>) -> Self {
        Self(body)
    }

    pub(crate) fn call(
        &self,
        ec: &ExecutionContext,
        service: &ServiceInfo<'_>,
        parameters: &Parameters,
    ) -> Result<Parameters, LibError> {
        (self.0)(ec, service, parameters)
    }
}

impl std::fmt::Debug for NativeServiceFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeServiceFn")
    }
}
pub type ToolConstructor = fn() -> Arc<dyn ToolFactory>;
pub type ResourceConstructor = fn(&Arc<ExecutionContextFactory>) -> Result<Arc<dyn ResourceProvider>, Error>;

/// A factory-owned tool lifecycle. Do not retain a strong reference to the execution context factory.
pub trait ToolFactory: Any + Send + Sync {
    fn name(&self) -> &str;
    fn pre_facade_init(&self, _factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        Ok(())
    }
    fn init(&self, _factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        Ok(())
    }
    fn get_instance(&self, parameters: &[Value]) -> Result<Arc<dyn Tool>, Error>;
    /// The TypeScript type of the instance, such as `{ greet(name: string): string }`, for `moqui types`. None types it
    /// loosely.
    fn type_declaration(&self) -> Option<&'static str> {
        None
    }
    fn destroy(&self) {}
    /// Runs when the factory is dropped, after its facade fields have been released.
    fn post_facade_destroy(&self) {}
}

/// A typed Rust instance with an explicit, optional plain-value JavaScript API.
pub trait Tool: Any + Send + Sync {
    fn methods(&self) -> &'static [&'static str] {
        &[]
    }
    fn call(&self, _ec: &ExecutionContext, method: &str, _parameters: &[Value]) -> Result<Value, Error> {
        Err(Error::new(format!("tool method {method} is not exposed")))
    }
    /// A property a script reads that is not a method, as a Groovy getter would be. None reads as `undefined`.
    fn property(&self, _ec: &ExecutionContext, _name: &str) -> Result<Option<Value>, Error> {
        Ok(None)
    }
}

/// A tool as a [`Value`], so a tool method can return another tool and a script keeps it on the context stack.
pub fn tool_value(tool: Arc<dyn Tool>) -> Value {
    Value::Object(HostObject::new(ToolHandle(tool)))
}

/// What a tool [`Value`] holds.
pub(crate) struct ToolHandle(pub(crate) Arc<dyn Tool>);

/// A file [`ResourceProvider::open`] found: what it is, and its content to read a window at a time.
pub type OpenedFile = Option<(ResourceStat, Arc<dyn crate::resource::source::ByteSource>)>;

/// Storage for a configured resource scheme. Mutating operations are opt-in.
/// Providers must apply their own outbound/sim policy before external I/O.
pub trait ResourceProvider: Send + Sync + std::fmt::Debug {
    fn read_bytes(&self, location: &str) -> Result<Vec<u8>, Error>;
    fn stat(&self, location: &str) -> Result<Option<ResourceStat>, Error>;
    /// The file at `location` to read a window at a time, for a response: `None` when no file is there. By default
    /// [`ResourceProvider::read_bytes`] on the first read, of the size `stat` gave.
    fn open(self: Arc<Self>, location: &str) -> Result<OpenedFile, Error>
    where
        Self: 'static,
    {
        let Some(stat) = self.stat(location)?.filter(|stat| stat.is_file) else {
            return Ok(None);
        };
        let at = location.to_string();
        let source = crate::resource::source::LazySource::new(stat.size, true, move || Ok(self.read_bytes(&at)?));
        Ok(Some((stat, Arc::new(source))))
    }
    fn list(&self, location: &str) -> Result<Vec<ResourceEntry>, Error> {
        Err(Error::new(format!("listing is not supported: {location}")))
    }
    fn write_bytes(&self, location: &str, _bytes: &[u8]) -> Result<(), Error> {
        Err(Error::new(format!("writing is not supported: {location}")))
    }
    /// Write the file at `location` from `input` (of `size` bytes, when known), as an upload streams: by default read
    /// into memory and [`ResourceProvider::write_bytes`].
    fn write_from(&self, location: &str, input: &mut dyn std::io::Read, _size: Option<u64>) -> Result<(), Error> {
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .map_err(|err| Error::new(format!("reading the content for {location}: {err}")))?;
        self.write_bytes(location, &bytes)
    }
    fn make_directory(&self, location: &str) -> Result<(), Error> {
        Err(Error::new(format!("mkdir is not supported: {location}")))
    }
    fn delete(&self, location: &str) -> Result<bool, Error> {
        Err(Error::new(format!("deleting is not supported: {location}")))
    }
    fn rename(&self, location: &str, _new_location: &str) -> Result<(), Error> {
        Err(Error::new(format!("renaming is not supported: {location}")))
    }
    /// True when this storage keeps versions of the file at `location` (`ResourceReference.supportsVersion`). The
    /// version methods below answer nothing unless it does.
    fn supports_version(&self, _location: &str) -> bool {
        false
    }
    fn version(&self, _location: &str, _version_name: &str) -> Result<Option<ResourceVersion>, Error> {
        Ok(None)
    }
    fn current_version(&self, _location: &str) -> Result<Option<ResourceVersion>, Error> {
        Ok(None)
    }
    fn root_version(&self, _location: &str) -> Result<Option<ResourceVersion>, Error> {
        Ok(None)
    }
    /// Every version of the file, newest first.
    fn version_history(&self, _location: &str) -> Result<Vec<ResourceVersion>, Error> {
        Ok(Vec::new())
    }
    /// The versions made from `version_name`.
    fn next_versions(&self, _location: &str, _version_name: &str) -> Result<Vec<ResourceVersion>, Error> {
        Ok(Vec::new())
    }
    /// The bytes of one version, or `None` when there is no such version.
    fn read_version(&self, _location: &str, _version_name: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(None)
    }
    /// Write a file with a note for the version it makes (a commit message on `git`). Storage that keeps no notes
    /// writes the bytes and drops the note.
    fn write_bytes_noted(&self, location: &str, bytes: &[u8], _note: Option<&str>) -> Result<(), Error> {
        self.write_bytes(location, bytes)
    }
    /// Apply a change set under the directory `root` ([`crate::context::ResourceFacade::apply_change_set`]): one
    /// version for all of it where the storage can (one commit on `git`), else each change on its own, in order. A
    /// change that fails is reported in the outcome and the others still apply.
    fn apply_changes(
        &self,
        root: &str,
        changes: &[ResourceChange],
        message: Option<&str>,
    ) -> Result<ChangeSetOutcome, Error> {
        Ok(crate::context::apply_changes_each(self, root, changes, message))
    }
    /// The last change of a file or directory: on storage with versions its current version, else its stat.
    fn last_change(&self, location: &str) -> Result<Option<ResourceChangeInfo>, Error> {
        crate::context::last_change_by_stat(self, location)
    }
    /// [`Self::last_change`] of each entry of a directory, by name.
    fn child_changes(&self, location: &str) -> Result<Vec<(String, ResourceChangeInfo)>, Error> {
        let mut found = Vec::new();
        for entry in self.list(location)? {
            if let Some(change) = self.last_change(&entry.location)? {
                found.push((entry.name, change));
            }
        }
        Ok(found)
    }
    /// The versions under a location, newest first, at most `limit`: a file's own versions here; `git` gives every
    /// commit that changed something under a directory.
    fn change_log(&self, location: &str, limit: usize) -> Result<Vec<ChangeLogEntry>, Error> {
        Ok(self
            .version_history(location)?
            .into_iter()
            .take(limit)
            .map(|version| ChangeLogEntry {
                version,
                paths: Vec::new(),
            })
            .collect())
    }
    /// The bytes of every file under a location (one file: its size), for a quota.
    fn tree_size(&self, location: &str) -> Result<u64, Error> {
        crate::context::tree_size_by_walk(self, location)
    }
    /// Write a file as it was in `version_name` back as its newest version, with a note; the new version's name. `git`
    /// puts back a directory with everything under it too, in one commit.
    fn restore_version(&self, location: &str, version_name: &str, note: Option<&str>) -> Result<Option<String>, Error> {
        let bytes = self
            .read_version(location, version_name)?
            .ok_or_else(|| Error::new(format!("version {version_name} not found: {location}")))?;
        self.write_bytes_noted(location, &bytes, note)?;
        Ok(self.current_version(location)?.map(|version| version.version_name))
    }
}

/// An implementation catalogue, not a service-definition registry. Built once per factory.
#[derive(Default)]
pub struct NativeRegistry {
    services: BTreeMap<String, NativeService>,
    /// Closures bound after boot with [`ExecutionContextFactory::register_service`].
    registered: RwLock<BTreeMap<String, NativeServiceFn>>,
    runners: BTreeMap<String, NativeService>,
    tools: BTreeMap<String, ToolConstructor>,
    resources: BTreeMap<String, ResourceConstructor>,
    templates: BTreeMap<String, Arc<dyn TemplateRenderer>>,
    service_types: BTreeMap<String, NativeService>,
}

fn insert<T>(map: &mut BTreeMap<String, T>, name: &str, value: T) -> Result<(), Error> {
    if name.trim().is_empty() {
        return Err(Error::new("native implementation identifier is empty"));
    }
    if map.contains_key(name) {
        return Err(Error::new(format!("duplicate native implementation {name}")));
    }
    map.insert(name.to_string(), value);
    Ok(())
}

impl NativeRegistry {
    /// Bind a function location, not a service name. Only service XML creates callable services.
    pub fn service_function(&mut self, location: &str, function: NativeService) -> Result<(), Error> {
        if crate::service::rust_services::get(location).is_some() {
            return Err(Error::new(format!("duplicate native implementation {location}")));
        }
        insert(&mut self.services, location, function)
    }
    pub fn runner(&mut self, name: &str, runner: NativeService) -> Result<(), Error> {
        insert(&mut self.runners, name, runner)
    }
    pub fn tool(&mut self, class: &str, constructor: ToolConstructor) -> Result<(), Error> {
        insert(&mut self.tools, class, constructor)
    }
    pub fn resource(&mut self, class: &str, constructor: ResourceConstructor) -> Result<(), Error> {
        insert(&mut self.resources, class, constructor)
    }
    pub fn template(&mut self, class: &str, renderer: Arc<dyn TemplateRenderer>) -> Result<(), Error> {
        if crate::script::template::renderer_by_class(class).is_some() {
            return Err(Error::new(format!("duplicate native implementation {class}")));
        }
        insert(&mut self.templates, class, renderer)
    }

    /// The native implementations of the linked crates and `components`, and the conf's service types. With
    /// `tolerant` (an artifacts-only boot of another project) a service type whose runner is not linked is logged and
    /// left out instead of failing the boot.
    pub(crate) fn discover(components: &[ComponentInfo], conf: &MNode, tolerant: bool) -> Result<Self, Error> {
        let mut registry = Self::default();
        crate::telemetry::register(&mut registry)?;
        // the framework's own tool factories (MailCapture; VCard and ICalendar are moqui-dav's), which
        // MoquiDefaultConf.xml selects
        // and the mail and notification captures for specs (MailCapture, NotificationCapture)
        crate::service::mail_capture::register(&mut registry)?;
        crate::notify::capture::register(&mut registry)?;
        for extension in crate::extension::linked() {
            (extension.register_native)(&mut registry)?;
        }
        for component in components {
            let mut entries = inventory::iter::<NativeComponent>
                .into_iter()
                .filter(|entry| entry.name == component.name);
            if let Some(entry) = entries.next() {
                if entries.next().is_some() {
                    return Err(Error::new(format!("duplicate native component {}", component.name)));
                }
                (entry.register)(&mut registry)?;
            }
        }
        if let Some(facade) = conf.first_named("service-facade") {
            for node in facade.named_iter("service-type") {
                let name = required(&node, "name")?;
                let class = required(&node, "runner-class")?;
                // The framework's built-in runners are not Java classes at runtime.
                if builtin_runner(&name, &class) {
                    continue;
                }
                if builtin_service_type(&name) {
                    return Err(Error::new(format!("cannot replace built-in service type {name}")));
                }
                let Some(runner) = registry.runners.get(&class).copied() else {
                    if tolerant {
                        tracing::error!("Leaving out service-type {name}: native runner {class} is not linked");
                        continue;
                    }
                    return Err(Error::new(format!(
                        "service-type {name}: native runner {class} is not linked"
                    )));
                };
                insert(&mut registry.service_types, &name, runner)?;
            }
        }
        Ok(registry)
    }

    /// The locations of the service functions, for the tests of the crates that register them.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn service_locations(&self) -> Vec<&str> {
        self.services.keys().map(String::as_str).collect()
    }

    pub(crate) fn service_at(&self, location: &str) -> Option<NativeServiceFn> {
        if let Some(function) = self.services.get(location).copied() {
            return Some(NativeServiceFn::new(Arc::new(function)));
        }
        read(&self.registered).get(location).cloned()
    }
    pub(crate) fn runner_for(&self, name: &str) -> Option<NativeServiceFn> {
        self.service_types
            .get(name)
            .copied()
            .map(|runner| NativeServiceFn::new(Arc::new(runner)))
    }
    /// [`ExecutionContextFactory::register_service`]: a location no built-in, catalogue or earlier closure has.
    pub(crate) fn register_service(&self, location: &str, run: Arc<NativeBody>) -> Result<(), Error> {
        if crate::service::rust_services::get(location).is_some() || self.services.contains_key(location) {
            return Err(Error::new(format!("duplicate native implementation {location}")));
        }
        insert(&mut write(&self.registered), location, NativeServiceFn::new(run))
    }
    pub(crate) fn template_for(&self, class: &str) -> Option<Arc<dyn TemplateRenderer>> {
        self.templates.get(class).cloned()
    }

    pub(crate) fn configure(&self, factory: &Arc<ExecutionContextFactory>, conf: &MNode) -> Result<(), Error> {
        if let Some(tools) = conf.first_named("tools") {
            let mut nodes = Vec::new();
            for node in tools.named_iter("tool-factory") {
                if node.attribute_is("disabled", "true") {
                    continue;
                }
                let class = required(&node, "class")?;
                let priority = node
                    .attribute("init-priority")
                    .unwrap_or_else(|| "10".to_string())
                    .parse::<i32>()
                    .map_err(|_| Error::new(format!("invalid tool init-priority: {class}")))?;
                nodes.push((priority, class));
            }
            nodes.sort();
            for (_, class) in nodes {
                // A dev component's crate is linked only into the packages with the dev tools, so a Lite or Maximal
                // binary on the dev conf has its component and not its tool factory (AGENTS.md: failure tolerance).
                let Some(constructor) = self.tools.get(&class) else {
                    tracing::error!("Leaving out tool-factory {class}: it is not linked into this binary");
                    continue;
                };
                factory.native_tools.add(constructor(), factory)?;
            }
        }
        if let Some(resource) = conf.first_named("resource-facade") {
            for node in resource.children_named("resource-reference") {
                let scheme = required(&node, "scheme")?;
                let class = required(&node, "class")?;
                if let Some(constructor) = self.resources.get(&class) {
                    factory.resource().install_provider(&scheme, constructor(factory)?)?;
                } else if scheme == "git" && class == GIT_RESOURCE_CLASS {
                    // moqui-git registers it; a binary without that crate leaves the scheme unsupported.
                } else if !builtin_resource(&scheme, &class) {
                    return Err(Error::new(format!(
                        "resource-reference {scheme}: implementation {class} is not linked"
                    )));
                }
            }
            for node in resource.named_iter("template-renderer") {
                let class = required(&node, "class")?;
                if crate::script::template::renderer_by_class(&class).is_none()
                    && self.template_for(&class).is_none()
                    && class != "org.moqui.impl.context.renderer.NoTemplateRenderer"
                {
                    return Err(Error::new(format!("template-renderer {class} is not linked")));
                }
            }
        }
        Ok(())
    }
}

/// The class `MoquiDefaultConf.xml` names for `git://` locations (moqui-git).
pub const GIT_RESOURCE_CLASS: &str = "org.moqui.impl.context.reference.GitResourceReference";

fn builtin_resource(scheme: &str, class: &str) -> bool {
    matches!(
        (scheme, class),
        (
            "http" | "https" | "file" | "ftp" | "jar" | "bundleresource" | "wsjar" | "runtime",
            "org.moqui.resource.UrlResourceReference"
        ) | ("classpath", "org.moqui.resource.ClasspathResourceReference")
            | (
                "component",
                "org.moqui.impl.context.reference.ComponentResourceReference"
            )
            | ("content", "org.moqui.impl.context.reference.ContentResourceReference")
            | ("dbresource", "org.moqui.impl.context.reference.DbResourceReference")
    )
}

fn builtin_runner(name: &str, class: &str) -> bool {
    let suffix = match name {
        "inline" => "InlineServiceRunner",
        "entity-auto" => "EntityAutoServiceRunner",
        "script" => "ScriptServiceRunner",
        "java" => "JavaServiceRunner",
        "remote-json-rpc" => "RemoteJsonRpcServiceRunner",
        "remote-rest" => "RemoteRestServiceRunner",
        _ => return false,
    };
    class == format!("org.moqui.impl.service.runner.{suffix}")
}

fn builtin_service_type(name: &str) -> bool {
    matches!(
        name,
        "inline"
            | "entity-auto"
            | "script"
            | "interface"
            | "remote-rest"
            | "rust"
            | "java"
            | "remote-json-rpc"
            | "camel"
    )
}

fn required(node: &MNode, attribute: &str) -> Result<String, Error> {
    node.attribute(attribute)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("{} is missing {attribute}", node.name())))
}

#[derive(Default)]
pub(crate) struct ToolSet {
    tools: Mutex<Vec<Arc<dyn ToolFactory>>>,
    initialized: AtomicBool,
    closed: AtomicBool,
    lifecycle: Mutex<()>,
}

impl ToolSet {
    fn add(&self, tool: Arc<dyn ToolFactory>, factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        let mut tools = lock(&self.tools);
        if tools.iter().any(|existing| existing.name() == tool.name()) {
            return Err(Error::new(format!("duplicate tool name {}", tool.name())));
        }
        tools.push(Arc::clone(&tool));
        drop(tools);
        tool.pre_facade_init(factory)
    }

    pub(crate) fn init(&self, factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        let _lifecycle = lock(&self.lifecycle);
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::new("tools are shut down"));
        }
        if self.initialized.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let tools = lock(&self.tools).clone();
        for tool in tools {
            if let Err(error) = tool.init(factory) {
                self.close();
                return Err(Error::new(format!(
                    "tool {} initialization failed: {error}",
                    tool.name()
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn type_declarations(&self) -> Vec<(String, &'static str)> {
        lock(&self.tools)
            .iter()
            .filter_map(|tool| tool.type_declaration().map(|text| (tool.name().to_string(), text)))
            .collect()
    }

    pub(crate) fn get(&self, name: &str) -> Option<Arc<dyn ToolFactory>> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        lock(&self.tools).iter().find(|tool| tool.name() == name).cloned()
    }

    pub(crate) fn shutdown(&self) {
        let _lifecycle = lock(&self.lifecycle);
        self.close();
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let tools = lock(&self.tools).clone();
        for tool in tools.iter().rev() {
            tool.destroy();
        }
    }
}

impl Drop for ToolSet {
    fn drop(&mut self) {
        self.shutdown();
        for tool in lock(&self.tools).iter().rev() {
            tool.post_facade_destroy();
        }
    }
}
