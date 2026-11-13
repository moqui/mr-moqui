# Components directory

The runtime's component directories become one directory, `runtime/components/`, whose children each hold a set of
components: `base` (the framework's, now `runtime/base-component`), `dev` (dev-only, now `runtime/dev-component`),
`test` (new: test-only add-ons) and `addon` (the add-on components of `addons.xml`, now `runtime/component`). With it,
a component's acceptance suite moves from `<component>/acceptance/` to `<component>/test/acceptance/`, so everything
that is test sits under `test/`. The idea is aabiabdallah's. Plan, Nov 12, 2026; references reviewed against
ce6b26ed on Nov 13, 2026, which added the built-in framework files (§3.8); being built.

## 1. Decisions

1. **Layout.** `runtime/components/{base,dev,test,addon}`. `components/` holds sets of components, not components,
   which is why its name differs from the old `component/`.
2. **The sets are explicit.** `runtime/components/components.xml` lists them, and only what it lists loads, as a Java
   `components.xml` does (§3.2). No directory loads because it happens to be there.
3. **Dropped.** The `ecomponent`, `deployed-component` and `mantle` component directories go from the conf, `moq`, the
   desktop runtime and the tests. A git deploy from System > Components clones into `components/addon`, and its
   database and bare storages write their files there too (§3.3).
4. **The archive layout matches.** `components/` sits beside the `moqui` binary in an archive, a container image and
   `/opt/moqui`, and `components/base/framework` is the canonical place of the framework component, both in a clone
   (`runtime/components/base/framework`) and in an archive (`components/base/framework`).
5. **`plans/archive/` stays as written.** Its 31 files that name the old paths are history; nothing there is edited.
6. **`/files/component/<name>/...`** is a new route of the test client's file server, resolved through the component
   list, so client specs stop naming where a component sits on disk (§3.5).
7. **A `test` component set** for add-on components that never go to production: cross-component regression and
   acceptance fixtures, pre-production validation (§3.2). The framework's tests and the apps' own tests (mr-mantle,
   mr-boss and the rest) stay in their components; a cross-component `mr-test` component may come later.
8. **`test/acceptance/`** replaces `acceptance/` in every component, beside the special subdirectories `test/` already
   has (`test/client`, `test/load`, `test/agent`), in mr-moqui and in each mr-* repository (§3.7).
9. **A production archive is test-free and dev-free by default** (§3.6). `./moq archive` leaves out every component's
   `test/` (with it the acceptance suite, load scripts and agent replays) and the `test` set unless `--with-tests`, and
   the `dev` set unless `--with-dev`. This follows the pattern of keeping production free of dev tools by default: the
   defaults suit a standard production instance, and whoever wants otherwise changes them or passes the options in
   their own scripts. It is the main way to keep tests off a production server; the `dev` and `test` purposes are the
   second.
10. **No migration.** The layout is not stable before 0.1.0, so there is no migration code; a developer moves their own
    clones from `runtime/component/` into `runtime/components/addon/` by hand. A boot that finds a non-empty old
    directory, and a test run that finds a component with a top-level `acceptance/`, each say where it goes (§3.3,
    §3.7), which is not migration code and costs nothing.

## 2. What the research found

Recounted on master at ce6b26ed (first counted at 8a57630b), with
`git grep -E 'base-component|dev-component|runtime/component'`: 317 files and 1,198 lines in mr-moqui.
`component://` is already resolved by name through the component list (`resource.rs`, `ResourceFacade::new`'s
name-to-location map), and none of its uses in production Rust depend on the layout. The work is where code rebuilds
the layout by hand.

| Area | Files / lines | What it is |
| --- | --- | --- |
| Markdown outside `plans/archive` | 59 / 301 | Docs, plans, READMEs, AGENTS.md: prose |
| `plans/archive` | 34 / 158 | History, left alone (§1.5) |
| Rust tests | 121 / 359 | Fixtures that build the three directories or name `runtime/base-component/framework` (`moqui-host/tests/it/devaide.rs` 46, `tests/gate3_browser.rs` 25) |
| Rust production code | ~40 / ~90 | §3.1, §3.8 |
| Client specs | 31 / 62 | `/files/runtime/base-component/...` and `/files/runtime/dev-component/...` URLs (§3.5) |
| `moq` | 77 lines | Every component task, suite selection, archive, crate linking, `git` over the repos |
| `scripts/moq-test.sh` | 59 lines | Its fixture trees |
| Build and packaging | ~30 lines | `Cargo.toml` members, paths and `exclude`; `moqui-embedded/build.rs`; `.gitignore`, `.ignore`; `podman/`; `systemd/moqui.service` |
| Conf | 2 files | `MoquiDefaultConf.xml`'s `component-list`; `MoquiDemoConf.xml`'s `location="dev-component/demo"` |
| Relative Markdown links | ~280 links | A link that leaves its set directory (`../../../docs/...` from a component) needs one more `../` after the move: base 62 in 6 files, dev 5 in 3, mr-boss 178 in 26, mr-home 15, mr-mantle 9, mr-pass 7; and 17 mr-moqui files link into the trees |

In the component repositories (their own git repos), the code is clean: mr-boss has 5 client specs fetching
`/files/runtime/component/mr-boss/...` and one error message, mr-l10n two localized strings quoting
`runtime/dev-component`; the rest of their hits (mr-boss 36 plan files, mr-marble 4, mr-pass 3, mr-mantle and mr-home
1 each) are docs.

The `acceptance/` move (§3.7): `acceptance/` directories in basic-comms (5 files), framework (3), tools (6), webroot
(3), example (3) and the template (1), the `moqui-host` test fixture `crates/moqui-host/tests/fixtures/test-component`
(1), mr-boss (247), mr-pass (27), mr-marble (9) and mr-mantle (3); text naming `acceptance/` in 39 files and 119 lines
of mr-moqui outside `plans/archive` and this plan, 393 lines in mr-boss, 49 in mr-pass, 8 in mr-marble and 5 in
mr-mantle.

## 3. Design

### 3.1 One place for the layout

The cleanup this move should leave behind, done first and on its own (P1), so the move itself is mechanical:

Line numbers are at ce6b26ed. "Test" marks a site inside a `#[cfg(test)]` module, which uses the clone-relative
constants rather than the runtime helpers.

- **The component containers.** The list `["base-component", "component", "dev-component"]` is spelled out in
  `moqui-host` `test_cmd/mod.rs` (295, 326), `test_cmd/agents.rs` (296), `test_cmd/skills.rs` (32),
  `devtools_remote.rs` (380), `qvt_report.rs` (71–72); `moqui-devkit` `project/cache.rs` (391); dev-aide `repos.rs`
  (216, and `get_component` 282), `repo.rs` (`clone_repository` 960), `workspace.rs` (1238, 1313–1326); and in tests
  `moqui-lib` `xml_edit/tokenize.rs` (417) and `moqui-core` `script/walker.rs` (117). `test_cmd/mod.rs` (789) and
  `agents.rs` (92) test `starts_with(runtime.join("dev-component"))` to boot on the dev purpose; `test_cmd/fixture.rs`
  (179–191) links the three into a scratch runtime; `locate.rs` (128–135, `component_root`) falls back to
  `<runtime>/component/<name>`. Each asks one function instead: the loaded component list where there is a factory,
  else a `moqui-core::context::layout` helper that names a runtime's component sets (and which are dev or test) without
  a boot.
- **The framework component.** `runtime/base-component/framework` or `<runtime>/base-component/framework` is joined in
  `factory.rs` (261, `default_framework_dir`, which `embedded_files.rs` 32 and `lib.rs` 2651 call), `locate.rs` (67),
  `moqui-entity/testing.rs` (243), `devkit/project/mod.rs` (81, 167, 228), `devtools_cmd.rs` (344, 371) and
  `desktop/client/conf.rs` (48); in tests `locate.rs` (161) and `resource.rs` (1244). One
  `layout::framework_dir(runtime)` and one `layout::REPO_FRAMEWORK` (the clone-relative path, for the
  `CARGO_MANIFEST_DIR` joins) replace them.
- **The runtime marker.** "A directory with `base-component/` is a runtime" is the test in `moqui-host/src/lib.rs`
  (901 `runtime_dir_in`, the lookup from the working directory; 915 `find_runtime_dir`, the desktop's search beside the
  executable), `test_client/mod.rs` (669) and `test_client/load_cmd.rs` (537). It becomes `layout::is_runtime(dir)`:
  `components/base` is a directory. It stays a directory test, as now, so a runtime whose framework is missing is
  still found and the built-in files refill it (§3.8); `components/base` rather than `components/` because a bare
  `components/` is common in other kinds of project. `devkit/project/mod.rs` (228) keeps its stricter test, the
  framework's `component.xml`.
- **The webroot.** `../../runtime/base-component/webroot/screen/webroot` in `desktop/client/mod.rs` (129) and
  `test_client/offline.rs` (47), and in tests `screen/pdf.rs` (223), `screen/statics.rs` (269), `llm/ui/openui.rs`
  (514) and `web/misconfig.rs` (123): one `layout::REPO_WEBROOT`.
- **What stays literal.** `include_str!` paths (16, in `screen/generate.rs`, `screen/statics.rs`,
  `script/actions.rs`), `moqui-browser/build.rs` (58), `moqui-embedded/build.rs` (34) and `Cargo.toml` cannot call a
  function; they change by replacement in P4. A test that walks the tree (§5) catches one that was missed.
- **`moq`.** The set directories are shell variables at the top of `moq` (`BASE_SET`, `ADDON_SET`, `DEV_SET`,
  `TEST_SET`), so P4 changes them in one place; its tasks (component tasks, suite selection, `component_crate_dirs`
  at 1438 and 1732, `stage_desktop_runtime`, the archive) use them.

### 3.2 The conf and the loader

`MoquiDefaultConf.xml`'s `component-list` becomes:

```xml
<component-list>
    <!-- runtime/components: the sets of components that its components.xml lists (plans/Components-Dir.md). -->
    <component-dir location="components"/>
</component-list>
```

and `runtime/components/components.xml` (tracked in mr-moqui) lists the sets, in load order:

```xml
<components>
    <component-dir location="base"/>
    <component-dir location="addon"/>
    <!-- Loaded only on a dev instance (./moq run, ./moq test); left out with a WARN on any other purpose.
        ./moq archive leaves this set out unless --with-dev. -->
    <component-dir location="dev" purpose="dev"/>
    <!-- Add-on components for testing: regression and acceptance fixtures, cross-component scenarios, pre-production
        validation. Loaded on a dev instance (where ./moq test runs) and a test instance (staging), never on
        production; ./moq archive leaves this set out unless --with-tests. -->
    <component-dir location="test" purpose="dev, test"/>
</components>
```

The loader (`moqui-core/src/context/component.rs`, `add_component_dir`, 244–349) already does all of this: it reads a
`components.xml` in a component directory (271–289), loads exactly what it lists and does not scan the directory,
nests `component-dir` elements, and passes a directory's `purpose` down to what it finds (a `component-dir`'s own
`purpose` replaces the outer one). The framework's same-directory check runs only in the scan branch, which is where
`base` is read. The additions: a WARN for a child directory of `components/` that the manifest does not list ("not in
components/components.xml, not loaded"), so a set or a component dropped in the wrong place is noticed rather than
silently missing; and a WARN, not the INFO of a `component-dir` that is not there, for a set the manifest lists that is
missing. A site that wants a set of its own lists it: in the tracked
`components.xml` for the framework's sets, or with a `component-dir` of its own (`components/my-set`) in its runtime
conf's `component-list`.

`test` is `purpose="dev, test"`, not `test` alone: `./moq test` and `./moq run` boot with `instance_purpose` `dev`, so a
`test`-only set would be invisible to the regression run. A pre-production server sets `instance_purpose` `test` (the
recommended values in `MoquiDefaultConf.xml` are production, dev and test). A component that loads on every purpose
still may not depend on one in `dev` or `test` (the existing rule).

### 3.3 Components that the tools place

- `./moq component-get`, `component-depends`, `component-zip`, `component-create` and the Dev AIDE (`repo.rs`,
  `repos.rs`, `workspace.rs`) write into `components/addon` by default; `--dev` (the Dev AIDE's "Dev only" check) into
  `components/dev`; a new `--test` (and a "Test only" check) into `components/test`. What a component depends on still
  goes into `addon`, as now.
- `component-create`'s template is `components/dev/template`.
- A git deploy (`moqui-devkit/src/deploy/git.rs`, `clone_directory` 980–992) clones into `components/addon` by default
  (now `component`, 984), and its "one named" choice is any set `components.xml` lists. The database and bare storages
  write a component's files to `DEPLOYED_DIR` (45; `deployed-component`, used at 224, 617 and 714); that goes, and they
  write to `components/addon` too, so every add-on is in one place whatever its storage. The docs and fixtures that name
  `deployed-component` follow: `docs/tools/Component-Deploy.md`, `docs/security/Attack-Surface.md`, `ServerEntities.xml`
  (115), `tools/test/ConfReload.spec.js` (77), `moqui-host/tests/it/component_deploy.rs`.
- The boot WARN (§1.10): a non-empty `runtime/component`, `base-component` or `dev-component` is reported once with
  where its components go; nothing is moved.

### 3.4 The archive and packages

`./moq archive`'s runtime is the binary, `MoquiConf.xml` and `components/` beside it; `README.txt` and the help say so.
`stage_desktop_runtime` stages `components` and `conf` (the list `base-component component ecomponent dev-component
conf` goes), `moqui-desktop/src/runtime_dir.rs` links `components` (`READ_ONLY`, 24–30, becomes `components`, `conf`,
`MoquiConf.xml`, in step with `stage_desktop_runtime`; its tests drop `ecomponent`), `podman/build.sh`
and the `Containerfile` copy `components/`, and `systemd/moqui.service` and `podman/README.md` name it.

### 3.5 `/files/component/<name>/...`

The test client's file server (`moqui-host/src/test_client/offline.rs`, `mapped`) serves `/files/<repo-relative>`
from the repository. A new form, `/files/component/<name>/<path>`, serves `<path>` under the directory of component
`<name>` from the runtime's component list (§3.1's helper; no boot), with the same rules: no empty, `.` or `..`
segment, the canonical file under the component's canonical directory, read-only. The repository has no top-level
`component/`, so the two forms do not collide; repo paths outside components (`/files/crates/moqui-lib/testdata/...`)
stay as they are. Served runs (`test-client --serve`, through the same `answer`) get it too.

Every client spec moves to it in P1, before the move: 61 lines in mr-moqui (`/files/runtime/base-component/webroot/...`
becomes `/files/component/webroot/...`, dev-aide's `/files/component/dev-aide/...`) and mr-boss's 5.
[docs/dev/JS-Test-Client.md](../docs/dev/JS-Test-Client.md), which describes `/files/`, gains the component form as the
one to use.

### 3.6 A production archive: no tests, no dev set

`./moq archive` has no filter for tests now (it leaves out `.git`, `target/`, logs, pid files and, with `--no-data`,
the data). By default it leaves out:

- `test/` of every component in the archive, the framework's included. After §3.7 that is all of a component's tests:
  the regression specs, `test/client`, `test/acceptance` (and its `client`), `test/agent` (replays) and `test/load`
  (load scripts);
- the whole `components/test/` set;
- the whole `components/dev/` set.

`--with-tests` keeps the first two, for an archive meant for a staging server that runs `moqui test` or client specs
against itself; `--with-dev` keeps the third, for a server that should carry the dev tools (they still load only on a
dev instance). The zip and tar paths both take the exclude patterns (`archive_write`'s `excludes`); `README.txt` lists
what was left out and the options that keep it. `stage_desktop_runtime` (the desktop app's runtime) follows the same
defaults, `components/dev` aside: the desktop app stages it now for the tray's Developer tools, and keeps doing so.
`./moq component-zip` leaves out `test/` by default too, with `--with-tests`, so a component zip for production is
streamlined the same way.

Leaving out `test/load` means the **Service Load Runner screen** (`moqui-host/src/load_runner/scripts.rs`, which lists
`component://<component>/test/load/*.load.js`) has no scripts on a production archive. That is intended: load testing is
done from other instances against a production configuration, not by a production server putting load on others. A
site that wants that builds its archive with `--with-tests`, or adds an option of its own.

### 3.7 `test/acceptance/`

A component's acceptance suite moves from `<component>/acceptance/` (and `acceptance/client/`) to
`<component>/test/acceptance/` (and `test/acceptance/client/`), so `test/` holds every kind of test: the regression
specs at its top, and the special subdirectories `client`, `acceptance`, `agent` and `load`. The suites stay separate
([the two suites](../docs/Build-Test.md#the-two-suites)); only the place changes.

- **`moqui test`** (`moqui-host/src/test_cmd/mod.rs`). `reserved_subdirs("test")` gains `acceptance` (and `load`, which
  holds no specs now but is another command's), so the regression suite does not pick up acceptance specs;
  `reserved_subdirs("test/acceptance")` is `client`, `agent`; `suite_tree` returns `test/acceptance` and
  `test/acceptance/client`; the acceptance checks on a spec's display path (`mod.rs` 401, `report.rs` 98, which test
  its second segment) test for the `test/acceptance` prefix; the client trees (597) follow. `reserved_subdirs` is at
  352–358, `suite_tree` 362–369, the duplicate-spec message 386; `report.rs` 125 and 152 are messages that say
  "move ... to acceptance/", and 406–411 its fixtures.
- **`moqui test-client`** (`test_client/mod.rs` 515–583): the client and process directories, and the
  acceptance-or-regression choice, on the new paths; `test_client/serve.rs` (16, 25, 151–154, 423–426) follows
  `suite_tree`. Help in `usage.rs` (358, 373–376, 479–481); CLI tests `crates/moqui/tests/cli/test.rs` (119) and
  `template.rs` (75).
- **`moq`**: `spec_components`, `served_components` and `offline_components` with `acceptance` and `acceptance/client`
  (the calls at 2551, 2556 and 2561 in `accept_suites`; `has_spec_file` 2519–2533; `spec_component_dirs` 4877–4894),
  `template_part_files` (4313), and `accept`, `accept-all` and their help (140–141, 795–816).
- **A stale `acceptance/`**: `moqui test` and `moq accept` refuse a component that still has a top-level
  `acceptance/` with an error naming `test/acceptance/`, so its specs are not silently skipped.
- **Docs**: [docs/Build-Test.md](../docs/Build-Test.md), AGENTS.md, the testing skill, the template's files and
  README, and every doc and plan outside `plans/archive` that names an `acceptance/` path (39 files in mr-moqui).
- **Built-in files**: `moqui-embedded`'s `COMPONENT_SKIP` (`build.rs` 30, `lib.rs` 40) drops `acceptance`, since
  `test` then covers it (§3.8).
- **Components**: `git mv` of `acceptance/` in basic-comms, framework, tools, webroot, example, the template and the
  `moqui-host` test fixture `tests/fixtures/test-component`; then one commit in each of mr-boss (247 files, 393 lines of
  text that name the path), mr-pass (27, 49), mr-marble (9, 8) and mr-mantle (3, 5), with each repository's specs that say `acceptance/` (`dependsOn`, `suite.js`, fixtures) and its
  docs. A component's `test/budget` is for the regression suite and does not change.

This is independent of the components move and can land first (P2; the component repositories in P6).

### 3.8 Built-in framework files

The binary carries the framework and tools components whole and a webroot stub (`crates/moqui-embedded`, built in
513e2e35 and ce6b26ed), compares the runtime's copies with them at startup, and writes them as a skeleton runtime when
there is none ([Run-Deploy](../docs/Run-Deploy.md), "Built-in framework files"). What ties it to the layout:

- **`build.rs`** (34) packs from `../../runtime/base-component`: a literal, replaced in P4 by
  `../../runtime/components/base`. `webroot-files.txt` (its comment at 7) is relative to the webroot component and
  keeps its paths.
- **The skeleton.** `write_skeleton` (`lib.rs` 190–207) writes `base-component/{framework,tools,webroot}` and the
  Skeleton set, whose paths are relative to `base-component` (`lib.rs` 60; its one file is
  `skeleton/webroot/screen/webroot/qapps.xml`). With the new conf it must also write `components/components.xml`: the
  conf's one `component-dir` is `components`, whose children are sets, not components, so without the manifest the
  scan finds no component there and tools and webroot do not load. The Skeleton set becomes relative to the runtime
  directory (`components/base/webroot/screen/webroot/qapps.xml`), and `build.rs` packs the tracked
  `runtime/components/components.xml` into it, so there is one copy. `write_skeleton` writes into
  `layout::framework_dir(runtime)`'s parent, and writes the manifest whenever it is missing, not only with the webroot.
- **What does not change.** `compare` and `unified_diff` report paths per component (`framework/conf/...`), so
  `moqui embedded list|diff` and their output stay as they are; `embedded_files.rs` (155–165) finds tools and webroot as
  siblings of the framework, which `components/base` keeps; `COMPILED_IN` (`lib.rs` 44–49) is framework-relative, while
  the matching `include_str!` paths move in P4 (§3.1). `ensure_runtime` (`embedded_files.rs` 31–52) compares against
  `default_framework_dir`, which delegates to the layout helper.
- **The runtime lookup** (`moqui-host/src/lib.rs` 898–916) and the desktop's `find_existing` (`runtime_dir.rs` 84–94)
  use `layout::is_runtime` (§3.1).
- **Tests**: `moqui-embedded/src/tests.rs` (38, 92–95, 168–172; 79–80 after P2), `moqui-host/src/embedded_files_tests.rs`
  (28–30), `lib_tests.rs` (27–34), and the skeleton acceptance test `crates/moqui/tests/cli/skeleton.rs` (183–186, which
  also asserts `runtime/components/components.xml`; ignored by default, run once in P4 since it changes:
  `cargo test -p moqui --test cli -- --ignored skeleton`).
- **Docs**: `runtime/README.md` (4–28), `docs/Run-Deploy.md` (185–187, 197, 209–210 with the manifest, 222, 756–760),
  `docs/Build-Test.md` (241–252), `usage.rs` (69–74), `moq`'s `help_archive` (1207, 1210) and `archive_readme` (4512,
  4543), `crates/moqui-desktop/README.md` (81, 221–224).

## 4. Phases

Each phase is one commit (or one per repository) with its focused tests, and `./moq test-framework` once at the end.

| Phase | What | Tests |
| --- | --- | --- |
| P1 | §3.1 helpers and their callers, on the old layout; `/files/component/<name>/` and every client spec on it | the touched crates' tests; `offline.rs` tests for the new form (found, outside the component, `..`, an unknown name); the client specs of webroot, basic-comms, tools, dev-aide and mr-boss |
| P2 | `test/acceptance/` (§3.7): the runners, `moq`, the stale-directory error, the framework's components, the test fixture, `moqui-embedded`'s skip list, and docs | `test_cmd` and `test_client` tests (a regression run leaves `test/acceptance` out, an acceptance run finds it, a top-level `acceptance/` is refused); `scripts/moq-test.sh`'s accept cases; the discovered acceptance specs the same before and after; the moved framework-side acceptance specs, run once (they are acceptance tests that changed) |
| P3 | The WARN for an unlisted child of `components/` (§3.2) and the boot WARN (§3.3) | `context::component` tests: the listed sets and their purposes, an unlisted child warned and not loaded |
| P4 | The move: `git mv` of the base and dev trees; `components.xml`; `MoquiDefaultConf.xml` and `MoquiDemoConf.xml`; `Cargo.toml` members, paths and `exclude` (`runtime/components/addon`, `test`, `dev`); `include_str!` and `build.rs` paths; `.gitignore` and `.ignore` (track `components/components.xml`, `base/`, the framework's four in `dev/`, the READMEs of `addon/` and `test/`; ignore the rest of `addon/`, `test/` and `dev/`); `moq` and `scripts/moq-test.sh`; Rust test fixtures; podman, systemd, desktop; the built-in files (§3.8); READMEs, docs, AGENTS.md (the `test-framework` scope), the testing and building skills, plans outside `plans/archive`; one more `../` in each relative link that leaves a moved set (§2); dropping `ecomponent`, `deployed-component` and `mantle` | `./moq test-framework`; `./moq test-apps` with the components moved by hand into `components/addon`; the skeleton acceptance test; no relative link broken that resolved before |
| P5 | `./moq archive` and `component-zip` without tests and the dev set, `--with-tests`, `--with-dev`; `component-get --test` and the Dev AIDE's Test only (§3.3, §3.6) | `scripts/moq-test.sh` cases: a default archive has no component `test/`, no `components/test/` and no `components/dev/`; `--with-tests` and `--with-dev` each put theirs back |
| P6 | The component repositories: `test/acceptance/` (§3.7) in mr-boss, mr-pass, mr-marble and mr-mantle; their relative links (§2); mr-boss's plans, mr-l10n's two strings, the docs of mr-marble, mr-pass, mr-mantle and mr-home | each component's regression specs that changed; the discovered acceptance specs the same before and after; moved acceptance specs run as the user asks |

## 5. Keeping it clean

A framework test (in `moqui-core`, beside the XML corpus test) fails on any `base-component`, `dev-component`,
`runtime/component/`, `ecomponent` or `deployed-component` in a tracked file outside `plans/archive/`, with a short allow list (this plan,
the boot WARN's own strings), so a stale path or a new hand-built one shows up at once rather than at the next move.
The stale-`acceptance/` error (§3.7) does the same for the suite's old place.

## 6. Open

Nothing now. Settled Nov 12, 2026: the sets are only what `components.xml` lists (§1.2); load scripts leave the archive
with the tests (§3.6); `acceptance/` moves under `test/` (§3.7); the dev set leaves the archive by default (§1.9).
Settled Nov 13, 2026: the database and bare deploy storages write to `components/addon` (§3.3); the runtime marker is
the `components/base` directory (§3.1); the skeleton writes `components/components.xml` (§3.8).
