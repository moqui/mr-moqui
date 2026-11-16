# Attack surface: development tools

This file covers what loads the dev set of components, the Dev AIDE app (`/apps/dev`) and its anonymous static mount (`/devstatic`), the framework's development services and the artifact index over REST (`/rest/s1/moqui/aide`), the demo lock-down component, the example and template components, the test-only hooks, and the load runner's targets. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## What loads the dev components

`runtime/components/components.xml` lists four component sets in load order: `base`, `addon`, `dev` with `purpose="dev"`, and `test` with `purpose="dev, test"`. Only a set listed there loads; a directory beside the file that it does not list is named in a WARN and not loaded (`crates/moqui-core/src/context/component.rs`, `add_component_dir`).

A component with purposes loads only when the instance purpose is one of them (`Discovery::allows`). A component found in a `component-dir` with a `purpose` takes the directory's purposes unless it has its own. A component left out is named in a WARN. An empty instance purpose matches no restricted component.

The instance purpose used for discovery is read in this order (`effective_instance_purpose`): the process property or environment variable `instance_purpose`; then `instance_purpose` or `instance.purpose` in `runtime/.env`; then the merged conf's `default-property`. `runtime/components/base/framework/conf/MoquiDefaultConf.xml` sets `production`. `runtime/conf/MoquiDevConf.xml` sets `dev`. A component's own conf merges after discovery.

At request time the dev tools ask again: `moqui_core::devtools::is_dev_instance()` is true only when the property or environment variable `instance_purpose` equals `dev` (`crates/moqui-core/src/devtools/mod.rs:27`).

| Component | Directory | `component.xml` | Loads when |
|---|---|---|---|
| `dev-aide` | `runtime/components/dev/dev-aide` | `purpose="dev"` | The instance purpose is `dev`. The purpose is on the component itself, so a copy in another directory has it too. |
| `example` | `runtime/components/dev/example` | no purpose; `disabled="${example_disabled ?: 'false'}"` | The dev set loads (purpose `dev`) and `example_disabled` is not `true`. |
| `template` | `runtime/components/dev/template` | `name="@component@"`, `disabled="true"` | Not as it stands: it is the skeleton a new component is copied from. |
| `demo` | `runtime/components/dev/demo` | no purpose; `disabled="${demo_disabled ?: 'true'}"`; `depends-on` `tools` | A conf names it. `runtime/conf/MoquiDemoConf.xml` has `<component name="demo" location="components/dev/demo"/>` in its `component-list` and keeps the production purpose. |

A conf's `component-list` may name a `component` or a `component-dir`; each goes through the same purpose check (`discover_for`). A conf that names a component with no purpose of its own loads it on any instance purpose.

`./moq archive` leaves out `components/dev` unless `--with-dev`, and `components/test` and each component's `test/`, `plan/` and `plans/` unless `--with-tests` (`moq`, `task` archive, the `excludes` list). The default conf of an archive is `conf/MoquiProductionConf.xml`. The task's help states that `--binary` builds the same file set in (`--embed production`, or `--embed all` with either option) and that the archive includes `runtime/.env`, `runtime/db` and `runtime/cert` unless `--no-data`. `--with-dev` keeps the whole dev set: `dev-aide`, `example`, `template` and `demo`.

## Dev AIDE

Dev AIDE (`runtime/components/dev/dev-aide`) is an editor, git client, entity, service and screen builder, and runner for code and data on the server's own files. Its component comment states that it gives its users code execution as the server's OS user.

| Path | On by default | Authentication | Authorization | Input |
|---|---|---|---|---|
| `/apps/dev`, `/qapps/dev` (`screen/DevAide.xml`, subscreens `Home`, `Workspace`, `Repositories`, `Artifacts`, `Entities`, `Services`, `Screens`, `Eca`, `Data`, `Tasks`, `Settings`) | Only when the component loads (instance purpose `dev`). Mounted under `webroot/apps.xml` as `dev` by the component's `MoquiConf.xml`. | Session. An API key reads; see the tiers. | Artifact group `DEV_AIDE_APP` for `ADMIN`, `DEV_AIDE_USERS` and `DEV_AIDE_EXTENDERS`, then the tier each service checks. | File paths, file content, git arguments, service names and parameters, JavaScript and XML Actions, entity names and filters, URLs of group servers and task sources. |
| `/devstatic` (`screen/devstatic.xml`) | Only when the component loads. Mounted on `webroot.xml` as `devstatic`, `menu-include="false"`. | None: `require-authentication="anonymous-view"`. | None. | The path under `/devstatic`. The directory `screen/devstatic/` holds eight JavaScript files of the Workspace client (`DevAide.js`, `DevAideBlocks.js`, `DevAideBrowser.js`, `DevAideEntity.js`, `DevAideGit.js`, `DevAideQvue.js`, `DevAideScreen.js`, `DevAideService.js`). |

### Groups, permissions and the artifact group

`data/DevAideSecurityData.xml` is seed data of the component, so its rows exist only where the component has loaded.

- Groups: `DEV_AIDE_USERS` and `DEV_AIDE_EXTENDERS`.
- Permissions: `DEV_AIDE_VIEW`, `DEV_AIDE_EXTEND`, `DEV_AIDE`. `ADMIN` and `DEV_AIDE_USERS` hold `DEV_AIDE`. `DEV_AIDE_EXTENDERS` holds `DEV_AIDE_EXTEND`. No row in the file grants `DEV_AIDE_VIEW` to a group.
- `DEV_AIDE_APP` holds the screen `component://dev-aide/screen/DevAide.xml`, the services `org\.moqui\.devaide\..*`, and `org.moqui.dev.DevToolsServices` `apply#XmlOps`, `get#SchemaDigest`, `get#XmlOutline`, `rebuild#ArtifactIndex` and `restart#Server`, each with `inheritAuthz="Y"`. `ADMIN`, `DEV_AIDE_USERS` and `DEV_AIDE_EXTENDERS` each have `AUTHZT_ALWAYS` `AUTHZA_ALL` on it. What an inheritable `ALWAYS` grant permits below it is in [Attack-Surface-Data.md](Attack-Surface-Data.md#entity-authorization-and-row-filters).
- `DEV_AIDE_ENTITIES` holds the entities `moqui\.dev\..*`, not inheritable: `ADMIN` `AUTHZT_ALWAYS` `AUTHZA_ALL`, `DEV_AIDE_USERS` `AUTHZT_ALLOW` `AUTHZA_ALL`.
- `DEV_AIDE_USERS` and `DEV_AIDE_EXTENDERS` get `AUTHZT_ALLOW` `AUTHZA_VIEW` on the framework's `AIDE_READ_API` group.

### Tiers as the services check them

Every dev-aide service sampled starts with `Context::of(ec, <tier>)`, which calls `tier::require` (`src/tier.rs`, `src/workspace.rs:86`). `tier::require` refuses when `is_dev_instance()` is false. It refuses a call that needs the extend tier or above on a request logged in by a header (an API key). The tier of a user is full with `DEV_AIDE` or membership of `ADMIN`, extend with `DEV_AIDE_EXTEND`, view with `DEV_AIDE_VIEW`, else none.

| Family (file) | View tier | Extend tier | Full tier |
|---|---|---|---|
| Files (`src/workspace.rs`) | `get#Roots`, `list#Directory`, `get#File`, `find#Files`, `search#Files`, `check#File`, `set#Presence` | `save#File`, `create#File`, `create#Directory`, `rename#Path`, `delete#Path` | `set#UpstreamEdits`, `set#Setting`, `store#Root`, `delete#Root`, `create#SiteComponent`, `create#Component`; `get#Credentials`, `store#Credential` and `delete#Credential` need the full tier and, for the first two as read, membership of `ADMIN` |
| Git (`src/vcs.rs`, `src/repo.rs`) | status, diff, log, commit, hunks, branches, tags, stashes, tree, file at a revision, blame, conflict sides, compare refs, `set#CurrentTask` | stage, unstage, discard (files and hunks), commit, fetch, pull, push, create branch, resolve conflict, abort operation, cancel operation, publish site | switch and merge branch; create tag; apply and pop stash; revert and cherry-pick; a forced push; clone repository; init repository; what goes through `branch_write` (`src/repo.rs:307`), which `add#Remote` calls and which `rename#Branch`, `delete#Branch` and `set#RemoteUrl`, with no tier call of their own in the outline read, appear to use |
| Entities and data (`src/entities.rs`) | `list#Entities`, `get#EntityView`, `get#ViewSql`, `check#DataFile`, `list#DataFiles`, `export#Records` | `load#ChangedRecords`, `load#SeedFile` | `plan#EntityDdl`, `run#EntityDdl` |
| Services (`src/services.rs`) | `list#Services`, `get#ServiceView`, `get#GeneratedJs`, `list#EcaRules`, `eval#Condition`, `try#Service` in the sim | `try#Service` with `real` and `confirm`, `try#Snippet`, `save#TrySpec`, `run#Spec` | |
| Screens (`src/screens.rs`) | `get#ScreenChildren`, `get#ScreenView`, `put#CanvasBuffer`, `resolve#Stamps` | `create#Subscreen`, update, move and remove screen | |
| Local run and worktrees (`src/local_run.rs`, `src/worktree.rs`) | `get#LocalJobs`, `get#Worktrees` | `start#LocalRun`, `create#Worktree`, `remove#Worktree` | |
| Group server (`src/hosting/`) | `get#Servers`, `start#Connect`, `finish#Connect`, `store#ServerToken`, `refresh#Server`, `apply#ServerSetup`, `remove#Server`, merge request reads, review threads and comments, `submit#Review`, checks, `rerun#Check`, `get#Agreement` | `open#MergeRequest`, `propose#Changes`, `checkout#MergeRequest`, `fork#Repository`, `sign#Agreement` | The parts of `apply#ServerSetup` and `remove#Server` that write `myaddons.xml` and `.env`, and not when the `shared` setting is on |
| Tasks (`src/tasks.rs`) | `get#Tasks`, `get#TaskSources`, `set#TaskSource`, `store#TaskSource`, `delete#TaskSource`, `create#Task` | `create#TaskBranch` | `store#TaskApp` |
| Artifacts and docs (`src/artifacts.rs`, `src/docs.rs`, `src/browser.rs`, `src/changeset.rs`) | search, detail, refs, definition, customizations, palette, `get#ArtifactDoc`, browse | `export#ArtifactDocs`, `get#ChangeSet`, `complete#Review` | |

The extend tier runs code on the server. `try#Service` with `real` calls any defined service with caller parameters and writes a `DAA_TRY` audit row. `try#Snippet` runs caller JavaScript or XML Actions in the sim. `run#Spec` runs a spec. `start#LocalRun` writes the steps of one job of the working tree's `ci.xml` to a script under the runtime's `tmp` and types `sh <script>` into a host session of the Terminal (`src/local_run.rs`).

The sim (`moqui_llm::approval::preview::run_in_sim_within`) runs a body as the current user and returns its out-parameters, messages, errors, the rows it would write and its log (`in_sim`, `src/services.rs`). A sim run of the extend tier or above runs within the calling dev-aide service's frame, so it has what `DEV_AIDE_APP` passes on. A sim run of the view tier has no such frame (`sim_within`). The sim needs embedded Postgres; without it `try#Snippet` and `eval#Condition` are refused.

`eval#Condition` (view tier) takes `condition`, an expression or a condition element, and `against`, a JSON object. It builds XML Actions around the condition (`<if condition="...">` or `<if><condition>...`), checks them and runs them in the sim.

`export#Records` (view tier) takes an entity name and an optional `filter` map of equalities, counts the matches, refuses a count over `MAX_EXPORT`, and returns the records as entity XML text (`EntityDataWriter`). The service does not turn authorization off and has no list of entities it refuses.

### File paths

A file path of a request is relative to a root and goes through `confine::resolve` (`src/confine.rs`). It refuses a NUL byte, a leading `/` or `\`, a drive letter, and any `..` part. It canonicalizes the deepest part that exists, so a symlink is followed, and refuses a result outside the root. It refuses a path with a `.git` part, a path whose first part under the root is `target`, and a path inside the runtime's `db`, `log`, `txlog`, `tmp`, `.env` or `cert`. It refuses a path inside `runtime/worktrees` unless the root itself is there.

The deny list has those six runtime names. `runtime/conf`, `runtime/save`, `runtime/sftp` and `runtime/install` are not on it.

`get#File` treats a file with a NUL in its first 8 KB as binary and applies `max-edit-bytes` (2097152 in the component's conf). `save#File` refuses content over `max-edit-bytes`. `get#FileAt` (view tier) resolves the path the same way and runs `git show <rev>:<path>` after `valid_rev`.

A write needs the extend tier, a root that Settings has not made read only, and passes `may_write` (`src/workspace.rs:131`). The upstream lock is lifted only for the full tier, by the user preference or the conf's `allow-upstream-edits` (false in the component's conf).

### Roots

The roots are the git repositories that hold the loaded components (found by looking up from each component directory for `.git`), each loaded component in no repository, `runtime/conf` when no repository holds it, and the rows of `moqui.dev.DevAideRoot` (`src/roots.rs`). `store#Root` (full tier) takes `location`, canonicalizes it, needs a directory, and refuses only a location inside one of the six denied runtime names. Any other directory the server's OS user can read becomes a root.

### Git

Git runs as a child process with an argument vector, never through a shell (`src/git.rs`). The command is the conf's `git-command`. Each call adds `-c core.quotepath=off -c color.ui=false` and sets `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0`, `GIT_LITERAL_PATHSPECS=1`, and empty `GIT_ASKPASS` and `SSH_ASKPASS`. The module comment states that git uses the trees' own config, hooks, signing and credentials. Paths follow `--`. A remote URL is refused when it is empty, starts with `-` or has a control character (`valid_url`, `src/repo.rs:871`). Network commands are refused when the `shared` setting is on and `network_git` is off (`network_allowed`).

A stored token reaches git through environment variables (`MOQUI_DEV_AIDE_GIT_TOKEN` and three numbered ones, with matching `..._USERNAME` names) and a `-c credential.<prefix>.helper=!f() {...}` argument that echoes them, after `-c credential.helper=` (`token_helper`, `src/vcs.rs`). The token is not in the argument vector.

### Settings, credentials and the group server

`moqui.dev.DevAideCredential.token`, `moqui.dev.DevAideTaskSource.token`, and `moqui.dev.DevAideServer` `gatewayKey`, `pendingVerifier` and `envPriorJson` are `encrypt="true"` (`entity/DevAideEntities.xml`).

`store#Credential` needs the full tier and `ADMIN`, turns authorization off, and stores a token for a root, optionally for one user, with the scheme, host and port of an HTTPS remote. It logs the root, the user and who stored it, not the token. `get#Credentials` returns the rows without the token.

The group server connection (`src/hosting/connect.rs`) is refused on a request logged in by a header. `start#Connect` takes `baseUrl`, stores a random 32-character `pendingState` and a 64-character PKCE verifier on the user's `DevAideServer` row, and redirects the browser to the server's authorize URL with this instance's own `/qapps/dev/Settings/serverSignedIn` as the redirect. `finish#Connect` finds the row by `pendingState`, requires its `userId` to be the current user, clears the pending fields, refuses after `PENDING_MILLIS`, and posts `code`, the verifier and the redirect to the server's token path (default `/sso/appToken`). The answer's `loginKey` is stored as the user's token.

The `serverSignedIn` transition has `require-session-token="false"`.

`apply#ServerSetup` writes instance files only for the full tier with `shared` off. It writes three names to `runtime/.env`: `llm_openai_url` (the server's offered gateway URL, checked by `on_server`), `llm_openai_api_key` (a key the server issues for scope `gateway`), and `llm_openai_model` (the request's `gatewayAlias`, default `default`). The earlier values of those three lines are kept in `envPriorJson`. `moqui_lib::env::update_dotenv_file` refuses a name with `=` or a line break and a value with a line break, and writes the file with mode 0600 on Unix.

### Transitions and request methods

`screen/DevAide/Workspace.xml` has 117 transitions. Each calls one service with `in-map="true"` and answers JSON. Reads have `read-only="true"`; no transition in the file has a `method` attribute. The same holds for the transitions read in `Home.xml` (`publish`, `createSite`, `createComponent`, `restart`), `Repositories.xml` (`fetchAll`, `pullAll`, `getComponent`, `cloneRepository`, `loadComponent`), `Tasks.xml` and `Settings.xml`. How a transition without `method` answers a GET, and when the session token is checked, is in [CSRF](Attack-Surface-Web.md#csrf).

`Workspace/restart` and `Home/restart` call `org.moqui.dev.DevToolsServices.restart#Server`, which reads no parameter.

### Notifications

Reload reports, presence, saves and git progress go out as notifications (`src/notify.rs`). `DevReload`, presence and save notices go to the dev-aide's users; git progress goes to the one user who started the command.

### Agent tools

The dev agent's tools are `artifact`, `source`, `check`, `propose_edit`, `try_service`, `vcs` and `hosting` (`src/agent/`). Their shared helper `context(least)` builds the same `Context` with a tier, so the dev instance check and the user's tier apply to a tool call that uses it (`src/agent/mod.rs`). In an agent session with worktrees, the session's worktree roots stand in for the main roots.

## Framework development services

`org.moqui.dev.DevToolsServices` is implemented in `crates/moqui-devkit/src/service.rs` and is part of the base components, so it exists on every instance.

- `restart#Server` needs a dev instance and `DEV_AIDE` or `ADMIN` (`allowed`), and a server that can restart itself. It logs a WARN with the user and restarts 200 ms after the answer. `allowed` does not look at how the request logged in.
- `apply#XmlOps` needs a dev instance and one of the three dev-aide permissions or `ADMIN` (`allowed_read`). It takes `text` and `ops`, and returns the edited text and its outline. It writes no file.
- `find#Artifacts` and the `get#Artifact*` reads choose their source in `choose_source`; see the next section.

## The artifact index over REST

| Path | On by default | Authentication | Authorization | Input |
|---|---|---|---|---|
| `GET /rest/s1/moqui/aide/projects/{projectId}/artifacts`, `/artifact`, `/refs`, `/bindings`, `/at`, `/extensions` (`runtime/components/base/tools/service/moqui.rest.xml`) | Yes, on every instance purpose. | As for the other `/rest/s1` resources ([Attack-Surface-Web.md](Attack-Surface-Web.md)); the seed's comment names an API key. | Artifact group `AIDE_READ_API` (the services and the REST path `/moqui/aide`): `ADMIN` `AUTHZT_ALWAYS` `AUTHZA_ALL`, `AIDE_READERS` `AUTHZT_ALLOW` `AUTHZA_VIEW`, and on a dev instance the two dev-aide groups. Then `choose_source`. | `projectId`, and the query parameters of each read (`artifactId`, `project` and others). |

`choose_source` (`crates/moqui-devkit/src/service.rs:146`) refuses a caller who has neither `AIDE_READ` or `ADMIN`, nor a dev-aide permission on a dev instance. With a `projectId` it reads the `moqui.devtools.DevArtifactProject` row with authorization off and answers only from that row's `directory`, and only when `allowed` is `Y` and the directory is not empty. Without a `projectId`, off a dev instance it refuses. On a dev instance a dev-aide reader may name a `project` directory or read the server's own index.

`get#Artifact` returns the artifact row, its parts (an entity's fields, a screen's transitions and forms) and, for a service, its declared parameters.

`moqui.devtools.DevArtifactProject` is in the group `AIDE_PROJECTS`, not inheritable, with `ADMIN` `AUTHZT_ALWAYS` `AUTHZA_ALL` (`runtime/components/base/framework/data/DevToolsSecurityData.xml`). `ADMIN` and `AIDE_READERS` hold `AIDE_READ`.

## Demo component

`runtime/components/dev/demo` narrows what `ADMIN` reaches on a public demo server. The summary for operators is in [Attack-Surface.md](Attack-Surface.md#public-demo-server). The rows as the component has them:

- Screens replaced by `component://demo/screen/Disabled.xml` (`MoquiConf.xml`): under `Tools.xml` `Sql`, `SqlTui`, `SqlDownload` and `Load`; under `Tools/Data.xml` `Load`, `Export` and `Snapshots`; under `Tools/Database.xml` `SpeedTest`, `Activity`, `Queries` and `Connections`; under `Tools/Service.xml` `ServiceRun`.
- Servers off in the same conf: A2A server and client, MCP server and client, the LLM gateway, `ssh-server` and `dav-server`. `email-client allowed-hosts` is `localhost,127.0.0.1`.
- `ADMIN` authorization changed to `AUTHZT_ALWAYS` `AUTHZA_VIEW` (`data/DemoSecurityData.xml`, seed): `TOOLS_APP`, `SYSTEM_APP`, `MOQUI_API`, `EntitySyncServices`, `SystemMessageServices`, `ResourceCommandServices`, `GitRepositories`, `REPORT_GIT_SYNC_ADMIN`, `REPORT_CONTACT_ADMIN`.
- `ADMIN` permissions ended by a past `thruDate`: `SearchRemote`, `DashboardRemote`, `ADMIN_PASSWORD`, `SSH_SFTP`, `SSH_GIT`, `MONITOR_INGEST`, `REPORT_SAVE_LOCATION`, `MONITOR_RECOVER`, `MONITOR_RECOVER_ADMIN`, `MONITOR_SETTINGS`.
- `EX_JOHN_DOE` membership of `ADMIN_ADV` ended (`data/DemoUserData.xml`, demo data).
- A pre-service SECA calls `demo.DemoServices.refuse#SharedAccountChange` before `org.moqui.impl.UserServices` `update#Password`, `reset#Password`, `update#UserAccount`, `disable#UserAccount`, `create#UserAuthcFactorEmail`, `create#UserAuthcFactorSms`, `create#UserAuthcFactorTotp`, `setup#UserAuthcFactorTotp`, `create#SingleUseAuthcCodes` and `create#SingleUseAuthcFactor` (`service/Demo.secas.xml`).

Rows of the base seed for `ADMIN` that the demo data does not overwrite include these `AUTHZA_ALL` grants: `DASHBOARDS_APP_ADMIN`, `CALC_GRID_ADMIN`, `CALC_GRID_AUTHOR_ADMIN`, `BASIC_COMMS_ADMIN`, `REPORT_BUSINESS_ADMIN`, `REPORT_DATASETS_ADMIN`, `REPORT_DELIVERY_ADMIN`, `REPORT_SHARE_ADMIN`, `TOOLS_DATABASE_LOAD_ADMIN` (the service `moqui.tools.DatabaseLoadServices.run#LoadWrap`), `AIDE_READ_API_ADMIN` and `AIDE_PROJECTS_ADMIN`.

Permissions the base seed grants to `ADMIN` that the demo data does not end: `ADMIN_ACCESS_MANAGE`, `ADMIN_ACCESS_VIEW`, `ADMIN_AUDIT_VIEW`, `ADMIN_BACKUP_DOWNLOAD`, `ADMIN_BACKUP_RUN`, `ADMIN_BACKUP_VIEW`, `ADMIN_CONNECTION_MANAGE`, `ADMIN_CONNECTION_VIEW`, `ADMIN_CONSTITUTION_GLOBAL`, `ADMIN_IDENTITY_GLOBAL`, `ADMIN_IDENTITY_MANAGE`, `ADMIN_IDENTITY_VIEW`, `ADMIN_OPERATION_MANAGE`, `ADMIN_OPERATION_VIEW`, `ADMIN_OVERVIEW_VIEW`, `ADMIN_SETTINGS_MANAGE`, `ADMIN_SETTINGS_VIEW`, `ADMIN_TRANSFER_RUN`, `ADMIN_TRANSFER_VIEW`, `AgentAdmin`, `AgentSkillAuthor`, `AgentUse`, `AIDE_READ`, `ChatBridgeAdmin`, `DRIVE_OWNER_CHANGE`, `EMAIL_ACCOUNT_OWNER_CHANGE`, `ExamplePerm`, `GIT_CI_ADMIN`, `MONITOR_EVAL`, `MONITOR_EVAL_RUN`, `MONITOR_INCIDENT`, `MONITOR_REPLAY`, `REPORT_OWNER_CHANGE`, `REST_SCHEMA`, `VoiceAdmin`.

The demo directory is in the dev set, so a default `./moq archive` does not carry it; `--with-dev` carries it together with `dev-aide`, `example` and `template`.

## Example and template components

- `example` mounts `component://example/screen/Gallery.xml` under `webroot/apps.xml` as `gallery` (`MoquiConf.xml`). Its conf mounts nothing else.
- `example/screen/PublicLink.xml` has `require-authentication="false"`. The example's conf does not mount it; its comment gives the `token-route` element a conf would add, with `org.moqui.resource.FileServices.check#PublicShareToken` as the token service. Its `files` transition is `read-only="true"` and passes `token`, `grant` and `path` from the request to a service call.
- `template` starts a component with `disabled="true"` in `component.xml`, a root screen with `require-authentication="true"`, and two security data files that grant `ADMIN` `AUTHZT_ALWAYS` `AUTHZA_ALL` on the app's group and on its API group. No XML file in the template has `require-authentication="false"` or `anonymous`.

## Test-only hooks

| Hook | What it does | What turns it on | On a production instance |
|---|---|---|---|
| `ec.factory.getTool('HttpFixtures')` (`crates/moqui-core/src/service/http_fixtures.rs`) | Answers outbound HTTP requests of the process from fixtures and records them. | `http_fixtures::allow()`, a process-wide flag. Its one caller outside tests is `crates/moqui-host/src/test_cmd/mod.rs:825`. | The tool factory is declared in `MoquiDefaultConf.xml`. The flag is not set. |
| `ec.factory.getTool('MailCapture')` (`crates/moqui-core/src/service/mail_capture.rs`) | A send is kept in memory (the most recent `MAX_CAPTURED`) and no connection is made; `getMessages()` returns subject, addresses, body, headers and attachment names. | The property or environment variable `mail_capture_sends` equal to `true`, or the tool's `enable()`, per factory. | The tool factory is declared in `MoquiDefaultConf.xml`. `get_instance` and `enable()` have no check of the instance purpose or of a test run. |
| `X-Moqui-Test-Peer` header (`crates/moqui-host/src/http/test_peer.rs`) | Sets the request's peer address for per-requester limits. | `test_peer::allow()`. Its one caller is `crates/moqui-host/src/test_cmd/serve.rs:175` (`moqui test --serve`). | The flag is not set. |
| `runtime/conf/MoquiLoadTestConf.xml` | Sets `tarpit-enabled="false"` for screens, screen transitions and services, and `ssh_port` 2222. | A run that names this conf. | Not merged unless named. |

## Load runner targets

A load run needs the `LOAD_RUNNER` permission (`crates/moqui-host/src/load_runner/mod.rs:77`) and takes `targetUrl`. `TargetPolicy::of_factory` (`load_runner/targets.rs`) allows this server's own origins, loopback, and the URL prefixes and `ssh://host:port` entries of `load_runner_targets` (empty by default). On a dev instance `any` is true and a run reaches any URL and any SSH host. `DEFAULT_MAX_RUNS` is 1 and `DEFAULT_MAX_USERS` is 1000 when `load_runner_max_runs` and `load_runner_max_users` are not set.

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- How `component_from_dir` reads `purpose` and `disabled` from a `component.xml`, and how a conf's `component` element loads `demo` although its `component.xml` defaults to disabled, was not read.
- How `/devstatic` resolves a request path to a file under `screen/devstatic/`, and whether a path can leave that directory, was not traced.
- What the sim fences for a body it runs (file reads, outbound requests, `ec.factory.getTool`) was not read; `eval#Condition` and the view tier's `try#Service` depend on it.
- Whether `EntityDataWriter` writes `encrypt="true"` fields decrypted in the text `export#Records` returns was not read.
- The value of `MAX_EXPORT` and of `PENDING_MILLIS` was not read.
- Whether `fetch#All`, `pull#All`, `publish#Site`, `abort#Operation` and `cancel#GitOperation` need a parameter was not read.
- `search#Files`, `find#Files` and `list#Directory` were not read: whether they list or search untracked and ignored files, and which names they hide.
- The bodies of `rename#Branch`, `delete#Branch`, `set#RemoteUrl`, `delete#Tag`, `push#Tag`, `create#Stash` and `drop#Stash` were not read, so their tier is not settled.
- `rename#Path`, `delete#Path`, `create#SiteComponent`, `create#Component` and `clone#Repository` (where they write, and the names they accept) were not read.
- `on_server`, `normalize_base`, `checked_url` and `src/hosting/client.rs` (which URLs a group server connection sends a token to) were not read.
- `store#TaskSource` and `get#Tasks` (view tier; a task source has a URL and a token) were not read.
- The agent tools' bodies were not read: `hosting` (a remote server's text as tool output), `propose_edit`, `vcs`, and whether a write asks for confirmation. The `dev.aide` capability check was not read.
- `get#XmlOutline`, `get#SchemaDigest`, `rebuild#ArtifactIndex`, `project_in`, `get#ArtifactAt`, `get#ArtifactBindings` and `get#ArtifactExtensions` were not read; whether any returns source text was not settled.
- Which screen or service writes `moqui.devtools.DevArtifactProject` was not traced.
- `demo.DemoServices.refuse#SharedAccountChange` was not read. Which screens outside `TOOLS_APP` and `SYSTEM_APP` reach the services behind the `ADMIN` permissions the demo data leaves in place was not traced; the `org.moqui.admin` backup, connection and transfer services are called from `tools/screen/System/` screens.
- Whether `ec.factory.getTool` is reachable from scripts a non-developer can author (report or CalcGrid expressions, agent tools) was not traced.
- The Terminal's check for a host session (`TERMINAL_HOST`) behind `start#LocalRun` was not read.
- The loopback checks of the test peer header were not read in code.
- The LSP (`crates/moqui-host/src/lsp.rs`), the test client (`crates/moqui-host/src/test_client/`), `devtools_cmd`, the remote devtools client (`moqui check` and `refs --server`), lint, typegen, the file watcher, fixture users and `ClientFixtures`, the example's Rust services and `Gallery.xml` were not read.
