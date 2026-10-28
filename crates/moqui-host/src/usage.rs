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
  (no command)     [options]                                Start the HTTP server (HTTP webapp_http_port, default 8080; HTTPS webapp_https_listen_port, default 8443)
  serve            [options]                                Start the HTTP server in the background, with no console log
  tui              [--path P] [--user U] [options]          The apps in this terminal, on this runtime

Data
  load             [name=value ...]                         Load data files into the configured database
  save-db          <cluster-dir> <dump-file>                pg_dump --format=custom of an embedded cluster
  load-save        <cluster-dir> <dump-file>                pg_restore --format=custom into an embedded cluster

Check
  lint             [file | --screens | --groovy <dir> ...]  Lint services, screens, or leftover Groovy
  convert          --groovy [--write] <dir> ...             Mechanical Groovy rewrites
  types            [--out dir]                              TypeScript declarations from the loaded definitions
  check            [paths] [--extensions] [--json]          Check Moqui XML files: schema, references, expressions
  refs             <name> [--in|--out] [--kind k] [--json]  What uses an artifact, or what it uses

Test
  test             [component ...] [options]                Run component JavaScript specs
  test-client      [component | path ...] [options]         Run component client specs against a running server

Help
  help             [command]                                This list, or the detail for one command
  version                                                   The version, package and features of this binary

Server options
  -p, --port N       HTTP port (webapp_http_port, default 8080). Also: port=N, -port=N, --port=N.
  --https-port N     HTTPS port to listen on (webapp_https_listen_port, default 8443), or 'off'. Certificates
                     are looked up by domain in runtime/cert (<domain>.crt and <domain>.key). Also: https-port=N.
  -c, --conf FILE    Runtime conf, relative to the runtime directory or absolute.
                     Also: conf=FILE, -conf=FILE, --conf=FILE. Same as MOQUI_CONF for this run.
  --no-run-es        Do not start OpenSearch (opensearch_run=false). Also: no-run-es, -no-run-es.
  -D name=value      Set a conf property for this run. Repeatable. Use underscore names
                     (entity_ds_db_conf), not dotted ones.

  moqui --port 8081 --conf conf/MoquiProductionConf.xml
  moqui -p 8081 -D entity_ds_db_conf=postgres

Environment
  MOQUI_RUNTIME      Runtime directory (default: the first with a base-component directory of
                     runtime in the working directory, runtime beside this executable, and the
                     directory of this executable)
  MOQUI_FRAMEWORK    Framework component (default: <runtime>/base-component/framework)
  MOQUI_CONF         Runtime conf merged over MoquiDefaultConf.xml, relative to the runtime
                     directory or absolute. Unset uses MoquiConf.xml in the runtime directory,
                     else conf/MoquiDevConf.xml, when that file exists. runtime/.env is read after
                     the conf merge and does not select it.
  MOQUI_INSTALL      Where the embedded Postgres is unpacked, in <dir>/postgres (default: install
                     in the runtime directory)
  webapp_http_port   HTTP port (default 8080). webapp_https_listen_port defaults to 8443 and cluster_https_port
                     to 8444. webapp_https_port is the HTTPS port in generated URLs (empty is 443).
  entity_ds_db_conf  postgres, mysql, or mysql8 for an external server; otherwise embedded

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
moqui tui [--path PATH] [--user NAME] [--serve] [--ascii] [--no-color] [--snapshot PATH [--size WxH]] [--agent]
          [--server URL [--forget]]
    The apps in this terminal (the tui render mode), on this runtime and in this process. No port is
    opened unless --serve, and nothing is written to stderr: the log files and the log pane (Alt+L) get
    the log. It asks for a login, as the browser does. F1 lists the keys; docs/TUI.md has the rest.

  --path PATH        The first screen (default /apps).
  --user NAME        Log in as NAME without a password. Only on an instance whose instance_purpose is dev.
  --serve            Also start the HTTP, SSH and cluster listeners, so the browser can work beside the TUI.
  --ascii            Plain ASCII for borders and symbols (also when TERM is linux or vt100).
  --no-color         Monochrome: reverse video and underline only (also when NO_COLOR is set).
  --snapshot PATH    Print the screen at PATH as text and exit, at --size (default 180x50).
  --agent            For a program: read command lines on stdin and answer each with the screen as text (look,
                     key, type, go, focus, set, do, find, elements, help, close), at --size (default 120x40).
                     Over SSH the same is `agent-apps`; docs/TUI.md, For agents.
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
  moqui load location=runtime/component/mr-mantle/data/SomeData.xml
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
moqui save-db <cluster-dir> <dump-file>
    pg_dump --format=custom of an embedded cluster.

Writes a custom-format dump of the embedded moqui database in that cluster directory. Starts the
postmaster when it is not running, and stops a postmaster this command started. A postmaster that
was already running keeps running. This is the embedded cluster only, not an external database
selected by entity_ds_db_conf. The usual directory is runtime/db/embedded.
./moq save-db also copies OpenSearch data. This command does not.

  moqui save-db runtime/db/embedded runtime/save/before-upgrade/moqui.dump

The job backup_EmbeddedDatabase_daily takes this same kind of dump while Moqui is running and rotates
the archives. Stop Moqui before load-save of that dump onto a cluster Moqui is using. A physical or
incremental archive is restored with the cluster stopped. See plans/PostgresBackup.md.
";

const LOAD_SAVE: &str = "\
moqui load-save <cluster-dir> <dump-file>
    pg_restore --format=custom into an embedded cluster.

Restores a custom-format dump with pg_restore --clean --if-exists into the embedded moqui database
in that cluster directory. Starts the postmaster when it is not running, and stops a postmaster this
command started. A postmaster that was already running keeps running. This is the embedded cluster
only. The usual directory is runtime/db/embedded.

  moqui load-save runtime/db/embedded runtime/save/before-upgrade/moqui.dump

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
  moqui lint --groovy runtime/component/mr-mantle
  moqui lint --groovy --all runtime/component/mr-mantle

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

  moqui convert --groovy runtime/component/mr-mantle
  moqui convert --groovy --write runtime/component/mr-mantle
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
moqui check [paths] [--extensions] [--json]
    Check Moqui XML files: schema, references, expressions.

Checks each file (a directory: the XML files in it) against the framework's XSDs, the names it uses
(entities, fields, services, service parameters, locations) against the loaded definitions, and its
JavaScript expressions and scripts. With no paths, checks the XML files git shows as changed under the
current directory. --extensions also checks every extension against what it extends: each screen-extend
file's base screen and the forms, containers, sections and transitions it names there, each
extend-entity's entity, and each ECA rule's service or entity. Each problem prints as path:line:col: severity: message [rule, skill#section]; --json
prints one JSON object per line. Exits 1 when there is an error, so it runs as a git or agent hook;
warnings do not fail it. This does not open a database.

  moqui check
  moqui check runtime/component/my-site/service/my/MyServices.xml
  moqui check --extensions --json
";

const REFS: &str = "\
moqui refs <name> [--in | --out] [--kind k] [--json]
    What uses an artifact, or what it uses.

name is an artifact id (entity:moqui.example.Example, service:a.B.run#Thing, screen:component://...,
form:component://...#Name), a full or short name (OrderHeader, place#Order), or a file. --in (the
default) lists the references to it in every component's files; --out lists the references made
inside its definition. For a screen or an entity, its transitions, forms and fields count as it.
--kind keeps the references whose other side is of that kind (entity, field, service, screen,
transition, form, anchor, seca, eeca, script, data ...). Each prints as path:line: how artifact;
--json prints one JSON object per line. This does not open a database.

  moqui refs moqui.example.Example
  moqui refs org.moqui.impl.UserServices.create#UserAccount --out
  moqui refs component://example/screen/Gallery/ExampleDetail.xml
";

const TEST: &str = "\
moqui test [component ...] [options]
    Run the JavaScript specs in each component's test directory.

With no component, this runs every component under the runtime that has test/*.spec.js. Specs in a
component run in the order named by test/suite.js, then the other *.spec.js files by name. Components
run in the order they load. --spec adds the specs named in dependsOn, in that same order. Each spec
runs in its own execution context on one database. Any failed feature exits 1. A component's
test/MoquiConf.xml merges after its MoquiConf.xml, in test runs only. A dependsOn name that the
spec's component lacks is found in the run's other components.

  moqui test
  moqui test mr-mantle
  moqui test mr-mantle --spec OrderToCashBasicFlow
  moqui test --db snapshot --junit runtime/log/junit --keep
  moqui test mr-mantle mr-marble --spec SimpleScreensRender --serve 127.0.0.1:18081

Options:
  --spec Name[,Name]              Specs to run. The name is the file name without .spec.js.
                                  Specs named in dependsOn are included, in suite order.
  --feature text                  Run features whose names contain this text.
  --db MODE                       none, fresh, snapshot, or existing. The default is fresh,
                                  or none when every selected spec sets database: false.
                                  none boots the factory and skips Postgres, including a build
                                  without embedded-postgres. ec.resource works; a spec that does
                                  not set database: false is refused before boot.
                                  fresh loads every data type into a scratch runtime and removes
                                  it at the end. It needs embedded-postgres.
                                  snapshot saves that load under runtime/save and restores it
                                  while the entity and data files and this binary are unchanged.
                                  existing uses the runtime's embedded database and changes its data.
  --junit dir                     Write TEST-<component>.<Spec>.xml into this directory.
  --keep                          Leave the scratch runtime in place.
  --serve [address]               When the specs pass, serve the scratch database on the address
                                  (127.0.0.1:0, a free port, by default; or a port alone) until
                                  Ctrl-C, SIGTERM, or the end of standard input when it is a pipe;
                                  then remove the scratch runtime unless --keep. Not with --db
                                  existing or --db none. A component named with no specs is there to load.
  --ready-file path               With --serve: write the server's URL here once it listens.
  --coverage                      After the run, list the services of each component's service/*.xml
                                  files that no spec called, with the total.
  --coverage-report file          Write that report to the file, with the call counts, and print only
                                  the total.
  --log-level level               Console level: trace, debug, info, warn, or error.
                                  The default is warn. The run prints the moqui.log path.

Agent evaluation (docs/Testing.md, Agent evaluation):
  moqui test tools --agents --llm-profile local-qwen --runs 3 --seed 7
  moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay

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
the runtime that has test/client/*.spec.js; a path is a test/client directory or one .spec.js file,
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
declaration share one server; a server that does not start fails its suite's specs only. The child's
output goes to server-<n>.log in the --out directory.

  moqui test-client framework
  moqui test-client framework --serve
  moqui test-client framework --spec SecurityAuthn --param user=john.doe
  moqui test-client runtime/component/my-site/test/client --server https://test.example.com
  moqui test-client --server http://localhost:8081 --wait 120 --junit build/test-results
  moqui test-client mr-marble --serve --spec FindScreens --param screen=Survey/FindSurvey --headed

Options:
  --server URL                    The server, http://localhost:8080 by default, or MOQUI_TEST_SERVER.
                                  Plain http is only for this machine unless --allow-http.
  --serve                         Start each suite's declared server, as above, instead of --server.
  --spec Name[,Name]              Specs to run, with the specs named in their dependsOn.
  --feature text                  Run features whose names contain this text.
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
  --chromium path                 The browser, else MOQUI_CHROMIUM, else runtime/chromium (./moq
                                  chromium-download), chromium or chromium-browser on the PATH, then
                                  google-chrome.
  --headed                        A visible browser, for debugging.
  --offline                       Run only specs with server: false. They open pages at
                                  https://moqui.test with no server and no listening port.
                                  Not with --serve.
  --insecure                      Accept an invalid server certificate (a WARN says so).
  --allow-http                    Allow plain http to another machine.
  --fail-on-skip                  A skipped spec or feature fails the run.
  --log-level level               Console level: trace, debug, info, warn, or error. The default is
                                  warn; log.info in a spec shows at info.
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

const VERSION: &str = "\
moqui version
    The version, the package this binary was built as (lite, enterprise, maximal, or custom), and every
    feature built in. See docs/Packaging.md.

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
        "test" => Some(TEST),
        "test-client" => Some(TEST_CLIENT),
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
            | "test"
            | "test-client"
            | "serve"
            | "tui"
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
        if let Some(value) = equals_value(arg, "port") {
            opts.push("webapp_http_port", parse_port(value)?.to_string());
        } else if arg == "-p" || arg == "--port" {
            let value = next_value(&args, index, arg, "a port number")?;
            opts.push("webapp_http_port", parse_port(value)?.to_string());
            index += 1;
        } else if let Some(value) = equals_value(arg, "https-port") {
            opts.push("webapp_https_listen_port", parse_https_port(value)?);
        } else if arg == "--https-port" {
            let value = next_value(&args, index, arg, "a port number or off")?;
            opts.push("webapp_https_listen_port", parse_https_port(value)?);
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
            opts.push("opensearch_run", "false");
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

/// An explicit empty or non-numeric port is an error. An unset `webapp_http_port` still means 8080.
fn parse_port(raw: &str) -> Result<u16, Error> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::new("webapp_http_port is not a port"));
    }
    raw.parse::<u16>()
        .map_err(|_| Error::new(format!("webapp_http_port {raw} is not a port")))
}

/// A port, or `off` to turn the HTTPS listener off.
fn parse_https_port(raw: &str) -> Result<String, Error> {
    let raw = raw.trim();
    if raw == "off" {
        return Ok(raw.to_string());
    }
    raw.parse::<u16>()
        .map(|port| port.to_string())
        .map_err(|_| Error::new(format!("webapp_https_listen_port {raw} is not a port or off")))
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
        "serve" => "moqui serve [options]",
        "test" => "moqui test [component ...] [options]",
        "test-client" => "moqui test-client [component | path ...] [options]",
        "tui" => {
            "moqui tui [--path PATH] [--user NAME] [--serve] [--ascii] [--no-color] [--snapshot PATH [--size WxH]] [--agent] [--server URL [--forget]]"
        }
        "help" => "moqui help [command]",
        "version" => "moqui version",
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
