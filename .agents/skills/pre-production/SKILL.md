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
8. **The source release**: produce the offline source archive from the clean release tag. Verify the unpacked artifact in the network-disabled release job with `./moq build --offline --release` and `./moq test-framework`.
9. **The upgrade**: boot the target runtime's conf once (a retired `default-property`, a `${}` in an attribute or a
   renamed attribute refuses the start, naming each and its replacement), check its names with `moqui conf env`, and
   write any setting the release renamed or added into its upgrade notes
   ([Run-Deploy](../../../docs/Run-Deploy.md#settings-and-the-environment)). Renamed so far (`RETIRED_ATTRIBUTES` in
   `conf_env.rs`): `pass-authority@grpc-host` and `telemetry/receiver@grpc-bind` to
   `webapp-list/webapp[webroot]@grpc-listen-bind`, `pass-authority@grpc-port` and `telemetry/receiver@grpc-port` to
   `webapp-list/webapp[webroot]@grpc-listen-port`, `ssh-server@login-dialog` to `ssh-server@access` (`true` is `open`,
   `false` is `login`, plans/Runtime-Config.md §3.4); check the runtime's conf files, `.env` and the service
   environment.
   The listener switches (plans/Runtime-Config.md §3.2; `RETIRED_VALUES` in `conf_env.rs`): `off` or an empty
   `http-listen-port`, `https-listen-port` or `grpc-listen-port`, and an empty `ssh-server@port`,
   `smtp-server@listen-port` or `media-server@port`, refuse the start naming the switch to set; a port alone, or
   `xmpp-server@domain` alone, no longer turns a listener on, so write each listener's `enabled` (the boot's upgrade
   warning names any that was set without it); `http-listen-port` no longer follows `http-port`; XMPP switched on
   listens on 5222 and 5223 unless they are written empty.

Report what ran, what passed, and each failure with its output; a step skipped is named as skipped.
