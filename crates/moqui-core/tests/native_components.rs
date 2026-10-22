//! Public API tests: linked implementations, runtime XML bindings, no core-private registration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use moqui_core::context::{
    BootOptions, Error, ExecutionContext, ExecutionContextFactory, Executor, ResourceEntry, ResourceStat,
};
use moqui_core::driver::{DbConnection, DbDriver};
use moqui_core::entity::{EntityFacade, EntityGroup};
use moqui_core::native::*;
use moqui_core::script::template::TemplateRenderer;
use moqui_core::transaction::{PoolConfig, TransactionManager};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

submit_component! { NativeComponent { name: "native-probe", register } }
submit_component! { NativeComponent { name: "duplicate-probe", register: duplicate } }

fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function("probe::echo", echo)?;
    registry.service_function("probe::unreferenced", echo)?;
    registry.service_function("probe::transaction_state", transaction_state)?;
    registry.runner("probe::Runner", runner)?;
    registry.tool("probe::First", || Arc::new(Factory::new("First", false)))?;
    registry.tool("probe::Second", || Arc::new(Factory::new("Second", false)))?;
    registry.tool("probe::Alias", || Arc::new(Factory::new("First", false)))?;
    registry.tool("probe::Fail", || Arc::new(Factory::new("Fail", true)))?;
    registry.tool("probe::Rich", || Arc::new(RichFactory))?;
    registry.resource("probe::Memory", |factory| Ok(factory.extension(|_| Memory::default())))?;
    registry.template("probe::Upper", Arc::new(Upper))
}
fn duplicate(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function("duplicate", echo)?;
    registry.service_function("duplicate", echo)
}
fn echo(_ec: &ExecutionContext, _info: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, LibError> {
    Ok(parameters.clone())
}
fn transaction_state(
    _ec: &ExecutionContext,
    _info: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    Ok(BTreeMap::from([(
        "active".into(),
        Value::Bool(moqui_core::transaction::thread_in_transaction()),
    )]))
}

fn runner(_ec: &ExecutionContext, info: &ServiceInfo<'_>, _parameters: &Parameters) -> Result<Parameters, LibError> {
    Ok(BTreeMap::from([(
        "result".into(),
        Value::string(format!(
            "{}:{}:{}:{}",
            info.name, info.service_type, info.location, info.method
        )),
    )]))
}
#[derive(Default)]
struct Events(Mutex<Vec<String>>);
impl Events {
    fn push(&self, value: String) {
        self.0.lock().unwrap().push(value);
    }
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}
struct Factory {
    name: &'static str,
    fail: bool,
    tool: OnceLock<Arc<Counter>>,
}
impl Factory {
    fn new(name: &'static str, fail: bool) -> Self {
        Self {
            name,
            fail,
            tool: OnceLock::new(),
        }
    }
}
impl ToolFactory for Factory {
    fn name(&self) -> &str {
        self.name
    }
    fn pre_facade_init(&self, factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        assert!(factory.entity().is_none());
        let events = factory.extension(|_| Events::default());
        events.push(format!("pre:{}", self.name));
        self.tool
            .set(Arc::new(Counter {
                value: AtomicUsize::new(0),
                ready: AtomicBool::new(false),
                events,
            }))
            .ok()
            .unwrap();
        Ok(())
    }
    fn init(&self, factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        assert!(factory.entity().is_some());
        assert!(ExecutionContext::with_current(|ec| Arc::ptr_eq(ec.factory(), factory)).unwrap());
        let tool = self.tool.get().unwrap();
        tool.events.push(format!("init:{}", self.name));
        if self.fail {
            return Err(Error::new("intentional failure"));
        }
        tool.ready.store(true, Ordering::Release);
        Ok(())
    }
    fn get_instance(&self, _parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        let tool = self.tool.get().unwrap();
        if !tool.ready.load(Ordering::Acquire) {
            return Err(Error::new("not initialized"));
        }
        Ok(tool.clone())
    }
    fn destroy(&self) {
        let tool = self.tool.get().unwrap();
        tool.ready.store(false, Ordering::Release);
        tool.events.push(format!("destroy:{}", self.name));
    }
    fn post_facade_destroy(&self) {
        self.tool.get().unwrap().events.push(format!("post:{}", self.name));
    }
}
struct Counter {
    value: AtomicUsize,
    ready: AtomicBool,
    events: Arc<Events>,
}
impl Tool for Counter {
    fn methods(&self) -> &'static [&'static str] {
        &["increment"]
    }
    fn call(&self, _ec: &ExecutionContext, method: &str, _parameters: &[Value]) -> Result<Value, Error> {
        if !self.ready.load(Ordering::Acquire) {
            return Err(Error::new("closed"));
        }
        assert_eq!(method, "increment");
        Ok(Value::I64(self.value.fetch_add(1, Ordering::Relaxed) as i64 + 1))
    }
}
/// A tool factory that binds a closure service over its state, and gives tools with properties and nested tools.
struct RichFactory;
impl ToolFactory for RichFactory {
    fn name(&self) -> &str {
        "Rich"
    }
    fn pre_facade_init(&self, factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        let label = String::from("from the factory");
        factory.register_service("probe::closure", move |_ec, info, parameters| {
            let mut out = parameters.clone();
            out.insert("label".into(), Value::string(format!("{label} at {}", info.location)));
            Ok(out)
        })?;
        let err = factory.register_service("probe::closure", echo).unwrap_err();
        assert!(
            err.to_string()
                .contains("duplicate native implementation probe::closure"),
            "{err}"
        );
        let err = factory.register_service("probe::echo", echo).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "a catalogue location: {err}");
        let err = factory
            .register_service("moqui_core::service::mail::send_template", echo)
            .unwrap_err();
        assert!(err.to_string().contains("duplicate"), "a built-in location: {err}");
        Ok(())
    }
    fn get_instance(&self, parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        let label = match parameters.first() {
            Some(Value::String(text)) => text.clone(),
            _ => "rich".to_string(),
        };
        Ok(Arc::new(RichTool { label }))
    }
    fn type_declaration(&self) -> Option<&'static str> {
        Some("{ echo(...args: unknown[]): unknown[] }")
    }
}
struct RichTool {
    label: String,
}
impl Tool for RichTool {
    fn methods(&self) -> &'static [&'static str] {
        &["echo", "child", "fail"]
    }
    fn call(&self, _ec: &ExecutionContext, method: &str, parameters: &[Value]) -> Result<Value, Error> {
        match method {
            "echo" => Ok(Value::List(parameters.to_vec())),
            "child" => Ok(tool_value(Arc::new(RichTool {
                label: format!("{} child", self.label),
            }))),
            "fail" => Err(Error::new("failed on purpose")),
            _ => unreachable!("not a method: {method}"),
        }
    }
    fn property(&self, _ec: &ExecutionContext, name: &str) -> Result<Option<Value>, Error> {
        Ok((name == "label").then(|| Value::string(self.label.clone())))
    }
}
#[derive(Default, Debug)]
struct Memory(Mutex<BTreeMap<String, Vec<u8>>>);
impl ResourceProvider for Memory {
    fn read_bytes(&self, location: &str) -> Result<Vec<u8>, Error> {
        self.0
            .lock()
            .unwrap()
            .get(location)
            .cloned()
            .ok_or_else(|| Error::new("not found"))
    }
    fn stat(&self, location: &str) -> Result<Option<ResourceStat>, Error> {
        Ok(self.0.lock().unwrap().get(location).map(|bytes| ResourceStat {
            is_file: true,
            size: bytes.len() as u64,
            last_modified: Some(42),
        }))
    }
    fn write_bytes(&self, location: &str, bytes: &[u8]) -> Result<(), Error> {
        self.0.lock().unwrap().insert(location.into(), bytes.into());
        Ok(())
    }
    fn list(&self, location: &str) -> Result<Vec<ResourceEntry>, Error> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with(location))
            .map(|key| ResourceEntry {
                name: key.rsplit('/').next().unwrap().into(),
                location: key.clone(),
                is_file: true,
            })
            .collect())
    }
    fn delete(&self, location: &str) -> Result<bool, Error> {
        Ok(self.0.lock().unwrap().remove(location).is_some())
    }
    fn rename(&self, location: &str, new_location: &str) -> Result<(), Error> {
        let bytes = self.read_bytes(location)?;
        self.delete(location)?;
        self.write_bytes(new_location, &bytes)
    }
}
struct Upper;
impl TemplateRenderer for Upper {
    fn render(&self, source: &str, _data: Option<&Value>) -> Result<String, Error> {
        Ok(source.to_uppercase())
    }
    fn check(&self, _source: &str) -> Result<(), Error> {
        Ok(())
    }
}
struct NoDb;
impl DbDriver for NoDb {
    fn connect(&self) -> Result<Box<dyn DbConnection>, LibError> {
        Err(LibError::new("test must not access a database"))
    }
}
struct Runtime(PathBuf);
impl Runtime {
    fn new(component: &str, config: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "mr-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let framework = root.join("framework");
        let probe = root.join("component/probe");
        std::fs::create_dir_all(framework.join("conf")).unwrap();
        std::fs::create_dir_all(probe.join("service")).unwrap();
        std::fs::write(framework.join("component.xml"), "<component name=\"framework\"/>").unwrap();
        std::fs::write(
            framework.join("conf/MoquiDefaultConf.xml"),
            format!("<moqui-conf><component-list><component-dir location=\"component\"/></component-list><service-facade><service-file location=\"component://{component}/service/Probe.xml\"/></service-facade></moqui-conf>"),
        )
        .unwrap();
        std::fs::write(
            probe.join("component.xml"),
            format!("<component name=\"{component}\"/>"),
        )
        .unwrap();
        std::fs::write(
            probe.join("MoquiConf.xml"),
            format!("<moqui-conf>{config}</moqui-conf>"),
        )
        .unwrap();
        Self(root)
    }
    fn boot(&self) -> Result<Arc<ExecutionContextFactory>, Error> {
        ExecutionContextFactory::boot_with_options(
            &self.0,
            self.0.join("framework"),
            BootOptions {
                executor: Executor::Inline,
                ..BootOptions::default()
            },
        )
    }
    fn services(&self, xml: &str) {
        std::fs::write(self.0.join("component/probe/service/Probe.xml"), xml).unwrap();
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn attach(factory: &ExecutionContextFactory) {
    let transactions = TransactionManager::new(
        Arc::new(NoDb),
        PoolConfig {
            min: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let framework = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = EntityFacade::open(
        &framework,
        "transactional",
        vec![EntityGroup::new("transactional", "postgres", transactions)],
    )
    .unwrap();
    factory.set_entity(facade);
}

#[test]
fn xml_defines_native_services_and_custom_runner_metadata() {
    let runtime = Runtime::new(
        "native-probe",
        "<service-facade><service-type name=\"probe\" runner-class=\"probe::Runner\"/></service-facade>",
    );
    runtime.services(
        r#"<services>
      <service verb="echo" noun="Value" type="rust" location="probe::echo" authenticate="false" transaction="ignore">
        <in-parameters><parameter name="message" required="true"/></in-parameters>
        <out-parameters><parameter name="message"/></out-parameters>
      </service>
      <service verb="run" type="probe" location="somewhere" method="perform" authenticate="false" transaction="ignore">
        <out-parameters><parameter name="result"/></out-parameters>
      </service>
    </services>"#,
    );
    let factory = runtime.boot().unwrap();
    attach(&factory);
    factory.init_tools().unwrap();
    let ec = factory.new_context();
    let _entered = ec.enter();
    let _authz = ec.disable_authz();
    assert!(moqui_core::service::lint(&[]).unwrap().is_empty());
    let params = BTreeMap::from([
        ("message".into(), Value::string("hello")),
        ("extra".into(), Value::string("removed")),
    ]);
    let result = moqui_core::service::sync("Probe.echo#Value", &params).unwrap();
    assert_eq!(result, BTreeMap::from([("message".into(), Value::string("hello"))]));
    moqui_core::service::sync("Probe.echo#Value", &BTreeMap::new()).unwrap();
    assert!(
        !ec.message().validation_errors().is_empty(),
        "required XML parameters must be validated"
    );
    ec.message().clear();
    let result = moqui_core::service::sync("Probe.run", &BTreeMap::new()).unwrap();
    assert_eq!(result["result"], Value::string("Probe.run:probe:somewhere:perform"));
    assert!(
        moqui_core::service::sync("probe::unreferenced", &params).is_err(),
        "linked function must not define a service"
    );
}

#[test]
fn native_services_keep_xml_authentication_and_transaction_policy() {
    let runtime = Runtime::new("native-probe", "");
    runtime.services(
        r#"<services>
      <service verb="secured" type="rust" location="probe::echo" transaction="ignore"/>
      <service verb="transactional" type="rust" location="probe::transaction_state" authenticate="false">
        <out-parameters><parameter name="active" type="Boolean"/></out-parameters>
      </service>
    </services>"#,
    );
    let factory = runtime.boot().unwrap();
    attach(&factory);
    let ec = factory.new_context();
    let _entered = ec.enter();
    assert!(moqui_core::service::lint(&[]).unwrap().is_empty());
    let error = moqui_core::service::sync("Probe.secured", &BTreeMap::new()).unwrap_err();
    assert_eq!(error.kind(), moqui_core::context::ErrorKind::AuthenticationRequired);
    let _authz = ec.disable_authz();
    assert!(moqui_core::service::sync("Probe.secured", &BTreeMap::new()).is_ok());
    let result = moqui_core::service::sync("Probe.transactional", &BTreeMap::new()).unwrap();
    assert_eq!(
        result["active"],
        Value::Bool(true),
        "native body must run inside the XML transaction"
    );
}

#[test]
fn tools_obey_priority_lifecycle_and_explicit_js_methods() {
    let runtime = Runtime::new(
        "native-probe",
        r#"<tools>
      <tool-factory class="probe::Second" init-priority="20"/>
      <tool-factory class="probe::First" init-priority="2"/>
      <tool-factory class="not::linked" disabled="true"/>
    </tools>"#,
    );
    runtime.services(
        r#"<services><service verb="increment" authenticate="false" transaction="ignore">
      <out-parameters><parameter name="result" type="Long"/></out-parameters>
      <actions><script>result = ec.getTool('First').increment();</script></actions>
    </service></services>"#,
    );
    let factory = runtime.boot().unwrap();
    let events = factory.extension(|_| Events::default());
    assert_eq!(events.all(), ["pre:First", "pre:Second"]);
    assert!(factory.get_tool("First", &[]).is_err());
    attach(&factory);
    factory.init_tools().unwrap();
    factory.init_tools().unwrap();
    let ec = factory.new_context();
    let _entered = ec.enter();
    let _authz = ec.disable_authz();
    assert!(moqui_core::service::lint(&[]).unwrap().is_empty());
    assert_eq!(
        moqui_core::service::sync("Probe.increment", &BTreeMap::new()).unwrap()["result"],
        Value::I32(1)
    );
    assert_eq!(
        moqui_core::script::eval_value("ec.factory.getToolFactory('First').getInstance().increment()").unwrap(),
        Value::I32(2)
    );
    assert_eq!(
        moqui_core::script::eval_value(
            "ec.factory.getToolFactory('missing') === null && typeof ec.getTool('First').destroy === 'undefined'"
        )
        .unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        factory
            .get_tool_typed::<Counter>("First", &[])
            .unwrap()
            .value
            .load(Ordering::Relaxed),
        2
    );
    factory.shutdown_tools();
    factory.shutdown_tools();
    assert_eq!(
        events.all(),
        [
            "pre:First",
            "pre:Second",
            "init:First",
            "init:Second",
            "destroy:Second",
            "destroy:First"
        ]
    );
    assert!(factory.get_tool("First", &[]).is_err());
    assert!(factory.init_tools().is_err());
}

#[test]
fn tool_objects_keep_their_methods_and_closures_bind_services() {
    let runtime = Runtime::new("native-probe", r#"<tools><tool-factory class="probe::Rich"/></tools>"#);
    runtime.services(
        r#"<services>
      <service verb="closure" noun="Value" type="rust" location="probe::closure" authenticate="false" transaction="ignore">
        <in-parameters><parameter name="note"/></in-parameters>
        <out-parameters><parameter name="note"/><parameter name="label"/></out-parameters>
      </service>
      <service verb="keep" noun="Tool" authenticate="false" transaction="ignore">
        <out-parameters><parameter name="text"/></out-parameters>
        <actions>
          <set field="tool" from="ec.factory.getTool('Rich', 'kept')"/>
          <set field="text" from="tool.label + ':' + tool.echo('x')[0] + ':' + tool.child().label"/>
        </actions>
      </service>
    </services>"#,
    );
    let factory = runtime.boot().unwrap();
    attach(&factory);
    factory.init_tools().unwrap();
    assert!(factory.get_tool_factory_typed::<RichFactory>("Rich").is_some());
    assert!(factory.get_tool_factory_typed::<Factory>("Rich").is_none());
    assert_eq!(
        factory.tool_type_declarations(),
        [("Rich".to_string(), "{ echo(...args: unknown[]): unknown[] }")]
    );
    let ec = factory.new_context();
    let _entered = ec.enter();
    assert!(moqui_core::service::lint(&[]).unwrap().is_empty());
    let params = BTreeMap::from([("note".into(), Value::string("hello"))]);
    let out = moqui_core::service::sync("Probe.closure#Value", &params).unwrap();
    assert_eq!(out["note"], Value::string("hello"));
    assert_eq!(out["label"], Value::string("from the factory at probe::closure"));
    // The definitions are read now, so a later binding would not be seen by them.
    let err = factory.register_service("probe::late", echo).unwrap_err();
    assert!(
        err.to_string().contains("after the service definitions were read"),
        "{err}"
    );

    // Kept on the context stack by XML Actions and read back, it is still the tool.
    let out = moqui_core::service::sync("Probe.keep#Tool", &BTreeMap::new()).unwrap();
    assert_eq!(out["text"], Value::string("kept:x:kept child"));

    moqui_core::script::clear_context_stack();
    let holds = |expression: &str| assert!(moqui_core::script::eval_condition(expression).unwrap(), "{expression}");
    holds("JSON.stringify(ec.getTool('Rich').echo(1, 'a', null)) === '[1,\"a\",null]'");
    holds("ec.factory.getTool('Rich').missing === undefined");
    holds("'echo' in ec.getTool('Rich') && !('nope' in ec.getTool('Rich'))");
    holds("Object.getOwnPropertyNames(ec.getTool('Rich')).join() === 'echo,child,fail'");
    holds("(() => { const t = ec.getTool('Rich'); try { t.label = 'x' } catch (e) {} return t.label === 'rich' })()");
    holds("(() => { try { ec.getTool('Rich').fail(); return false } catch (e) { return String(e.message).includes('failed on purpose') } })()");
    moqui_core::script::set_context_name(
        "given",
        tool_value(Arc::new(RichTool {
            label: "from rust".into(),
        })),
    )
    .unwrap();
    holds("given.label === 'from rust' && given.child().label === 'from rust child'");
    moqui_core::script::clear_context_stack();
}

#[test]
fn resources_and_templates_need_neither_services_nor_tools() {
    let runtime = Runtime::new(
        "native-probe",
        r#"<resource-facade>
      <resource-reference scheme="memory" class="probe::Memory"/>
      <template-renderer extension="upper" class="probe::Upper"/>
    </resource-facade>"#,
    );
    let first = runtime.boot().unwrap();
    let second = runtime.boot().unwrap();
    let resource = first.resource();
    resource.write_bytes("memory://bucket/test.upper", b"hello").unwrap();
    assert_eq!(
        resource.get("memory://bucket/test.upper").unwrap().read_text().unwrap(),
        "hello"
    );
    assert_eq!(
        resource
            .stat("memory://bucket/test.upper")
            .unwrap()
            .unwrap()
            .last_modified,
        Some(42)
    );
    assert_eq!(resource.get("memory://bucket/").unwrap().children().unwrap().len(), 1);
    assert_eq!(resource.list("memory://bucket/").unwrap()[0].name, "test.upper");
    assert!(second.resource().stat("memory://bucket/test.upper").unwrap().is_none());
    let ec = first.new_context();
    let _entered = ec.enter();
    assert_eq!(
        moqui_core::script::eval_value("ec.resource.template('memory://bucket/test.upper')").unwrap(),
        Value::string("HELLO")
    );
    resource
        .rename("memory://bucket/test.upper", "memory://bucket/renamed.upper")
        .unwrap();
    assert!(!resource.get("memory://bucket/test.upper").unwrap().exists());
    assert!(resource
        .rename("memory://bucket/renamed.upper", "runtime://bad")
        .is_err());
    assert!(resource.delete("memory://bucket/renamed.upper").unwrap());
    assert!(resource.make_directory("memory://bucket/dir").is_err());
}

#[test]
fn missing_duplicate_and_inactive_implementations_fail_clearly() {
    for (component, config, expected) in [
        (
            "native-probe",
            "<tools><tool-factory class=\"missing\"/></tools>",
            "tool-factory missing",
        ),
        (
            "native-probe",
            "<resource-facade><resource-reference scheme=\"memory\" class=\"missing\"/></resource-facade>",
            "resource-reference memory",
        ),
        (
            "native-probe",
            "<resource-facade><template-renderer extension=\"unknown\" class=\"missing\"/></resource-facade>",
            "template-renderer missing",
        ),
        (
            "native-probe",
            "<service-facade><service-type name=\"custom\" runner-class=\"missing\"/></service-facade>",
            "native runner missing",
        ),
        ("duplicate-probe", "", "duplicate native implementation duplicate"),
        (
            "native-probe",
            "<tools><tool-factory class=\"probe::First\"/><tool-factory class=\"probe::Alias\"/></tools>",
            "duplicate tool name First",
        ),
        (
            "unlinked-component",
            "<tools><tool-factory class=\"probe::First\"/></tools>",
            "not linked",
        ),
    ] {
        let runtime = Runtime::new(component, config);
        let error = runtime.boot().err().expect("boot should fail");
        assert!(error.message().contains(expected), "{error}");
    }
}

#[test]
fn failed_init_cleans_up_in_reverse_order() {
    let runtime = Runtime::new(
        "native-probe",
        r#"<tools>
      <tool-factory class="probe::First" init-priority="1"/>
      <tool-factory class="probe::Fail" init-priority="2"/>
    </tools>"#,
    );
    let factory = runtime.boot().unwrap();
    attach(&factory);
    let events = factory.extension(|_| Events::default());
    assert!(factory
        .init_tools()
        .unwrap_err()
        .message()
        .contains("intentional failure"));
    assert_eq!(
        events.all(),
        [
            "pre:First",
            "pre:Fail",
            "init:First",
            "init:Fail",
            "destroy:Fail",
            "destroy:First"
        ]
    );
    drop(factory);
    assert_eq!(&events.all()[6..], ["post:Fail", "post:First"]);
}

#[test]
fn runtime_config_can_disable_a_component_tool() {
    let runtime = Runtime::new("native-probe", "<tools><tool-factory class=\"probe::First\"/></tools>");
    std::fs::create_dir_all(runtime.0.join("conf")).unwrap();
    std::fs::write(
        runtime.0.join("conf/MoquiDevConf.xml"),
        "<moqui-conf><tools><tool-factory class=\"probe::First\" disabled=\"true\"/></tools></moqui-conf>",
    )
    .unwrap();
    let factory = runtime.boot().unwrap();
    assert!(factory.get_tool_factory("First").is_none());
    assert!(factory.extension(|_| Events::default()).all().is_empty());
}

#[test]
fn component_conf_service_files_are_used_for_discovery() {
    let runtime = Runtime::new(
        "native-probe",
        r#"<service-facade>
        <service-file location="component://native-probe/service/Extra.xml"/>
    </service-facade>"#,
    );
    runtime.services("<services/>");
    std::fs::write(
        runtime.0.join("component/probe/service/Extra.xml"),
        r#"<services>
        <service verb="echo" type="rust" location="probe::echo" authenticate="false" transaction="ignore"/>
    </services>"#,
    )
    .unwrap();
    let factory = runtime.boot().unwrap();
    let ec = factory.new_context();
    let _entered = ec.enter();
    assert!(moqui_core::service::configured_service_files()
        .unwrap()
        .iter()
        .any(|file| file.ends_with("Extra.xml")));
    assert!(moqui_core::service::lint(&[]).unwrap().is_empty());
}

#[test]
fn unresolved_service_implementations_are_lint_failures() {
    let runtime = Runtime::new("native-probe", "");
    runtime.services(
        r#"<services>
      <service verb="missing" type="rust" location="not::linked"/>
      <service verb="unknown" type="not-linked"/>
    </services>"#,
    );
    let factory = runtime.boot().unwrap();
    let ec = factory.new_context();
    let _entered = ec.enter();
    let failures = moqui_core::service::lint(&[]).unwrap();
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(failures.iter().any(|failure| failure.contains("not registered")));
    assert!(failures.iter().any(|failure| failure.contains("unknown type")));
}

#[test]
fn shutdown_waits_for_workers_before_closing_tools() {
    let runtime = Runtime::new("native-probe", "<tools><tool-factory class=\"probe::First\"/></tools>");
    let factory = ExecutionContextFactory::boot_with_options(
        &runtime.0,
        runtime.0.join("framework"),
        BootOptions {
            executor: Executor::Threads,
            ..BootOptions::default()
        },
    )
    .unwrap();
    attach(&factory);
    factory.init_tools().unwrap();
    factory.request_pool().start(1, 2).unwrap();
    let tool = factory.get_tool_typed::<Counter>("First", &[]).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker_factory = Arc::clone(&factory);
    let worker_tool = Arc::clone(&tool);
    let result = factory
        .request_pool()
        .try_submit(Arc::clone(&factory), move || {
            worker_factory.service_pools().call(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                assert!(worker_tool.ready.load(Ordering::Acquire));
                Ok(())
            })
        })
        .unwrap();
    started_rx.recv().unwrap();
    let closing_factory = Arc::clone(&factory);
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    let closing = std::thread::spawn(move || {
        closing_factory.shutdown();
        closed_tx.send(()).unwrap();
    });
    assert!(closed_rx.recv_timeout(std::time::Duration::from_millis(50)).is_err());
    assert!(tool.ready.load(Ordering::Acquire));
    release_tx.send(()).unwrap();
    result.recv().unwrap().unwrap();
    closing.join().unwrap();
    assert!(!tool.ready.load(Ordering::Acquire));
    factory.shutdown();
}

#[test]
fn tools_are_isolated_and_drop_closes_them() {
    let runtime = Runtime::new("native-probe", "<tools><tool-factory class=\"probe::First\"/></tools>");
    let first = runtime.boot().unwrap();
    let second = runtime.boot().unwrap();
    attach(&first);
    attach(&second);
    first.init_tools().unwrap();
    second.init_tools().unwrap();
    let first_tool = first.get_tool_typed::<Counter>("First", &[]).unwrap();
    let second_tool = second.get_tool_typed::<Counter>("First", &[]).unwrap();
    assert!(!Arc::ptr_eq(&first_tool, &second_tool));
    drop(first);
    assert!(!first_tool.ready.load(Ordering::Acquire));
    assert!(second_tool.ready.load(Ordering::Acquire));
}

/// The framework's VCard and ICalendar tools, selected as MoquiDefaultConf.xml selects them, reach a script with
/// plain values both ways.
#[test]
fn framework_contact_and_calendar_tools_reach_scripts() {
    let default_conf = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework/conf/MoquiDefaultConf.xml"),
    )
    .unwrap();
    for class in [moqui_core::pim::VCARD_CLASS, moqui_core::pim::ICALENDAR_CLASS] {
        assert!(
            default_conf.contains(&format!("<tool-factory class=\"{class}\"")),
            "{class}"
        );
    }
    let runtime = Runtime::new(
        "native-probe",
        &format!(
            "<tools><tool-factory class=\"{}\"/><tool-factory class=\"{}\"/></tools>",
            moqui_core::pim::VCARD_CLASS,
            moqui_core::pim::ICALENDAR_CLASS
        ),
    );
    runtime.services(
        r#"<services>
      <service verb="use" noun="Tools" authenticate="false" transaction="ignore">
        <out-parameters><parameter name="text"/><parameter name="starts" type="List"/><parameter name="ics"/></out-parameters>
        <actions><script><![CDATA[
            const vcard = ec.factory.getTool('VCard')
            const cards = vcard.parse('BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Vee Card\r\nN:Card;Vee;;;\r\nEMAIL;TYPE=work,pref:vee@example.com\r\nEND:VCARD\r\n')
            const written = vcard.write([{partyId: 'P1', firstName: 'Vee', lastName: 'Card', name: 'Vee Card',
                emails: [{emailAddress: 'vee@example.com', contactMechPurposeId: 'EmailPrimary'}]}], '4.0')
            text = [ec.factory.getToolFactory('VCard') != null, ec.factory.getToolFactory('Nope') == null, cards.length,
                cards[0].firstName, cards[0]['email1.label'], written.split('\r\n')[2]].join('|')
            const calendar = ec.factory.getTool('ICalendar')
            // 2027-05-10 10:00 in Chicago, weekly five times; the second week from the range
            starts = Array.from(calendar.expand('FREQ=WEEKLY;COUNT=5', Date.UTC(2027, 4, 10, 15), 'America/Chicago', ['20270524T100000'],
                null, Date.UTC(2027, 4, 16), Date.UTC(2027, 5, 30)))
            const parsed = calendar.parse(calendar.write({method: 'REQUEST', events: [{uid: 'u1', summary: 'Sync', start: Date.UTC(2027, 4, 10, 15),
                end: Date.UTC(2027, 4, 10, 16), timeZone: 'America/Chicago', rrule: 'FREQ=WEEKLY;COUNT=5',
                organizer: {name: 'Ada', email: 'ada@example.com'}, attendees: [{email: 'bob@example.com', rsvp: true}]}]}))
            ics = [parsed.method, parsed.events.length, parsed.events[0].timeZone, parsed.events[0].start === Date.UTC(2027, 4, 10, 15),
                parsed.events[0].attendees[0].partstat].join('|')
        ]]></script></actions>
      </service>
    </services>"#,
    );
    let factory = runtime.boot().unwrap();
    attach(&factory);
    factory.init_tools().unwrap();
    assert!(factory
        .get_tool_factory_typed::<moqui_core::pim::VCardToolFactory>("VCard")
        .is_some());
    let ec = factory.new_context();
    let _entered = ec.enter();
    let out = moqui_core::service::sync("Probe.use#Tools", &BTreeMap::new()).unwrap();
    assert_eq!(
        out["text"],
        Value::string("true|true|1|Vee|* Work|UID:urn:moqui:mantle.party.Party:P1")
    );
    let week = 7 * 24 * 3_600_000i64;
    let first = 1_809_961_200_000i64; // 2027-05-10T15:00:00Z
    assert_eq!(
        out["starts"],
        Value::List(vec![
            Value::I64(first + week),
            Value::I64(first + 3 * week),
            Value::I64(first + 4 * week)
        ])
    );
    assert_eq!(out["ics"], Value::string("REQUEST|1|America/Chicago|true|NEEDS-ACTION"));
}
