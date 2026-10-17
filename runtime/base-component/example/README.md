# example

Examples of Rust in an MR Moqui component, kept with the framework and added to over time. The component is off by
default.

## Turning it on

- For one run: `./moq run -D example_disabled=false`, or `./moq test example -D example_disabled=false`.
  `component.xml` has `disabled="${example_disabled ?: 'true'}"`, so the property, as a `-D` option or an
  environment variable, decides.
- In a conf: name it in the `component-list`, which loads a component even when its `component.xml` is disabled:

  ```xml
  <component-list><component name="example" location="base-component/example"/></component-list>
  ```

The binary always links this crate. Its implementations are only used when the component loads.

## What is here

| File | Shows |
| --- | --- |
| `Cargo.toml` | A component crate on `moqui-core` and `moqui-lib`; here from the workspace, by path in `runtime/component`. |
| `src/lib.rs` | `submit_component!`, which puts the crate's implementations in the linked catalogue under the component name. |
| `src/services.rs` | A `type="rust"` service function bound in the catalogue at `service_location!(echo)`, `example::services::echo`. |
| `src/example_tool.rs` | A `ToolFactory` with `pre_facade_init` (binding a closure service over its state), `init`, `get_instance`, `type_declaration` and `destroy`. Two `Tool`s show methods, a property, `getTool` parameters, and a method that returns another tool. |
| `MoquiConf.xml` | Selects the tool factory by the class the crate registers, `example::ExampleToolFactory`. |
| `service/example/ExampleServices.xml` | The two rust service definitions, and XML Actions that keep the tool in a field and call it. |
| `test/Example.spec.js` | Specs for each of these. |

## Rust in your own component

Put `Cargo.toml` and `src/` at the root of the component, in `runtime/component/<name>`, as this one is. A crate there
is not a member of the mr-moqui workspace, so where this `Cargo.toml` inherits it names its own values: a `version`,
`edition = "2021"` and so on in `[package]`, and these dependencies, with no `[lints]` table:

```toml
[dependencies]
moqui-core = { path = "../../../crates/moqui-core", default-features = false }
moqui-lib = { path = "../../../crates/moqui-lib" }
tracing = "0.1"
```

`./moq build` finds the crate (a `Cargo.toml` beside a `component.xml`) and links it into
`target/{release,debug}/moqui` with the framework, through a generated crate in `target/moqui-app`.

- **Selection.** `submit_component!` names the component; the crate's code is used only when that component loads. The
  component's XML picks what runs: service definitions name the locations, and `MoquiConf.xml` selects tool factories
  by class, with `init-priority` and `disabled`.
- **Lifecycle.**
  - `pre_facade_init` runs at the end of the factory boot, with no entity facade yet. Bind closure services there.
  - `init` runs once the entity facade is attached, before data loads and the first request.
  - `destroy` runs at shutdown, in reverse order, after the worker pools stop, and also when an init fails.
- **Tools in scripts.** A script calls a name in `Tool::methods` as a method. It reads any other name through
  `Tool::property`. Arguments and results convert as service parameters do. The object stays the tool when the
  context stack keeps it, so `<set field="tool" from="ec.factory.getTool('X')"/>` followed by `tool.method()` works.
- **Rust services.** They take `(&ExecutionContext, &ServiceInfo, &Parameters)` and run in the caller's execution
  context and transaction. `moqui_core::user` and the other facades work as they do in the framework's own services.

The full guide is `docs/Native-Components.md`.
