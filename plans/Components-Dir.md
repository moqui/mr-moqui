# Components directory

The runtime's component directories become one directory, `runtime/components/`, whose children each hold a set of
components: `base` (the framework's, now `runtime/base-component`), `dev` (dev-only, now `runtime/dev-component`),
`test` (new: test-only add-ons) and `addon` (the add-on components of `addons.xml`, now `runtime/component`). With it,
a component's acceptance suite moves from `<component>/acceptance/` to `<component>/test/acceptance/`, so everything
that is test sits under `test/`. The idea is aabiabdallah's. Plan, Nov 12, 2026; nothing built. It waits for the active
work on master to land, since it moves 1,332 tracked files.

## 1. Decisions

1. **Layout.** `runtime/components/{base,dev,test,addon}`. `components/` holds sets of components, not components,
   which is why its name differs from the old `component/`.
2. **The sets are explicit.** `runtime/components/components.xml` lists them, and only what it lists loads, as a Java
   `components.xml` does (§3.2). No directory loads because it happens to be there.
3. **Dropped.** The `ecomponent`, `deployed-component` and `mantle` component directories go from the conf, `moq`, the
   desktop runtime and the tests. A git deploy from System > Components clones into `components/addon` (§3.3).
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

Counted on master at 8a57630b, with `git grep -E 'base-component|dev-component|runtime/component'`: 306 files and
1,145 lines in mr-moqui. `component://` is already resolved by name through the component list (`resource.rs`,
`ResourceFacade::new`'s name-to-location map), and none of its 187 uses in production Rust depend on the layout. The
work is where code rebuilds the layout by hand.

| Area | Files / lines | What it is |
| --- | --- | --- |
| Markdown outside `plans/archive` | 58 / 278 | Docs, plans, READMEs, AGENTS.md: prose |
| `plans/archive` | 31 / 145 | History, left alone (§1.5) |
| Rust tests | ~45 / 334 | Fixtures that build the three directories or name `runtime/base-component/framework` (`moqui-host/tests/it/devaide.rs` alone 44) |
| Rust production code | ~30 / ~70 | §3.1 |
| Client specs | ~35 / 61 | `/files/runtime/base-component/...` and `/files/runtime/dev-component/...` URLs (§3.5) |
| `moq` | 74 lines | Every component task, suite selection, archive, crate linking, `git` over the repos |
| `scripts/moq-test.sh` | 59 lines | Its fixture trees |
| Build and packaging | ~25 lines | `Cargo.toml` members, paths and `exclude`; `.gitignore`, `.ignore`; `podman/`; `systemd/moqui.service` |
| Conf | 2 files | `MoquiDefaultConf.xml`'s `component-list`; `MoquiDemoConf.xml`'s `location="dev-component/demo"` |

In the component repositories (their own git repos), the code is clean: mr-boss has 5 client specs fetching
`/files/runtime/component/mr-boss/...` and one error message, mr-l10n two localized strings quoting
`runtime/dev-component`; the rest of their hits (mr-boss 36 plan files, mr-marble 4, mr-pass 3, mr-mantle and mr-home
1 each) are docs.

The `acceptance/` move (§3.7): `acceptance/` directories in basic-comms (5 files), framework (3), tools (5), webroot
(3), example (3) and the template (1), mr-boss (206), mr-pass (27), mr-marble (9) and mr-mantle (3); text naming
`acceptance/` in 34 files and 110 lines of mr-moqui outside `plans/archive`, 340 lines in mr-boss, 49 in mr-pass, 8 in
mr-marble and 3 in mr-mantle.

## 3. Design

### 3.1 One place for the layout

The cleanup this move should leave behind, done first and on its own (P1), so the move itself is mechanical:

- **The component containers.** The list `["base-component", "component", "dev-component"]` is spelled out in about
  ten places: `moqui-host` `test_cmd/mod.rs` (295, 326), `test_cmd/agents.rs` (296), `test_cmd/skills.rs` (32),
  `devtools_remote.rs` (380), `qvt_report.rs` (71); `moqui-devkit` `project/cache.rs` (391); `moqui-lib`
  `xml_edit/tokenize.rs` (417); `moqui-core` `script/walker.rs` (117); dev-aide `repos.rs` (216). Each asks one
  function instead: the loaded component list where there is a factory, else a `moqui-core::context::layout` helper
  that names a runtime's component containers (and which are dev or test) from the conf without a boot, as
  `component::discover` already does.
- **The framework component.** `runtime/base-component/framework` or `<runtime>/base-component/framework` is joined in
  `factory.rs` (261), `locate.rs` (67, 161), `resource.rs` (1244), `moqui-entity/testing.rs` (243),
  `devkit/project/mod.rs` (81, 167, 228), `devtools_cmd.rs` (344, 371) and `desktop/client/conf.rs` (48). One
  `layout::framework_dir(runtime)` and one `layout::REPO_FRAMEWORK` (the clone-relative path, for the
  `CARGO_MANIFEST_DIR` joins) replace them.
- **The runtime marker.** "A directory with `base-component/` is a runtime" is the test in `moqui-host/src/lib.rs`
  (864), `devkit/project/mod.rs` (228), `test_client/mod.rs` (669) and `test_client/load_cmd.rs` (537). It becomes
  `layout::is_runtime(dir)`: `components/base/framework/component.xml` is there.
- **The webroot.** `../../runtime/base-component/webroot/screen/webroot` in `screen/pdf.rs`, `screen/statics.rs`,
  `desktop/client/mod.rs`, `llm/ui/openui.rs` and `test_client/offline.rs`: one `layout::REPO_WEBROOT`.
- **What stays literal.** `include_str!` paths (16, in `screen/generate.rs`, `screen/statics.rs`,
  `script/actions.rs`), `moqui-browser/build.rs` and `Cargo.toml` cannot call a function; they change by replacement
  in P4. A test that walks the tree (§5) catches one that was missed.

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

The loader (`moqui-core/src/context/component.rs`, `add_component_dir`) already does all of this: it reads a
`components.xml` in a component directory, loads exactly what it lists, nests `component-dir` elements, and passes a
directory's `purpose` down to what it finds. The only addition is a WARN for a child directory of `components/` that
the manifest does not list ("not in components/components.xml, not loaded"), so a set or a component dropped in the
wrong place is noticed rather than silently missing. A site that wants a set of its own lists it: in the tracked
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
- A git deploy (`moqui-devkit/src/deploy/git.rs`, `clone_directory`) defaults to `components/addon`, and its "one
  named" choice is any set `components.xml` lists; the old `deployed-component` default goes.
- The boot WARN (§1.10): a non-empty `runtime/component`, `base-component` or `dev-component` is reported once with
  where its components go; nothing is moved.

### 3.4 The archive and packages

`./moq archive`'s runtime is the binary, `MoquiConf.xml` and `components/` beside it; `README.txt` and the help say so.
`stage_desktop_runtime` stages `components` and `conf` (the list `base-component component ecomponent dev-component
conf` goes), `moqui-desktop/src/runtime_dir.rs` links `components` (and its tests drop `ecomponent`), `podman/build.sh`
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
  its second segment) test for the `test/acceptance` prefix; the client trees (597) follow.
- **`moqui test-client`** (`test_client/mod.rs` 526–573): the client and process directories, and the
  acceptance-or-regression choice, on the new paths.
- **`moq`**: `spec_components`, `served_components` and `offline_components` with `acceptance` and `acceptance/client`
  (2550–2560), and `accept`, `accept-all` and their help.
- **A stale `acceptance/`**: `moqui test` and `moq accept` refuse a component that still has a top-level
  `acceptance/` with an error naming `test/acceptance/`, so its specs are not silently skipped.
- **Docs**: [docs/Build-Test.md](../docs/Build-Test.md), AGENTS.md, the testing skill, the template's files and
  README, and every doc and plan outside `plans/archive` that names an `acceptance/` path (34 files in mr-moqui).
- **Components**: `git mv` of `acceptance/` in basic-comms, framework, tools, webroot, example and the template; then
  one commit in each of mr-boss (206 files, 340 lines of text that name the path), mr-pass (27, 49), mr-marble (9, 8)
  and mr-mantle (3, 3), with each repository's specs that say `acceptance/` (`dependsOn`, `suite.js`, fixtures) and its
  docs. A component's `test/budget` is for the regression suite and does not change.

This is independent of the components move and can land first (P2).

## 4. Phases

Each phase is one commit (or one per repository) with its focused tests, and `./moq test-framework` once at the end.

| Phase | What | Tests |
| --- | --- | --- |
| P1 | §3.1 helpers and their callers, on the old layout; `/files/component/<name>/` and every client spec on it | the touched crates' tests; `offline.rs` tests for the new form (found, outside the component, `..`, an unknown name); the client specs of webroot, basic-comms, tools, dev-aide and mr-boss |
| P2 | `test/acceptance/` (§3.7): the runners, `moq`, the stale-directory error, the framework's components and docs; then mr-boss, mr-pass, mr-marble and mr-mantle | `test_cmd` and `test_client` tests (a regression run leaves `test/acceptance` out, an acceptance run finds it, a top-level `acceptance/` is refused); `scripts/moq-test.sh`'s accept cases; each component's moved acceptance specs, run once as the user asks (they are acceptance tests that changed) |
| P3 | The WARN for an unlisted child of `components/` (§3.2) and the boot WARN (§3.3) | `context::component` tests: the listed sets and their purposes, an unlisted child warned and not loaded |
| P4 | The move: `git mv` of the base and dev trees; `components.xml`; `MoquiDefaultConf.xml` and `MoquiDemoConf.xml`; `Cargo.toml` members, paths and `exclude` (`runtime/components/addon`, `test`, `dev`); `include_str!` and `build.rs` paths; `.gitignore` and `.ignore` (track `components/components.xml`, `base/`, the framework's four in `dev/`, the READMEs of `addon/` and `test/`; ignore the rest of `addon/`, `test/` and `dev/`); `moq` and `scripts/moq-test.sh`; Rust test fixtures; podman, systemd, desktop; READMEs, docs, AGENTS.md (the `test-framework` scope), the testing and building skills, plans outside `plans/archive`; dropping `ecomponent`, `deployed-component` and `mantle` | `./moq test-framework`; `./moq test-apps` with the components moved by hand into `components/addon` |
| P5 | `./moq archive` and `component-zip` without tests and the dev set, `--with-tests`, `--with-dev`; `component-get --test` and the Dev AIDE's Test only (§3.3, §3.6) | `scripts/moq-test.sh` cases: a default archive has no component `test/`, no `components/test/` and no `components/dev/`; `--with-tests` and `--with-dev` each put theirs back |
| P6 | The component repositories' remaining prose: mr-boss's plans, mr-l10n's two strings, the docs of mr-marble, mr-pass, mr-mantle and mr-home | each component's regression specs that changed |

## 5. Keeping it clean

A framework test (in `moqui-core`, beside the XML corpus test) fails on any `base-component`, `dev-component`,
`runtime/component/` or `ecomponent` in a tracked file outside `plans/archive/`, with a short allow list (this plan,
the boot WARN's own strings), so a stale path or a new hand-built one shows up at once rather than at the next move.
The stale-`acceptance/` error (§3.7) does the same for the suite's old place.

## 6. Open

Nothing now. Settled Nov 12, 2026: the sets are only what `components.xml` lists (§1.2); load scripts leave the archive
with the tests (§3.6); `acceptance/` moves under `test/` (§3.7); the dev set leaves the archive by default (§1.9).
