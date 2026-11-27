---
name: pre-commit
description: The checks to run before committing in mr-moqui or a component repository, chosen by what changed (Rust, framework artifacts, conf, an application, docs), and how to read their results. Use before every commit.
---

# Before a commit

The table and its detail are in [Build-Test, Validation](../../../docs/Build-Test.md#validation); this is the
procedure.

1. **See what changed, in every repository.** mr-moqui and each component repository under
   `runtime/components/<set>/` are separate repositories (`./moq git-status`). Classify the changes: Rust in a workspace
   crate, feature-gated code or `Cargo.toml`, a crate outside the workspace, a golden's source, the framework's
   components or conf or `moq`/`scripts`, an application, an acceptance test, docs.
2. **Run each row that applies** (a check named twice runs once):
   - Rust: `cargo lint`, `cargo lint-min`, then `./moq test-framework`. Both lints, for any crate: `lint-min` builds
     the host's whole graph with no default features and is what catches an item only a feature uses.
   - Feature gates, features or dependencies: also `scripts/check-packages.sh`.
   - The desktop, mobile or client-tauri crate: `./moq test-clients` (its crate alone: `./moq test-clients desktop`), and
     `scripts/check-android.sh` for Android.
   - A golden's source (`moqui-host.d.ts`, a proto, the bundled screens): rewrite it by its variable, read the diff.
   - The framework's components, the conf or its XSDs, `moq`, `scripts/`: `./moq test-framework`.
   - An application: `./moq test-apps`, and `moqui lint --screens` for a changed screen.
   - An acceptance test you added or changed: that test alone (`./moq accept <component> --spec Name`). Its
     component's whole acceptance suite only with the user's permission; no other acceptance test, no load test, no
     `./moq test-all` unless the user asks.
   - JS lockfiles, build inputs, fonts or patches: `./moq verify-js`, plus focused browser specs for the affected consumers.
   - Docs only: no build; check the links.
3. **Read the results** ([Reading the results](../../../docs/Build-Test.md#reading-the-results)):
   - `cargo test` stops at the first failing binary: `cargo test --workspace --no-fail-fast` to see them all.
   - A test that fails only in the full run: run it alone, twice. Passing alone is flakiness under load (report it);
     failing alone is a regression.
   - A spec that fails only after other specs: bisect on the run's prefix, fix the spec that left the rows.
   - After a Rust change, `--nobuild` tests the old binary (`no service named ...`): build first.
4. **Format only your files** (`rustfmt --edition 2021 --config-path rustfmt.toml <file>`, Prettier for JS and XML),
   then read `git diff -U0` for hunks that are not yours and put them back.
5. **Commit in each repository that changed**, one topic a commit, the message saying what changed and why (and any
   behaviour a caller sees change). Fix a failure before committing; if one is unrelated and fails alone on the
   committed tree too, say so in the report rather than hide it.
