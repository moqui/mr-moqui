/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Command list and per-command help for the `moqui` binary. Plain text, no color.

use moqui_lib::error::Error;

const OVERVIEW: &str = "\
moqui: the Moqui server

Usage: moqui [command] [args...]      moqui help [command]      moqui <command> --help

With no command, start the HTTP server.

Server
  (no command)     [options]                                Start the HTTP server (HTTP port default 8080, HTTPS 8443: moqui help serve)
  serve            [options]                                Start the HTTP server in the background, with no console log
  tui              [--path P] [--user U] [options]          The apps in this terminal, on this runtime
  operator         status | init | unseal | seal | ...      The sealed start: a server's operator socket
  vault            --server URL --user U <command>          A vault client on this machine: your key, vaults and items

Data
  load             [name=value ...]                         Load data files into the configured database
  save-db          <cluster-dir> <dump-file> [--recipient]  pg_dump --format=custom of an embedded cluster
  load-save        <cluster-dir> <dump-file> [--identity]   pg_restore --format=custom into an embedded cluster

Check
  lint             [file | --screens | --groovy <dir> ...]  Lint services, screens, or leftover Groovy
  convert          --groovy [--write] <dir> ...             Mechanical Groovy rewrites
  types            [--out dir]                              TypeScript declarations from the loaded definitions
  check            [paths] [--extensions] [--json]          Check Moqui XML files: schema, references, expressions
  refs             <name> [--in|--out] [--kind k] [--json]  What uses an artifact, or what it uses
  autodoc          [--out dir] [--kinds k,...] [options]    Markdown docs of the artifacts, with diagrams
  lsp              [--server URL]                           A language server for editors and coding agents
  dev-cert         [--shared] [--replace] [--rotate]        Prepare the localhost development certificate
  install          [--from DIR] [--with-data] [--force]     Install this runtime for the current user
  embedded         info | list | diff | extract | checksum  The runtime and programs built into this binary
  conf             env [--names] [text]                     Each conf attribute with the environment name that sets it

Test
  test             [component ...] [options]                Run component JavaScript specs
  test-client      [component | path ...] [options]         Run component client specs against a running server
  test-load        [component | path] [--script Name]       Run a component's load script against a running server
  ci-run           [register | poll] [options]              Run a pipeline job as its runner (in a job's container)

Help
  help             [command]                                This list, or the detail for one command
  version                                                   The version, package and features of this binary

Server options
  -p, --port N       The plain HTTP port to listen on, or off (--http-port N); it sets http-port and
                     http-listen-port. Without it HTTP follows http-listen-port: off in the dev conf, which
                     listens on HTTPS 8443 only; 8080 otherwise. (For moqui test-client and test-load, -p is
                     the HTTPS port of the server they reach.) Also: port=N, -port=N, --port=N.
  --https-port N     HTTPS port to listen on (the webroot webapp's https-listen-port, default 8443), or 'off'. Certificates
                     are looked up by domain in runtime/cert (<domain>.crt and <domain>.key). Also: https-port=N.
  -c, --conf FILE    Runtime conf, relative to the runtime directory or absolute.
                     Also: conf=FILE, -conf=FILE, --conf=FILE. Same as MOQUI_CONF for this run.
  --no-run-es        Do not start OpenSearch (search-facade/cluster@run false). Also: no-run-es, -no-run-es.
  -D name=value      Set a conf property for this run, or a conf attribute by its environment name
                     (moqui_conf__ssh_server__port; moqui help conf). Repeatable. Use underscore names
                     (moqui_conf__ssh_server__port), not dotted ones.

  moqui --port 8081 --conf conf/MoquiProductionConf.xml
  moqui -p 8081 -D moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name=postgres

Environment
  MOQUI_SHARED_RUNTIME Shared directory (absolute path, default ~/.mr-moqui), or off.
  --installed        As the first argument: run the installed runtime (~/.mr-moqui, moqui install) from
                     any directory, reading only its files and this binary's.
  MOQUI_RUNTIME      Runtime directory (default: runtime in the working directory when it has a
                     components/base directory, else the working directory itself when it has one, else
                     a new runtime in the working directory; never found from this executable, so one
                     binary runs several instances). A runtime with no framework component gets a
                     skeleton one from the files built into this binary.
  MOQUI_FRAMEWORK    Framework component (default: <runtime>/components/base/framework)
  MOQUI_CONF         Runtime conf merged over MoquiDefaultConf.xml, relative to the runtime
                     directory or absolute. Unset uses MoquiConf.xml in the runtime directory,
                     else conf/MoquiDevConf.xml, when that file exists. runtime/.env is read after
                     the conf merge and does not select it.
  MOQUI_INSTALL      Where the programs built in are unpacked, in <dir>/postgres, <dir>/opensearch and
                     <dir>/chromium (default: install in the runtime directory)
  MOQUI_EMBEDDED_SHA256_RUNTIME, _POSTGRES, _OPENSEARCH, _CHROMIUM
                     The checksum each part built in must have, checked at startup (moqui help embedded)
  moqui_conf__webapp_list__webapp__WEBROOT__http_port
                     HTTP port (default 8080); __https_listen_port the HTTPS one (8443),
                     moqui_conf__cluster_member__https_port the cluster's (8444); __https_port is the HTTPS port
                     in generated URLs (empty is 443).
  moqui_conf__entity_facade__datasource__TRANSACTIONAL__database_conf_name
                     postgres, mysql, or mysql8 for an external server; otherwise embedded (moqui help conf)

Other conf properties are environment variables with underscore names.
";

const SERVE: &str = "\
moqui serve [options]
    Start the HTTP server in the background.

The options are the server options of a foreground start (moqui help). The command returns when
the server is listening and prints the address and the process id. Log lines go to moqui.log and
moqui-error.log, and to the search log indexes. The console is quiet. The server keeps this
working directory. Stop it with SIGTERM to that process id.

  moqui serve
  moqui serve --port 8081 --conf conf/MoquiProductionConf.xml
";

const TUI: &str = "\
moqui tui [--path PATH] [--user NAME] [--serve] [--ascii] [--no-color] [--keys SCHEME] [--snapshot PATH [--size WxH]]
          [--agent] [--server URL [--forget]]
    The apps in this terminal (the tui render mode), on this runtime and in this process. No port is
    opened unless --serve, and nothing is written to stderr: the log files and the log pane (Alt+L) get
    the log. It asks for a login, as the browser does. F1 lists the keys; docs/ui/TUI.md has the rest.

  --path PATH        The first screen (default /apps).
  --user NAME        Log in as NAME without a password. Only on an instance whose instance@purpose is dev.
  --serve            Also start the HTTP, SSH and cluster listeners, so the browser can work beside the TUI.
  --ascii            Plain ASCII for borders and symbols (also when TERM is linux or vt100).
  --no-color         Monochrome: reverse video and underline only (also when NO_COLOR is set).
  --keys SCHEME      modern or classic, for this run only. Otherwise the scheme chosen in the palette (Keys) or the
                     browser's Appearance, else the theme's, else modern; --agent uses classic.
  --snapshot PATH    Print the screen at PATH as text and exit, at --size (default 180x50).
  --agent            For a program: read command lines on stdin and answer each with the screen as text (look,
                     key, type, go, focus, set, do, find, elements, help, close), at --size (default 120x40).
                     Over SSH the same is `agent-apps`; docs/ui/TUI.md, For agents.
  --server URL       Reach a running server over HTTP instead of this runtime (https, or http to localhost). The
                     login is asked for once and a login key is kept in ~/.config/moqui/tui/<host>.toml.
  --forget           With --server: remove that stored key.

  moqui tui
  moqui tui --user john.doe --path /apps/gallery/List
  moqui tui --snapshot /apps/gallery/Detail --size 100x40
  moqui tui --agent --user john.doe
  moqui tui --server https://erp.example.com
";

const LOAD: &str = "\
moqui load [name=value ...]
    Load data files into the configured database.

Each argument is name=value, and each name is given once. With no types and no location, this loads
seed, seed-initial, and install. types= with an empty value, or types=all, loads every data type.

  moqui load
  moqui load types=seed,seed-initial
  moqui load types=all components=mr-mantle
  moqui load location=runtime/components/addon/mr-mantle/data/20-install/GeographyEurope.xml
  moqui load raw=true timeout=600

Flags:
  types=a,b                       Data types. Omitted: seed, seed-initial, install.
                                  Empty or all: every data type.
  components=a,b                  Component names. Empty means all.
  location=FILE                   Load only this data file.
  timeout=SECONDS                 Transaction timeout for each data file.
                                  Omitted: 600 seconds; each file is one transaction.
  raw=true|false                  Empty-database load: turn off entity ECA, audit log,
                                  foreign-key create, and the data feed.
  disableEntityEca=true|false     Turn off entity ECA rules.
  disableAuditLog=true|false      Turn off the entity audit log.
  disableFkCreate=true|false      Do not create foreign keys.
  disableDataFeed=true|false      Turn off the entity data feed.

Booleans accept true/false, yes/no, y/n, and 1/0.
";

const SAVE_DB: &str = "\
moqui save-db <cluster-dir> <dump-file> [--recipient <age1...>]
    pg_dump --format=custom of an embedded cluster.

Writes a custom-format dump of the embedded moqui database in that cluster directory. Starts the
postmaster when it is not running, and stops a postmaster this command started. A postmaster that
was already running keeps running. This is the embedded cluster only, not an external database
selected by the transactional datasource's database-conf-name. The usual directory is runtime/db/embedded.
./moq save-db also copies OpenSearch data. This command does not.

  moqui save-db runtime/db/embedded runtime/save/before-upgrade/moqui.dump

With --recipient, an age public key (as age-keygen prints), each dump is sealed to it as pg_dump
writes it, and only the sealed bytes reach the disk; name the file .dump.age. Only that key's
identity restores it (load-save --identity).

The job backup_EmbeddedDatabase_daily takes this same kind of dump while Moqui is running and rotates
the archives. Stop Moqui before load-save of that dump onto a cluster Moqui is using. A physical or
incremental archive is restored with the cluster stopped. See plans/PostgresBackup.md.
";

const LOAD_SAVE: &str = "\
moqui load-save <cluster-dir> <dump-file> [--identity <file>|-]
    pg_restore --format=custom into an embedded cluster.

Restores a custom-format dump with pg_restore --clean --if-exists into the embedded moqui database
in that cluster directory. Starts the postmaster when it is not running, and stops a postmaster this
command started. A postmaster that was already running keeps running. This is the embedded cluster
only. The usual directory is runtime/db/embedded.

  moqui load-save runtime/db/embedded runtime/save/before-upgrade/moqui.dump

A sealed dump (.age, from save-db --recipient or database-backup@recipient) needs --identity, the
age identity file of its recipient, or - to read it from stdin. Each sealed dump is decrypted whole
into an owner-only file under runtime/db before anything is restored, and that file is deleted after.

Stop Moqui before restoring a dump from backup_EmbeddedDatabase_daily onto a cluster it is using.
A physical or incremental archive is restored with the cluster stopped. See plans/PostgresBackup.md.
";

const LINT: &str = "\
moqui lint [file | --screens [report-file] | --groovy [--all] <dir> ...]
    Lint services, screens, or leftover Groovy.

  moqui lint
  moqui lint service/org/moqui/impl/BasicServices.xml
  moqui lint --screens
  moqui lint --screens /tmp/screen-lint.txt
  moqui lint --screens --qvt-static
  moqui lint --groovy runtime/components/addon/mr-mantle
  moqui lint --groovy --all runtime/components/addon/mr-mantle

Forms:
  moqui lint [file]
      Lint service expressions. With no file, uses the service files named in MoquiDefaultConf.xml.
      Prints each failure and exits 1 when any fail.

  moqui lint --screens [report-file]
      Lint screen and form XML under the runtime components. The report goes to the file, or to
      stdout when the file is omitted. Expression failures exit 1.

  moqui lint --screens --qvt-static [report-file]
      Compatibility report for server-static qvt. Writes the report to the file or to stdout.
      Does not fail the process.

  moqui lint --groovy <dir> ...
      Report Groovy idioms and run-time hazards left under those directories. At least one directory.
      Idioms that are also valid JavaScript (`new Date`, `.add(` on a decimal, `catch (e)`) are counted
      in the totals and not flagged; --all flags them too.
";

const CONVERT: &str = "\
moqui convert --groovy [--write] <dir> ...
    Mechanical Groovy rewrites.

Rewrites map literals, context + [...], Elvis, and EDI grammar files under the given directories.
Prints the changes. --write writes them to the files. At least one directory is required.

  moqui convert --groovy runtime/components/addon/mr-mantle
  moqui convert --groovy --write runtime/components/addon/mr-mantle
";

const TYPES: &str = "\
moqui types [--out dir]
    TypeScript declarations from the loaded definitions.

Writes declarations for the loaded entity, service, and screen definitions. --out dir and --out=dir
set the directory. The default is types/ inside the runtime directory. This does not open a database.

  moqui types
  moqui types --out /tmp/types
  moqui types --out=runtime/types
";

const CHECK: &str = "\
moqui check [paths] [--extensions] [--json] [--server URL] [--project DIR | --java DIR]
moqui check --agreement
    Check Moqui XML files: schema, references, expressions; or the contributor agreement.

Checks each file (a directory: the XML files in it) against the framework's XSDs, the names it uses
(entities, fields, services, service parameters, locations) against the loaded definitions, and its
JavaScript expressions and scripts. With no paths, checks the XML files git shows as changed under the
current directory. --extensions also checks every extension against what it extends: each screen-extend
file's base screen and the forms, containers, sections and transitions it names there, each
extend-entity's entity, and each ECA rule's service or entity. Each problem prints as path:line:col: severity: message [rule, skill#section]; --json
prints one JSON object per line. Exits 1 when there is an error, so it runs as a git or agent hook;
warnings do not fail it. This does not open a database. --server URL asks a running dev instance
instead (its MCP check tool, with the login key moqui tui --server keeps, or MOQUI_LOGIN_KEY).
--project DIR reads another Moqui project, its layout told from marker paths (no walk of the tree):
Java Moqui when DIR has framework/src/main/java or framework/src/main/groovy (or framework/build.gradle
with .java or .groovy files under framework/src) beside runtime/; else MR Moqui when DIR has
runtime/components/base/framework (a clone) or components/base/framework (a runtime). Neither is an
error naming these. A Java checkout's framework artifacts come from framework/ and its components from
runtime/, read with MR Moqui's framework conf (beside this executable, or MOQUI_FRAMEWORK). --java DIR
reads DIR as a Java Moqui checkout whatever its sources. With neither, a Java checkout is found when
MOQUI_RUNTIME is its runtime or the working directory is in it. With --server, the project is a
directory on the server's machine, read there into memory on first use and kept fresh with its files
(no restart). A definition that does not resolve is logged and left out. A repository's ci.xml is checked
against mr-git-ci.xsd as any other.

--agreement asks the repository's group server whether the next commit's author (git's user.email) has
signed its contributor agreement (AUTHORS.md; /rest/s1/moqui/hosting/agreement): the server is
MOQUI_GROUP_SERVER, else the origin of an https origin remote. The key MOQUI_GROUP_TOKEN is sent only to
MOQUI_GROUP_SERVER when the origin remote is on it (the same origin, or for SSH the same host), never to a
server read from the remote alone, and no redirect is followed; plain http only to a loopback IP literal.
An author who has not signed fails it (exit 1), for a pre-commit hook, unless the commit stages only
AUTHORS.md (the signature itself); a server that cannot be asked or does not decide it is a warning.

  moqui check
  moqui check runtime/components/addon/my-site/service/my/MyServices.xml
  moqui check --extensions --json
  moqui check --agreement
";

const REFS: &str = "\
moqui refs <name> [--in | --out] [--kind k] [--json] [--server URL] [--project DIR | --java DIR]
    What uses an artifact, or what it uses.

name is an artifact id (entity:moqui.example.Example, service:a.B.run#Thing, screen:component://...,
form:component://...#Name), a full or short name (OrderHeader, place#Order), or a file. --in (the
default) lists the references to it in every component's files; --out lists the references made
inside its definition. For a screen or an entity, its transitions, forms and fields count as it.
--kind keeps the references whose other side is of that kind (entity, field, service, screen,
transition, form, anchor, seca, eeca, script, data ...). Each prints as path:line: how artifact;
--json prints one JSON object per line. This does not open a database. --server URL asks a running dev
instance's artifact index instead (its MCP artifact tool).
--project DIR reads another Moqui project, its layout told from marker paths (no walk of the tree):
Java Moqui when DIR has framework/src/main/java or framework/src/main/groovy (or framework/build.gradle
with .java or .groovy files under framework/src) beside runtime/; else MR Moqui when DIR has
runtime/components/base/framework (a clone) or components/base/framework (a runtime). Neither is an
error naming these. A Java checkout's framework artifacts come from framework/ and its components from
runtime/, read with MR Moqui's framework conf (beside this executable, or MOQUI_FRAMEWORK). --java DIR
reads DIR as a Java Moqui checkout whatever its sources. With neither, a Java checkout is found when
MOQUI_RUNTIME is its runtime or the working directory is in it. With --server, the project is a
directory on the server's machine, read there into memory on first use and kept fresh with its files
(no restart). A definition that does not resolve is logged and left out.

  moqui refs moqui.example.Example
  moqui refs org.moqui.impl.UserServices.create#UserAccount --out
  moqui refs component://example/screen/Gallery/ExampleDetail.xml
  moqui refs --project ~/src/moqui 'update#mantle.order.OrderHeader'
  moqui refs --server https://localhost:8443 --project ~/src/moqui --out \
      ~/src/moqui/runtime/components/addon/my-site/screen/MySite/Order.xml
";

const AUTODOC: &str = "\
moqui autodoc [--out DIR] [--kinds k,...] [--components c,...] [--packages p,...] [--include-source]
              [--templates DIR] [--clean] [--project DIR | --java DIR]
    Markdown docs of the loaded artifacts, like JavaDoc, with Mermaid diagrams.

Writes a tree into DIR (default autodoc): README.md, then entity/ by package, service/ and screen/ by
their path under the component's service/ and screen/, eca/ by the rules file's path and rest/ by the
REST API file: a doc per entity, service, screen, ECA rules file and REST API file, a page per entity
package and service and screen directory (an E-R diagram, a class diagram, a subscreen tree), and in
each artifact's doc what uses it and what it uses. --kinds (entity, service, screen, eca, rest),
--components and --packages (prefixes by whole segments: mantle.order, mantle/order,
SimpleScreens/Order) choose what to write; the indexes list it with what the directory had before.
--include-source puts each service's definition in its doc. --templates DIR takes the templates of
DIR (Entity.fancy, Service.fancy ...) in place of the framework's of the same name
(component://framework/template/autodoc). Only the files whose text changed are written, and
DIR/.autodoc.json lists what was; --clean removes the files it lists whose artifacts are gone, never a
file it does not list. A doc that does not build is reported and the rest are written (exit 1). This
does not open a database. --project DIR (or --java DIR) reads another Moqui project, as moqui refs
--project does.

  moqui autodoc
  moqui autodoc --out ../my-site-docs/autodoc --components my-site --clean
  moqui autodoc --kinds entity --packages mantle.order --out /tmp/order-docs
";

const LSP: &str = "\
moqui lsp [--server URL] [--project DIR | --java DIR]
    A language server for editors and coding agents, over stdio.

Speaks the Language Server Protocol on stdin and stdout: diagnostics of each open Moqui XML file as it
changes (the checks of moqui check), go to definition and references (the artifact index, read into
memory and kept current with the open files), hover (an artifact's summary, an element's or
attribute's documentation from the XSDs), completion (elements, attributes, enumerated values, entity
and service names) and document and workspace symbols. --server URL answers from a running dev
instance instead, as moqui check --server does. Logs go to stderr. In the Dev and Debug packages.
--project DIR (or --java DIR) reads another Moqui project, as moqui refs --project does.

  moqui lsp
  moqui lsp --server https://dev.example.com
";

const TEST: &str = "\
moqui test [component ...] [options]
    Run the JavaScript specs in each component's test directory.

With no component, this runs every component under the runtime that has test/*.spec.js, in test or a
subdirectory of it other than client and agent. A spec's name is its file name, unique in its
component. Specs in a component run in the order named by test/suite.js, then the other *.spec.js
files by name. Components
run in the order they load, and a spec runs after the specs its dependsOn names, in any component (a
dependsOn cycle is an error). --spec adds the specs named in dependsOn, and their components. Each
spec runs in its own execution context on one database. Any failed feature exits 1. A component's
test/MoquiConf.xml merges after its MoquiConf.xml, in test runs only. A dependsOn name that the
spec's component lacks is found in the other components. A regression spec over 15 seconds is
named at the end of the run: move it to test/acceptance/ or delete it. A run with no --spec leaves out
the fixture specs a served client suite of their component names (test/client/suite.js server.specs)
unless a spec that runs depends on them: the served run makes them.

  moqui test
  moqui test mr-mantle
  moqui test mr-mantle --spec OrderToCashBasicFlow
  moqui test --db snapshot --junit runtime/log/junit --keep
  moqui test mr-mantle mr-marble --spec SimpleScreensRender --serve 127.0.0.1:18081

Options:
  --spec Name[,Name]              Specs to run. The name is the file name without .spec.js;
                                  component/Name picks one component's. Specs named in dependsOn
                                  are included, from any component, and run first.
  --feature text                  Run features whose names contain this text.
  --acceptance                    Run the acceptance suite, the specs of test/acceptance/ (other
                                  than client and agent), in place of test/. --spec and dependsOn
                                  resolve across both trees; a regression run never reads
                                  test/acceptance/.
  --workers N                     Run the specs in N child processes, each on its own copy of the
                                  --db snapshot database (made once first). A suite.js chain and
                                  what dependsOn names stay in one worker; a spec that reads another
                                  spec's data names it in dependsOn. Not with --serve.
  --db MODE                       none, fresh, snapshot, or existing. The default is fresh,
                                  or none when every selected spec sets database: false.
                                  none boots the factory and skips Postgres, including a build
                                  without embedded-postgres. ec.resource works; a spec that does
                                  not set database: false is refused before boot.
                                  fresh loads every data type into a scratch runtime and removes
                                  it at the end. It needs embedded-postgres.
                                  snapshot saves that load under runtime/save and restores it
                                  while the components the run loads, their entity and data
                                  files and the crate version are unchanged (a relink
                                  keeps it; remove runtime/save/test-* to reload).
                                  existing uses the runtime's embedded database and changes its data.
  --junit dir                     Write TEST-<component>.<Spec>.xml into this directory.
  --keep                          Leave the scratch runtime in place.
  --serve [address]               When the specs pass, serve the run's database on the address
                                  (127.0.0.1:0, a free port, by default; or a port alone) until
                                  Ctrl-C, SIGTERM, or the end of standard input when it is a pipe;
                                  then remove the scratch runtime unless --keep. With --db existing
                                  it serves the runtime's own database, for one whose cluster was
                                  put in place beforehand (a copy of a loaded one); with no specs to
                                  run the server starts at once, with no test factory before it.
                                  Not with --db none. A component named with no specs is there to
                                  load.
  --ready-file path               With --serve: write the server's URL here once it listens.
  --coverage                      After the run, list the services of each component's service/*.xml
                                  files that no spec called, with the total.
  --coverage-report file          Write that report to the file, with the call counts, and print only
                                  the total.
  --log-level level               Console level: trace, debug, info, warn, or error.
                                  The default is warn. The run prints the moqui.log path.

Agent evaluation (docs/Build-Test.md, Agent evaluation):
  moqui test tools --agents --llm-profile local-qwen --runs 3 --seed 7
  moqui test tools --agents --replay runtime/components/base/tools/test/agent/replay

  --agents                        Run the tasks of test/agent/*.agent.js instead of the specs.
                                  --spec picks agent spec files, --feature tasks by name.
  --llm-profile model             The model (an llm-facade or LlmModel id) the agent runs on. The
                                  run is skipped, not failed, when its endpoint does not answer.
  --replay dir                    Play the recorded model replies of <dir>/<spec>/<task>-<n>.jsonl
                                  and run the tools live, with no model.
  --runs N                        Runs of each task (pass@1 and pass^N). The default is 1.
  --seed S                        The seed of the first run; run n sends S + n - 1.
  --min-pass F                    The pass@1 a live run needs, 0.75 by default; a replay needs
                                  every task to pass.
  --write-baseline                Write test/agent/baseline-<profile>.json from this run.

Skill retrieval (plans/LLM-A2A-Assist.md 7.5):
  moqui test tools --skills

  --skills                        Run each component's test/skill-retrieval.json over the skill
                                  files of the framework and every component, with no model and
                                  no database: recall@3 (0.9 needed) and strong false matches
                                  (none allowed).
";

const TEST_CLIENT: &str = "\
moqui test-client [component | path ...] [options]
    Run the client specs in each component's test/client directory against a running server.

The specs have the format of moqui test (spec, feature, where, the conditions, suite.js), but reach
the server only over HTTP, WebSocket and headless Chromium: http, session, login, logout, ws,
browser, load, parallel, server, params, files, skip, waitUntil and sleep instead of ec and call. With no component, this runs every component under
the runtime that has test/client/*.spec.js (in test/client or a subdirectory of it other than
process); a path is a test/client directory or one .spec.js file,
for a machine with the test files and no runtime. This opens no database, binds no port, reads no
conf or runtime/.env, and writes only under --out and --junit, so it runs beside a server in the same
runtime directory. It first checks that the server answers. Any failed feature exits 1.

A spec that opens a browser page gets one Chromium for the spec file, with a temporary profile that is
removed after it; with no Chromium to start, its features are skipped. A page's problems (uncaught
exceptions, console errors, responses of 400 and up, failed loads, render errors and error
notifications) fail its feature unless allowPageProblems is set; a failure leaves each open page's
failed-<feature>-<n>.png and .txt in the --out directory.

--serve starts the server each suite declares in test/client/suite.js, suite([...], { server: {
components, specs, conf } }), as a child moqui test <components> --spec <specs> --db snapshot --serve
with MOQUI_CONF set to the conf, runs that suite against it, then stops it. Components with the same
declaration, or the same conf and properties with named specs, share one server; a server that does not start fails its suite's specs only. The child's
output goes to server-<n>.log in the --out directory.

--proofs runs the security proofs of test/proofs instead: each feature is filed under the requirement
its name starts with (v5.0.0-6.2.6: ...), and the run writes proofs.json and proofs.md into the --out
directory, the rows of the matrix test/proofs/suite.js names (matrix: \"...\") with each requirement's
proofs and their results. A passing feature filed under no requirement fails the run. The profile
says what the specs may assume of the server: fixtures with --serve, whose server made the suite's
fixtures, else deployment, where no fixture user signs in and a spec takes an account from --param.

  moqui test-client framework
  moqui test-client framework --serve
  moqui test-client framework --proofs --serve
  moqui test-client framework --proofs --server https://test.example.com --param user=auditor
  moqui test-client framework --spec SecurityAuthn --param user=john.doe
  moqui test-client runtime/components/addon/my-site/test/client --server https://test.example.com
  moqui test-client --server http://localhost:8081 --wait 120 --junit build/test-results
  moqui test-client mr-marble --serve --spec FindScreens --param screen=Survey/FindSurvey --headed
  moqui test-client mr-marble --serve --spec FindScreens --param phase=before --video --out /tmp/run

Options:
  --server URL                    The server, https://localhost:8443 by default, or MOQUI_TEST_SERVER.
                                  Plain http is only for this machine unless --allow-http.
  --serve                         Start each suite's declared server, as above, instead of --server.
                                  Its SSH listener, when it has one, is where ssh.connect goes.
  --ssh host:port                 Where ssh.connect goes when a spec names no host and port (a port
                                  alone is on the server's host), or MOQUI_TEST_SSH; server.ssh.
  --spec Name[,Name]              Specs to run, with the specs named in their dependsOn.
  --feature text                  Run features whose names contain this text.
  --acceptance                    Run the acceptance client specs of test/acceptance/client in place
                                  of test/client; with --serve, against the server
                                  test/acceptance/client/suite.js declares, its specs resolved
                                  across test/ and test/acceptance/.
  --proofs                        Run the security proofs of test/proofs in place of test/client,
                                  and write the summary by requirement (proofs.json, proofs.md).
  --profile fixtures|deployment   What the specs may assume of the server (the profile global):
                                  fixtures with --serve, else deployment; needsFixtures(what) skips
                                  a feature in the deployment profile.
  --param name=value              Repeatable; params.name in specs. MOQUI_TEST_<NAME> variables
                                  are params too (MOQUI_TEST_LOAD_PASSWORD is params.loadPassword),
                                  for a password that should not show in ps.
  --junit dir                     Write TEST-<component>.client.<Spec>.xml into this directory.
  --out dir                       Where saved responses, screenshots, PDFs, failure files and load
                                  results go. The default is
                                  runtime/tmp/test-client/<yyyyMMdd-HHmmss>-<pid>, or
                                  test-client-out/<run> without a runtime.
  --wait seconds                  Poll until the server answers, for a server started in the background.
  --timeout seconds               What waitUntil, a WebSocket receive and a browser action wait by
                                  default (10).
  --chromium path                 The browser, else screen-facade/pdf@chromium-path, else runtime/chromium (./moq
                                  chromium-download), else an installed Chromium, then Google Chrome
                                  or Microsoft Edge.
  --headed                        A visible browser, for debugging.
  --video                         Record each spec's pages (the CDP screencast) into
                                  <out>/video/<Spec>.webm, with <Spec>.timeline.json: each feature's
                                  start, end, status and reason. Needs ffmpeg on the PATH; without
                                  ffmpeg or Chromium an error says so and only the timelines are
                                  written.
  --offline                       Run only specs with server: false. They open pages at
                                  https://moqui.test with no server and no listening port.
                                  Not with --serve.
  --insecure                      Accept an invalid server certificate (a WARN says so), and any SSH
                                  host key; else a key not pinned is taken only from this machine.
  --allow-http                    Allow plain http to another machine.
  --fail-on-skip                  A skipped spec or feature fails the run.
  --log-level level               Console level: trace, debug, info, warn, or error. The default is
                                  warn; log.info in a spec shows at info.
";

const TEST_LOAD: &str = "\
moqui test-load [component | path] [--script Name] [options]
moqui test-load --compare <result.json> <result.json> [...]
    Run a load script of a component (test/load/<Name>.load.js) against a running server: its
    scenarios' virtual users over HTTP, WebSocket and SSH from this process, as moqui test-client runs
    client specs.

A path is a component directory, its test/load directory, or one .load.js file. With no --script it
lists the component's scripts with their scenarios and params. This opens no database, binds no
port and reads no conf. A script whose plan uses service is refused: its calls run in a server's
process (Tools > Load). While it runs it prints a line every 5 seconds (35s  10 users  241.2/s  0
errors  p95 38ms), then the table by name and the result's path. Ctrl-C stops the users before their
next request, still writes the result, and exits 130; a second Ctrl-C ends at once, with no result.
Otherwise it exits 1 when a threshold of the script is broken or no request succeeded. Percentiles are
a histogram's, within about 1.6%.

  moqui test-load mr-marble
  moqui test-load mr-marble --script OrderToCash --server https://localhost:8443 --param pdf=false --label \"MR\"
  moqui test-load mr-marble --script OrderToCash --users 50 --duration 300 --ramp-up 30 --out /tmp/otc
  moqui test-load runtime/components/addon/mr-marble/test/load/OrderToCash.load.js --server https://test.example.com
  moqui test-load --compare /tmp/otc/java/result.json /tmp/otc/mr/result.json

Options:
  --script Name                   The script test/load/Name.load.js of the component.
  --server URL                    The server, https://localhost:8443 by default, or MOQUI_TEST_SERVER.
                                  Plain http is only for this machine unless --allow-http.
  --ssh host:port                 Where the script's ssh.connect goes when it names no host and port
                                  (a port alone is on the server's host), or MOQUI_TEST_SSH.
  --param name=value              Repeatable; a param of the script's plan. MOQUI_TEST_<NAME>
                                  variables fill the params --param leaves out (MOQUI_TEST_PASSWORD
                                  is password), for a password that should not show in ps.
  --users N, --ramp-up seconds, --duration seconds, --iterations N
                                  Every scenario's settings over the script's defaults. --duration
                                  alone replaces the script's iterations, and --iterations alone its
                                  duration; with both, a user stops at whichever comes first.
  --rate N                        Start N iterations a second (an arrival rate) with --users the most
                                  at once; a start no user is free for within an interval is dropped,
                                  and the result counts it. With --rate, --iterations is the starts
                                  in all, and it replaces the script's stages.
  --scenario name                 Run only this scenario; the settings above are its.
  --label text                    Kept in the result's run block, and shown in Tools > Load.
  --timeout seconds               What a request waits for its answer, unless the script's request
                                  sets its own, and a WebSocket receive (60).
  --out dir                       Write <dir>/result.json. The default is
                                  runtime/tmp/test-load/<yyyyMMdd-HHmmss>-<pid>/result.json (UTC), where
                                  Tools > Load > Runs lists it, or test-load-out/<run> without a runtime.
  --compare a.json b.json ...     Print two or more result files side by side, with each value's change
                                  from the first in percent; the files run up to the next option. No
                                  target or run with it.
  --insecure                      Accept an invalid server certificate (a WARN says so), and any SSH
                                  host key; else a key not pinned is taken only from this machine.
  --allow-http                    Allow plain http to another machine.
  --log-level level               Console level: trace, debug, info, warn, or error (warn).
";

const CI_RUN: &str = "\
moqui ci-run [--workspace DIR] [--shell SHELL]
moqui ci-run register [--name NAME] [--platform TEXT]
moqui ci-run poll [--token-file FILE] [--pass-env NAME,...] [--step-user USER] [--once] [--interval SECONDS]
                  [--work DIR] [--shell SHELL] [--keep-processes]
    The runner of a pipeline job (docs/interfaces/Pipelines.md). With no command it runs the one job
    its environment names, as the server starts it in the job's container: it gets the job, checks
    the commit out shallow with git, restores the caches, runs each step with the shell in the
    checkout, sends the log lines as they are written, uploads the reports, the artifacts and the
    caches, and posts the result. It reads no runtime, conf or database; it needs git and a shell.

    register and poll are a registered runner's, a long-lived machine that asks for jobs: register
    once with the pool's token (the runner token is printed, once), then poll with the runner token,
    kept in a file only the runner's user reads. A registered runner's steps do not get its
    environment: only PATH, HOME, USER, LOGNAME, LANG, LANGUAGE, TZ, TMPDIR, SHELL, TERM, the LC_
    variables and those --pass-env names, then the job's own variables; never a runner token. The
    runner token expires (the server's git-ci runner-token-days, 90); poll rotates it halfway and
    writes the new one to its token file. With --step-user the runner runs as root and each step as
    that user, who owns the checkout and its home but cannot read the runner's token file; the
    reports, artifacts and caches are read by a child of this program as that user (ci-run collect),
    which poll checks the step user can run before it takes a job. One step user per runner
    process: before it reads a job's files the runner kills that user's processes of the job, so
    poll refuses a step user another runner holds.

  MOQUI_CI_SERVER_URL=https://moqui.example.com MOQUI_CI_TOKEN=... MOQUI_CI_JOB_RUN_ID=100123 moqui ci-run
  MOQUI_CI_SERVER_URL=... MOQUI_CI_POOL_TOKEN=... moqui ci-run register --name mac-mini-1 > ~/.moqui-ci-token
  MOQUI_CI_SERVER_URL=... moqui ci-run poll --token-file ~/.moqui-ci-token --pass-env CARGO_HOME

Exit codes: 0 the job succeeded, 1 a step failed, 2 an error (no checkout, the server not reached),
3 the server no longer takes the job's calls (cancelled, or past its time).

Environment
  MOQUI_CI_SERVER_URL             The server, as the runner reaches it.
  MOQUI_CI_TOKEN                  The job token: good for this job run, while it runs. It is not
                                  passed on to the steps.
  MOQUI_CI_JOB_RUN_ID             The job run.
  MOQUI_CI_POOL_TOKEN             register: the pool's registration token.
  MOQUI_CI_RUNNER_TOKEN_FILE      poll: the file that holds the runner's token (or --token-file).
  MOQUI_CI_RUNNER_TOKEN           poll: the runner's token itself; works, with a warning, since a
                                  process's environment can be read by the steps it runs.

Options
  --workspace DIR                 The empty directory the commit is checked out into and the steps
                                  run in (src in the working directory).
  --shell SHELL                   The shell of the steps (sh), run as SHELL -c <command>.
  --name NAME, --platform TEXT    register: what the runner is called, and what it says it is
                                  (the OS and architecture).
  --token-file FILE               poll: the file that holds the runner's token.
  --pass-env NAME,...             poll: the runner's variables the steps get besides the base ones
                                  (repeatable).
  --once                          poll: ask once, run the job if there is one, and exit with its code.
  --interval SECONDS              poll: the wait between asks with no job (10).
  --work DIR                      poll: where each job gets a directory, removed after it; a
                                  directory of the runner's user no one else may write, made when
                                  missing and set to mode 700 (711 with --step-user). Without it a
                                  new moqui-ci-<random> in the temporary directory.
  --step-user USER                poll (unix): run each step as USER, not as the runner, which then
                                  runs as root; the checkout and the steps' home are handed to USER,
                                  and USER's processes a job left are killed after it.
  --keep-processes                poll: leave the processes of the runner's user a job left running;
                                  by default each is killed after the job (never as root), so a runner
                                  runs as a user of its own.
";

const HELP: &str = "\
moqui help [command]
    This list, or the detail for one command.

  moqui help
  moqui help load
  moqui load --help

moqui help, moqui --help, moqui -h, and moqui -? print the command list.
moqui help <command> and moqui <command> --help (or -h) print that command.
";

const EMBEDDED: &str = "\
moqui embedded info | list | diff [<set>/<path>] | extract [<runtime dir>]
moqui embedded checksum [--env] [runtime|postgres|opensearch|chromium ...]
moqui embedded unpack [postgres|opensearch|chromium|all ...]
    The runtime files and the programs built into this binary.

The runtime built in is at one of three levels, chosen when the binary is built (./moq build --embed):
framework (the framework and tools components less their tests, a webroot stub and the runtime confs),
production (also the whole webroot and every base and add-on component less its tests and plans: the file
set of ./moq archive) or all (also the tests, plans and the dev and test sets). A runtime reads each of
these files that is not on disk from the binary; a file on disk is used in its place. With no runtime
moqui makes only its directories; at the framework level the runtime boots, loads the seed data, answers
/rest, /Login, /status and the html /apps (System, Tools, Dashboards), and runs moqui tui, and at the
production level it is the whole instance. At startup moqui logs how many files it reads from the binary,
and each file on disk that differs from the built-in one (changed, added on disk, or a component the
binary does not have): as warnings for the framework, tools and webroot, as information for the other
components. See docs/Run-Deploy.md, Built-in runtime files.

Postgres (embedded-postgres), OpenSearch and Chromium (./moq build --embed-opensearch, --embed-chromium)
may be built in too. Each is unpacked into the install directory (MOQUI_INSTALL, default install/ in the
runtime) the first time it is needed and runs as its own process; OpenSearch keeps its config, data and
logs in runtime/opensearch. An install in runtime/opensearch or runtime/chromium comes first.

  info      The level, the files and size of each set, the components and the programs built in.
  list      Every built-in file, as framework/<path>, tools/<path>, webroot/<path>,
            components/<set>/<component>/<path>, skeleton/<path> or conf/<path>, with its size.
  diff      A unified diff of each changed file (built in as the old side; an added file is one line,
            files not on disk one count), or of the one file named.
  extract   Write each built-in file that is not on disk into the runtime (the one moqui uses, or the
            directory named); files already there are kept.
  checksum  The SHA-256 of each part built in, in sha256sum format, or of the parts named: runtime (the
            packed runtime files), postgres, opensearch and chromium (each the archive file the build
            embedded). --env prints them as MOQUI_EMBEDDED_SHA256_<PART>=<hex>.
  unpack    Unpack the programs built in into the install directory now, as their first use would (for
            a container image); all, or none named, is every one the binary has.

Checksums at startup: the server and moqui load check each part named in the file moqui_embedded_checksums
names (a property, environment variable or .env entry), else <runtime>/embedded.sha256 when it is there,
and in MOQUI_EMBEDDED_SHA256_RUNTIME, _POSTGRES, _OPENSEARCH and _CHROMIUM, which take the place of the
file's line. A part that differs, or one named that the binary does not have, is logged and moqui does not
start. Lines naming other files are skipped, so the file also serves sha256sum -c --ignore-missing.

  moqui embedded info
  moqui embedded diff
  moqui embedded diff framework/conf/MoquiDefaultConf.xml
  moqui embedded diff components/addon/mr-mantle/component.xml
  moqui embedded extract /tmp/runtime
  moqui embedded checksum > runtime/embedded.sha256
  moqui embedded checksum --env runtime
  moqui embedded unpack all
";

const CONF: &str = "\
moqui conf env [--names] [text]
    Each attribute of the conf this runtime merges (MoquiDefaultConf.xml, the components' MoquiConf.xml, the
    runtime conf), as NAME=value  # selector, and for one an environment name set, which one and from where.
    NAME is the environment name that sets the attribute over every conf file: moqui_conf__, then each element
    from the root, a keyed element's key value next (in capitals), and the attribute last, with __ between them
    (plans/archive/Conf-Paths.md). Case and punctuation within a segment do not matter. Precedence: -D, then the
    environment, then runtime/.env. A secret's value is masked.
    --names  The names only, one per line
    text     Only the attributes whose name or selector holds the text

  moqui conf env ssh_server
  moqui conf env --names
  moqui_conf__ssh_server__port=2222 moqui
";

const OPERATOR: &str = "\
moqui operator <command> [--socket PATH]
    The client of a server's operator socket (plans/Sealed-Secrets.md §3.3, docs/Run-Deploy.md, Sealed
    start). A server starts sealed: no listener opens until it is unsealed, and while sealed it answers only
    here. The socket is /run/moqui-operator/moqui.sock (systemd/moqui-operator.socket, or moqui operator bind
    in a container), else for a user other than root runtime/db/operator.sock, or --socket; no conf names it.
    The server's own user runs only status; root and the socket's group run the rest. Passphrases and shares are read with echo off, or a line each from
    standard input when it is not a terminal.
    status                   The state (sealed, unsealing, unsealed), the method and the recovery shares
    init [--method M]        Make the root key, seal.json and the recovery shares (printed once). M is tpm, kms,
                             transit, passphrase, shamir or dev (dev is refused on a production instance); default
                             seal@method
    init --take-shares       Read, once, the recovery shares an automatic first start holds in memory
    unseal [--recovery]      Give the passphrase or one share; --recovery a recovery share, which opens any
                             method's seal; an automatic method unseals with nothing entered
    seal                     Drop every key from memory now and close the listeners
    rekey --method M         Wrap the root key under another method (or new input), with the recovery
                             shares; the root key and the recovery shares stay
    rotate-root              A new root key: seal.json's wraps, bootstrap secrets and data key rewrapped and
                             switched in one rename, with the recovery shares
    export-seal FILE         Write the recovery bundle (seal.json, the database identity), mode 0600: it goes
                             with the offline backups; a restore needs it and the recovery shares
    init --from FILE         Restore a recovery bundle on a clean runtime; then unseal --recovery, and rekey
                             to this host's method
    secret put NAME [--file PATH] [--force]
                             Keep a secret by name, once unsealed: a conf selector the conf leaves empty or
                             at CHANGEME (entity-facade/datasource[transactional]@password,
                             llm-facade/endpoint[openai]@api-key), or tls-key/<name> for runtime/cert/<name>.crt.
                             A bootstrap secret goes to seal.json, any other to the SecretValue table. The value
                             is read with echo off, from standard input, or from PATH, and is never printed.
                             A new entity-facade@crypt-pass needs --force; the old one becomes crypt-pass-old
    secret list              The kept secrets' names, kinds (bootstrap or value) and generations
    secret rm NAME           Remove a kept secret (not a crypt pass or a datasource password)
    bind PATH [--group G] -- COMMAND ...
                             As root: make the operator socket at PATH (root's, mode 0660 with group G or
                             0600) and run COMMAND with it open, for a server that then drops to its own user

  moqui operator status
  moqui operator init --method tpm
  moqui operator unseal
";

const VAULT: &str = "\
moqui vault --server URL --user USERNAME <command>
    A vault client on this machine, against a server's vault services (docs/security/Vaults.md). The vault crypto
    runs here: the key passphrase, your personal key and every vault key stay in this process. Each run signs in
    with your password, and your second factor's code when your account has one; nothing of the sign-in is kept.
    key init               Make your personal vault key: a key passphrase (twice), then its recovery code to keep
    key show               Your personal key's fingerprint
    key check <username>   Compare a user's fingerprint with them by another channel, then store your signed check
    list                   Your vaults
    open <vaultId> [--item ID]
                           Open a vault through its signed member list and list its items, or print one item
    leases [--vault ID]    Leases waiting for your approval and those you asked for, or one vault's leases
    approve <leaseId>      Show what a signed lease names, check the server's lease key once, and approve it here
    What this client accepted of each vault's member list is kept in ~/.config/moqui/vault/state.json, and a server
    that hands back an older one is refused.

  moqui vault --server https://moqui.example.com --user ada key init
  moqui vault --server https://moqui.example.com --user ada open vault-2f9c
  moqui vault --server https://moqui.example.com --user ada approve lease-7d01c2
";

const VERSION: &str = "\
moqui version
    The version, the package this binary was built as (lite, enterprise, maximal, or custom), and every
    feature built in. See docs/Build-Test.md.

  moqui version
  moqui --version
";

/// The command list: groups, one line per command, and the environment variables that select the runtime.
#[must_use]
pub fn overview() -> &'static str {
    OVERVIEW
}

/// The page for one command, or `None` when `name` is not a command.
#[must_use]
pub fn command_help(name: &str) -> Option<&'static str> {
    match name {
        "load" => Some(LOAD),
        "serve" => Some(SERVE),
        "tui" => Some(TUI),
        "save-db" => Some(SAVE_DB),
        "load-save" => Some(LOAD_SAVE),
        "lint" => Some(LINT),
        "convert" => Some(CONVERT),
        "types" => Some(TYPES),
        "check" => Some(CHECK),
        "refs" => Some(REFS),
        "autodoc" => Some(AUTODOC),
        "lsp" => Some(LSP),
        "dev-cert" => Some("moqui dev-cert [--shared] [--replace] [--rotate]\nPrepare the localhost development certificate without starting a server.\n  --shared   sign it with the per-user dev CA in ~/.mr-moqui, made from this checkout's when there is none (./moq trust-dev-cert)\n  --replace  move an older self-signed runtime/cert/localhost or default pair aside (into cert/replaced-<time>) first\n  --rotate   make a new development root and issuer, then new certificates; trust the new root again"),
        "install" => Some("moqui install [--from DIR] [--with-data] [--force]\nInstall the binary and runtime under ~/.mr-moqui. Use moqui --installed to run it."),
        "embedded" => Some(EMBEDDED),
        "conf" => Some(CONF),
        "operator" => Some(OPERATOR),
        "vault" => Some(VAULT),
        "test" => Some(TEST),
        "test-client" => Some(TEST_CLIENT),
        "test-load" => Some(TEST_LOAD),
        "ci-run" => Some(CI_RUN),
        "help" => Some(HELP),
        "version" => Some(VERSION),
        _ => None,
    }
}

/// Help requested by this argument list.
///
/// `help`, `--help`, `-h`, and `-?` are the command list, or one command when the next argument names it.
/// `<command> --help` and `<command> -h` are that command's page. Server options plus `--help` or `-h` are
/// the command list. Anything else is `None`, so the caller runs the command.
///
/// # Errors
///
/// Returns an error when the topic is not a command, or when `help` is given more than one argument.
pub fn help_request(args: &[String]) -> Option<Result<&'static str, Error>> {
    let first = args.first()?.as_str();
    if matches!(first, "help" | "--help" | "-h" | "-?") {
        return Some(topic(args));
    }
    if args.iter().skip(1).any(|arg| arg == "--help" || arg == "-h") {
        if let Some(text) = command_help(first) {
            return Some(Ok(text));
        }
        if is_server_invocation(args) {
            return Some(Ok(OVERVIEW));
        }
        return Some(Err(unknown_command(first)));
    }
    None
}

/// Arguments for a server run: no subcommand, or no arguments at all.
#[must_use]
pub fn is_server_invocation(args: &[String]) -> bool {
    match args.first().map(String::as_str) {
        None => true,
        Some(first) if is_known_command(first) => false,
        Some(first) => looks_like_server_option(first),
    }
}

fn is_known_command(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "--help"
            | "-h"
            | "-?"
            | "version"
            | "--version"
            | "load"
            | "save-db"
            | "load-save"
            | "lint"
            | "convert"
            | "types"
            | "check"
            | "refs"
            | "autodoc"
            | "lsp"
            | "trust-dev-cert"
            | "dev-cert"
            | "install"
            | "embedded"
            | "test"
            | "test-client"
            | "test-load"
            | "ci-run"
            | "serve"
            | "tui"
            // the browser terminal's helper (`crate::terminal_helper`), hidden from the help
            | "term-exec"
            | "term-hold"
    )
}

fn looks_like_server_option(arg: &str) -> bool {
    arg.starts_with('-') || arg == "no-run-es" || arg.contains('=')
}

/// Property assignments from a server argument list, in order. A later name replaces an earlier one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerOptions {
    properties: Vec<(String, String)>,
}

impl ServerOptions {
    fn push(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.properties.push((name.into(), value.into()));
    }

    /// The last assignment of `name`, if there was one.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&str> {
        self.properties
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Assign every property. Later names replace earlier ones.
    pub fn apply(&self) {
        for (name, value) in &self.properties {
            moqui_lib::env::set_property(name, value);
        }
    }
}

/// The webroot webapp's `http-port` by its environment name, which `-p` sets (plans/archive/Conf-Paths.md §1).
pub const HTTP_LISTEN_PORT: &str = "moqui_conf__webapp_list__webapp__WEBROOT__http_listen_port";
pub const HTTP_PORT: &str = "moqui_conf__webapp_list__webapp__WEBROOT__http_port";
/// The webroot webapp's `https-listen-port` by its environment name, which `--https-port` sets.
pub const HTTPS_LISTEN_PORT: &str = "moqui_conf__webapp_list__webapp__WEBROOT__https_listen_port";

/// Parse server options. This does not start the server or set properties.
///
/// # Errors
///
/// Returns an error for an unknown option, a missing value, a port that does not parse, or a `-D` name
/// that is not an underscore property name.
pub fn parse_server_args(args: &[impl AsRef<str>]) -> Result<ServerOptions, Error> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let mut opts = ServerOptions::default();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        if let Some(value) = equals_value(arg, "port").or_else(|| equals_value(arg, "http-port")) {
            set_http_port(&mut opts, value)?;
        } else if matches!(arg, "-p" | "--port" | "--http-port") {
            let value = next_value(&args, index, arg, "a port number")?;
            set_http_port(&mut opts, value)?;
            index += 1;
        } else if let Some(value) = equals_value(arg, "https-port") {
            opts.push(HTTPS_LISTEN_PORT, parse_https_port(value)?);
        } else if arg == "--https-port" {
            let value = next_value(&args, index, arg, "a port number or off")?;
            opts.push(HTTPS_LISTEN_PORT, parse_https_port(value)?);
            index += 1;
        } else if let Some(value) = equals_value(arg, "conf") {
            require_conf(value)?;
            opts.push("MOQUI_CONF", value);
        } else if arg == "-c" || arg == "--conf" {
            let value = next_value(&args, index, arg, "a file")?;
            require_conf(value)?;
            opts.push("MOQUI_CONF", value);
            index += 1;
        } else if matches!(arg, "no-run-es" | "-no-run-es" | "--no-run-es") {
            opts.push("moqui_conf__search_facade__cluster__run", "false");
        } else if arg == "-D" {
            let token = next_value(&args, index, arg, "name=value")?;
            let (name, value) = split_property(token)?;
            opts.push(name, value);
            index += 1;
        } else if let Some(token) = arg.strip_prefix("-D") {
            let (name, value) = split_property(token)?;
            opts.push(name, value);
        } else {
            return Err(Error::new(format!(
                "moqui: unknown option '{arg}'\nRun 'moqui help' for the command list."
            )));
        }
        index += 1;
    }
    Ok(opts)
}

/// `port=N`, `-port=N`, or `--port=N` (and the same for another name). A flag with no `=` is `None`.
fn equals_value<'a>(arg: &'a str, name: &str) -> Option<&'a str> {
    let rest = if let Some(rest) = arg.strip_prefix("--") {
        rest
    } else if let Some(rest) = arg.strip_prefix('-') {
        rest
    } else {
        arg
    };
    rest.strip_prefix(name)?.strip_prefix('=')
}

fn next_value<'a>(args: &'a [&str], index: usize, flag: &str, what: &str) -> Result<&'a str, Error> {
    args.get(index + 1)
        .copied()
        .ok_or_else(|| Error::new(format!("{flag} needs {what}\nRun 'moqui help' for the command list.")))
}

fn require_conf(value: &str) -> Result<(), Error> {
    if value.is_empty() {
        Err(Error::new("conf file is empty"))
    } else {
        Ok(())
    }
}

/// An explicit empty or non-numeric port is an error. An unset `http-port` still means 8080.
fn set_http_port(opts: &mut ServerOptions, raw: &str) -> Result<(), Error> {
    if raw.trim() == "off" {
        opts.push(HTTP_LISTEN_PORT, "off".to_string());
    } else {
        let port = parse_port(raw)?.to_string();
        opts.push(HTTP_PORT, port.clone());
        opts.push(HTTP_LISTEN_PORT, port);
    }
    Ok(())
}

fn parse_port(raw: &str) -> Result<u16, Error> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::new("http-port is not a port"));
    }
    raw.parse::<u16>()
        .map_err(|_| Error::new(format!("http-port {raw} is not a port")))
}

/// A port, or `off` to turn the HTTPS listener off.
fn parse_https_port(raw: &str) -> Result<String, Error> {
    let raw = raw.trim();
    if raw == "off" {
        return Ok(raw.to_string());
    }
    raw.parse::<u16>()
        .map(|port| port.to_string())
        .map_err(|_| Error::new(format!("https-listen-port {raw} is not a port or off")))
}

fn split_property(token: &str) -> Result<(&str, &str), Error> {
    let Some((name, value)) = token.split_once('=') else {
        return Err(Error::new(format!("-D needs name=value, not '{token}'")));
    };
    if name == "create-or-update" || valid_property(name) {
        return Ok((name, value));
    }
    if name.contains('.') {
        return Err(Error::new(format!(
            "-D {name}: use the underscore form of the property name ({})",
            name.replace('.', "_")
        )));
    }
    Err(Error::new(format!("-D {name}: not a valid property name")))
}

fn valid_property(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// An argument list that names no command.
pub fn unknown_command(name: &str) -> Error {
    Error::new(format!(
        "usage: moqui [command] [args...]\nmoqui: unknown command '{name}'\nRun 'moqui help' for the command list."
    ))
}

/// A known command with the wrong arguments. The message is that command's usage line.
pub fn command_usage(command: &str) -> Error {
    let synopsis = match command {
        "load" => "moqui load [name=value ...]",
        "save-db" => "moqui save-db <cluster-dir> <dump-file>",
        "load-save" => "moqui load-save <cluster-dir> <dump-file>",
        "lint" => "moqui lint [file | --screens | --groovy <dir> ...]",
        "convert" => "moqui convert --groovy [--write] <dir> ...",
        "types" => "moqui types [--out dir]",
        "autodoc" => "moqui autodoc [--out DIR] [--kinds k,...] [--components c,...] [--packages p,...] [--include-source] [--templates DIR] [--clean]",
        "serve" => "moqui serve [options]",
        "test" => "moqui test [component ...] [options]",
        "test-client" => "moqui test-client [component | path ...] [options]",
        "test-load" => "moqui test-load [component | path] [--script Name] [options]",
        "ci-run" => "moqui ci-run [register | poll] [options]",
        "tui" => {
            "moqui tui [--path PATH] [--user NAME] [--serve] [--ascii] [--no-color] [--keys SCHEME] [--snapshot PATH [--size WxH]] [--agent] [--server URL [--forget]]"
        }
        "help" => "moqui help [command]",
        "version" => "moqui version",
        "conf" => "moqui conf env [--names] [text]",
        "operator" => {
            "moqui operator status | init [--method M | --take-shares | --from FILE] | unseal [--recovery] | seal \
             | rekey --method M | rotate-root | export-seal FILE | secret put NAME [--file PATH] [--force] | secret list \
             | secret rm NAME [--socket PATH]"
        }
        "vault" => "moqui vault --server URL --user USERNAME key init | key show | key check <username> | list | open <vaultId>",
        "embedded" => {
            "moqui embedded info | list | diff [<set>/<path>] | extract [<runtime dir>] | checksum [--env] [<part>...] \
             | unpack [<program>...]"
        }
        _ => "moqui [command] [args...]",
    };
    let help = if command_help(command).is_some() {
        format!("moqui help {command}")
    } else {
        "moqui help".to_string()
    };
    Error::new(format!("usage: {synopsis}\nRun '{help}' for details."))
}

fn topic(args: &[String]) -> Result<&'static str, Error> {
    if args.len() > 2 {
        return Err(command_usage("help"));
    }
    match args.get(1).map(String::as_str) {
        None => Ok(OVERVIEW),
        Some(name) => command_help(name).ok_or_else(|| {
            Error::new(format!(
                "moqui: unknown command '{name}'\nRun 'moqui help' for the command list."
            ))
        }),
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
