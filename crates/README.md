# crates

The framework's Rust crates. Which crate holds what, and the rule for where new code goes, are in
[Practices-Rust §1](../docs/dev/Practices-Rust.md#1-crates-and-where-code-goes); read that guide before working on
Rust code here.

- **The workspace** is every crate here except three: `moqui-desktop`, `moqui-mobile` and `moqui-client-tauri` are
  workspaces of their own (the Tauri apps), built and tested by the commands in their READMEs, and `cargo lint` and
  `cargo test` at the root do not reach them.
- **The binary** is `moqui` (crates/moqui), or the generated `target/moqui-app` when a component under
  `runtime/components/addon` has crates of its own; `./moq build` makes either ([Build-Test](../docs/Build-Test.md#build)).

## Test and check

While working, `cargo test -p <crate>` for the crate you change. Before a commit, for any change here:

```sh
cargo lint          # clippy on the workspace, warnings denied
cargo lint-min      # the host and every crate it links, with no default features
./moq test-framework
```

and `scripts/check-packages.sh` when feature gates, features or dependencies changed. The rest, by what changed
(goldens, the crates outside the workspace, conf), is in [Build-Test, Validation](../docs/Build-Test.md#validation).
