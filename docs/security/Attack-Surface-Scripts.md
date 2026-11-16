# Attack surface: scripts, templates and parsers

This file covers the code that runs inside the server as script: the JavaScript engine and what a script can call, the places where text that is not a source file is evaluated as an expression, a template or a script, how output is escaped when a screen renders, and the parsers that read bytes from outside (XML, zip, tar, images, spreadsheets). This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## What a script can reach

### The engine and its limits

- Server scripts run on quickjs-oxide, pinned by git revision in the workspace `Cargo.toml` with `default-features = false`. The binding is `crates/moqui-core/src/script/` (`oxide/` for the engine, `host/` for the functions a script calls).
- Each thread that runs scripts has one runtime. Its heap cap is 64 MiB (`HEAP_CAP` in `crates/moqui-core/src/script/oxide/engine.rs`).
- Script runs nested on one thread are counted. The limit is 64 (`MAX_SCRIPT_DEPTH` in `crates/moqui-core/src/script/run.rs`).
- A script ends when the thread's deadline passes or the thread's interrupt flag is set (`thread_interrupted` in `run.rs`). The limit is computed by `script_limit_for` from the `resource-facade` `script-timeout` setting, whether the thread is a request worker, and the webapp `request-timeout` (`webapp_request_timeout`). The default request timeout constant is 300 seconds.
- A thread can be restricted to a list of host functions (`restrict_host_calls` in `crates/moqui-core/src/script/runtime.rs`). The engine binder checks it on each host call (`host_call_allowed` in `oxide/bind.rs`). The one caller is the load runner's user threads (`crates/moqui-host/src/test_client/mod.rs`). No other script class uses it.

### Classes of script

| Class | Where it runs | What it can call | Limits |
|---|---|---|---|
| Server script: a service script, XML actions, a screen's actions and transitions, a widget attribute expression, and any text passed to `ec.resource.expression`, `condition`, `expand`, `expandNoL10n` or `script` | The thread's runtime, with the caller's context | The whole host table: every `ec.*` member listed below, and the globals such as `System` and `StringUtilities` | 64 MiB heap, depth 64, the thread's deadline |
| Locked template: the `fancy-locked` and `html.fancy-locked` template extensions (`MoquiDefaultConf.xml` `template-renderer`), and a Shop theme package (`crates/moqui-core/src/script/theme/`, through `template::render_locked_bounded`) | A separate runtime (`crates/moqui-core/src/script/oxide/templates.rs`) | Was not read (see Open questions) | `LOCKED_TIME_LIMIT` 5 seconds and `LOCKED_MEMORY_LIMIT` 32 MiB in `crates/moqui-core/src/script/template.rs`; a theme page also has its own `Limits` (sections, blocks, depth, include depth, bytes) |
| Standalone runtime: `Standalone::start` in `crates/moqui-core/src/script/standalone.rs` | A runtime of its own. `start` installs no host function: the host object implements only `interrupt` | Nothing of the server | The caller's heap cap and deadline. The OpenUI check uses 64 MiB and 5 seconds (`crates/moqui-llm/src/ui/openui.rs`). The Vue SFC compile check uses 16 MiB and 2 seconds (`crates/moqui-llm/src/ui/vue_sfc.rs`) |
| Load runner user script | A thread under `restrict_host_calls` | Only the host functions the harness allows; a service the harness calls runs unrestricted (`HostCallsLifted`) | As a server script |
| Decimal expression: `crates/moqui-lib/src/decimal_expr.rs` | No script engine. The module imports `rust_decimal` and the decimal helpers of `moqui_lib::object` only | Numbers, named inputs, operators and `if`, `min`, `max`, `abs`, `round`, `floor`, `ceil` | 2000 characters (`MAX_LENGTH`) and a nesting limit |

The template renderers by file extension are in `MoquiDefaultConf.xml`: `fancy` and `html.fancy` are the trusted renderer, which runs on the script runtime with the live context; `md` and `markdown` are Markdown; `md.fancy` and `markdown.fancy` are Markdown through the trusted renderer; `html` is not a template.

### Host members by capability

A server script has all of these. None of them checks a permission of its own unless the row says so. Authorization of an entity operation or a service call is the artifact authorization of the execution context ([Attack-Surface-Data.md](Attack-Surface-Data.md#entity-authorization-and-row-filters)).

| Capability | Members | What was read |
|---|---|---|
| Evaluate text as code | `ec.resource.expression(text, location, map)`, `ec.resource.condition(...)`, `ec.resource.expand(template, location, map)`, `ec.resource.expandNoL10n(...)`, `ec.resource.script(location)` (`crates/moqui-core/src/script/oxide/resource.rs`) | The text compiles and runs inside the calling run, so its bare names are the caller's and `ec` is in scope. `expand` looks the template up as a localized message first, then runs the result as a JavaScript template literal when it has a `$`. `script` reads a file through `crate::context::locate`, which serves `classpath`, `component` and `file` locations and refuses `content:`. In production a script location's text is kept and not read again. |
| Turn authorization off | `ec.artifactExecution.disableAuthz()`, `enableAuthz()`, `runAsSystem(body)`, `runAsUser(location, body)`, `disableEntityEca()`, `disableTarpit()`, `ec.entity.fastFindOne(entity, useCache, disableAuthz, ...)` (`crates/moqui-core/src/script/host/tools.rs` shim near line 918) | The members are on every server script's `ec`. |
| Start a process | `ec.resource.runTokenCommand(command)` (`tools.rs` line 721 and `run_command_first_line`) | The command is split on whitespace and started with `std::process::Command`, with no shell, stdin closed and a 30 second limit after which the child is killed. The first line of its output is returned. The only check before the start is the effect fence (`Effect::Process`). |
| Outbound HTTP | `ec.service.rest()` (`RestClient` in the `tools.rs` shim; `rest_send`) | The request is refused in sim mode and under the effect fence (`Effect::RemoteCall`). Internal addresses are refused only when the script sets `denyInternal` to true on the client. See [Attack-Surface-Outbound.md](Attack-Surface-Outbound.md). |
| Properties and environment | `System.getProperty(name)` (`crates/moqui-core/src/script/host/globals.rs` line 562) | It returns `moqui_lib::env::get_prop_or_env(name)`: the property map, then the process environment, then the `.env` values, and again with `_` replaced by `.`. The lookup has no check of a property's `is-secret` attribute, and no Rust code outside the tests reads that attribute. `MoquiDefaultConf.xml` declares `entity_ds_crypt_pass`, `entity_ds_crypt_pass_old` and `session_cookie_key` as `default-property` elements with `is-secret="true"`. |
| SQL | `ec.entity.runSqlStatement`, and the host functions `runSqlCountConf`, `runSqlUpdateConf` and `testConfConnection`, which open a connection from a configuration map passed by the script and run the SQL text on it (`tools.rs`) | Registered in the host table. |
| Resources and files | `ec.resource.getLocationReference`, `getLocationText`, `getLocationStream`, `readLines`, `digest`, `hmac`, and the host functions `resourcePutText`, `resourcePutBytes`, `resourceDelete`, `resourceMove`, `resourceCopy` | The resource facade serves `file`, `runtime`, `component`, `classpath`, `dbresource`, `http` and `https` locations and the schemes of registered providers (`crates/moqui-core/src/context/resource.rs`). A location with no scheme is a `file` location. Each write, move and delete checks the effect fence (`Effect::Resource`). |
| Archives | `ec.resource.gunzip(source, target, maxBytes)`, `extractArchive(source, target, limits)`, `writeTarGz(target, entries)` (`crates/moqui-core/src/script/host/archive.rs`) | See [Parsers of untrusted input](#parsers-of-untrusted-input). |
| HTML helpers | `ec.resource.markdownHtml(text)`, `StringUtilities.cleanHtml(text)`, `encodeHtml` helpers in `globals.rs` | `cleanHtml` is the sanitizer of [Rendering and escaping](#rendering-and-escaping). |
| Server administration | `clearAllCaches`, `loadData`, `writeData`, `checkTables`, `getThreadList`, `getRequestDetails`, `sharedGet`, `sharedSet`, `artifactPush` and the other methods registered as `tools` in `tools.rs` | Registered in the host table; the shims that expose each one were not all read. |

The other host modules are `entities.rs`, `service.rs`, `user.rs`, `web.rs`, `ec_web.rs`, `screen.rs`, `message.rs`, `notification.rs`, `search.rs`, `sftp.rs`, `pass.rs`, `upload.rs`, `edi.rs`, `cache.rs`, `transaction.rs`, `l10n.rs`, `logger.rs`, `files.rs`, `theme.rs` and `native_tools.rs` under `crates/moqui-core/src/script/host/`. Other crates add paths with `host::register` before the first script runs.

### The effect fence

- The fence is a thread-local stack in `crates/moqui-entity/src/entity/fence.rs`. Its frames are a read fence, a lifted stretch, a sim preview and a permit for one remote origin.
- `ALLOWED_ENTITIES` lists the entities a thread under the read fence may write: sequence, authorization failure, tarpit, login history, visit, visitor, session, and the agent run and model call records.
- The effects are `AsyncCall`, `RemoteCall`, `Print`, `Mail`, `Sql`, `Resource`, `Notify` and `Process`. Each is checked where the effect happens, with `fence::check_effect`: in `script/host/tools.rs` (process, HTTP), `context/resource.rs` (resource writes), `outbound.rs`, `notify/message.rs`, `service/call.rs`, `sftp.rs`, `service/backup.rs`, `service/backup_store.rs`, `service/web_push.rs`, `service/chat_link.rs`, `service/system_sync/`, and in `moqui-mail`, `moqui-git`, `moqui-devkit` and `moqui-llm`.
- The fence is entered only at these places: a screen render whose `fenced` flag is set and the preview of proposed screen text (`crates/moqui-core/src/screen/render.rs` lines 293 and 764); an agent tool run (`crates/moqui-llm/src/run/step.rs`, `tools/lazy.rs`); the agent's facts, prompts and constitution providers (`facts.rs`, `assist/prompts.rs`, `constitution/resolve.rs`); an approval preview as a sim fence (`approval/steps.rs`, `approval/preview.rs`); a remote MCP tool as a permit (`tools/mcp_client.rs`).
- A script that runs for a web request, a REST call, a job or a service call outside those places is not fenced. The fence belongs to the thread: work handed to another thread is not fenced.

## Who can author code that runs

Every row is a place where text that is not a source file of a component is evaluated on the script runtime as a server script, with everything in [Host members by capability](#host-members-by-capability). Where the text is a record, whoever may write that record authors code.

### From a request

| Path | Text evaluated | Check before it runs | Method |
|---|---|---|---|
| Tools app, `Tools/Data/Export`, transition `EntityExport` (`runtime/components/base/tools/screen/Tools/Data/Export.xml`) | None now: `filterMap` is parsed with `JSON.parse`, never evaluated (plans/Security-Review.md SEC-TLS-01 and SEC-SCR-01, fixed; it was `ec.resource.expression`). | The screen's authorization: the `TOOLS_APP` group grants `ADMIN` all actions on `component://tools/screen/Tools.xml` (`runtime/components/base/tools/data/ToolsSecurityData.xml`). | The transition is not `read-only`: a POST with the session token, and URL parameters are refused ([CSRF](Attack-Surface-Web.md#csrf)). |
| Service `org.moqui.impl.PrintServices.print#ScreenDocument` | The `screenParametersStr` parameter, with `crate::script::eval_map` (`crates/moqui-core/src/service/tools.rs` line 206). It runs when `screenLocation` names a screen. | The Rust body has no permission check before the evaluation. It is a POST resource of the `moqui` REST API at `print/printers/{networkPrinterId}/print/screen` (`runtime/components/base/tools/service/moqui.rest.xml`). The `MOQUI_API` group grants `ADMIN` `AUTHZT_ALWAYS` on the REST path `/moqui`, inherited. | POST |
| ETL mapping field `expression` (`crates/moqui-tools/src/etl/transform.rs` line 415) | The expression, with `row` and `value` as names | Refused unless the user has the sealed permission `GROOVY_SHELL_WEB` (`may_run_scripts` in `crates/moqui-tools/src/etl/access.rs`), to save, check and run. See [Attack-Surface-Tools.md](Attack-Surface-Tools.md). | POST |
| Admin shell command `js <code>` (`crates/moqui-host/src/command/shell.rs` line 237) | The code, as an expression or a function body | The shell's own access. See [Attack-Surface-SSH.md](Attack-Surface-SSH.md). | Shell |

`GROOVY_SHELL_WEB` is one of the `sealed-permissions` of `user-facade` `authz` in `MoquiDefaultConf.xml`: a `UserGroupPermission` for it can be written only by a user who already has it. The first two rows above do not use it.

### From a record

| Record or value | Where it is evaluated | As |
|---|---|---|
| A notification's title, and its topic's `linkTemplate` | `crates/moqui-core/src/notify/message.rs` `expand_template`: the text given to `title()` of `ec.makeNotificationMessage()`, and the topic's templates, are expanded when the message is sent, with the message map's entries as names. A caller's `link()` is a URL taken as written, not expanded (plans/Security-Review.md SEC-SCR-05, fixed) | `expand` |
| A localized message | `ec.resource.expand` localizes the template first and expands the localized text (`oxide/resource.rs` line 227) | `expand` |
| `EmailTemplate` `fromAddress`, `fromName`, `subject` | `crates/moqui-mail/src/implementation.rs` line 558 | `expand` |
| `EmailTemplateAttachment` `attachmentCondition` and `forEachIn` | `crates/moqui-mail/src/implementation.rs` lines 1118 and 1136 | condition, expression |
| A document definition's title template, and a link's `condition`, `linkUrl` and `label` | `crates/moqui-report/src/lib.rs` near line 699, with the search hit's fields as names | `expand`, condition |
| A system message remote's `sendUrl`, and the SFTP name templates of a message type | `crates/moqui-core/src/service/system_sync/message.rs` line 104, `system_sync/sftp.rs` line 456 | `expand` |
| An entity sync detail's `filterMap` | `crates/moqui-core/src/service/system_sync/entity_sync.rs` line 344 | expression |
| An identity provider's claim mapping `expression` | `crates/moqui-core/src/service/sso.rs` line 634, with the provider's claims as names. A plain name with no destination expression is read as a claim and is not evaluated. | expression |
| A rule condition (service ECA, email ECA, entity ECA) | `crates/moqui-core/src/service/rule_actions.rs` `condition_allows`: text that does not start with `<` is an expression | condition |
| An authorization filter's `filterMap` and a filter condition | `crates/moqui-core/src/user/authz.rs` line 626, `crates/moqui-core/src/user/filter.rs` lines 252 and 274 | expression, condition |
| An EDI structure file | `crates/moqui-core/src/edi.rs` `read_structure`: the file at a location is evaluated whole | expression |
| A subscreen's menu title | A `moqui.screen.SubscreensItem` row adds a subscreen under a screen: its `subscreenLocation` names the screen file and its `menuTitle` the title (`crates/moqui-core/src/screen/definition.rs`, the rows read near line 1694). The title, like a `subscreens-item` `menu-title` and a screen's `default-menu-title` in screen XML, is localized and then expanded when it holds `${`: in the menus (`screen/menu.rs` line 168) and in the breadcrumb titles of a render (`screen/render.rs` `level_title`) | `expand` |
| A screen definition | `crates/moqui-core/src/screen/definition.rs` line 910 reads the screen XML through the resource facade, so a screen location may be any scheme the facade serves, `dbresource`, `http` and `https` included. A screen holds actions and expressions. | screen |

A notification title is a template, so text placed in it is code. Callers pass a fixed template and put the values in the message map: `org/moqui/call/Voice.js`, `org/moqui/chat/ChatServices.xml`, `mantle/order/PurchaseApprovals.js`, and since SEC-SCR-05 was fixed the Smithy deployment and push mirror notices (`mantle/smithy/SmithyPipelines.js`, `SmithyImport.js`), which built theirs from the repository path, the environment name and the mirror URL. `mantle/party/PartyServices.xml` (the data document feed's notices) uses `dataDocument.documentTitle`, an administrator's configured template, as Java does.

### From screen XML

The screen templates (`runtime/components/base/framework/template/screen/ScreenHtml.js`, `ScreenQvt.js`, `ScreenTui.js`) evaluate widget attributes with `ec.resource.expression` and `ec.resource.condition` (`text-map`, `condition`, `hide`, `from`, `cid-map`, a currency unit) and expand `${}` in attribute text with `ec.resource.expandNoL10n`. The input is the attribute text of the screen definition, not the value it produces: a value that an expression returns is not expanded again.

### Model output and themes

- The OpenUI parser and the Vue SFC compile check run a library's code over model output in a standalone runtime, which has no `ec` and no host function. See [Attack-Surface-AI.md](Attack-Surface-AI.md).
- A Shop theme package renders in the locked renderer. The only caller of `template::render_locked_bounded` outside the template module is `crates/moqui-core/src/script/theme/compose.rs`.

## Rendering and escaping

- `ScreenHtml.js` (the html render mode) has an escape helper `e`. The outputs read in this pass use it except as listed below.
- The display, label and link text outputs are escaped unless the widget's `encode` attribute is `false`; with `encode="false"` the value is written as it is (`ScreenHtml.js` lines 612 to 737, and the generated form-list code at line 2329).
- An `editable` widget writes its text as it is unless `encode="true"` (`ScreenHtml.js` line 849).
- The HTML view widget cleans its `html` attribute with `StringUtilities.cleanHtml` and escapes its `text` attribute (`ScreenHtml.js` near line 1810).
- The sanitizer is `moqui_lib::safe_html::clean` (`crates/moqui-lib/src/safe_html.rs`): ammonia 4 with its default allow-list, plus `data-mention-id`, `data-record-entity` and `data-record-id` on a `span`. Its callers are the service parameter check, `StringUtilities.cleanHtml`, the theme adapter's `sanitize_rich_text` (`crates/moqui-core/src/script/theme/markup.rs`) and the desktop client's offline compose (`crates/moqui-client/src/offline.rs`).
- HTML is checked on input by the service parameter rule (`crates/moqui-core/src/service/validate.rs` `clean_html`): text with a `<` is an error unless the parameter has `allow-html`. With `allow-html="safe"` the text is cleaned by the sanitizer. With `allow-html="any"` it is kept as it came. A list parameter's string items are checked the same way. A value that does not pass through a service parameter is not checked.
- Markdown is converted with pulldown-cmark 0.13 with its `html` feature.

### PDF through Chromium

- PDF output starts a headless Chromium (`crates/moqui-host/src/pdf.rs`, `crates/moqui-host/src/chromium.rs`). The binary is found at startup; without one PDF is not available.
- The launch arguments are `--headless=new`, `--remote-debugging-address=127.0.0.1`, `--remote-debugging-port=0`, `--no-first-run`, `--no-default-browser-check`, `--disable-gpu`, `--disable-extensions`, `--disable-background-networking`, `--disable-component-update`, `--disable-sync`, `--metrics-recording-only`, `--password-store=basic`, `--use-mock-keychain`, then `--proxy-server=127.0.0.1:9`, `--proxy-bypass-list=<-loopback>`, `--force-webrtc-ip-handling-policy=disable_non_proxied_udp` and a `--user-data-dir` under the runtime's `tmp` directory. `--no-sandbox` is not among them.
- The DevTools port is a random port on 127.0.0.1. The server reads it from `DevToolsActivePort` in the profile directory.
- A page opens at `about:blank`. Every request the page makes that pauses in the `Fetch` domain is failed with `BlockedByClient`. The proxy arguments send every other connection, loopback included, to 127.0.0.1 port 9.
- One document has 60 seconds by default (`screen-facade` `pdf` `timeout`). A PDF preview is at most 25 MiB.

## Parsers of untrusted input

| Input | Code | Limits and refusals |
|---|---|---|
| XML (screens, services, entities, data files, form XML) | `crates/moqui-lib/src/node/parse.rs` | A `<!DOCTYPE` is refused ("DOCTYPE is not allowed"), an `<!ENTITY` is refused, and any other declaration is refused (`declaration_error`). An entity reference that `decode_entity` does not know is an error. |
| Zip in a data load | `crates/moqui-entity/src/entity/load_feed/zip.rs` | Entries named `.xml`, `.csv`, `.json` and `.zip` are loaded as streams; other entries are ignored. Nothing is written to disk under an entry's name. An entry name that starts with `/` or `\`, or has a `..` part, stops the load. A nested zip is read into memory, at most 256 MiB, and nesting is at most 4 deep. The size of an `.xml`, `.csv` or `.json` entry is not limited here. |
| Tar, gzip or not, from a script | `ec.resource.extractArchive` in `crates/moqui-core/src/script/host/archive.rs` | Directories are skipped. An entry that is not a regular file (a symbolic link, a hard link, a device) is skipped. An absolute or parent path is skipped. The defaults are 4 GiB of expanded data and 1,000,000 entries, and the caller's `limits` replace them. Each file is written through the resource facade under the target location. |
| Gzip from a script | `ec.resource.gunzip` | The expanded size is limited to `maxBytes`, 4 GiB when the script gives none. |
| Images | `crates/moqui-core/src/service/images.rs`; `thumbnail_png` in `crates/moqui-core/src/script/host/files.rs` | The `image` crate is 0.25 with the formats bmp, gif, jpeg, png, tiff and webp. `images.rs` reads through `ImageReader` with `Limits`, an input side of at most 30,000 and an output side of at most 16,383. `thumbnail_png` decodes with `image::load_from_memory` and sets no limits of its own; its output side is at most 4096. |
| Spreadsheets | calamine 0.38 without default features; previews in `files.rs` | A sheet preview is at most 1000 rows and 200 columns (100 and 26 by default). A text preview is at most 1,000,000 characters (100,000 by default). |
| Regular expressions in Rust | the `regex` crate, version 1 | The crate matches in time linear in the input. |
| Lines of a file from a script | `ec.resource.readLines` | 1000 lines and 16 MiB by default. |

The zip crate is 9.1 with only the `deflate` feature. The tar crate is 0.4 without default features.

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- What the locked template runtime holds was not read in `crates/moqui-core/src/script/oxide/templates.rs`. The module comment of `template.rs` says it holds only the snapshot of a map, `format`, `localize` and `t`.
- The body of `script_limit_for` in `run.rs` was not read, so which threads run scripts with no time limit is not stated.
- Whether `eval` and the `Function` constructor are available to a server script was not read. Whether the engine can load a native module or a module from a location was not read.
- Which regular expression engine quickjs-oxide uses for a script's `RegExp`, and its backtracking limits, was not read.
- Whether a `file` location may name any absolute path, for a read and for a write, was not read (`file_path` in `crates/moqui-core/src/context/locate.rs`, `location_path` in `context/resource.rs`).
- The comment on `ec.artifactExecution.runAsSystem` says it works only from a service's own code. The check in `run_as_system` was not read.
- `ec.service.rest().send()` goes through the host function `restOpen`, which was not read; whether it honors `denyInternal` was not traced.
- The Rust body of `ec.resource.markdownHtml` was not read. The shim's comment says its output is not sanitized.
- Who may write each record of [From a record](#from-a-record), and through which screens and REST resources, was not traced: `LocalizedMessage`, `EmailTemplate`, `EmailTemplateAttachment`, the document definitions, the system message remotes and types, the entity sync details, the identity provider mappings, the authorization filters, and the rule records.
- Which records can name a screen location (`SubscreensItem`, a scheduled screen, a print or mail attachment's screen), and who may write a `dbresource://` file, was not traced.
- The other callers of `org.moqui.impl.PrintServices.print#ScreenDocument` (`mantle/facility/Printing.js`, `mantle/facility/PrintServices.xml`) were not read.
- The Tools screens Service Run and SQL, the report and dataset expressions of `crates/moqui-report`, alert rules, dashboards, agent skills and prompts, and the application rule engines (mail rules, marketing automations, price rules) were not read.
- Spreadsheet formulas (`crates/moqui-calc`, `crates/moqui-core/src/service/spreadsheet_calc.rs`): whether a formula can reach the network, a file or a service was not read.
- The qvt and tui render modes' escaping, the output of terminal escape sequences from data in the tui mode, formula injection in CSV and XLSX export, JSON embedded in pages, and error pages that echo input were not read. The uses of `encode="false"` and of `editable` in the screens were not listed.
- Whether a `file://` request from a page Chromium prints pauses in the `Fetch` domain was not checked. How the HTML reaches the page (`print_page`) was not read.
- Whether another XML parser is in the build was not settled: no manifest under `crates/` has a dependency line for `quick-xml`, `roxmltree`, `xml-rs` or `xmltree`, and a comment in the workspace `Cargo.toml` says IronCalc brings `quick-xml`. Whether a YAML parser is in the build was not settled: no manifest under `crates/` names one, and the workspace `Cargo.toml` was not read past line 292.
- The tar extraction of the CI runner (`crates/moqui-git/src/ci/runner/archive.rs`), EDI parsing, multipart and JSON body limits, CSV parsing, and number parsing extremes were not read.
