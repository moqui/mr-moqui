# Load runner

A load script is a JavaScript file in a component that declares virtual users and what each does: HTTP requests and
WebSocket messages to any server, commands and terminal sessions over SSH, or calls of this server's services in its
own process. One engine runs it from three places: Tools ›
Load in a running server, the `moqui test-load` command, and a client spec's `load({ script })`. A run records every
request in a histogram per name and in one-second bins, so a run of any length keeps bounded memory and has a time
series, and a finished run is a JSON result file that Tools lists, compares and imports. Tools › Load replaces Java's
Service Load Runner screen. The design and build record is [plans/archive/LoadRunner.md](../../plans/archive/LoadRunner.md); this guide says what was
built: running a script from Tools and from the command, reading a result, and the security of a run in a server.
Writing a script is in [JS-Test-Load](../dev/JS-Test-Load.md), and the harness the users run on (sessions, `http`,
`files`, `assert`, the `load(opts)` options) in [JS-Test-Client](../dev/JS-Test-Client.md#load-and-parallel).

The code:

- `crates/moqui-host/src/test_client/load/`, the engine:
  - `mod.rs`: a load, its users, the live snapshot and the stops;
  - `record.rs`, `bins.rs`: the samples and the time series;
  - `schedule.rs`: closed scenarios, stages, arrival rates and a live target;
  - `pace.rs`: think and pause times;
  - `script.rs`: reading a plan and a scenario's settings;
  - `stats.rs`: the result, thresholds and the console table.
- The histogram: `crates/moqui-lib/src/histogram.rs`.
- The JavaScript side: `crates/moqui-host/src/test_client/prelude.js` (`plan`, `scenario`, `step`, `pause`, `fail`,
  `service`, `ssh`, `response.urlParam`, `load({ script })`).
- WebSocket and SSH samples: `crates/moqui-host/src/test_client/ws.rs` and `ssh.rs` (the SSH client, on the login of
  the SFTP client, `crates/moqui-host/src/ssh/client.rs`).
- The command: `crates/moqui-host/src/test_client/load_cmd.rs`, its help in `crates/moqui-host/src/usage.rs`, and
  `./moq test-load`.
- Runs in a server: `crates/moqui-host/src/load_runner/`:
  - `run.rs`: the registry, a run's thread, stops and shutdown;
  - `service_call.rs`: `service.call` in process;
  - `targets.rs`: what a run may reach, and the limits;
  - `results.rs`, `compare.rs`: the result files, import and compare;
  - `server.rs`: the server samples and Monitor annotations.
- Services: `runtime/base-component/framework/service/org/moqui/dev/LoadServices.xml`. Screens:
  `runtime/base-component/tools/screen/Tools/Load.xml` and `Load/`.
- Scripts: `runtime/base-component/tools/test/load/ServiceCall.load.js` (one service called again and again, what the
  Java screen did), `runtime/base-component/tools/test/load/AgentApps.load.js` (the apps over SSH as an agent drives
  them, each step an `agent-apps` command) and `runtime/component/mr-marble/test/load/OrderToCash.load.js` (a sales
  order through the Marble ERP screens, on MR Moqui or a Java MarbleERP).

## Writing a script

A script is `<component>/test/load/<Name>.load.js`: `plan({...})` once, for its title, the kinds of load it makes,
its params and thresholds, and `scenario(name, {...})` once for each group of users, with `run(user, iteration)`
and its users, duration, iterations, pauses, rate or stages. The API is in [JS-Test-Load](../dev/JS-Test-Load.md):

- [`plan(options)`](../dev/JS-Test-Load.md#planoptions) and [`params`](../dev/JS-Test-Load.md#params), with how a
  value is resolved from the run, the environment and its default;
- [`scenario(name, options)`](../dev/JS-Test-Load.md#scenarioname-options);
- [rates and stages](../dev/JS-Test-Load.md#rates-and-stages), with dropped starts and coordinated omission;
- [the globals](../dev/JS-Test-Load.md#globals): `step`, `pause`, `fail`, `service.call`, `response.urlParam`, and
  those of a client spec;
- [requests, names and errors](../dev/JS-Test-Load.md#requests-names-and-errors), and the samples of
  [WebSocket and SSH](../dev/JS-Test-Load.md#websocket-and-ssh).

A scenario's settings are defaults. A run overrides them: the New run form in Tools, the options of the command
below, or the settings of a client spec's `load({ script })`.

## Running from Tools › Load

Tools › Load (`/apps/tools/Load`; the "Load testing" links on the Tools home) needs the `LOAD_RUNNER` permission.

- **Load scripts** lists the `test/load/*.load.js` files of the loaded components with their plans. A file that does
  not evaluate is listed with its error.
- **New run** (from a script, or from a run with Run again) has the target, the plan's params (a secret one as a
  password field, a closed list as a drop-down), each scenario's users, ramp, duration and iterations, and a label.
  Start opens the run's page.
- **Runs** lists the running loads and then the finished runs, newest first, with find options by script, status
  and start date. Select two to four for Compare; Import result adds a result file.
- **A run's page** polls every 2 seconds while the run is running: number cards (active users, requests, per second,
  errors, dropped for a rate scenario, p50, p95, p99), charts of requests and errors, of p50, p95 and p99, and of active
  users, the table by name, the first errors, thresholds, the generator panel, and for a run on this server the server
  panel. In the terminal (`tui`) it shows the numbers as they are when it renders, with Refresh.

**Targets.**

| Target | What it is |
| --- | --- |
| This server's services | `service.call` in process. Required, and only offered, for a plan whose `uses` has `service` |
| This server over HTTP | Requests to this server's own URL. The default |
| Another server | Requests to a URL the target rule allows ([Security](#security)). "Accept an invalid certificate" is offered here, and shown on the run |

**Limits.** One running load per instance (`load_runner_max_runs`, 1) and 1,000 users a run in all
(`load_runner_max_users`): two loads at once spoil each other's numbers, and each user is a thread with its own engine
in the server's process. An instance short of memory sets a lower `load_runner_max_users`.

**While it runs.**

- **Users and rate.** The Scenarios panel changes a scenario's users, or a rate scenario's starts per second, from now
  on, in place of the rest of its stages (`update#LoadRunUsers`). For a closed scenario users above the new target
  finish their iteration and wait, and more start for a higher one. For a rate scenario both its rate and its most users
  at once are set there. The run's user limit counts every user already started. A plain closed scenario's users are
  not held by its schedule until its target is set.
- **Stop** stops each user after its current iteration. **Stop now** stops each before its next request: an iteration
  under way ends as it is, and the script on the user's thread, a service's own included, is interrupted. A service
  in a transaction rolls back; one with `transaction="ignore"` can be left half done.
- **Its starter.** Every 10 seconds the run checks that the user who started it can still log in and holds
  `LOAD_RUNNER`. When not it stops now, and the result says why (`run.stoppedBecause`).
- **Shutdown** stops every running load now and writes its result as stopped. A process that dies leaves no result.

**After it ends.**

- **Run again** opens New run filled with the settings the run used. A secret param is asked again. For an imported
  run it fills only the target kind, since the file's URL, params and settings are not trusted.
- **Download result** gives the result file; **Delete** removes the run's directory.
- **Import result** (on Runs) keeps a result file from another machine, such as one `moqui test-load` wrote, as a run
  of its own with the importer as its user (the file's is kept as `run.importedUser`). A file that is not a load
  result, or over 16 MiB, is refused.
- **Compare** shows two to four runs as columns, the first the base: requests, per second, errors, error rate, p50,
  p95 and p99, then each name's count, p50, p95, p99 and errors, every value after the first with its change from the
  first in percent.

**Where runs are kept.** Nowhere in the database. A running load is in the memory of the server process that runs it,
which answers its page's poll and takes its stop. A finished run is `runtime/tmp/test-load/<runId>/result.json`
(`runId` is `<yyyyMMdd-HHmmss>-<pid>`), and the list of runs is those files, so whatever clears `runtime/tmp` clears
the runs: download a result to keep it. Behind a load balancer without sticky sessions, a poll or a stop that reaches
another node answers that the run is not on this server.

A run on this server writes a Monitor annotation at its start and its end (when search is on), so dashboards show the
load window ([Monitoring](Monitoring.md#deploy-markers-and-annotations)).

The services are `org.moqui.dev.LoadServices`: `find#LoadScripts`, `start#LoadRun`, `stop#LoadRun`,
`update#LoadRunUsers`, `get#LoadRunStatus`, `find#LoadRuns`, `get#LoadRunResult`, `delete#LoadRun`,
`import#LoadRunResult` and `compare#LoadRuns`.

## Running from the command

`moqui test-load` runs a script over HTTP, WebSocket and SSH from its own process, against a server that is already
running. It is a
pure client, as `moqui test-client` is: it opens no database, reads no conf and binds no port, so any package runs it,
on this machine or another.

```sh
./moq run -c conf/MoquiLoadTestConf.xml -p 8090             # the server, in another terminal
./moq test-load mr-marble                                   # list the component's scripts and their params
./moq test-load mr-marble --script OrderToCash -p 8090 --param pdf=false --label "MR"
moqui test-load mr-marble --script OrderToCash --users 50 --duration 300 --ramp-up 30 --out /tmp/otc
moqui test-load runtime/component/mr-marble/test/load/OrderToCash.load.js --server https://test.example.com
moqui test-load --compare /tmp/otc/java/result.json /tmp/otc/mr/result.json
```

The target is a component name, a component directory, its `test/load` directory or one `.load.js` file. With no
`--script` it lists the scripts with their scenarios and params. A script whose plan uses `service` is refused: its
calls run only in a server's process. A script that uses `ssh` connects to `--ssh host:port` unless it names a host:

```sh
./moq run -c conf/MoquiLoadTestConf.xml -p 8090             # SSH on 2222 too, with room for many sessions
./moq test-load tools --script AgentApps -p 8090 --ssh 2222 --users 10
```

| Option | |
| --- | --- |
| `--script Name` | `test/load/Name.load.js` of the component |
| `--server URL` | The server; `http://localhost:8080` by default, or `MOQUI_TEST_SERVER`. `./moq test-load -p N` is `http://localhost:N` |
| `--ssh host:port` | Where `ssh.connect` goes when the script names no host and port; a port alone is on the server's host. Or `MOQUI_TEST_SSH` |
| `--param name=value` | A param of the plan, repeatable. `MOQUI_TEST_<NAME>` variables fill the params `--param` leaves out (`MOQUI_TEST_PASSWORD` is `password`), for a password that should not show in `ps` |
| `--users`, `--ramp-up`, `--duration`, `--iterations` | Every scenario's settings over the script's. `--duration` alone replaces the script's iterations, `--iterations` alone its duration |
| `--rate N` | N iterations started a second, with `--users` the most at once; it replaces the script's stages, and `--iterations` is then the starts in all |
| `--scenario name` | Run only this scenario; the settings above are its |
| `--label text` | Kept in the result's `run` block and shown in Tools |
| `--timeout seconds` | What a request waits for its answer (60) |
| `--out dir` | Write `<dir>/result.json` |
| `--insecure`, `--allow-http` | Accept an invalid certificate and any SSH host key; allow plain http to another machine |
| `--log-level level` | The console level (warn) |
| `--compare a.json b.json ...` | Print two or more result files side by side, as Compare does. No run |

While it runs it prints a line every 5 seconds (`35s  10 users  241.2/s  0 errors  p95 38ms`), then the table by name
and the result's path. The result goes to `runtime/tmp/test-load/<yyyyMMdd-HHmmss>-<pid>/result.json` (UTC), where
Tools › Load › Runs on the same runtime lists it without an import, or to `test-load-out/<run>` in the working
directory without a runtime.

**Exit status.** 0 when the run completed with its thresholds met; 1 when it could not start, a threshold was broken,
or no request succeeded; 130 after Ctrl-C, which stops the users before their next request and still writes the
result. A second Ctrl-C ends at once, with no result.

From a client spec, `load({ script: "test/load/OrderToCash.load.js", params, scenarios: { sales: { users: 2,
iterations: 1 } } })` runs a script of the spec's component and returns the result
([JS-Test-Client](../dev/JS-Test-Client.md#load-and-parallel)), so an acceptance spec checks that a script still walks
its flow (`mr-marble/test/acceptance/client/OrderToCash.spec.js`).

## Reading a result

A result is the JSON that `load()` returns, `moqui test-load` writes and Tools keeps.

| Field | |
| --- | --- |
| `users`, `iterations`, `requests`, `errors` | Counts over all users. `errors` is every error: a response of 400 and up (unless the request's `expect` allows it), a transport error, a throw, a `fail()` |
| `duration`, `rps` | Seconds from the first user's timed start to the last user's end; requests per second |
| `errorRate` | Errors per attempt (each request, and each error that was not a request's), so at most 1 |
| `latency` | `min`, `mean`, `p50`, `p90`, `p95`, `p99`, `max` in milliseconds over every request |
| `names` | For each name: the `latency` fields, `kind` (`http`, `raw`, `ws`, `ssh`, `step`, `service`), `count`, `errors`, `outcomes` (an HTTP status, `ok`, `error`, `timeout`, `closed`, an exit status as `exit 1`, `refused`), for HTTP `statuses` (`"0"` for a transport error), and `bins` of `count`, `errors` and `mean` every `binSeconds` (10 to start) |
| `scenarios` | For each scenario its `users` started, `iterations` and `errors`, and for a rate scenario `dropped` |
| `dropped` | The scheduled starts dropped over every rate scenario |
| `bins`, `binSeconds` | The time series ([Bins](#bins)) |
| `thresholds`, `passed` | Each limit with `name`, `metric`, `limit`, `value` and `passed`; `passed` is false when one is broken |
| `generator` | Where the load was made ([The generator](#the-generator)) |
| `server` | For a run from Tools on this server: `samples` and `cores` ([The server samples](#the-server-samples)) |
| `run` | `script`, `scenarios` (the settings used), `params` (no secret), `target`, `label`, `start`, `end`, `status` (`completed`, `stopped`, or `failed` when the run's thread failed), `stopReason` (`stop`, `stop-now`, `timeout`, or null). A Tools run adds `runId`, `user`, `targetKind`, `insecure` and `scenarioSettings` |
| `errorSamples` | The first 20 errors: `name`, `status`, `scenario`, `user`, `iteration`, `message` |

**Percentiles are approximate.** Each name keeps a log-linear histogram (64 steps in each power of two, 1 µs to an
hour), not its samples, so a percentile is within about 1.6% of the exact nearest-rank value. `min`, `mean` and `max`
are exact. Do not check an exact percentile of a handful of samples.

### Bins

One bin a second from the run's start: `t`, `users` (active), `requests`, `errors`, `dropped` (the starts scheduled in
that second and dropped), `mean`, `p50`, `p95`, `p99` and `max`, computed from that second's samples when the bin
closes. Past 7,200 bins neighbours merge and the bin length doubles, so a long run keeps its whole span; a merged
bin's percentiles are the mean of its two, weighted by requests, an approximation. A running load's snapshot, which
the run's page draws, has the bins thinned to at most 300 points.

### The generator

`generator` has `where`, `cores`, `cpu` (the generating process's CPU share of the machine over the run, on Linux)
and `scriptShare` (the users' time spent in script rather than waiting on a request or a pause).

- **`same-machine`** is every run of the command or of a client spec: the generator is a process of its own. It is
  not told whether the server is on the same machine.
- **`in-target`** is a run from Tools against this server (its services, or itself over HTTP): the users are
  threads of the server's own process, and `generator.note` says that `cpu` is then the server's work and the load's
  together.
- **`in-server`** is a run from Tools against another server: the generator is this server's process.

A high `cpu` or `scriptShare` means the numbers are the generator's, not the server's.

### The server samples

A run from Tools on this server (either of its targets) samples the server every 5 seconds from the status map, as
the old Service Load Runner screen polled it: `at` (seconds from the start), `loadPercent` and `load` (the machine's
load), `cpu` (the process's share since the last sample), `memoryMb` and `threads`. At most 720 are kept; past that
every other one is dropped. A run against another server has none: the target's own numbers are its own Monitor's.

## Notes on measuring

- **Tarpits.** A server with the demo data answers 429 once a user passes 120 hits of a screen in 60 seconds: the
  demo `ALL_SCREENS` tarpit is on `ALL_USERS`, which every user is in, so a row for the load user cannot exempt it.
  Run the server on `conf/MoquiLoadTestConf.xml` (in both MR Moqui's and Java's runtime), which turns off the screen,
  transition and service tarpits and the login history. A run from Tools turns the tarpit off only for its own
  in-process service calls; its HTTP requests meet the target's tarpits like any other. A wall of 429s in a name's
  outcomes is the usual sign.
- **The generator's ceiling.** A user is a thread with an interpreted script engine
  ([JS-Test-Client](../dev/JS-Test-Client.md#load-and-parallel) gives its size and the scale of users a process).
  Measured on 2026-11-12 (an AMD Ryzen 9 9900X, 12 cores and 24 threads, release build) with a script of one GET
  against a stub on the same machine that answers at once (`load_ceiling` in `crates/moqui/tests/cli/test_load.rs`, an
  ignored test): 30,000 requests a second from 1 user, about 77,000 from 4 to 16, and 64,500 from 256. A request
  costs the generator 33 to 45 µs of CPU, so a generator core makes 22,000 to 30,000 of them a second. At the plateau
  the generator used 3.5 of the 24 CPUs, so it was not its CPU that stopped there. A script that does much work
  between requests (large regular expressions over each page) reaches the ceiling sooner; the generator block's
  `cpu` and `scriptShare` show it. Past it, run several `moqui test-load` processes; their results are not merged,
  and Compare shows them side by side.
- **The embedded database's buffers.** The embedded Postgres has 128MB of shared buffers unless the conf says
  otherwise. It had 16MB until 2026-11-12, and on that the order to cash walk slowed run after run as its data grew,
  each backend evicting dirty pages all the time and their writeback stalling every query for up to a second.
  `conf/MoquiLoadTestConf.xml` gives the transactional datasource `shared-buffers="256MB"`; set more there, or on the
  conf of a server measured, for a larger load ([Database](Database.md)).
- **Capacity from another machine.** A run from Tools shares the server's CPUs and, for services, its process. That
  is fine for finding a knee or a regression, not for a capacity number. For capacity, copy the binary and the
  component's `test/load` directory (a checkout is simplest) to another machine, run `moqui test-load --server
  https://<host>` there, and import its result into Tools to keep it with the others.
- **Comparing servers.** The same script can walk MR Moqui and a Java Moqui (OrderToCash does), and Compare lines up
  the names. The pages are not the same byte for byte, so it compares the same business steps, not equal work per
  request; the database and the PDF renderer differ too unless set up alike.
- **Service calls.** Java's screen also showed entity finds and writes per second for the calls. MR Moqui keeps no
  per-context artifact statistics, so the server panel has the machine and process numbers only.

## Security

- **Permission.** Every `LoadServices` service checks `LOAD_RUNNER` (sealed, granted to `ADMIN_ADV`) and
  authorization for all actions on the Tools app, whatever calls it. Without `LOAD_RUNNER` the Tools menu entry is
  disabled, the dashboard box is not shown, and `Load.xml` shows Not available. The demo lock-down replaces
  `Tools/Load` with its Disabled screen.
- **Scripts come from files.** A run names a script by location, only a `test/load/*.load.js` file of a loaded
  component; no screen takes script text, which would be code execution in the server. A result file never supplies a
  script or a target: Run again and an import resolve and check them anew.
- **What a script reaches in a server.** A user's thread runs with no execution context entered and may call only the
  client harness's host functions: `ec` and the server's own globals do nothing for it, and the browser, `parallel`, a
  nested `load` and `response.save` are refused. `files` stays inside the script's component. Its requests go through
  the target rule before each request and redirect.
- **In-process service calls.** `service.call` runs as the user who started the
  run, through the service facade with authorization required and the transaction its definition gives, in a context
  of the user's thread with the tarpit off. A service that user is not authorized for is counted as an error and does
  not run. Java ran them as anonymous with authorization off. The call is in process, so a service's
  `allow-remote="false"` does not refuse it, as it does not refuse the user's own screens. A call that leaves the
  context changed (authorization or entity ECA rules off, another user) is an error, and the context is replaced; a
  transaction left open is rolled back and is an error.
- **Targets.** A run reaches this server, loopback, and the URL prefixes of the `load_runner_targets` property
  (comma- or space-separated); a dev instance reaches any URL. Otherwise a holder could aim the server at another site,
  a request forgery and denial of service tool. Plain `http` to another machine is refused unless an entry names it,
  since a login crosses in the clear: a prefix carries its scheme, and this server's own http URL counts only on a
  loopback host. A prefix is matched on the path a request is really sent to, after `..` and `.` segments are
  resolved. An `ssh.connect` reaches this server's own SSH listener (the default of a run against this server),
  loopback, and the `ssh://host:port` entries of `load_runner_targets` (port 22 when it names none), and declares
  `uses: ["ssh"]`; a dev instance reaches any host. A host key not pinned is taken from loopback, or anywhere when the
  run is insecure.
- **Secrets.** A `secret` param is held in memory for the run, never written to the result, the snapshot or a log
  line, and asked again on Run again.
- **Result files.** A run is read, deleted and imported only by its `runId`, a directory name with no path in it.

[Attack-Surface](../security/Attack-Surface.md#load-runs) has the same rules among the server's other tools.
