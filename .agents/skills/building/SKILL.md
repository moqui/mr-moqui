---
name: building
description: How to build mr-moqui without wasting the build cache or the machine: one toolchain, incremental builds, a target directory per worktree, and the limits for parallel agents. Use before building or working in a worktree.
---

# Building

- Building: use one consistent compiler version to preserve cached artifacts; prefer incremental development builds over optimized release builds for iteration; avoid unnecessary cache clearing.
- Worktrees: keep build caches separate to avoid cross-worktree lock contention and artifact churn; concurrent builds still compete for CPU and memory, so measure build performance without competing builds.
- Parallel agents: each worktree build is a cold build of the whole workspace, and sccache cannot share incremental workspace crates across paths. Build Rust only when the change touches `.rs` files; otherwise run specs on an already built `moqui` binary. Cap each build with `-j 4` (or `CARGO_BUILD_JOBS=4`), keep at most two worktrees building at once, run only the specs for the change, and build a release binary once, at the end.
- A second worktree never shares this one's `target/` (`CARGO_TARGET_DIR`): workspace crates are fingerprinted by their
  path within the workspace, so both trees write the same artifact names, and cargo reports the other tree's binary
  as fresh without rebuilding. Give each worktree its own target directory, or copy a built binary aside before
  switching trees.
- Rust is built only when `.rs`, `Cargo.toml` or `Cargo.lock` changed; specs and screens run on the binary already
  built (`--nobuild`). After a Rust change, build before a spec run, or it tests the old code.
