# Component hot-deploy and live conf reload

> Archived 2026-11-04: Phases 0 to 4 built (groundwork, live conf reload, load from a local directory, deploy from
> git, cluster fan-out and the start's restore), with the items the first build left out finished the same day (see
> "As built"). The live checks are done. The guide is [Component-Deploy](../../docs/tools/Component-Deploy.md);
> nothing is left open.

## Context

Today the set of components and the merged conf are fixed for the life of the process. `factory.components` is a plain
`Vec` (`crates/moqui-core/src/context/factory.rs:147`), `ResourceFacade.components` is a fixed map
(`context/resource.rs:382`), and `conf_text` is an immutable `String`. About 30 consumers parse the conf once into
extensions that cannot be replaced. Adding a component, or changing a `MoquiConf.xml` or the runtime conf, takes a restart
(`docs/Run-Deploy.md:1263`; devkit `reload.rs` reports "restart to apply").

The goal has two parts:
1. **Component hot-deploy.** From the System app, deploy a component from a git URL and branch: clone it, load it live,
   and load its data (seed, seed-initial, install by default, chosen from the types its files use).
2. **Live conf reload.** Apply what changed in conf files, where that is reasonable.

Both rest on one core piece: re-merge the conf, compare it with the running conf by liveness unit, apply the units that
can be applied live, and name the ones that need a restart. Because the merge is recomputed in full, a setting that a
later component or the runtime conf overrides correctly shows as no change.

Much already reloads and is reused: entity relink with additive DDL (`moqui-entity/src/entity/relink.rs`), and the
service, SECA, EECA, REST, screen and skill reloads in `crates/moqui-devkit/src/reload.rs`. Also reused: the screen
cache `forget`/`clear`, `load_entity_data` (`entity/load_feed/loader.rs:77`), and the git `import` into a `db`
repository (`crates/moqui-git/src/lib.rs:1016`).

**Decisions:**
- **Gate.** Deploy needs a new sealed `ADMIN_ADV` permission. In production it is also off unless
  `component_deploy_enabled=true`; otherwise it is refused with a vague error. Dev and test instances need only the
  permission.
- **Storage.** The user picks it per component, on any instance:
  - **database** (default): a `db` GitRepository;
  - **bare repo on disk**: a `remote` mirror under `runtime/git/<id>`;
  - **working clone**: a normal clone in a component dir, which can be edited and committed.
- **Conf files.** On dev instances the file watcher applies changes. Everywhere there is a System › Conf screen, a
  `reload#Conf` service, and SIGHUP.

## As built (2026-11-04)

Where the build differs from the phases below:

- **Running conf.** A conf reload diffs by liveness unit (`conf_reload/catalog.rs`, `diff.rs`). A change that waits
  for a restart is kept out of the running conf, so a later reload lists it again until the restart.
- **Restart-class sections.** These stay restart-class, not applied: `telemetry` (Monitor › Settings changes it for a
  while), new `tool-factory`, `resource-reference` and `load-entity` entries, and `edge`.
- **Properties.** A property that a restart-class unit reads is not set live, so the two never disagree.
- **Screen generation.** The conf generation is folded into the screen config generation, so a definition built under
  the old conf is stale even in production.
- **SIGHUP.** It reloads only when stdin is not a terminal. Under `./moq run` in a terminal it still ends the server.
- **Guard.** The `web-protection` `connections` and `requests` children (the listener limits) are restart-class. The
  guard takes the rest in place.
- **Entity relink.** Relink runs its DDL against the new model on its own thread (`SharedModel::with_staged`) before
  the swap. On Postgres it sets a 5 s lock timeout and retries once. A failed step keeps the old model and directories.
- **Record caches.** Single-record finds are list-cache entries, so `cache::forget_model` already clears them.
- **Data load.** The JS `makeDataLoader` with types or components also gathers the components' `data` files, and
  `conf_data_locations` reads the merged conf.
- **Working clone.** It uses the system `git` command (clone, fetch, fast-forward, status). gitoxide cannot update a
  worktree in place. The database and bare storages are in-process (`moqui-git` `checkout.rs`).
- **Git in devkit.** `moqui-devkit` has an optional `moqui-git` dependency under its `git` feature, which the host's
  `git` feature turns on. `deploy/git_off.rs` is the Lite stub.
- **Load order.** A component staged for a git deploy is previewed after the loaded components. The deploy then loads
  it through discovery, which checks the order again.
- **Remove.** It moves the directory to `.removed/` beside it and takes effect at the next start.
- **Dashboard.** System › Components is added. The dashboard's read-only component list stays.
- **Cluster.** Only the database storage is fanned out (`component-deploy` topic). The others, and a load from a local
  directory, are refused while the server has live cluster members.
- **Live checks** (done 2026-11-04): a fetch from a real remote, and a deploy that reaches a second cluster member.

Finished 2026-11-04, after a review of the plan against the code:
- **Working clone fetch.** The system `git` clone and fetch of a working clone pass the sim mode and `RemoteCall` fence
  checks first, as the in-process fetch does (`deploy/git.rs` `network_allowed`).
- **Relink reads the running conf.** The entity model's conf text is replaced with each conf reload
  (`EntityFacade::replace_conf_text`, called by the factory's `replace_conf`), so a relink parses the running conf and
  `conf_data_locations` sees the `load-data` entries a loaded component's `MoquiConf.xml` added. A new `load-entity`
  entry stays restart-class: the facade's entity files are the ones it opened with.
- **Rollback.** A rolled-back load drops the component's `component://` root (`replace_components` drops the roots of
  components the new list leaves out).
- **Rust check.** `.rs` files in `src/` count as Rust code too, not only a `Cargo.toml`.
- **`load#ComponentData`** loads a loaded component's data again (Load Data on System › Components).
- **Restart modes.** `mode=restart-now` on `load#Component`, `deploy#Component` and `update#Component` (Deploy and
  Restart, Update and Restart), and Restart Now on System › Conf, all where `restart#Server` is allowed
  (`get#PendingRestart` answers `restartAllowed`).
- **Remove of a dirty working clone** answers the changes (`dirty`) and asks again; `confirmDirty` removes it anyway.
- **Dev AIDE.** Get and Clone preview the component and offer Load when nothing waits for a restart
  (design-dev-aide §4.7, §6.5).
- **Docs.** A deploy section in `docs/security/Attack-Surface.md`; the guide covers the above.

Left as built, on purpose:
- Deploy from Git is a box on `System/Components.xml`, not a subscreen, and Conf is `System/Conf.xml`, not a
  directory.
- A deploy whose load fails puts the files back in `.staging-<name>`, not `.failed/`, so Discard or a new preview
  handles them.
- Only a failure of the conf step rolls a load back. A problem in a file of the reload step is that file's problem,
  and the rest go on (the failure-tolerance rule in AGENTS.md).
- `Extension::after_artifacts_changed` takes no ChangeSet, and only moqui-report uses it. Skills resync through the
  devkit's own hook (`reload.rs` `sync_skills`).
- ScreenConf and the screen check interval are built again by `ScreenFacade::reconf`, not `conf_derived`.
  ClientAddressing is read once, and its attributes are restart-class.
- `components()` answers an `Arc<Vec<ComponentInfo>>`.
- The catalog coverage test checks the top-level elements of `MoquiDefaultConf.xml`.
- A component with Rust code is refused live even when the binary links it: native code registers only at a start.
- The conf step of a load fails only when the running conf does not parse, which no fixture can bring about, so there
  is no test of that rollback. The deploy `it` test checks the rollback's own step instead: putting the earlier
  component list back drops the component's `component://` root.

## Phase 0: groundwork refactors (no behaviour change)

- **Swappable state on the factory.** `factory.rs` gets:
  - `components: ArcSwap<Arc<[ComponentInfo]>>`, with `components()` returning an `Arc<[ComponentInfo]>` (`load_full`,
    because callers do file I/O while iterating);
  - `conf: ArcSwap<ConfState { text: Arc<str>, cache: OnceLock<ConfCache>, generation: u64 }>`, with `conf_text()`
    returning `Arc<str>`;
  - `framework_component_dir` kept as its own field (it currently borrows from the list, `factory.rs:503`);
  - `BootOptions.test_confs` kept, so a re-merge under `moqui test` stays the same.

  arc-swap is already a dependency of moqui-core. Fix the ~56 `components()` and ~29 `conf_text()` call sites where
  the compiler says so.
- **Resource roots.** `ResourceFacade.components` goes behind an `RwLock` and gets `register_components()`.
  `locate::LAST_BOOTED` is updated by the same call.
- **Split entity relink** (`relink.rs`) into three steps:
  1. **Prepare.** Load and link the new model offline, and refuse new problems.
  2. **DDL.** Run it against the prepared model with `SET LOCAL lock_timeout` and a retry, as `search/postgres/mod.rs:1458`
     does.
  3. **Swap.** Swap the model in.

  Today relink swaps first, so an `extend-entity` column is used before it exists, and a failed relink leaves the
  directories registered. After this change, `set_component_entity_dirs` is called only after a successful swap, and the
  record caches of extended entities are cleared (`cache::forget_model` clears only list and count caches). Relink
  should parse the current conf, not the boot conf (`relink.rs:55`).
- **Data locations.** Move `component_data_locations` and `data_directory_locations` (`moqui-host/src/lib.rs:2250-2330`)
  into `moqui-entity::load_feed`, beside `location_in_components`, taking `(name, path)` pairs. Do the move in its own
  commit (Practices-Rust §1).
- **Data-load gaps.** `conf_data_locations` should read `load-data` from the merged conf, not only
  `MoquiDefaultConf.xml` (`load_feed/mod.rs:152`). This also fixes JS `makeDataLoader` with `components` and no
  location (`tools_host.rs:1486`), which gathers none of a component's `data/` files today.

## Phase 1: live conf reload

**Module.** A new directory module, `crates/moqui-core/src/context/conf_reload/`:
- `catalog.rs`: the liveness units;
- `diff.rs`: the comparison;
- `mod.rs`: `preview()` and `apply()` returning a `ConfReport`;
- `tests.rs`.

**Diffing by liveness unit, not by merge key.** The merge is imperative code (`conf.rs:103-163`). It has two-attribute
keys, rules with and without names, and appended lists (`component-list`, `load-entity`, `load-data`, webapp actions,
`url-pattern`), so there is no key table to share.
- The catalog is a list of selectors, each with a class: **Live**, **Apply(group)** or **Restart(reason)**.
- A unit's value is the ordered `write_string` of the nodes its selector matches, so appended lists compare as
  sequences.
- After the catalogued subtrees are removed, any remaining difference is **Restart**. Drift therefore over-reports a
  restart and never silently skips one.
- A test checks that every top-level element `merge_config_nodes` handles has catalog entries.

**`${property}` expansion.** A changed property is mapped to the units whose raw text contains `${name`, and those
units are checked too.

**`default-property`.** Keep `boot_set: HashMap<name, value>` (today it is thrown away at `factory.rs:391`).
- A new name is set only if it resolves empty.
- A changed value is applied only if the current value still equals the boot value; otherwise the report says
  "overridden by environment".
- A removed name is Restart.
- CHANGEME secrets are generated as at boot.
- These names are never changed live (Restart):
  - `instance_purpose` and `MOQUI_CONF`;
  - `entity_ds_*` and `content_ds_*`;
  - `cluster_*`, `instance_id`, `node_role` and `edge_*`;
  - key and secret names;
  - `*_port`, `*_host` and `*_bind`;
  - `moqui_log_*`;
  - time zones and `default_locale`;
  - `search_backend` and `elasticsearch_*`;
  - `entity_add_missing_*` and `entity_on_start_*`;
  - `webapp_trusted_proxies`.

**`factory.conf_derived::<T>(build)`.** It caches `(generation, Arc<T>)` and rebuilds `T` when the conf generation
changes. These state builders move to it:
- `ScreenConf`. Add the conf generation to the definition currency check (`screen/cache.rs:514`) and the sparse cache,
  so conf mounts take effect.
- `RootScreens` (`screen/info.rs`).
- The `TemplateRegistry` renderer list (`script/template.rs:402`).
- Token routes (`screen/token_route.rs:85`).
- `WebappScripts` (`web/servlet.rs:639`). Changed `after-startup`/`before-shutdown` actions are reported as "runs at
  next start".
- `ClientAddressing` (`web/address.rs:146`).
- The screen `check_interval_ms`.
- The edge conf (`edge/conf.rs:272`).

Two cases change state in place instead:
- **Web protection.** Replace the Guard's `GuardConf` in place, keeping its ban and rate counters
  (`moqui-host/src/http.rs:138`).
- **Telemetry.** Replace the boot config (`telemetry/mod.rs:505`), then reapply the live_settings overrides
  (`live_settings/mod.rs:400`).

**Initial classification.** This becomes the doc table.
- **Live once the conf swaps** (read on each use). Before an entry is marked Live, its reader is checked to read per
  use; there are about 81 `conf_setting`/`conf_number`/`conf_flag` sites. The entries:
  - webapp attributes (`http-host`, `https-*`, `status-ips`), `error-screen`, `response-header`, `public-site`, and the
    session timeout;
  - `artifact-execution-facade` and `user-facade`;
  - `pdf` and `service-file`;
  - `email-client`, `dav-server`, git-server request settings, `terminal` and `dev-aide`.
- **Apply.**
  - `screen-facade` screens, `subscreens-item` and `screen-text-output`; `root-screen`; `token-route`;
  - servlet, filter and listener;
  - `web-protection` rules;
  - telemetry;
  - `default-property` (with the rules above);
  - new `cache` entries (`CacheFacade` create; a changed existing cache is Restart);
  - new `tool-factory`, `resource-reference`, `template-renderer` and `script-runner` entries, only when their
    implementation is linked (`NativeRegistry`);
  - `load-entity` (relink).
- **Restart.**
  - ports, binds, listeners, `endpoint`;
  - worker, request and job pools;
  - `datasource`, `database-list`, `transaction-facade`;
  - `search-facade`, `llm-facade`, `service-type`;
  - the logging directory;
  - the ssh, smtp, git-server worker and gRPC listeners;
  - session `flush`/`max`, `dev-reload`, `component-list`;
  - any removal that no hook handles;
  - anything not in the catalog.

**Report and status.**
- `ConfReport` lists each unit with its class, its old and new values, and what was applied.
- Units that need a restart are kept on the factory as "pending restart". The System dashboard and the Conf screen
  show them.
- The swapped conf text is the desired state. Restart-class consumers keep their boot values until the next start.

**Extension hook.** Add `Extension::after_artifacts_changed(&factory, &ChangeSet)` (`core/extension.rs:35`).
- moqui-report uses it to drop its definitions cache (`definitions.rs:60`), which has no invalidation today.
- moqui-llm uses it to resync skills.

**Triggers.** A reload runs under the reload `running` lock (`reload.rs:192`).
- **Dev watcher.** `reload.rs` hands `ArtifactKind::Conf` for a `MoquiConf.xml` or the runtime conf to
  `conf_reload::apply` instead of "restart to apply". `component.xml` stays Restart. Watch the runtime conf at its
  actual path: `runtime/MoquiConf.xml` and an absolute `MOQUI_CONF` are dropped today (`reload.rs:264`,
  `watch.rs:84`).
- **Services.** `org.moqui.impl.ComponentServices.preview#ConfReload` and `reload#Conf`, in a new
  `ComponentServices.xml` under `framework/service/org/moqui/impl/`. They are registered by the devkit `Extension` and
  need ADMIN.
- **SIGHUP.** moqui-host adds a `SignalKind::hangup()` handler beside the terminate handler at `http.rs:604`. It runs
  `reload#Conf` with authz disabled and logs the report to the `moqui.monitor.audit` target. Add
  `ExecReload=/bin/kill -HUP $MAINPID` to `systemd/moqui.service`.
- **System › Conf screen** (`runtime/base-component/tools/screen/System/Conf/`). It shows the running conf against the
  files, each unit with its class, an Apply button and the pending-restart list. The Restart button appears where
  `restart#Server` is allowed.

## Phase 2: load a component from a local directory

This is the deploy core, without git. It is used by "Load" on components found on disk, and by phase 3.

Code: `crates/moqui-devkit/src/deploy/` (`mod.rs`, `preflight.rs`, `apply.rs`, `data.rs`, `tests.rs`). It lives in
devkit because the reload pipeline is there. devkit is linked in every host build.

**Preflight** (no state changes):
- Parse `component.xml`. Check the name, purpose and `depends-on`; every dependency must already be loaded, and the
  report lists any that are missing. Refuse a name that is already loaded.
- Refuse Rust code (a `Cargo.toml` or `src/*.rs`) unless a linked `NativeComponent` matches.
- Parse every XML file.
- Re-run `discover` and require the result to equal the current set plus this component, with the other components in
  the same relative order (`component.rs:193, 291, 443`).
- Recompute `merge_component_confs` in full with the new list, and compute the unit diff.
- Prepare the entity model offline. Refuse an `extend-entity` that adds primary-key fields or changes a field's type.
- Scan the data types used: `entity-facade-xml@type`, the CSV and JSON equivalents, and `seed-data` blocks. Count the
  files of each type.

**Apply**, in order:
1. DDL.
2. Swap: the entity model, then the entity dirs and EECAs, then the resource roots and `LAST_BOOTED`, then the
   components, then `ConfState`.
3. Apply hooks.
4. `reload_paths` on the component's files, leaving out `MoquiConf.xml` and `component.xml`. This covers services,
   SECAs (after services), REST (report replaced roots), screens (clear), templates and script texts, skills and
   reports (`after_artifacts_changed`). On dev instances, `watch_tree` the new component.
5. Data: the chosen types, default `seed,seed-initial,install`. Use `load_entity_data` with one transaction per file;
   on an error, log the file and go on. EECAs and feeds stay on: never use the facade-wide load flags on a live server
   (`lib.rs:2335`, `:2109`). Then run `search::index_feeds_on_start_empty`.
6. Record.

**Rollback.** A failure in steps 2–4 swaps back the previous model, component and conf Arcs and reruns step 4's
reloads. The DDL stays; it is additive and harmless. Data loads are not undone; the report gives each file's outcome,
and a rerun is safe.

**Modes.** When the conf diff has Restart units, the preview offers:
- **Deploy now:** apply the live parts and leave the rest pending restart.
- **Deploy at next restart:** files in place and recorded, nothing applied. The data load runs once after the next
  boot.
- **Deploy and restart:** only where restart is allowed.

**Entities.** `moqui.server.DeployedComponent` and `DeployedComponentHistory`, in `ServerEntities.xml`. The fields are
the name, the source (directory or git), the storage, the location, `gitRepositoryId`, the branch, the deployed commit,
the data types loaded, the status (`deployed`, `pending-restart`, `pending-data`, `failed`, `removed`), the user, the
date and the last report. A boot step loads the data of `pending-data` rows once the factory is up.

**Remove** is restart-only. It moves the directory out (a working clone with changes needs a confirm), marks the row
`removed`, and tells the user that tables and data stay.

**Cluster.** In this phase, refuse when `cluster::live_member_count > 1`; the other members would have neither the
files nor the model.

**Services and screens.**
- Services in `ComponentServices.xml`: `scan#Components` (on disk, not loaded), `preview#ComponentLoad`,
  `load#Component`, `load#ComponentData`, `remove#Component`.
- A System › Components screen lists the loaded and deployed components, the ones found on disk with Load, and the
  pending-restart list. It replaces the read-only `ComponentInfoList` on `System/dashboard.xml`.
- dev-aide's `get#Component` and `clone#Repository` (`runtime/dev-component/dev-aide/src/repos.rs`, `repo.rs`) offer
  Load instead of the restart banner when the preview has no Restart units.

## Phase 3: deploy from git

**Storage modes:**

| Mode | Repository | Files the server loads |
| --- | --- | --- |
| Database (default) | `import#GitRepository` into a `db` GitRepository (branch only) | `runtime/deployed-component/<name>` |
| Bare repo on disk | `remote` GitRepository mirror, `fetch#GitRepository` | `runtime/deployed-component/<name>` |
| Working clone | Normal clone; target dir chosen (default `runtime/component/<name>`) | The clone itself |

- Add `<component-dir location="deployed-component"/>` to the `component-list` in `MoquiDefaultConf.xml`, so deployed
  components load at boot.
- **New moqui-git functions** (and services):
  - `write_tree(repository_id, commit, target)` writes a commit's tree to a plain directory. Symlinks and LFS pointers
    are reported.
  - `clone_worktree(url, branch, credentials, target)` and `fast_forward_worktree(path)` (refused when the clone has
    local changes or the branches have diverged). These need gix's worktree-checkout and status features; enable them.
  - The changed paths between two commits, from the existing `diff.rs`.
- **Credentials.** The `db` and bare modes keep them on the GitRepository as today (`accessToken` encrypted). A working
  clone keeps them on `DeployedComponent`, also encrypted.
- **Staging.** Files go to a dot-prefixed staging directory, which discovery skips (`component.rs:286`). The **preview**
  runs on the staging paths. **Deploy** renames the directory into place, re-runs the checks on the final paths, then
  runs the phase 2 apply. A failure moves the directory to `.failed/`.
- **Update.** Fetch the branch, compute the changed paths from the old commit to the new, and preview. Then:
  1. Swap the tree; the old one goes to `.previous/` for rollback.
  2. Prepare, DDL and swap the entity model if entity files changed.
  3. Run `reload_paths` on the changed files.
  4. Run the conf reload if `MoquiConf.xml` changed.
  5. Load data. Default: `seed` for changed files, and all chosen types for files new in the commit.
- **Gate.**
  - A new sealed permission, `COMPONENT_DEPLOY`, on `ADMIN_ADV` (`DEFAULT_SEALED_PERMISSIONS` in
    `service/security_checks.rs:31`, `SecurityTypeData.xml`).
  - In production, also `component_deploy_enabled=true` (a `default-property`, empty by default); otherwise refused
    with a vague error.
  - URLs: `https` and `ssh` only, no `file://`. In production, the private-network check webhooks use. The RemoteCall
    fence and sim mode apply as for fetch.
  - Every deploy, update and remove is audited.
- **Services:** `preview#ComponentDeploy`, `deploy#Component`, `preview#ComponentUpdate`, `update#Component`.
- **Screen:** System › Components › Deploy.
  - A form for the URL, branch, storage, target dir (working clone) and credentials. Its pattern is
    `System/Resource/GitRepositoryList.xml`.
  - The preview shows the component and version, dependency status, Rust check, conf units by class (restart-needed
    first), entity changes, and the data types found with file counts (seed, seed-initial and install checked).
  - The mode buttons from phase 2.

## Phase 4: cluster and containers

These two make the database mode work in a cluster and in containers. The phase 2 cluster refusal stays until this
lands.

- **Cluster topic `component-deploy`.** Add it to `apply_remote` in `cluster/mod.rs:254`. A deploy or update with
  database storage publishes it after commit. Each member writes the tree from the shared database repository and runs
  the phase 2 apply without the data step. The bare and working-clone modes stay single-member and are refused while
  clustered.
- **Boot restore.** After boot, a `DeployedComponent` row whose tree is missing or at another commit (a fresh container)
  is written from the database repository. The host then boots the factory again in-process once, as it already does
  after an empty-database load (`moqui-host/src/lib.rs:1575-1587`).

## Docs

- **New `docs/tools/Component-Deploy.md`:** deploying and updating, storage modes, the gate, conf reload (watcher,
  screen, service, SIGHUP), and the table of conf units by class.
- **Updates:**
  - `docs/Run-Deploy.md:1263` (component set) and its conf section;
  - `plans/design-dev-aide.md` §4.7 and §6.5 (restart banner);
  - `docs/security/Attack-Surface.md` (the deploy surface);
  - the stale relink note in the `reload.rs` module doc.

## Verification

**Per crate.** Run `cargo lint`, `cargo lint-min` (moqui-host changes), and `cargo test -p` for moqui-core,
moqui-entity, moqui-devkit, moqui-git and moqui-host. Format only the changed files. Every new test runs well under
15 s. No slow tests.

**Unit tests:**
- **`conf_reload` (core):**
  - a new mount is Apply;
  - the same mount overridden by the runtime conf is no change;
  - appended `load-entity` lists compare as sequences;
  - an unknown element is Restart;
  - `${prop}` mapping;
  - the `default-property` provenance rules;
  - the catalog-coverage test.
- **Relink (entity):**
  - prepare, DDL, swap;
  - a failed DDL keeps the old model and the old dirs;
  - an `extend-entity` column is present before the swap.
- **Deploy (devkit, or a `moqui-host` `tests/it` module where the database is needed).** Use a `testdata/` component
  with an entity, an `extend-entity`, a service, a SECA, a conf mount, a token route and seed data. Check:
  - the service is callable;
  - the mount resolves;
  - the table and column exist;
  - the seed row loaded;
  - a fixture with `service-type` reports Restart;
  - an injected failure in step 3 rolls back.
- **moqui-git:**
  - `write_tree` from a database repository built in the test;
  - a working clone and a fast-forward from a local bare fixture. `file://` is allowed at the function level; the
    service refuses it.
- **Screens:** a `moqui test` spec renders System › Components and System › Conf.

**Live check** (`./moq run`):
- edit `runtime/conf/MoquiDevConf.xml` and see the watcher apply it;
- edit a production-style conf, `kill -HUP`, and read the logged report;
- Load a component that `./moq component-get` placed;
- deploy a small component from a git URL in each storage mode, then Update after a new commit. This needs network
  access; ask first.
