# Conf reload and component deploy

A running server can take changes to its conf files without a restart, where what reads the setting allows it, and
names the rest as waiting for a restart. System › Conf shows the merged conf with the file each value came from, and
edits a conf file. A running server can also load a component placed in a component directory, without a
restart. The design and status are in [plans/archive/Component-Deploy.md](../../plans/archive/Component-Deploy.md).

The code: `crates/moqui-core/src/context/conf_reload/` (the catalog, the diff, the apply and the view),
`crates/moqui-devkit/src/deploy/` (the component load and the services), and `crates/moqui-host/src/http.rs` (SIGHUP
and the web protection hook). Services: `org.moqui.impl.ComponentServices`. Screens: System › Conf (its Settings,
Edit and Changes tabs) and System › Components.

## Reloading the conf

A reload merges the conf again from the files, as boot does: `MoquiDefaultConf.xml`, each component's
`MoquiConf.xml` in component order, then the runtime conf on top
([Run-Deploy](../Run-Deploy.md#runtime-directory-and-conf)). It compares the result with the running conf. A
setting that a later component or the runtime conf overrides therefore shows no change.

There are four ways to reload:

- **System › Conf › Changes.** The tab shows every change between the files and the running conf, with its class and
  what applying it would do. **Apply Changes** reloads. Under **Waiting for a Restart**, **Restart Now** restarts the
  server (`restart#Server`) where it can: a dev instance, a user with `DEV_AIDE` or in `ADMIN`, and a server that can
  restart itself (`./moq run`, or `moqui` as a server).
- **SIGHUP.** `systemctl reload moqui` sends it; the unit in `systemd/` has the `ExecReload` line. The report goes to
  the log, on the `moqui.monitor.audit` target. SIGHUP reloads only when stdin is not a terminal. Under `./moq run` in
  a terminal it ends the server, as before.
- **The dev file watcher.** On a dev instance, a saved `MoquiConf.xml` or runtime conf reloads by itself. The report is
  in the reload log. A `component.xml` change still takes a restart.
- **System › Conf › Edit.** Saving a conf file reloads ([Viewing and editing the conf](#viewing-and-editing-the-conf)).
- **The services.** `preview#ConfReload` changes nothing. `reload#Conf` applies. `get#PendingRestart` lists what the
  last reload left waiting. Each needs the `ADMIN` group.

Reloads are applied one at a time, so a save, the file watcher and SIGHUP never replace the running conf at once.

## Viewing and editing the conf

**Settings** (System › Conf, the first tab) shows the merged conf as a tree. The conf is merged again from the files,
as a reload does, with each value's file and line recorded (`moqui_lib::node::with_merge_sources`, which only this
merge turns on). The files are listed in merge order at the top. Each element shows:

- its key, such as `webapp[webroot]`;
- its class chip: live, apply or restart, with the reason as the chip's tooltip;
- the files and lines it came from: the element's own file first, then each file merged into it;
- its attributes. An attribute set by a later file than the element's own shows that file. An attribute whose class
  differs from the element's shows its class. A `${}` value also shows its expanded value.
- "not running yet" on a value the files hold that the running conf does not: one waiting for a restart, or one not
  reloaded yet;
- for a `default-property` that the environment, `runtime/.env` or `-D` sets, the value that wins;
- the default conf's comments above the element: two lines, and all of them when clicked.

Secrets are masked as `••••••`: a secret `default-property` value, an attribute such as `password`, `crypt-pass` or
`api-key`, and an expanded value that uses a secret property. Search keeps the elements whose name, key or an
attribute matches. The terminal shows the same tree. Viewing needs the `ADMIN` group (`get#ConfView`).

**Edit** changes one conf file at a time. Choose the file, or open it from a source on Settings, which goes to its
line. Reading and saving need the sealed `CONF_EDIT` permission, which `ADMIN_ADV` has. The editor is the framework's
XML code editor:

- completion comes from `moqui-conf-3.xsd`;
- the gutter marks each element's class: **L** live (in effect when saved), **A** apply (built again when saved) and
  **R** restart (waits for a restart). Its tooltip names the attributes whose class differs, such as a `webapp`'s
  `http-port`;
- the default conf's comments show above the elements of the file that merge into an element they describe: two
  lines, and all of them when clicked;
- the lint (`check#ConfFile`) marks parse errors, schema warnings and, on its line, each change that would wait for a
  restart.

**Review and save** shows the diff, and every change the reload would make with its class. A change another file
holds that is not running yet is listed too, because the reload applies it with this one. Saving (`save#ConfFile`)
writes the file whole, through a file beside it renamed over it, and logs the user on `moqui.monitor.audit`. It then
reloads: what can be live is in effect at once, and what waits for a restart is named, with a link to Changes and
Restart Now.

A save is refused when:

- the file is the default conf: a framework update replaces it, so override its settings in the runtime conf;
- the server cannot write the file;
- the file changed on disk since it was read: read it again;
- the text does not parse, its root is not `moqui-conf`, or the files do not merge with it.

The services name a file by its place in the merge (`fileIndex`), never by a path. In the terminal, and as a form in
the browser, Edit is `EditTui`: a text editor with Save and reload.

## What changes live

Each part of the conf has a class:

| Class | Meaning |
|---|---|
| live | Read on each use: in effect once the conf is replaced |
| apply | Built from the conf: built again after the reload |
| restart | Read once at boot: waits for a restart |

A change that waits for a restart is kept out of the running conf. What the server runs on stays consistent with what
was set up at boot. Every later reload lists the change again until a restart.

An element added or removed whole waits for a restart when anything in it needs one. For example, a new `webapp` with
an `http-port` waits whole.

| Conf | Class |
|---|---|
| `screen-facade`: render modes, `screen` mounts (`subscreens-item`), its settings | apply |
| `webapp`: `root-screen`, `token-route`, servlets, filters, listeners, the request and login actions, `before-shutdown` | apply |
| `webapp`: `web-protection` rules (rate limits, bans, rules); the guard keeps its clients and their connections | apply |
| `webapp`: `web-protection` `connections` and `requests` (listener limits) | restart |
| `webapp`: `error-screen`, `public-site`, `response-header`, `session-config` timeout, host and CORS attributes | live |
| `webapp`: ports, `https-enabled`, `trusted-proxies`, `client-ip-header`, `transfer-workers`, `endpoint`, `after-startup`, session `flush` and `max` | restart |
| `cache-list`: a new `cache` | apply |
| `cache-list`: a changed or removed `cache` | restart |
| `default-property`: a new property, or one whose value the conf set | apply |
| `resource-facade`: `template-renderer` | apply |
| `resource-facade`: `resource-reference` | restart |
| `artifact-execution-facade`, `user-facade`, `screen-facade/pdf`, `service-file`, `report`, `terminal`, `email-client`, `dav-server`, `git-server` request settings, `dev-aide` | live |
| `tools`, `telemetry`, `logging`, `transaction-facade`, `entity-facade`, `database-list`, `search-facade`, `llm-facade`, `service-facade` pools and `service-type`, `ssh-server`, `smtp-server`, `edge`, `dev-reload`, `component-list` | restart |
| an element the catalog does not know | restart |

Monitor › Settings changes log levels and telemetry rates for a while
([Monitoring](Monitoring.md#changing-settings-for-a-while)).

### Properties

A `default-property` follows the boot rules:

- A new property is set unless the environment, `runtime/.env` or `-D` already set it. Those win, and the change has
  no effect.
- A secret still at its `CHANGEME` default is generated and saved to `runtime/.env`, as at boot.
- A changed value is set only while the property still has the value the conf set. Otherwise something else set it,
  and that wins.
- A removed property, and a changed secret, wait for a restart.
- So does every property the process was set up with: `instance_purpose`, the datasource (`entity_ds_*`,
  `content_ds_*`), cluster, edge, port, host, bind and key properties, time zones and locale, and the search backend.
- A property that a restart-class setting reads also waits for a restart, so the two never disagree.
- A setting that reads a property set live is listed with the change, as `property`.

## Loading a component

A component placed in a component directory (`runtime/component`, by `./moq component-get` or a git clone) loads at
the next start. System › Components can also load it without a restart.

**Found in a Component Directory** lists the components that discovery finds, as boot finds them, and that are not
loaded. **Preview** shows what loading one would do:

- Why it cannot be loaded now, if it cannot:
  - a component it depends on is not loaded;
  - it has Rust code, which needs `./moq build` and a restart;
  - one of its XML files does not parse;
  - an entity change the table check cannot make: a primary key field added, or a field's type changed;
  - the loaded components no longer match the directories;
  - the server is in a cluster, whose other members would not have it.
- The conf changes its `MoquiConf.xml` makes, each with its class ([What changes live](#what-changes-live)).
- Its new entities and the ones it extends.
- Its data files by the type they declare. Files that declare no type load with any choice.

**Load** loads it now, in this order:

1. **Entities.** The tables of its new and extended entities are made against the new entity model, before the model
   is swapped in. No request reads a column before it exists. A failed step leaves the model and the components as
   they were.
2. **Registration.** The component is registered, so `component://` locations in it resolve.
3. **Conf.** The conf is merged again and what can be live is applied. A change that needs a restart waits for it.
4. **Files.** Its services, ECA rules, REST APIs, screens, templates and skills go through the reload pipeline, as a
   dev reload does, and the report definitions are read again. A problem in one of its files is reported with the file.
5. **Data.** The data types chosen, by default seed, seed-initial and install. Each file loads in its own transaction;
   one that fails is reported and the rest go on. The entity ECA rules and data feeds run as usual. The search indexes
   of new documents are built after.

**At the next start** records the component instead. The next start loads it as boot loads every component, then
loads its data. **With a restart now**, shown only where the server can restart for this user (as Restart Now on
System › Conf), records it the same way and restarts the server at once.

**Load Data** on a row of **Loaded** loads the data files of the types named (`seed` by default) of a loaded component
again, each file in its own transaction. A rerun is safe: a record that is there is updated.

`moqui.server.DeployedComponent` keeps each loaded component, with its data types and its state, and
`DeployedComponentHistory` keeps each load. **Remove** leaves a deployed component out at the next start: its directory
moves to `.removed/` beside it. What it loaded stays until the restart, and its tables and data stay after. A working
clone with changes that are not committed, or commits not pushed, is not removed at first: the screen lists the changes
and asks again with **Remove Anyway**, which moves the clone aside with its changes.

The services are `scan#Components`, `preview#ComponentLoad`, `load#Component` (`mode` now, restart or restart-now,
`dataTypes`), `load#ComponentData` (`componentName`, `dataTypes`) and `remove#Component` (`confirmDirty`; out `dirty`
with the changes when it did not remove a working clone). Each needs the `ADMIN` group.

The Dev AIDE's Repositories page does the same preview after **Get** or **Clone**: when nothing in the component waits
for a restart it offers **Load**, which runs `load#Component`; otherwise it says the component loads after a restart,
and why.

## Deploying from git

**Deploy from Git** on System › Components fetches a branch of a git URL and keeps it one of three ways, chosen per
component:

| Storage | Repository | The files the server loads |
|---|---|---|
| Database (the default) | a `db` GitRepository, `component-<name>`, that imports the branch | written to `runtime/deployed-component/<name>` |
| Bare repository on disk | a `remote` GitRepository mirror in `runtime/git/component-<name>` | written to `runtime/deployed-component/<name>` |
| Working clone | a normal clone made by the `git` command, which must be installed | the clone, in a component directory (`component` unless named) |

A developer can edit and commit in a working clone. The written files of the other two are output: an update replaces
them.

**Fetch and Preview** fetches the branch and writes its files to `.staging-<name>` beside where they go, where
discovery does not see them. It shows the same preview a load shows. A staged component then has three actions:

- **Deploy** moves the files into place and loads the component, as Load does.
- **At Next Start** moves them into place for the next start.
- **Deploy and Restart**, where the server can restart for this user, moves them into place for the next start and
  restarts the server.
- **Discard** drops them.

If the load fails, the files go back to where they were staged.

A component deployed from git has two more actions:

- **Check for Update** fetches the branch again and lists the files that changed between the deployed commit and the
  head.
- **Update** puts the new files in place and keeps the old ones in `.previous/`. A working clone is fast-forwarded
  instead, and is refused when it has changes of its own or has diverged from the branch. The update then:
  1. relinks the entities when an entity file changed;
  2. applies the conf when its `MoquiConf.xml` changed;
  3. reloads the changed files;
  4. loads the changed data files (`seed` unless other types are named).
- **Update and Restart**, where the server can restart for this user, updates and then restarts the server, for the
  changes that wait for a restart.

Deploying runs the component's code and loads its data, which may include security data. So it needs:

- the sealed `COMPONENT_DEPLOY` permission, seeded on `ADMIN_ADV`, which only a holder can grant;
- on a production instance, also `component_deploy_enabled=true`. Without it, deploy is refused with no detail.

The URL must be `https://` or `ssh://` (or `user@host:path`). A local path and `file://` are refused. On a production
instance, a host at a loopback, private or link-local address is refused too. Sim mode and the remote call fence stop
a fetch, as they stop every fetch. The access token is kept encrypted on the GitRepository and the
`DeployedComponent` row. A working clone sends the token as an HTTP header and never writes it into the clone's conf.

The services are `preview#ComponentDeploy` (`url`, `branch`, `storage`, `directory`, `username`, `accessToken`),
`deploy#Component` (`mode` now, restart or restart-now), `discard#ComponentDeploy`, `preview#ComponentUpdate` and
`update#Component` (`mode` now or restart-now).

## Clusters and containers

A component deployed with the **database** storage reaches every member of a cluster. Its repository is in the shared
database. Deploy and Update publish on the `component-deploy` cluster topic, and each other member then works from the
shared repository:

- it writes the commit's files to its own `runtime/deployed-component`;
- it loads the component, or applies the update, as the deploying member did;
- it does not load the data, which is in the shared database already.

A member started later finds the component the same way. So does a new container whose filesystem is fresh. At start,
a component kept in the database whose directory is missing is written out from its repository at the deployed
commit, and the factory starts again once to load it. A component removed on another member has its files here moved
aside the same way.

These are refused while the server has live cluster members, because the other members would not have the files:

- the bare and working-clone storages;
- a load from a local directory.

Restart each member instead, with the component in place on each.

Removing a component live is not supported: **Remove** takes effect at the next start.
