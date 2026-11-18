---
name: pre-production
description: The validation of mr-moqui before a release or a production deploy: the lints of the whole tree, the whole regression suite, every acceptance test, the client apps, optional load tests, the release build and archive, and the upgrade's conf check. Use only when preparing a release or a deploy, or when the user asks.
---

# Before a release or a production deploy

The list and its detail are in [Build-Test, Before a release](../../../docs/Build-Test.md#before-a-release). Each step
runs on the result of the one before; a failure stops the release until it is fixed.

1. **The tree.** Clean working trees, every repository at the release's commits: `./moq git-status`, and
   `./moq component-get` for the components `addons.xml` names.
2. **The lints**: `cargo lint`, `cargo lint-min`, `scripts/check-packages.sh`.
3. **The regression suite**: `./moq test-all`.
4. **The acceptance suite**: `./moq accept-all --junit <dir>` (every component's `test/acceptance/`, and the Rust
   ignored and opt-in tests). Keep the JUnit directory for the record. A test that fails on two runs in a row is fixed
   or deleted.
5. **The client apps that ship**: the desktop and mobile tests (their READMEs), `scripts/check-android.sh`, and the
   soak tests the plans name for a release.
6. **Load (optional)**: the components' load scripts (`./moq test-load <component> --script <Name>`) against a release
   build with production-like data, compared with the last release's results ([Load-Runner](../../../docs/tools/Load-Runner.md)).
7. **The build**: `./moq build --release`, `./moq archive`, `moqui embedded checksum`, as CI's `release` pipeline does.
8. **The upgrade**: boot the target runtime's conf once (a retired `default-property` or a `${}` in an attribute
   refuses the start, naming each), check its names with `moqui conf env`, and write any setting the release renamed
   or added into its upgrade notes ([Run-Deploy](../../../docs/Run-Deploy.md#settings-and-the-environment)).

Report what ran, what passed, and each failure with its output; a step skipped is named as skipped.
