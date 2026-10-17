# Writing a native component

A component is still a directory with `component.xml`, optional `MoquiConf.xml`, and service/screen/entity XML. Add a Cargo library for native code: `Cargo.toml` and `src/` at the component root, as a Java component had `build.gradle` and `src/`.

## Building

**With `./moq build` (the usual way).** Put the component in `runtime/component/<name>`. `./moq build` finds each crate there (a `Cargo.toml` beside a `component.xml`) and generates `target/moqui-app`, a binary crate that names every component crate (`use my_component as _;`) and calls `moqui_host::main()`. The binary is still `target/release/moqui`, or `target/debug/moqui` with `--debug`. Its `Cargo.lock` starts from the repository's. Run `./moq build` again after adding or removing a crate. The crate depends on the framework by path:

```toml
[dependencies]
moqui-core = { path = "../../../crates/moqui-core", default-features = false }
moqui-lib = { path = "../../../crates/moqui-lib" }
```

**With an application-owned binary.** An application can own its `Cargo.toml` and lockfile instead, in its own workspace, with path or Git dependencies on the same MR revision:

```rust
use my_component as _; // retain the component's linked catalogue entry

fn main() {
    moqui_host::main();
}
```

Components kept in the framework repository, such as `runtime/base-component/example`, are workspace members that `crates/moqui` links the same way.

A `component.xml` with `disabled="true"`, which can be a property (`disabled="${my_component_disabled ?: 'true'}"`), is skipped when a component directory is scanned, unless a conf `component-list` names it. A linked crate whose component is not loaded contributes nothing.

When a crate brings new dependencies, check their licenses and advisories over the whole graph: `cargo deny --manifest-path target/moqui-app/Cargo.toml check`, from the repository directory so its `deny.toml` applies.

## XML-defined native service

In the component library:

```rust
use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeComponent, NativeRegistry, Parameters, ServiceInfo, submit_component};
use moqui_lib::error::Error as LibError;

submit_component! {
    NativeComponent { name: "my-component", register: implementations }
}

fn implementations(registry: &mut NativeRegistry) -> Result<(), Error> {
    // service_location! is the function's Rust path, here "my_component::echo"; any identifier works.
    registry.service_function(moqui_core::service_location!(echo), echo)
}

fn echo(_ec: &ExecutionContext, _service: &ServiceInfo<'_>, parameters: &Parameters)
    -> Result<Parameters, LibError>
{
    Ok(parameters.clone())
}
```

`component.xml` must name `my-component`. Its normal runtime discovery is what enables this crate's implementations. Merely linking the crate does not enable its code for another runtime.

In `service/Example.xml`:

```xml
<services>
    <service verb="echo" type="rust" location="my_component::echo">
        <in-parameters><parameter name="message" required="true"/></in-parameters>
        <out-parameters><parameter name="message"/></out-parameters>
    </service>
</services>
```

Call `Example.echo` through `ec.service` as usual. The implementation binding does **not** create this service.

An implementation that needs state, such as a tool factory's, can be a closure bound on the factory in `ToolFactory::pre_facade_init`, before any service definition is read:

```rust
let counter = Arc::clone(&self.counter);
factory.register_service("my_component::counter_value", move |_ec, _service, _parameters| {
    Ok(Parameters::from([("count".into(), Value::I64(counter.load(Ordering::Relaxed)))]))
})?;
```

Catalogue functions and closures share one lookup per factory; a location bound twice, or already a built-in, is an error. XML defines authentication, validation, transaction policy, outputs and ECAs. A native function receives the current EC and can use its factory, entities and tools. Native locations are explicit identifiers, not dynamically loaded Rust symbols.

## Tools

Register a constructor with `registry.tool("my_component::ClientFactory", constructor)`. Implement `native::ToolFactory`, returning `Arc<dyn Tool>` from `get_instance`. Select it in the component's `MoquiConf.xml`:

```xml
<moqui-conf>
    <tools><tool-factory class="my_component::ClientFactory" init-priority="20"/></tools>
</moqui-conf>
```

- `pre_facade_init` runs during core boot, before entity attachment. Lower numeric priorities run first, ties sort by implementation identifier.
- `init` runs after host entity/search setup, before startup data loading. Direct core embedders call `factory.init_tools()` after attaching their facades. Initialization is serialized and idempotent; it must not recursively initialize or shut down tools.
- `factory.shutdown()` stops request/service workers before closing tools; direct embedders should call it after stopping listeners. `destroy` runs in reverse order on shutdown or failed initialization; `post_facade_destroy` runs in reverse order when the factory's facade fields have been released. Cleanup must tolerate partial initialization and must not panic.
- Never retain a strong `Arc<ExecutionContextFactory>` in a tool/provider/renderer: it would form a cycle. Use a weak reference or independent factory-local state.
- Rust: `factory.get_tool_typed::<MyTool>("Client", &parameters)` for an instance, `factory.get_tool_factory_typed::<MyFactory>("Client")` for the factory itself.
- JS: `ec.getTool("Client", ...parameters)`, `ec.factory.getTool(...)`, or `ec.factory.getToolFactory("Client").getInstance(...)`. A missing factory returns `null` from `getToolFactory`; `getTool` throws. No Java class argument is used.
- Implement `Tool::methods()` and `Tool::call()` to expose explicit JS methods accepting/returning plain `Value` data, and optionally `Tool::property()` for names that read as properties (Groovy getters). Neither reflection nor arbitrary Rust objects cross the boundary. Stateful/session tools may return a fresh instance each time. The instance must reject calls after shutdown if outstanding references remain.
- The script object holds the tool, not data: kept on the context stack (`<set field="tool" from="ec.factory.getTool('Client')"/>`) and read back, it still has its methods. A method can return another tool with `native::tool_value(Arc<dyn Tool>)`, and Rust code can put one in the context the same way. The object is read-only.
- `ToolFactory::type_declaration()` gives the instance a TypeScript type, which `moqui types` writes to `tools.d.ts` (`MoquiToolMap`), so `ec.factory.getTool("Client")` is typed.

## Other implementation bindings

- `registry.resource(class, constructor)` + `<resource-reference scheme="aws3" class="..."/>`: implement `ResourceProvider`. Constructors can obtain factory-local tools/state. Avoid I/O during construction when it requires a tool's normal initialization. Reads, stat, listing and mutations route through the same provider; unsupported mutations return errors. Providers are responsible for sim/outbound policy.
- `registry.template(class, Arc<dyn TemplateRenderer>)` + `<template-renderer extension="cwiki" class="..."/>`: reuse the existing renderer interface. No service or tool is required. Registry renderer objects are created per factory.
- `registry.runner(class, function)` + `<service-type name="custom" runner-class="..."/>`: XML services with `type="custom"` call this runner. `ServiceInfo` contains the XML service name/type/location/method. Keep runner state in a tool or factory extension. Built-in service types cannot be overridden.

Existing built-in XLSX/image/SFTP/IPP support is unchanged. This API does not load JARs, Java tool classes, Java widget renderers, servlet filters, FOP, Hazelcast or Drools. Port component code and explicitly select its Rust implementation identifiers. Rebuild the application whenever native code/dependencies change.

Executable public-API examples and tests: `crates/moqui-core/tests/native_components.rs`. A worked component, off by default, with specs: `runtime/base-component/example` (`./moq test example -D example_disabled=false`).
