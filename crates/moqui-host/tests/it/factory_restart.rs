//! Empty-db startup destroys the bootstrap factory before initializing against seeded data.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::sync::{Arc, Mutex, Weak};

use moqui_core::context::{Error, ExecutionContextFactory};
use moqui_core::native::{NativeComponent, NativeRegistry, Tool, ToolFactory};
use moqui_lib::object::Value;

static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static FACTORIES: Mutex<Vec<Weak<ExecutionContextFactory>>> = Mutex::new(Vec::new());

moqui_core::native::submit_component! { NativeComponent { name: "restart-probe", register } }

fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.tool("restart::Probe", || Arc::new(Probe))
}

struct Probe;
impl ToolFactory for Probe {
    fn name(&self) -> &str {
        "RestartProbe"
    }
    fn init(&self, factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
        FACTORIES.lock().unwrap().push(Arc::downgrade(factory));
        let entity = factory.entity().unwrap();
        let count = if entity.table_exists("moqui.basic.Enumeration")? {
            entity.find("moqui.basic.Enumeration")?.disable_authz().count()?
        } else {
            0
        };
        EVENTS
            .lock()
            .unwrap()
            .push(if count == 0 { "empty" } else { "seeded" }.into());
        Ok(())
    }
    fn get_instance(&self, _: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        Err(Error::new("lifecycle probe has no instance"))
    }
    fn destroy(&self) {
        EVENTS.lock().unwrap().push("destroy".into());
    }
}

#[test]
#[ignore = "slow: starts embedded Postgres and loads seed data"]
fn empty_db_recreates_factory_once_and_existing_db_does_not() {
    if !crate::common::own_process() {
        return;
    }
    let root = std::env::temp_dir().join(format!("moqui-factory-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let runtime = root.join("runtime");
    let component = runtime.join("component/probe");
    std::fs::create_dir_all(&component).unwrap();
    std::fs::create_dir_all(runtime.join("conf")).unwrap();
    std::fs::write(component.join("component.xml"), "<component name=\"restart-probe\"/>").unwrap();
    std::fs::write(
        runtime.join("conf/MoquiDevConf.xml"),
        r#"<moqui-conf>
        <component-list><component-dir location="component"/></component-list>
        <tools empty-db-load="seed,seed-initial,install"><tool-factory class="restart::Probe"/></tools>
        </moqui-conf>"#,
    )
    .unwrap();
    let framework = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    moqui_lib::env::set_property("cluster_https_port", "0");
    moqui_lib::env::set_property("cluster_ssh_port", "0");
    let bind = ([127, 0, 0, 1], 0).into();
    let server = moqui_host::start_server(&runtime, &framework, root.join("db"), bind).unwrap();
    assert_eq!(*EVENTS.lock().unwrap(), ["empty", "destroy", "seeded"]);
    {
        let factories = FACTORIES.lock().unwrap();
        assert!(factories[0].upgrade().is_none(), "bootstrap factory must be released");
        assert!(Arc::ptr_eq(&factories[1].upgrade().unwrap(), server.factory()));
    }
    drop(server);
    assert_eq!(*EVENTS.lock().unwrap(), ["empty", "destroy", "seeded", "destroy"]);
    EVENTS.lock().unwrap().clear();
    let server = moqui_host::start_server(&runtime, &framework, root.join("db"), bind).unwrap();
    assert_eq!(*EVENTS.lock().unwrap(), ["seeded"]);
    drop(server);
    assert_eq!(*EVENTS.lock().unwrap(), ["seeded", "destroy"]);
    std::fs::remove_dir_all(root).unwrap();
}
