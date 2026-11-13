# The add-on set

Put add-on component directories here to load them when Moqui runs (`../components.xml` lists this set);
`./moq component-get <name>` checks one out, and a git deploy from System > Components puts one here.

A component can have a Rust crate, a `Cargo.toml` and `src/` beside its `component.xml`, as a Java Moqui component could
have a `build.gradle`. `./moq build` links each one here into the moqui binary through a crate it generates in
`target/moqui-app`; run it again after adding or removing a crate. The guide is `docs/dev/Native-Components.md`, and
`runtime/components/dev/example` has an example of each part and the `Cargo.toml` for a crate here.
