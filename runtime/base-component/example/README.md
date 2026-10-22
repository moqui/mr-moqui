# example

Examples of Rust in an MR Moqui component, kept with the framework and added to over time, and the screen archetype
gallery of the IronStone BOSS style guide (`screen/Gallery.xml`, at `/qapps/gallery` once signed in). The component
is on by default while the framework is young.

## Turning it off

- For one run: `./moq run -D example_disabled=true`. `component.xml` has `disabled="${example_disabled ?: 'false'}"`,
  so the property, as a `-D` option or an environment variable, decides.
- In a conf: a `component-list` that names components loads only those, so one that leaves `example` out turns it
  off; one that names it loads it even when its `component.xml` is disabled:

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
| `screen/Gallery.xml`, `screen/Gallery/*.xml`, `*.qvue` | The screen archetype gallery of the IronStone BOSS style guide (`runtime/component/ironstone-boss/plan/style-guide.md` §3, §4 and §10): one screen per archetype over framework data, mounted at `/qapps/gallery` by `MoquiConf.xml`; `Board` and `Calendar` are `.qvue` screens, and `QuickTask` a `.qvue` quick-create kind (its own query through `inject: ["mScreen"]`, a `$q.dialog` confirm, `shellRefresh()` in a tray tab). |
| `data/ExampleSecurityData.xml` | Seed authorization: the gallery for the ADMIN group. |
| `entity/ExampleEntities.xml`, `data/ExampleDemoData.xml` | `moqui.example.Example` (Java's example record: a type, a status in a flow, a size, and a start and end) with its types, statuses and flow as seed data and eleven demo records, for the gallery's `Views` screen: one form-list as a board by status or type (`group-fields`, `all-rows`) and a calendar (style-guide §8.2, `views`, `view-board`, `view-calendar`) whose moves go through `update#moqui.example.Example`, with a peek pane (`peek-url`, `screen/Gallery/ExampleDetail.xml`), and a work breakdown as a tree (`view-tree`) over the inline rows of `get#GalleryWorkItems` in `service/example/ExampleServices.xml`. |
| `test/Gallery.spec.js` | Renders every gallery screen in its render mode through `ec.screen.makeTest()` (`./moq test example`). |
| `test/gallery-shots.mjs` | Screenshots and text of `/qapps` screens through Chrome DevTools against a running server, for checking the gallery in light and dark and at phone width. |

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
