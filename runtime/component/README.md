Put your add-on component directories in this directory to load them when Moqui runs.

## Rust in a component

A component can have a Rust crate, as a Java Moqui component could have a `build.gradle`: a `Cargo.toml` and `src/`
beside its `component.xml`. `./moq build` finds each one here and links it into the moqui binary with the framework,
through a crate it generates in `target/moqui-app`. The binary is still `target/release/moqui`, or `target/debug/moqui`
with `--debug`. Run `./moq build` again after adding or removing a crate.

The crate depends on the framework by path (`moqui-core = { path = "../../../crates/moqui-core", default-features =
false }` and `moqui-lib = { path = "../../../crates/moqui-lib" }`) and submits its implementations under the
component's name with `moqui_core::native::submit_component!`. Its code is used only when the component loads, and the
component's XML picks it:

- service definitions name `type="rust"` locations the crate binds;
- `MoquiConf.xml` selects tool factories (`<tool-factory class="..."/>`), resource schemes, template renderers and
  custom service types by the identifiers the crate registers.

Scripts get a tool with `ec.factory.getTool("Name")` and call the methods it exposes.

The full guide is `docs/Native-Components.md`. `runtime/base-component/example` has an example of each part,
with specs (`./moq test example -D example_disabled=false`); its `src/` works as a template, and its `README.md` has
the `Cargo.toml` for a crate here, which is not a member of the repository's workspace.
