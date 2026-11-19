# Patched crates

`[patch.crates-io]` in the workspace's `Cargo.toml` replaces these crates.io crates with the copies here, for every
package that depends on them. A workspace of its own that builds the framework names them again, as cargo reads
patches only from the root of the workspace it builds: `crates/moqui-desktop/Cargo.toml` (kept equal by
`crates/moqui-client/tests/app_workspaces.rs`) and the generated `target/moqui-app` (`root_patch_table`,
scripts/moq/build.sh). Each is the crate's published source, less its benchmarks, with the changes below marked
`MR Moqui patch` in the code; each is excluded from the workspace, so it is built as a dependency, and its manifest
allows the warnings the published source has. A fork on GitHub, as stun-proto's ([Forks](../docs/Build-Test.md#forks)),
is the place for one that is offered upstream.

## yrs 0.28.1

`Any::decode` (`src/any.rs`) read a map or an array of a Yjs update by a recursion once a level with no limit, so an
update of arrays nested two bytes a level (a co-editing update is up to 16 MiB) overflowed the thread's stack, which
aborts the process; and a value that deep is dropped by a recursion too (plans/Security-Review-Medium.md SEC-TLS-95).

- `src/any.rs`: `decode` reads at most `MAX_DECODE_DEPTH` (128, as serde_json) levels of maps and arrays; a deeper value
  is `Error::UnexpectedValue`, so `Update::decode_v1` refuses the update. What is decoded is at most that deep, so its
  drop and its encoding are bounded too. `Any::from_json` already stops at serde_json's 128.
- `Cargo.toml`: no `[[bench]]` targets; `[lints.rust] warnings = "allow"`.
- `LICENSE`: upstream's MIT license, from the y-crdt repository at the `v0.28.1` tag (the published crate has none).

Test: `an_update_nested_far_past_the_decode_limit_is_refused_on_a_small_stack` (`crates/moqui-collab/src/tests.rs`).

To move to a newer yrs: copy its published source here (less `benches` and their `[[bench]]` tables), apply the same
change if upstream has not, run that test, and go back to the crates.io release once upstream bounds the depth.
