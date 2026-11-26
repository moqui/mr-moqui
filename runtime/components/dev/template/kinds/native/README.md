# @component@

@Title@, a native MR Moqui component: Rust in `src/` that `./moq build` links into the binary, with the service
definitions that name its functions.

| Path | What is here |
| --- | --- |
| `component.xml` | The component: its name (the one `src/lib.rs` submits its implementations under) and version. |
| `Cargo.toml` | The crate, on `moqui-core` and `moqui-lib` by path. It is not a member of the mr-moqui workspace. |
| `src/lib.rs` | `submit_component!` and `echo`, a function bound at the location `@package@::echo`. |
| `service/@package@/@Class@Services.xml` | `echo#Message`, a `type="rust"` service at that location. |
| `data/@Class@SecurityData.xml` | Seed: the component's services, by their package, for the ADMIN group. |
| `test/@Class@Native.spec.js` | The regression suite: the service answers. |

## Build, run and test

- `./moq build` finds the crate (a `Cargo.toml` beside the `component.xml`) and links it into the binary; build again
  after a change to `src/` or `Cargo.toml`, then restart. In the Dev AIDE the restart banner's Rebuild runs it.
- `./moq test @component@` runs its regression specs on the binary that links it.

Tools, other bindings and the lifecycles: `docs/dev/Native-Components.md` in mr-moqui; a worked component is
`runtime/components/dev/example`.
