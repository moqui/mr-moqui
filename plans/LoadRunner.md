# LoadRunner

Status 2026-11-10: L1 to L6 are built (2026-11-10); L7, other kinds of load, is deferred. The
guide is [docs/tools/Load-Runner.md](../docs/tools/Load-Runner.md), which says what was built where this plan
differs. Corrections the build made are marked "Built:" below. The plan turns the Tools app's Service Load Runner screen into
a general load runner: load scripts in JavaScript on the `moqui test-client` harness, run from Tools › Load, from a
command, or from a client spec, against this server's services in process or any server over HTTP, with room for SSH
and other loads later. The first script is an order to cash walk that runs against Moqui Java (MarbleERP) and MR Moqui
(mr-marble). The decisions are in [§3](#3-decisions); the script location, the command's name, runs kept without
database records, the permission's name and authorization on in-process calls were settled with the user on
2026-11-10.

Contents: [1 what there is](#1-what-there-is-now) · [2 what to build](#2-what-to-build) · [3 decisions](#3-decisions) ·
[4 load scripts](#4-load-scripts) · [5 the engine](#5-the-engine) · [6 kinds of load](#6-kinds-of-load) ·
[7 runs in a server](#7-runs-in-a-server) · [8 Tools › Load](#8-tools--load) · [9 command and specs](#9-the-command-and-specs) ·
[10 security](#10-security) · [11 first script](#11-the-first-script-order-to-cash) · [12 code layout](#12-code-layout) ·
[13 phases](#13-phases) · [14 acceptance tests](#14-acceptance-tests-to-write) · [15 docs](#15-docs-to-change) ·
[16 risks](#16-risks-and-open-questions) · [17 later](#17-later)

## 1. What there is now

**The Service Load Runner screen** (`runtime/base-component/tools/screen/Tools/Service/ServiceLoadRunner.xml`) is the
Java screen with a `tui` twin. It calls `ec.service.getLoadRunner()`, which MR Moqui does not have
([Screen-TUI-Impl](archive/Screen-TUI-Impl.md): "the port does not have"), so the screen shows a form and no run. In
Java (`ServiceFacadeImpl.LoadRunner`, `framework/src/main/groovy/org/moqui/impl/service/ServiceFacadeImpl.groovy:613`)
it is one in-memory runner per server:

- a list of services, each with a parameters expression, target threads, a run delay, a ramp delay, and time bins;
- each call runs on a scheduled executor thread, logged in as anonymous with authorization off;
- per service: count, errors, average, standard deviation, min, max, runs per second, the last result, and the
  artifact type statistics of the calls (entity find, create, update, delete counts and times);
- the screen polls two JSON transitions every 2 seconds and draws four charts with moving averages.

It can only call services in the same server, has no scenario of several steps, keeps no history, and needs the
`SERVICE_LOAD_RUNNER` permission (sealed, on `ADMIN_ADV`).

**`load(opts)` in `moqui test-client`** ([JS-Test-Client, Load and parallel](../docs/dev/JS-Test-Client.md#load-and-parallel);
`crates/moqui-host/src/test_client/load.rs`, `prelude.js`, `mod.rs` `run_load`) already runs virtual users from
outside a server. Each user is a thread with its own script engine and HTTP session (cookie jar and session token).
It has `users`, `duration`, `iterations`, `rampUp`, `think`, and `setup`, `scenario` and `teardown` functions. Each
request is timed in Rust and grouped under a name; the result has counts, errors, statuses and percentiles per name,
a console table, and a JSON file. The framework's `LoadBasic` acceptance spec uses it, on `conf/MoquiLoadTestConf.xml`
(the tarpits off). What it lacks for this plan:

| Lacking | Today |
| --- | --- |
| A script file of its own | The functions are passed as source text from a spec, so they cannot use the file's helpers or constants |
| More than one group of users | One scenario per `load` |
| Live numbers, and a stop | The call blocks until the end; samples merge when a user's thread ends |
| Numbers over time | Totals only, no time series |
| Bounded memory | Every sample is kept (8 bytes each), so a long, fast run grows without limit |
| Pauses between requests | Only `think` between iterations |
| Reading an id out of a redirect | `location` and `url` as text; the script parses the query itself |
| Loads other than HTTP | `http`, `http.raw`; `ws` is not timed |
| An arrival rate | A closed model: a slow server lowers the load on itself |

**The JMeter script** (`PopCommerce/jmeter/PopcOrderToCash.jmx` in a Java checkout) is the model of a load to run: 10
threads, 5 loops each, a Gaussian pause of 200 ± 50 ms before every request, cookies cleared each loop, and 30
requests a loop: log in, create a sales order, add three product items and a shipping charge, place, approve, print
(two PDFs), ship, pick, pack, finalize the invoice, record the payment, log out. It reads `orderId`, `shipmentId`,
`invoiceId` and the session token out of the HTML with CSS selectors and regular expressions, and fails the loop when
one is missing. It was last changed in 2017 and no longer runs as written: the links it reads as `Shipment #<id>` and
`Invoice #<id>` are now "Shipment <id>" and "Invoice <id>" (§11).

## 2. What to build

1. **One engine.** The client harness's `load` grows the missing parts. The Tools screen, a command and client specs
   all run it; there is no second runner.
2. **Load scripts.** A JavaScript file in a component (`test/load/<Name>.load.js`) that declares its parameters and one or
   more scenarios, written with the client harness's globals (`http`, `session`, `ws`, `files`, `sleep`, `log`) plus
   what this plan adds (`service`, `step`, `pause`, later `ssh`).
3. **Kinds of load.** HTTP to any server (this one, another MR Moqui, a Moqui Java server); this server's services in
   process, as the old screen did; and a sample model that is not HTTP-shaped, so SSH commands, `agent-apps` sessions,
   interactive shells and WebSocket messages fit later without a change to the statistics.
4. **Tools › Load.** A section of the Tools app, apart from Services, with its own dashboard box: choose a script, set
   its parameters and users, start, watch it live, stop it, keep the runs, and compare runs (Java against MR, before
   against after).
5. **The first script.** Order to cash through the Marble ERP screens, one file for both servers. The walk turns out
   to be the same on both (§11), so its parameters are the target and a few options, not a server flavor.

Not in scope: reading or writing JMeter's `.jmx`; a recorder or a visual script builder; a distributed runner that
spreads the generator across machines or a cluster's nodes (§17); replacing the opt-in speed tests ([Profiling2](Profiling2.md)), which measure single
paths without a client.

## 3. Decisions

D2, D5, D8, D9 and D10 are the user's decisions of 2026-11-10; the others are this plan's.

| # | Decision | What and why |
| --- | --- | --- |
| D1 | Where the engine lives | In `moqui-host`, growing `test_client/load.rs` into a directory module. It needs reqwest, the sessions and the thread runner that are there, and the host is the only crate above both devkit's spec runner and core |
| D2 | What a load script is | Its own file kind, `<component>/test/load/<Name>.load.js`, beside `test/client/`, with `plan({...})` and `scenario(name, {...})` at the top level, **evaluated whole in each user's engine**. That removes the source-text limit of `load()`: helpers and constants at the top of the file work in every scenario. `moqui test` reads only the top level of `test/` and `test-client` only `test/client/`, so neither picks the scripts up. A client spec stays a test; it can run a load script with `load({ script })` and assert on the result |
| D3 | Where the generator runs | From Tools: threads in the server's own process. That is the only way to call services in process, and it needs no child process or second binary. From the command: in the `moqui` client process, on this machine or another, which is what a measurement of a server under real load should use. A result says which it was |
| D4 | Statistics | A log-linear histogram per name (about 2,000 counters, 1.6% relative error, 1 µs to an hour) and one-second bins, so memory is bounded and there is a time series. Percentiles come from the histogram. The exact nearest-rank percentiles of today's `load()` become approximate; say so in the guide |
| D5 | Where runs are kept | No entity and no database rows. A running load is in the memory of the process that runs it, which answers the page's poll and takes its stop. A finished run is its result JSON file under `runtime/tmp/test-load/`, and the list of runs is those files. This assumes one server, or a load balancer with sticky sessions: another node does not see a running load or this node's files. A runner spread across a cluster, with runs every node sees, is deferred (§17) |
| D6 | One run at a time | One running load per instance by default (`load_runner_max_runs`, 1), and at most 1,000 users a run (`load_runner_max_users`). Two loads at once spoil each other's numbers |
| D7 | Where it is in Tools | `Tools/Load.xml` and `Tools/Load/`, a "Load testing" box on the dashboard. `Tools/Service/ServiceLoadRunner.xml` is deleted (one route per screen); what it did is the shipped script `ServiceCall.load.js` |
| D8 | The command's name | `moqui test-load` (`./moq test-load`), after `moqui test-client`: a component's `test/load/` scripts as `test-client` runs its `test/client/` specs |
| D9 | The permission | Rename `SERVICE_LOAD_RUNNER` to `LOAD_RUNNER` (still sealed, on `ADMIN_ADV`), since it is no longer about services. Pre-release, so no migration |
| D10 | In-process service calls | Run as the user who started the run, with authorization required and the tarpit off for the run's contexts. Java ran them as anonymous with authorization off. An override may prove needed (a service worth loading that the user is not granted); it is added when a real case shows it, not before (§16). Recorded in [Parity-Diffs](Parity-Diffs.md) when built |
| D11 | Reading HTML | No CSS selectors in L1. The first script needs none: an id comes from the redirect's URL (`response.urlParam`) or from a link's `href` by a regular expression (§11). If scripts come to need them, a small matcher in `moqui-lib` over html5ever, which is already linked, and no new crate (§17) |
| D12 | Scripts come from files | The run form names a script in a component; no script text is typed into the screen. A typed script would be code execution in the server, which is `GROOVY_SHELL_WEB`'s class of tool |

## 4. Load scripts

A load script is `<component>/test/load/<Name>.load.js`. Its top level declares one plan and its scenarios. The runner
evaluates the file once to read the plan (for the form, the checks, and the command's script list), and once in each
virtual user's engine, where the scenario's functions then run. The top level makes no request: a request there
throws "a load script makes no request at its top level".

```js
// runtime/base-component/tools/test/load/ServiceCall.load.js: what the Service Load Runner screen did
plan({
    title: "Service call",
    description: "One service called again and again in this server",
    uses: ["service"],
    params: {
        serviceName: { default: "org.moqui.impl.BasicServices.echo#Data", description: "The service to call" },
        parameters: { default: '{"textIn1": "load"}', description: "Its parameters, as JSON" },
    },
})

const parameters = JSON.parse(params.parameters)     // once per user: the file is evaluated in each user's engine

scenario("calls", {
    users: 10, rampUp: 5, duration: 60, think: 100,
    run(user, iteration) {
        service.call(params.serviceName, { ...parameters, index: user * 1000000 + iteration })
    },
})
```

**`plan(options)`**

| Option | |
| --- | --- |
| `title`, `description` | Shown in the script list and on a run |
| `uses` | The kinds of load the script makes: `"http"` (the default), `"service"`, later `"ssh"`. The runner refuses a run that cannot supply one: `service` needs a server process, `http` needs a target URL |
| `params` | `{ name: { default, description, options, secret, required } }`. The type is the default's (text, number, boolean). `options` is a closed list (a drop-down). A `secret` is a password field, and is not written to the run record, the result or a log. The resolved values are the global `params` |
| `thresholds` | `{ errorRate, p95, p99, names: { "<name>": { p95 } } }`: limits a run is checked against at its end. The result's `passed` is false when one is broken; the command then exits 1 |

**`scenario(name, options)`** is a group of users (a JMeter thread group). A run starts every scenario of the script
together unless it names some.

| Option | |
| --- | --- |
| `users`, `rampUp`, `duration`, `iterations`, `think` | As in `load()`. These are defaults: the run form and the command override them per scenario |
| `pause` | A wait before each request, not timed: milliseconds, `[min, max]`, or `{ gaussian: [mean, deviation] }` (JMeter's Gaussian Random Timer). `pause(...)` in the script waits once |
| `setup(user)`, `run(user, iteration)`, `teardown(user)` | `setup` once per user before the timed part, `run` repeated, `teardown` after. `user` is the user's number from 0 across the scenario |
| `newSession` | `true` starts each iteration with a fresh `http` session (JMeter's "clear cookies each iteration") |
| `rate`, `stages` | An arrival rate (iterations started per second, `users` the most at once), and the target over time: `[{ duration, users }]` or `[{ duration, rate }]`, linear within each stage (§5, L6) |

**Added to the harness's globals** (in a load script and in any client spec):

| Global | |
| --- | --- |
| `step(name, fn)` | Runs `fn` and records its whole time as a sample `name` (kind `step`), with the requests inside still counted under their own names. A JMeter transaction controller |
| `pause(ms \| [min, max] \| { gaussian: [mean, deviation] })` | A wait that is not timed |
| `response.urlParam(name)` | A query parameter of the response's `url` (the last one, after redirects), or of its `location` when the redirect was not followed; null when it is not there. A transition's redirect carries the new record's id this way |
| `service.call(name, parameters, { name, expect })` | A service of this server, in process (§6). Outside a server it throws, naming `http.post` on the service's REST path |
| `fail(message)` | Ends the iteration as an error with the message; `assert` does the same |

What the functions get: `params`, `server` (the target), `http` (the user's own session), and everything declared at
the top of the file. Values do not cross between users; a user keeps state in its own variables.

A script's files: `files.read` and `files.list` stay inside the script's component, so a script reads its data
(`test/load/data/customers.csv`) from there.

## 5. The engine

All in `crates/moqui-host/src/test_client/load/` (the present `load.rs` split along its seams) and the preludes.

**Samples.** A sample is a name, a kind (`http`, `raw`, `service`, `step`, later `ws`, `ssh`), an outcome label (an
HTTP status, `ok`, `error`, later an exit status), the elapsed time, and for an error its message. Today's `Observed`
carries an HTTP status as a number; the label makes the statistics the same for every kind. An HTTP response of 400
and up is an error unless the request has `expect`, as now.

**Recording.** A user's thread keeps the samples of the current second in its own buffer, with no lock, and merges
them into the run's shared state at most once a second and when it ends. The shared state has, per name, a histogram,
counts by outcome and an error count; and the first 20 error samples.

**Histogram.** `moqui_lib::histogram`: counters by power of two with 64 steps in each, values in microseconds. It
gives min, mean, max and any percentile, merges by adding, and is about 14 KB. Percentiles are within 1.6% of the
value.

**Bins.** One per second from the run's start: active users, requests, errors, mean, p50, p95, p99 and max, computed
from that second's samples when the bin closes, after which the samples are dropped. A run keeps at most 7,200 bins;
past that, neighbours merge and the bin length doubles. Per name the bins keep count, errors and mean at 10 seconds.
The live snapshot (§7) carries the bins thinned to at most 300 points.

**Live result and stop.** `Load` gets `snapshot()` (totals, the per-name table, the bins, active users) that reads the
shared state at any time, and two stops: after each user's current iteration, and before each user's next request
(what the timeout does today). The blocking `load()` of a spec keeps its behaviour and gains the bins in its result.

**Users.** Scenarios start together, each with its own ramp. A user's thread evaluates the preludes, then the script
file, then runs its scenario by name (`run_script_on_thread` in devkit's testkit, beside `run_on_thread`). Each
scenario's users are numbered from 0.

**Pacing.** `think` between iterations as now. `pause` before each request, in Rust, from a per-thread random
generator; a Gaussian wait is clamped at zero. Built: the generator is a SplitMix64 per thread seeded from
`getrandom` (`pace.rs`), not the `rand` crate.

**The result** keeps today's fields and adds:

| Field | |
| --- | --- |
| `scenarios` | Per scenario: users, iterations, errors |
| `bins`, `binSeconds` | The time series |
| `names.<name>.kind`, `.outcomes` | The kind, and counts by outcome label (`statuses` stays for HTTP) |
| `thresholds`, `passed` | Each limit with its value, and the verdict |
| `generator` | Where the generator ran (`in-target`, `same-machine`; built: no `remote`, `same-machine` is every command run whatever machine the server is on, `in-target` a Tools run against this server and `in-server` a Tools run against another server), its cores, its process CPU share over the run, and the share of the users' time spent in script rather than waiting on a request or a pause |
| `run` | The script, the scenario settings used, the parameters without secrets, the target URL, a label, start and end |

The generator block is there so that a run limited by the generator can be seen: a high CPU share or script share
means the numbers are the generator's, not the server's.

**An arrival rate** (L6). `rate` on a scenario (iterations started per second) with `users` as the most that may run
at once. Each iteration has a scheduled start; a sample's time is measured from its request's send as now, and the
iteration's `step` time from its scheduled start, so a server that stalls does not hide the wait (coordinated
omission). Iterations that could not start for lack of a free user are counted as `dropped`: a start is dropped when
no user has taken it by the time of the start after it (one interval late, and at least 25 ms), or when the scenario
ends with it due. Each iteration of a rate scenario is also a `<scenario> iteration` step sample from its scheduled
start. The semantics of stages and of a live change (`Load::set_target`, `update#LoadRunUsers`) are in
`test_client/load/schedule.rs` and [JS-Test-Load](../docs/dev/JS-Test-Load.md#rates-and-stages).

## 6. Kinds of load

**HTTP** is what the harness has: sessions with a cookie jar and the session token, forms, JSON, multipart, raw
requests, redirects on request. Added: `pause`, `urlParam`, `newSession`, and a `User-Agent` of `moqui-load/<version>`.
The target is any URL the run may reach (§10): this server by its own address, another MR Moqui, a Moqui Java server.
A server with the demo data needs its tarpits off (`conf/MoquiLoadTestConf.xml` on both MR and Java); a run shows the
share of 429 answers on its page, since that is the usual reason for a wall of errors. Built: no share of 429s; they
show in each name's outcomes.

**Services in process.** In a server, each user's thread has an ExecutionContext of its own for the thread's life (as
the server specs' `parallel` threads do), logged in as the user who started the run (D10), with the tarpit off.
`service.call` runs the service through the service facade with the transaction its definition gives, times it from
the call to its return, takes the context's error messages as the sample's error, clears them, and returns the
out-parameters as plain values. A transaction left open after a call is rolled back and counted as an error. The
call is on the user's thread, so the script engine, not a worker pool, sets the concurrency, as the Java runner's
threads did.

The Java screen also showed entity finds and writes per second for the calls. MR Moqui keeps no per-context artifact
type statistics; §16 has the options.

**Later kinds** (§17) fit the sample model without changing it: `ws` sends and receives as named samples; `ssh.exec`
(a command, such as an `agent-apps` step, with its exit status as the outcome); an interactive shell with `send` and
`expect`; a browser page per user.

## 7. Runs in a server

There is no entity for a run (D5). The host keeps a registry of the loads running in its process, and a finished run
is a file.

**A running load** is in the registry under its `runId` (`<yyyyMMdd-HHmmss>-<pid>`, the name of its directory): the
script, the label, the user who started it, the target, the settings, the parameters (secrets held here and nowhere
else), and the engine's `Load`, whose `snapshot()` answers the page's poll straight from memory and whose stops the
page's buttons call. Nothing is written to the database while a load runs.

**A finished run** is `runtime/tmp/test-load/<runId>/result.json`: the result of §5, whose `run` block has the status
(`completed`, `stopped` or `failed`), the user and the label. The list of runs is the running loads and then the
result files, newest first, read for their `run` block and summary (cached by the file's time). The command writes
its results under the same directory by default (§9), so a run made from the command on this runtime is in the list
without an import; a result from another machine is imported, which copies it into a directory of its own. Deleting
a run deletes its directory. The files are under `tmp`, so whatever clears it clears the runs; download a result to
keep it.

**One node.** A poll or a stop that reaches a node which is not running the load answers that the run is not on this
server, and the page says so. With one server, or sticky sessions, that does not happen.

**Services**, `org.moqui.dev.LoadServices`, Rust services the host registers through an extension entry (as
`moqui-host`'s SIP audio does), defined in `framework/service/org/moqui/dev/LoadServices.xml`:

| Service | |
| --- | --- |
| `find#LoadScripts` | The `test/load/*.load.js` files of the loaded components with their plans. A file that does not evaluate is listed with its error, and the others still list |
| `start#LoadRun` | `scriptLocation`, `targetKind`, `targetUrl`, `parameters`, `scenarios` (overrides), `label`; answers `runId`. Checks the permission and the Tools authorization, that the location is a load script of a component, that the target is allowed (§10), the limits of D6, and the plan's `uses` |
| `stop#LoadRun` | `runId`, `now`: after each user's current iteration, or before each user's next request |
| `get#LoadRunStatus` | A running load's snapshot, or a finished run's summary, for the page's poll |
| `find#LoadRuns` | The running loads and the result files, with find options by script and status, newest first |
| `get#LoadRunResult`, `delete#LoadRun`, `import#LoadRunResult`, `compare#LoadRuns` | The full result; delete the run's directory; a result file from another machine as a run; two to four runs side by side |

**Lifecycle.** `start#LoadRun` puts the load in the registry and starts it on a thread of the host, then returns. At
its end the run's thread writes the result file and takes the load out of the registry. At shutdown
(`before_shutdown`) running loads are stopped and their results written as `stopped`. A load cut off by the process
dying leaves no result. When the target is this server, the run writes a Monitor annotation at its start and end, so
dashboards show the load window
([Monitoring, Deploy markers and annotations](../docs/tools/Monitoring.md#deploy-markers-and-annotations)).

## 8. Tools › Load

`Tools/Load.xml` holds the JSON transitions; its screens are in `Tools/Load/`. The dashboard gets a "Load testing"
box (Load scripts, Runs), shown with the permission, and loses the "Service load runner" link.

| Screen | Archetype | What it has |
| --- | --- | --- |
| `Scripts.xml` | List | The scripts: title, component, description, scenarios with their default users. The row link opens New run. An empty state says where a script goes (`<component>/test/load/`) and links to the guide |
| `NewRun.xml`, `NewRun.qvue`, `NewRun.tui.json` | Task | Target: this server's services (when the script uses `service`), this server over HTTP, or another server with its URL. The plan's parameters, each as its kind (text, number, drop-down, password). Each scenario's users, ramp, duration, iterations. A label. One primary, Start, which opens the run's page. Opened from a run with Run again, it is filled from that run |
| `Runs.xml` | List | Started, script, label, target, status chip, users, requests, per second, errors, p95, by whom. Find options by script, status and date. A row selection action Compare (2 to 4 runs). Import result (a result file from another machine). Phone columns: script and label; status and p95 |
| `Run.xml`, `Run.qvue`, `Run.tui.json` | Detail | Record header: script and label, status chip, target, started and ended, user. Actions: while running, Stop (primary; after the current iterations) and Stop now (danger, confirmed); after, Run again (primary), Download result, Delete (danger, confirmed, naming the run). Body: number cards (active users, requests, per second, errors and rate, p50, p95, p99); charts of requests and errors per second, of p50, p95 and p99, and of active users; the table by name (kind, count, errors, mean, p50, p90, p95, p99, max, outcomes); the first errors; the generator panel; for a run on this server, the server panel (CPU load and memory from the status map). A "Not available" state (no such run, or a load running on another server) links to Runs |
| `Compare.xml` | Report | The runs as columns: summary rows, then each name with count, per second, p50, p95, p99 and errors, and the change against the first run in percent. Built: no per second by name |

The form of New run is built from the plan and the run's page draws live charts, which the XML widgets do not
express, so those two have a `.qvue` with a `tui` twin; the lists and Compare are plain XML screens. The run's page
polls `Load/runStatus` every 2 seconds while the run is running (the pattern [Monitoring](Monitoring.md) names for
live data) and stops polling when it ends. In `tui` the page shows the numbers as they are when it renders, with
Refresh, and the charts as the `chart` component the old twin used.

The demo lock-down (`runtime/dev-component/demo/MoquiConf.xml`) replaces `Load` with its Disabled screen in place of
`Service/ServiceLoadRunner`, and `Demo.spec.js` names the new path.

## 9. The command and specs

```sh
./moq test-load mr-marble --script OrderToCash --server http://localhost:8080 --param pdf=false --label "Java 3.1"
moqui test-load mr-marble --script OrderToCash --users 50 --duration 300 --ramp-up 30 --out /tmp/otc
moqui test-load runtime/component/mr-marble/test/load/OrderToCash.load.js --server https://test.example.com --insecure
moqui test-load --compare /tmp/otc/java/result.json /tmp/otc/mr/result.json
```

`moqui test-load [component | path] [--script Name]` follows `moqui test-client`: a component name, a `test/load`
directory or one `.load.js` file, against a server that is already running (`http://localhost:8080` by default, or
`MOQUI_TEST_SERVER`; `./moq test-load -p N`). It is a pure client: no database, no conf, no listener. It takes
`--server`, `--param`, `MOQUI_TEST_*` variables (for a password), `--insecure`, `--allow-http`, `--out` and
`--log-level` as `test-client` does, and `--users`, `--ramp-up`, `--duration`, `--iterations` (for every scenario, or
`--scenario name` to run and set one), `--label`. With no `--script` it lists the component's scripts with their
parameters. While running it prints a line every 5 seconds (`35s  10 users  241.2/s  0 errors  p95 38ms`), and at the
end the table and the path of the result. The output directory is `runtime/tmp/test-load/<yyyyMMdd-HHmmss>-<pid>` by
default (`test-load-out/<run>` in the working directory without a runtime), with `result.json` in it, which is where
Tools › Load › Runs finds it (§7). Ctrl-C stops the users and still writes the result. It exits 1 when a threshold
is broken or no request succeeded. A script that uses `service` is refused with the reason. `--compare` prints two
or more result files side by side.

In a client spec, `load({ script: "test/load/OrderToCash.load.js", params, scenarios: { sales: { users: 2, iterations: 1 } } })`
runs a script of the spec's component and returns the result, so an acceptance spec asserts that a script still walks
its flow. `load({ scenario() {...} })` with functions stays as it is.

## 10. Security

- **Permission and authorization.** `LOAD_RUNNER` (D9), sealed, on `ADMIN_ADV`, and all actions on the Tools app,
  checked in the services whatever calls them, as the SQL services do. The screens check them too.
- **Scripts.** Only a `test/load/*.load.js` file of a loaded component (D12). A user thread has no `ec`: the script reaches
  the server only through `service.call`, as the starting user with authorization required (D10), and through requests.
- **Targets.** A run reaches this server, loopback, and the URL prefixes in the `load_runner_targets` property; a dev
  instance reaches any URL. Otherwise a holder could aim the server at another site (a request forgery and a
  denial of service tool). Plain `http` to another machine is refused unless the target list names it, since a login
  crosses in the clear. `insecure` (an invalid certificate accepted) is a run setting, shown on the run.
- **Secrets.** A `secret` parameter is held in memory for the run and is not in the result, the snapshot or a log
  line. Run again asks for it again.
- **Result files.** A run is read, deleted and imported only by its `runId`, a directory name under
  `runtime/tmp/test-load/` with no path in it; an import is checked to be a result before it is kept.
- **Limits.** One run an instance and 1,000 users a run by default (D6). The run's contexts skip the tarpit; the
  target's own tarpits still apply to HTTP.
- **Demo.** Replaced by the Disabled screen (§8).
- [Attack-Surface](../docs/security/Attack-Surface.md) gets the screen's row, the target rule and the sealed
  permission's new name.

## 11. The first script: order to cash

`runtime/component/mr-marble/test/load/OrderToCash.load.js` (the mr-marble repository). One scenario, `sales`, with the
JMeter script's defaults: 10 users, 1 second ramp, 5 iterations, a Gaussian pause of 200 ± 50 ms, a new session each
iteration.

**What the two servers share.** Each JMeter step was mapped onto MarbleERP (Java, with SimpleScreens) and mr-marble
by reading both trees. The walk is the same on both:

- Both mount Marble ERP at `/apps/marble`, where a GET answers plain HTML. (`/qapps` on both, and `/vapps` on Java,
  send only the shell.)
- The screens, transition names, services and their parameters are the same files, line for line in most places:
  `Order/FindOrder.xml` `createOrder`; `Order/OrderDetail.xml` `addProductItem`, `createItem`, `placeOrder`,
  `approveOrder`, `shipOrderPart`; `Shipment/ShipmentDetail.xml` `pickShipment`, `packItem`, `packShipment`;
  `Accounting/Invoice/EditInvoice.xml` `updateStatus`, `recordPayment`.
- The demo ids the script uses (`CustJqp`, `ORG_ZIZI_RETAIL`, `ZIRET_WH`, `DEMO_1_1`, `john.doe`) are Mantle's and
  webroot's demo data on both. Neither needs PopCommerce.
- Both take the session token as the `X-CSRF-Token` header, which the harness's sessions send, and both send it in
  the login response's headers, where the harness reads it. Both refuse a transition POST with a query string, so
  parameters go in the form.
- A transition's redirect carries the target screen's declared parameters, so `createOrder` answers a `Location` of
  `.../Order/OrderDetail?orderId=<id>`. The shipment and invoice ids are not in a redirect (they are a service's
  outputs, and a SECA's); they are in the `href` of the "Shipment <id>" link on OrderDetail
  (`.../Shipment/ShipmentDetail?shipmentId=<id>`) and of the "Invoice <id>" link on ShipmentDetail
  (`.../Accounting/Invoice/EditInvoice?invoiceId=<id>`).
- Both need their tarpits off (`conf/MoquiLoadTestConf.xml` is in both runtimes): the demo data answers 429 after 120
  hits of a screen in 60 seconds.

So the script needs no flavor switch. Its parameters:

| Parameter | Default | |
| --- | --- | --- |
| `appPath` | `/apps/marble` | The app's mount; `/apps/PopcAdmin` runs the walk on a Java server with PopCommerce |
| `user`, `password` | `john.doe`, `moqui` (secret) | |
| `customerPartyId`, `vendorPartyId`, `facilityId`, `productId` | `CustJqp`, `ORG_ZIZI_RETAIL`, `ZIRET_WH`, `DEMO_1_1` | |
| `orderItems`, `quantity` | 3, 10 | Product items an order, and the quantity of each |
| `pdf` | `true` | The two `/fop` requests. Java renders them with FOP; MR Moqui prints with Chromium, so a server without one answers that PDF is not available. `false` leaves the step out |
| `logout` | `false` | The JMeter script logs out each loop. A logout ends every session of that user on both servers, so users sharing one account must not; `true` is for a run with one user |

**The steps**, with the sample name of each (the same on both servers, so Compare lines them up):

| Name | Request | |
| --- | --- | --- |
| `login` | `POST /Login/login` (username, password) | A new session each iteration; the token comes from this response's headers |
| `dashboard` | `GET {app}/dashboard` | |
| `find orders` | `GET {app}/Order/FindOrder` | |
| `create order` | `POST {app}/Order/FindOrder/createOrder` (customerPartyId, vendorPartyId, facilityId), followed | `orderId = response.urlParam("orderId")`; `fail` when it is null |
| `add product item` | `POST {app}/Order/OrderDetail/addProductItem` (orderId, orderPartSeqId 01, productId, quantity, unitAmount, itemTypeEnumId ItemProduct), `orderItems` times | |
| `add shipping charge` | `POST .../OrderDetail/createItem` (itemDescription, quantity 1, unitAmount 30, itemTypeEnumId ItemShipping) | |
| `place order`, `approve order` | `POST .../OrderDetail/placeOrder`, `.../approveOrder` (orderId) | |
| `order detail pdf`, `order pdf` | `GET /fop{app}/Order/OrderDetail`, `/fop{app}/Order/PrintOrder` (orderId) | With `pdf`; checks the `%PDF` magic |
| `ship order` | `POST .../OrderDetail/shipOrderPart` (orderId, orderPartSeqId 01), followed | `shipmentId` from the `ShipmentDetail?shipmentId=` link in the page |
| `find shipments`, `shipment detail` | `GET {app}/Shipment/FindShipment`, `.../ShipmentDetail` (shipmentId) | |
| `pick shipment` | `POST .../ShipmentDetail/pickShipment` (shipmentId) | |
| `pack item` | `POST .../ShipmentDetail/packItem` (shipmentId, productId, quantity × orderItems) | |
| `pack shipment` | `POST .../ShipmentDetail/packShipment` (shipmentId), then `GET .../ShipmentDetail` | `invoiceId` from the `EditInvoice?invoiceId=` link |
| `find invoices`, `invoice detail` | `GET {app}/Accounting/Invoice/FindInvoice`, `.../EditInvoice` (invoiceId) | |
| `finalize invoice` | `POST .../EditInvoice/updateStatus` (invoiceId, statusId InvoiceFinalized) | The invoice of a packed shipment may already be finalized; L2 finds out what the transition answers then, and the script skips the step when the page shows it |
| `record payment` | `POST .../EditInvoice/recordPayment` (invoiceId, statusId PmntDelivered) | |
| `logout` | `POST /Login/logout` | With `logout` |
| `order to cash` | | A `step` around the whole iteration: the time of one business process |

**To find out in L2**, on the first runs: that the link `href`s are as read from the source on both servers in HTML;
what `/fop{app}/Order/OrderDetail` answers on MR Moqui (only PrintOrder's PDF has been checked there); whether many
sessions of one account at once meet a limit on either server; and how reserving the same product from many users
at once behaves (Mantle's `AssetReservationMultipleThreads` spec covers the service, not the screens). mr-marble's
migration status lists these screens as migrated and none as validated in a browser, so the first runs are also a
test of them; a defect found goes to mr-marble, not into the script.

**What L2 found.** Building and running the script corrected this section in a few places:

- Every transition is followed and the page it lands on is checked for an error alert. A transition that fails
  answers the same redirect as one that succeeds, and the error shows only on the next page. The alert markup differs:
  Java writes `alert alert-danger`, MR `class="alert" data-type="danger"`.
- `add shipping charge` also sends `orderId` and `orderPartSeqId` 01, which `create#OrderItem` requires.
  `add product item` sends a unit amount of 18.99, as the JMeter script did.
- `record payment` sends `effectiveDate` (epoch milliseconds, which both servers read without a time zone). Without
  it Mantle's `PaymentCheckEffectiveDate` refuses the payment on both servers.
- `finalize invoice` never runs with the demo data. Packing leaves the invoice Finalized on both servers, through
  `create#SalesShipmentInvoices`. MR leaves it In Process only for a shipper with `salesInvoiceFinalizedAtShip`.
- The login is the script's own `POST /Login/login`, named `login`. The harness's `http.login` posts `/rest/login`.
- The link hrefs are as read from the source on mr-marble and on Java. `/fop/apps/marble/Order/OrderDetail` answers a
  PDF on MR (Chromium). Ten sessions of one account at once meet no limit on either server.
- Java refuses every transition over plain http when `webapp_https_enabled` is true, so a Java run over http needs
  it off. That is the default.

**The runs** (2026-11-10). One machine, an AMD Ryzen 9 9900X with 12 cores and 24 threads. The generator was
`moqui test-load` in its own process. Defaults: 10 users, ramp 1 s, 5 iterations, a Gaussian pause of 200 ± 50 ms,
and a new session each iteration. The generator's CPU share was 0.2%.

| Run | Server | Requests | Per second | Errors | p50 | p95 | p99 | Order to cash p50 / p95 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Java, pdf, cold | Java MarbleERP, `moqui.war` of 2026-11-09, H2, `MoquiLoadTestConf` | 1,040 | 28.4 | 1 | 26 ms | 930 ms | 1,319 ms | 506 / 12,386 ms |
| Java, pdf, warm | the same, a second run | 1,050 | 42.9 | 0 | 22 ms | 36 ms | 45 ms | 449 / 494 ms |
| Java, no pdf | the same | 950 | 42.3 | 0 | 20 ms | 36 ms | 49 ms | 387 / 436 ms |
| MR, pdf, before FOR NO KEY UPDATE | mr-marble, release build, embedded Postgres 18.6, `MoquiLoadTestConf` | 1,047 | 39.5 | 1 | 35 ms | 59 ms | 111 ms | 725 / 1,729 ms |
| MR, pdf, again, before FOR NO KEY UPDATE | the same | 1,047 | 39.0 | 1 | 36 ms | 65 ms | 91 ms | 741 / 1,647 ms |
| MR, no pdf, before FOR NO KEY UPDATE | the same | 929 | 34.1 | 9 | 32 ms | 71 ms | 1,483 ms | 1,122 / 2,474 ms |
| MR, no pdf, no-key (first run after start) | the same, with mr-mantle's organization locks `FOR NO KEY UPDATE` | 950 | 38.0 | 0 | 37 ms | 138 ms | 338 ms | 692 / 2,245 ms |
| MR, pdf, no-key | the same | 1,050 | 40.1 | 0 | 35 ms | 72 ms | 100 ms | 766 / 848 ms |
| MR, pdf, no-key, second | the same | 1,050 | 36.6 | 0 | 39 ms | 113 ms | 326 ms | 848 / 1,761 ms |
| MR, pdf, no-key, third | the same | 1,050 | 30.9 | 0 | 77 ms | 237 ms | 553 ms | 2,802 / 3,326 ms |

- **Per step**, warm Java against MR (pdf): MR's p50 is 30 to 110% higher on almost every step. For example, create
  order is 22 vs 32 ms, add product item 27 vs 42 ms, and find invoices 9 vs 20 ms.
- **MR's errors and tail before FOR NO KEY UPDATE were lock waits and deadlocks in Postgres**, not slow requests. Two transactions deadlock:
  one holds the organization's `party` row `FOR UPDATE` and waits on `gl_account_organization ... FOR UPDATE`, while
  the other inserts an `invoice`, whose foreign key check needs `FOR KEY SHARE` on that party row. Pack shipment answers
  500, and record payment and create order sometimes do too. Their p95 and p99 near 1 s are Postgres's deadlock
  timeout.
  - Java on H2 has none of this.
  - Fixed (2026-11-10): the entity layer has a `for-no-key-update` lock (Postgres `FOR NO KEY UPDATE`, which does
    not conflict with a foreign key check), and every mr-mantle organization lock uses it, except the party
    conversion and merge services, which change Party key columns. The four runs after the fix had no errors and no
    deadlock in the server log, and Java and MR now both run the walk clean.
  - The third pdf run after the fix slowed through the run (its order-to-cash mean went from 0.9 s in the first 10 s
    to 3.2 s at the end) with no error or lock wait logged; not looked into yet.
- **The cold Java run** spent its first 15 seconds compiling. Its one error is FOP failing to transform the first
  OrderDetail PDF.
- **What differed in the setup:**
  - the database (H2 in process against Postgres over a socket), which is the largest difference;
  - the PDF renderer (FOP against Chromium);
  - the pages themselves (§16.7).
- **A fair comparison of the two frameworks** needs both servers on Postgres; the MR deadlock is fixed.
- **The result files** are under `runtime/tmp/test-load/` of this checkout:
  - Java: `20261110-154200-2060829` (cold), `-154243-2061599`, `-154308-2062112`;
  - MR: `20261110-153128-2034648`, `-153237-2037672`, `-153304-2040024`;
  - MR after FOR NO KEY UPDATE: `20261110-185343-489917` (no pdf), `-185414-491481`, `-185441-495824`,
    `-185509-499159`.

## 12. Code layout

| Path | What |
| --- | --- |
| `crates/moqui-lib/src/histogram.rs` | The log-linear histogram (no ExecutionContext, so the lowest crate) |
| `crates/moqui-host/src/test_client/load/` | `mod.rs` (a load, its users, snapshot, stop), `record.rs` (the thread's buffer and the merge), `bins.rs`, `stats.rs` (the result, the table, thresholds), `pace.rs` (think, pause), `schedule.rs` (built: stages, the arrival rate, a live target), `script.rs` (reading a plan, starting users on a script file) |
| `crates/moqui-host/src/test_client/load_cmd.rs` | `moqui test-load`: options, the script list, the progress line, `--compare`; `usage.rs` and `lib.rs` for the help and the dispatch; `moq` for `./moq test-load` |
| `crates/moqui-host/src/test_client/prelude.js` | `plan`, `scenario`, `step`, `pause`, `service`, `fail`, `urlParam` on a response, `load({ script })` |
| `crates/moqui-devkit/src/testkit/run.rs` | `run_script_on_thread`: a thread that evaluates files and then calls a function, with or without an entered ExecutionContext |
| `crates/moqui-host/src/load_runner/` | The server side, under the `test-client` feature with an off stub: `mod.rs` (the extension entry and the services), `run.rs` (the registry, the run's thread, stop, shutdown), `results.rs` (the result files: list, read, import, delete, compare), `service_call.rs` (the in-process call), `targets.rs`; built also `scripts.rs` (the components' scripts), `compare.rs` and `server.rs` (the server samples, the Monitor annotations) |
| `runtime/base-component/framework/service/org/moqui/dev/LoadServices.xml`, `data/SecurityTypeData.xml`, `conf/MoquiDefaultConf.xml` | The services; the permission and its place in the sealed list. No entity |
| `runtime/base-component/tools/screen/Tools/Load.xml`, `Tools/Load/` | The screens of §8 |
| `runtime/base-component/tools/test/load/ServiceCall.load.js` | The shipped script |
| `runtime/component/mr-marble/test/load/OrderToCash.load.js` | The first script |

## 13. Phases

Each phase ends with its regression tests passing, `cargo lint` (and `cargo lint-min`, the host changes), and
`./moq test-framework` before its commit; L2 and the mr-marble parts also `./moq test-apps`. Screens are linted with
`moqui lint --screens`. The acceptance tests of §14 are written in the phase named there and run once when written;
running them is not part of Done.

| Phase | Builds | Regression tests |
| --- | --- | --- |
| **L1 Engine** | The sample model with kinds and outcome labels; the histogram; the per-second merge and bins; `snapshot()` and the two stops; `pause`; `step`; `urlParam`; several scenarios; `plan` and `scenario` with whole-file users; `load({ script })`; the result's new fields and thresholds | Rust unit tests: histogram percentiles and merge against sorted samples; bins closing and merging past the cap; the Gaussian pause's mean within bounds and never negative. The harness's own client specs (`crates/moqui-host/tests/fixtures/test-component/test/client/Load.spec.js`): a script file with a top-level helper, two scenarios, a stop, a threshold broken, a top-level request refused |
| **L2 Command and first script** | `moqui test-load`, `./moq test-load`, `--compare`; `OrderToCash.load.js`; the run against mr-marble and against a Java MarbleERP, with the two result files and what differed recorded in this plan | CLI tests in `crates/moqui/tests/cli/`: the options, a script list, a refusal of a `service` script. No regression spec runs the walk (§14) |
| **L3 Runs in a server** | The permission rename; `LoadServices`; the registry, the run's thread, snapshot and stop; the result files and the list of runs; shutdown; `service.call` in process; `ServiceCall.load.js`; the targets rule | `tools/test/LoadRunner.spec.js`: a holder starts `ServiceCall` for 2 users and 2 iterations and the run ends completed with 4 requests and is in `find#LoadRuns`; a non-holder is refused for the permission with the same parameters; a service the starting user is not authorized for is counted as an error, not run; a location outside a component's `test/load/` is refused; a target not allowed is refused; a second run while one runs is refused; stop ends a run; a service error is counted and the next call still runs |
| **L4 Tools › Load** | The screens of §8, the dashboard box, the old screen and its link removed, the demo lock-down, Attack-Surface | In `LoadRunner.spec.js`: `Run` rendered once in `qvt` and once in `tui` for a finished run (it has its own templates in both), each with all of its checks (the header, a name's row, no `Error rendering`); `NewRun` the same for `ServiceCall`. The demo spec's path |
| **L5 Compare and import** | `compare#LoadRuns`, `Compare.xml`, `import#LoadRunResult`, Run again, the Monitor annotation, the generator panel's CPU share | `compare#LoadRuns` on two made-up results gives the percent changes; an import of a result file makes a completed run and refuses a file that is not a result |
| **L6 Rates and stages** | The arrival rate with scheduled starts and `dropped`; `stages` (users over time); changing a scenario's users while it runs, from the run's page | Rust unit tests of the schedule with a fake clock; a fixture spec where a slow fixture endpoint shows the wait in the step time |
| **L7 Other kinds** | `ws` samples; `ssh.exec` and an `agent-apps` session; an interactive shell with `send` and `expect` (§17 has the sketch). Its own short plan first | Decided there |

L1 and L2 give the comparison of Java and MR from the command. L3 and L4 give the Tools app its runner back. L5 on
can wait for use to show what matters.

## 14. Acceptance tests to write

| Test | Suite and phase | What it checks |
| --- | --- | --- |
| `mr-marble/acceptance/client/OrderToCash.spec.js` | Acceptance (a complete scenario over HTTP, well over the regression budget), L2 | `load({ script })` with 2 users and 1 iteration against the suite's server: no errors, every step's name has 2 samples, and the two orders, shipments and invoices it made are in their final statuses, read over REST |
| `framework/acceptance/client/LoadBasic.spec.js` (changed) | Acceptance, L1 | Its "mixed reads and writes" still passes on the new statistics, and the result has bins |
| `tools/acceptance/client/LoadRunner.spec.js` | Acceptance (a browser walk), L4 | A `LOAD_RUNNER` holder opens Load scripts, starts `ServiceCall` from New run, sees the request count grow on the run's page, stops it, finds it in Runs as Stopped, and compares it with a second run |

A run of the order to cash script against a Java server is done by hand in L2 and recorded here; it is not a test in
either suite, since no suite starts a Java server.

## 15. Docs to change

- `docs/tools/Load-Runner.md`, new: writing a script, the API added, running from Tools and from the command, reading
  a result, comparing, the tarpit and generator notes, what is approximate.
- [JS-Test-Client](../docs/dev/JS-Test-Client.md): the Load section points to it, and gains `load({ script })`,
  `step`, `pause`, `urlParam`, the bins and the approximate percentiles.
- [Build-Test](../docs/Build-Test.md) and the `moq` help: `test-load`.
- `runtime/base-component/tools/README.md`: the `Load/` row, `test/load/`; mr-marble's README: its script.
- [Attack-Surface](../docs/security/Attack-Surface.md), [Parity-Diffs](Parity-Diffs.md) (D10, the removed screen),
  [Parity-Mapping](Parity-Mapping.md) (`ServiceFacadeImpl.LoadRunner` to `load_runner`).
- [Remaining-Work](Remaining-Work.md): a load runner row with what is left (§17, L7), pointing here; the arrival-rate
  model, server samples and compare are built.

## 16. Risks and open questions

1. **The generator's own ceiling.** A user is a thread with an interpreted script engine (about 1 MB and 7 ms to
   start, [JS-Test-Client](../docs/dev/JS-Test-Client.md#load-and-parallel)). A few hundred users a process is the
   scale. [Performance](Performance.md)'s target of 100,000 a second is far past what scripted users generate from
   one process; that target stays with the speed tests and a native driver. L1 measures the ceiling (requests a
   second per core for a script of one request against a fixture that answers at once) and the guide states it; the
   `generator` block makes a run at the ceiling visible.
2. **The generator inside the target.** A run from Tools against the same server shares its CPUs and, for services,
   its threads. That is what the Java runner did, and it is fine for finding a knee or a regression, not for a
   capacity number. The result says `in-target`; the guide says to use the command from another machine for capacity.
3. **Entity finds and writes per second.** The old screen had them from per-context artifact statistics, which MR
   Moqui does not keep. Options: read the telemetry metrics (`db.client.operation.duration`, a minute's resolution,
   too coarse for a live chart); or add cheap per-context counters (finds, writes) that the in-process call reads
   before and after. Recommended: leave it out of L3, and add the counters in L5 if the server panel proves wanted.
4. **Runs without records.** A load cut off by the process dying leaves nothing, the runs are files under `tmp` on
   one node, and a cluster without sticky sessions shows a running load on one node only (D5). That is accepted for
   now; the distributed runner of §17 is where shared run records would come in.
5. **Engines in the server process.** 1,000 users are 1,000 engines and threads in the server (about 1 GB at the
   harness's measured size). The default limit is there for that; quiet instances favour memory, so the limit is a
   property an instance lowers.
6. **Approximate percentiles.** Specs that compare a p95 with a limit keep working; one that checks an exact
   percentile of a tiny sample (the harness's own fixture spec) changes to the histogram's value.
7. **Java and MR are not the same walk byte for byte.** The pages differ, so the comparison is of the same business
   steps, not of equal work per request. The script names each step the same on both, and the write-up of L2 says
   where a step does more on one side.
8. **An authorization override for in-process calls** (D10). Authorization is required to start with. If a load
   needs a service its starter is not granted, the choices are a grant in data for a load user, or an override on
   the run behind a sealed permission of its own. Decide on the first real case.
9. **A stop now interrupts a service's script.** A load run's stop now ends whatever script runs on a user's thread,
   a service's own included (`moqui_core::script::interrupt_thread_on`). A service with a transaction rolls back; one
   with `transaction="ignore"` can be left half done, as a request's timeout leaves it.

**What the build settled** (L1 to L6):

- 1: the ceiling was not measured; JS-Test-Client gives the scale (a few hundred users a process) and the `generator`
  block's `cpu` and `scriptShare` show a run at it. Still to measure.
- 2: a run from Tools against this server is `in-target`, with a `generator.note` and the server samples; one
  against another server is `in-server`, since the generator is this server's process and not the target's.
- 3: no per-context counters were added; the server panel has the machine's load, the process's CPU share, memory
  and threads (`load_runner/server.rs`).
- 5: the limits are `load_runner_max_runs` and `load_runner_max_users`, as planned.
- 6: percentiles are the histogram's, within about 1.6%, and the guide and `moqui help test-load` say so.
- 8: still open: no override; a service the starter is not granted is an error.
- 9: built as described. A run also stops now when its starter can no longer log in or loses `LOAD_RUNNER` (checked
  every 10 seconds), and the result says why.

## 17. Later

- **SSH and interactive loads** (L7), sketched so that L1's sample model holds them:

  ```js
  plan({ title: "Terminal walk", uses: ["ssh"] })
  scenario("agent", {
      users: 20, think: [500, 1500],
      setup() { this.apps = ssh.connect({ user: params.user, password: params.password }).agentApps("apps/marble") },
      run() {
          this.apps.command("go Order/FindOrder", { name: "find orders" })     // one exec, its exit status the outcome
          this.apps.command("key Enter", { name: "open first" })
      },
      teardown() { this.apps.close() },
  })
  ```

  and for a shell, `const sh = conn.shell(); sh.send("status\r"); sh.expect(/uptime/, { name: "status", timeout: 5 })`.
  The host has an SSH client (`crates/moqui-host/src/ssh/client.rs`, russh).
- **WebSocket samples** (`/notws` subscribers held open while HTTP users write), and **browser users** (a Chromium
  page per user, a handful at most).
- **A distributed runner**: the generator spread across a cluster's nodes from Tools, or the command on several
  machines, with one merged result. It needs what D5 leaves out: run records every node sees, a stop that reaches
  every node, and histograms merged across them (they merge by adding, which L1's histogram allows).
- **The target's own numbers**: for another MR Moqui, its status map and metrics read over REST during the run; a
  `traceparent` on each request so Monitor's traces of a run can be found.
- **Data feeders**: `data.csv(path)` with a row per user or per iteration.
- **CSS selectors on a response** (D11): `response.select(css)` giving `{ tag, text, attrs }` for tag, `#id`, `.class`,
  `[attr]`, `[attr=value]`, descendant and child, when a script needs more than a URL parameter or a regular expression.
- **Saved profiles and schedules**: a named set of script, parameters and settings, and a run on a schedule with its
  thresholds as an alert.
- **A purge** of old runs and their result files.
