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
| `screen/Gallery.xml`, `screen/Gallery/*.xml`, `*.qvue` | The screen archetype gallery of the IronStone BOSS style guide (`runtime/component/mr-boss/plan/style-guide.md` §3, §4 and §10): one screen per archetype over framework data, mounted at `/qapps/gallery`; `Board`, `Calendar` and `QuickTask` show `.qvue` screens. |
| `data/ExampleSecurityData.xml` | Seed authorization: the gallery for the ADMIN group. |
| `entity/ExampleEntities.xml`, `data/ExampleDemoData.xml` | `moqui.example.Example` (a type, a status flow, a size, a start and end) with seed and demo data, for the gallery's `Views` screen: one form-list as a board, a calendar and a tree. |
| `test/Gallery.spec.js` | Renders every gallery screen in its render mode through `ec.screen.makeTest()` (`./moq test example`). |
| `test/client/GalleryShots.spec.js` | Screenshots of the gallery in the `/qapps` shell (`moqui test-client example --serve --spec GalleryShots`): light at 1280, List and Detail in the dark and at phone width, and the Assist drawer on List. |

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

`./moq build` finds the crate (a `Cargo.toml` beside a `component.xml`) and links it into the binary with the
framework. How the crate's code is selected, the tool and service lifecycles, and calling tools from scripts are in
`docs/Native-Components.md`.
