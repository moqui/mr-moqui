# Load scripts: the `.load.js` API

A load script is a JavaScript file, `<component>/test/load/<Name>.load.js`, that declares groups of virtual users
and what each user does: HTTP requests to a server, or calls of a server's services in its own process. Three things
run it, on one engine: the `moqui test-load` command (`./moq test-load`), Tools › Load in a running server, and a
client spec's `load({ script })`. This page is the API a script is written with. Running a script, reading its
result and the security rules of a run in a server are in [Load-Runner](../tools/Load-Runner.md).

The other JavaScript test APIs are [JS-Testkit](JS-Testkit.md), the server specs of `moqui test`, and
[JS-Test-Client](JS-Test-Client.md), the client specs of `moqui test-client`, whose globals a load script also has.
Building, the two suites and the commands are in [Build-Test](../Build-Test.md).

The code is `crates/moqui-host/src/test_client/prelude.js` (`plan`, `scenario`, `step`, `pause`, `fail`, `service`,
`load`), `crates/moqui-host/src/test_client/load/` (the engine: `script.rs` reads a plan and a scenario's settings,
`schedule.rs` the stages and rates, `pace.rs` the think and pause times) and
`crates/moqui-host/src/load_runner/` (a run in a server, `service_call.rs` for `service.call`).

## A script

The top level of the file calls `plan({...})` once and `scenario(name, {...})` once for each group of users. The
file is evaluated once to read the plan, and once whole in the engine of each user, so constants and helpers at its
top level are there in every scenario, each user with its own copy.

```js
plan({
    title: "Order to cash",
    description: "A sales order through Marble ERP's screens",
    uses: ["http"],
    params: {
        appPath: { default: "/apps/marble", description: "The app's mount" },
        user: { default: "john.doe", description: "The account every virtual user logs in as" },
        password: { default: "moqui", secret: true, description: "Its password" },
        pdf: { default: true, description: "Get the order as PDF" },
    },
    thresholds: { errorRate: 0, names: { "create order": { p95: 500 } } },
})

const app = params.appPath.replace(/\/+$/, "")          // once per user, in its own engine

scenario("sales", {
    users: 10, rampUp: 1, iterations: 5, pause: { gaussian: [200, 50] }, newSession: true,
    run(user, iteration) {
        step("order to cash", () => {
            http.post("/Login/login", { form: { username: params.user, password: params.password }, name: "login" })
            const created = http.post(app + "/Order/FindOrder/createOrder",
                { form: { customerPartyId: "CustJqp" }, follow: true, name: "create order" })
            const orderId = created.urlParam("orderId")
            if (!orderId) fail(`create order: no orderId in ${created.url}`)
            // ...
        })
    },
})
```

The rules of the top level:

- `plan()` and `scenario()` are called only there. Either one called later, or in a client spec, throws
  (`plan() is called at the top level of a load script`). A second `plan()` throws, and so does a second scenario
  of the same name.
- The top level makes no request. `http.request` (and so `get`, `post`, `login` and the rest), `http.raw`,
  `ws.connect`, `browser.newPage` and `service.call` there throw "a load script makes no request at its top level".
- Reading the plan may take 30 seconds at most, since the top level only declares.
- A script with no `scenario()` cannot run.
- `moqui test` and `moqui test-client` run only `*.spec.js` files, so neither runs a `.load.js`.

The scripts in the repository: `runtime/base-component/tools/test/load/ServiceCall.load.js` (one service called
again and again in the server), `runtime/component/mr-marble/test/load/OrderToCash.load.js` (a sales order through
the Marble ERP screens), and `runtime/dev-component/example/test/load/ScriptReach.load.js` (a fixture that proves
what a user's engine reaches in a server run).

## `plan(options)`

| Option | |
| --- | --- |
| `title`, `description` | Shown in the script list and on a run |
| `uses` | The kinds of load the script makes: `"http"` (the default, when `uses` is left out) and `"service"`. A run that cannot make one is refused: `service` needs a run in a server with the service target, and `moqui test-load` refuses a script that uses it; `http` needs a target URL. In a server run a request of a kind the plan does not declare throws (`the load script makes no HTTP request: its plan does not declare uses: ["http"]`) |
| `params` | The script's parameters ([`params`](#params)) |
| `thresholds` | Limits the run is checked against at its end: `errorRate`, and in milliseconds `min`, `mean`, `p50`, `p90`, `p95`, `p99`, `max`, over every request, or under `names: { "<name>": {...} }` for one name. An unknown metric, or a limit that is not a number, is refused before the users start. A name with no samples breaks its limits |

## `params`

`plan({ params })` declares each parameter as `{ default, description, options, secret, required }`, or as its
default value alone (`{ appPath: "/apps/marble" }`). `plan()` resolves them, and the global `params` is then the
frozen object of the resolved values. Before `plan()` runs, and in a script with no `plan()`, `params` holds the
values the run gave, as they were given.

| Field | |
| --- | --- |
| `default` | The value when the run gives none. Its type is the parameter's: a number, `true` or `false`, else text |
| `description` | Shown beside the field in Tools and in the command's listing |
| `options` | A closed list. A value outside it is refused, and Tools shows a drop-down |
| `secret` | The value is kept out of the result, the live snapshot, log lines and error messages, Tools shows a password field, and Run again asks for it anew. The script list shows no default for it |
| `required` | With no default, a run that gives no value is refused (`the load script's parameter password is required`) |

A value is resolved in this order:

1. What the run gives: `--param name=value` of `moqui test-load`, the New run form in Tools, or `params` of a
   client spec's `load({ script, params })`.
2. For a parameter the run leaves out or gives empty, the environment: a `MOQUI_TEST_<NAME>` variable, in camel
   case (`MOQUI_TEST_APP_PATH` is `appPath`). Only `moqui test-load` reads these; `MOQUI_TEST_SERVER` is the server,
   not a parameter. A variable that names no declared parameter is ignored, where a `--param` that names none is
   refused.
3. The `default`, or `null` when there is none.

Then the value is made the type of its default. For a number default, text that is not a finite number is refused.
For a `true` or `false` default, `true`, `yes` and `1` are true and `false`, `no` and `0` are false, in any case,
and anything else is refused. For a text default, a number or boolean becomes its text. When the plan declares any
parameter, a run that gives one it does not declare is refused, naming those it has. A plan that declares none takes
whatever the run gives.

## `scenario(name, options)`

A scenario is a group of users that starts together with the other scenarios, each with its own ramp and its users
numbered from 0. Its settings are defaults: a run overrides them
([Load-Runner](../tools/Load-Runner.md#running-from-the-command)).

| Option | |
| --- | --- |
| `users` | The number of users, a whole number from 1 to 10,000; 1 by default. With `stages` or a `rate` it is the most at once ([Rates and stages](#rates-and-stages)) |
| `duration`, `iterations` | Seconds the scenario repeats for, and iterations per user. At least one is needed (stages give a duration). With both, a user stops at whichever comes first. With a `rate`, `iterations` is the scenario's starts in all |
| `rampUp` | Seconds over which the users start, evenly. With `rate` and no `stages`, the seconds over which the rate rises from 0 |
| `think` | A wait between a user's iterations, not timed: milliseconds, `[min, max]` (uniform) or `{ gaussian: [mean, deviation] }` (clamped at 0). It ends when the scenario's duration does |
| `pause` | A wait of the same forms before each request of the scenario, not timed (JMeter's Gaussian Random Timer for the Gaussian form) |
| `newSession` | `true` starts each iteration on a fresh `http` session, with no cookies or token (JMeter's "clear cookies each iteration") |
| `rate`, `stages` | An arrival rate, and the target over time ([Rates and stages](#rates-and-stages)) |
| `setup(user)` | Once per user before the timed part, such as its login. Its requests are not counted |
| `run(user, iteration)` | Required. Repeated: one call is one iteration. `user` is the user's number from 0, `iteration` its count from 0 |
| `teardown(user)` | Once per user after the timed part |

`this` in `setup`, `run` and `teardown` is an object of the user's own, kept across its iterations. A user stopped
before its start along the ramp runs neither `setup` nor `teardown`. A time in seconds is cut to 30 days, and a
pause to an hour.

**State.** Values do not cross between users: each has its own engine. Keep a user's state in `this`, or in
variables at the top of the file, which each user has its own copy of.

**Files.** `files.read` and `files.list` stay inside the script's component, so a script reads its data
(`test/load/data/customers.csv`) from there.

### Rates and stages

Without `rate` a scenario is closed: each user waits for its answer before its next request, so a slow server lowers
the load on itself.

- **Stages of users.** `stages: [{ duration, users }]` ramps the target users linearly within each stage, from the
  stage before's target (the first from 0), and holds the last one's until `duration` when that is longer. User `u`
  (from 0) starts an iteration only while `u` is under the target; a user above it finishes its iteration and waits,
  untimed and not counted as active. `users` is the most (by default the largest stage's). `rampUp` is not used with
  stages: the first stage is the ramp.
- **An arrival rate.** `rate` is iterations started per second, more than 0 and at most 10,000, and `users` the most
  that run at once (by default the highest rate rounded up, which assumes an iteration takes a second or less: give
  more for longer ones). `stages: [{ duration, rate }]` changes the rate over time the same way. Starts are on a
  schedule from when the scenario's first user is ready, and a free user takes the next one once it is due. With a
  rate, `iterations` is the scenario's starts in all.
- **Dropped starts.** A start no user has taken by the time of the next one (or 25 ms after its own, if that is
  later), or still due when the scenario ends, is dropped: it does not run, and the result counts it under
  `dropped`.

A scenario has 1 to 100 stages, all of users or all of rates, each with a `duration` over 0. A stage's `users` is a
whole number from 0 to 10,000 and its `rate` from 0 to 10,000. `rate` and `stages` together are refused. With
stages and no `duration`, the scenario lasts as long as its stages.

**Coordinated omission.** In a closed model a server that stalls for a second stalls its users too, so the requests
they would have sent in that second are never sent and never timed, and the percentiles look better than what a user
saw. With a rate the schedule keeps going while the server stalls. A request is still timed from its send, but the
first `step` of an iteration is timed from its scheduled start when it started late, and each iteration is also a
sample `<scenario> iteration` (kind `step`) timed from its scheduled start, pauses left out. The wait for a free user
shows in those two, and starts that no user could take show as `dropped`. Read a rate run's latency from the
`<scenario> iteration` row and the dropped count, not from the request rows alone.

## Globals

A script has the globals of a client spec ([JS-Test-Client](JS-Test-Client.md#globals)) and these:

| Global | |
| --- | --- |
| `plan(options)`, `scenario(name, options)` | The top level's declarations, above |
| `params` | The resolved parameters ([`params`](#params)), frozen |
| `step(name, fn)` | Runs `fn`, records its whole time as one sample `name` of kind `step` (a JMeter transaction controller), and returns what `fn` returned. The requests inside still count under their own names; a step is not a request in the totals. A throw in `fn` makes the step an error and is thrown on |
| `pause(ms \| [min, max] \| { gaussian: [mean, deviation] })` | A wait that is not timed. A stop ends it |
| `fail(message)` | Throws, so the iteration ends as an error with the message (`failed` with none), as a failed `assert` does |
| `service.call(name, parameters, { name, expect })` | In a run in a server only: the service in process, as the user who started the run ([Load-Runner, Security](../tools/Load-Runner.md#security)). It returns the out-parameters as plain values, and the sample is named the service, or `name`. An error (the service's messages, a throw, a transaction left open) ends the iteration, unless `expect` lists `"error"`; then the call returns `{ error }` and is not counted as an error. From `moqui test-load` or a client spec it throws, naming `http.post` on the service's REST path |
| `response.urlParam(name)` | A query parameter of the response's final `url`, else of its `location` (a redirect not followed), decoded; null when neither has it. A transition's redirect carries the new record's id this way |

What a script has from the client harness:

| Global | |
| --- | --- |
| `http` | The user's own default session: cookies and the session token of one user. `get`, `post`, `put`, `patch`, `delete`, `options`, `head`, `request`, `raw`, `login`, `logout`, `formLogin`, `cookies`, `clearCookies` and `token` ([HTTP](JS-Test-Client.md#http)). With `newSession` it is a fresh one each iteration |
| `session(opts)`, `login(user, password)`, `logout()` | Another session of the user's, and the REST login and logout on `http` |
| `ws` | WebSocket ([WebSocket](JS-Test-Client.md#websocket)) |
| `hmac` | The System Message HMAC signers |
| `files.list(dir, regex)`, `files.read(path)` | The script's own component, read-only |
| `server` | `{ url, host, port, secure }` of the target |
| `sleep(ms)`, `waitUntil(fn, opts)` | A plain wait, and a poll until a function's value is true |
| `assert` and its methods, `dec`, `sum` | As in a spec ([JS-Testkit](JS-Testkit.md#assert)). A failed `assert` ends the iteration as an error |
| `log.trace`, `debug`, `info`, `warn`, `error` | To the console or the server's log at that level |

What a script does not have:

- `ec`, `call`, `checkData`, `screenTest` and `uploadedFile` throw, each naming the client equivalent, as in a
  client spec. In a run in a server the user's thread has no execution context entered, and `ec` cannot be deleted
  or replaced there.
- `spec`, `feature` and `skip` are defined, but a load script is not a spec: nothing runs a feature it declares.
- In a run in a server the browser (`browser.newPage`, a page, a locator), `parallel`, a nested `load` and
  `response.save` are refused, and the thread may call only the client harness's host functions
  ([Load-Runner, Security](../tools/Load-Runner.md#security)).
- A user's functions run in the user's own engine, so they do use the file's top-level names. This differs from
  `load({ scenario })` in a client spec, whose functions run from their source text and see none of the spec's
  variables.

## Requests, names and errors

Each request of `run` is timed in Rust from send to the last byte of the body, and grouped under its `name` option,
else its method and path without the query (`GET /rest/s1/moqui/basic/enums`; `RAW <target>` for `http.raw`). Give
the same names on two servers and Compare lines them up: OrderToCash names each step of the walk the same on MR
Moqui and on Java.

An error is a response of 400 and up, a transport error, or a throw in `run` (a `fail()`, a failed `assert`). With
`expect: [statuses]` on a request, any status outside the list is an error and those in it are not, so a list that
allows a 401 names 200 too if 200 is also fine. An error ends that user's iteration (the request throws in `run`,
and a `try` around it can catch it) and is counted once; the next iteration starts after the think time. Outside
the timed part, in `setup` and `teardown`, nothing is recorded, and an error still throws.

A load user's requests carry `User-Agent: moqui-load/<version>`, unless the request or its session names another.

## From a client spec

`load({ script: "test/load/Name.load.js", params, scenarios: { name: { users, iterations, ... } } })` runs a script
of the spec's component and returns the result, so an acceptance spec checks that a script still walks its flow
(`mr-marble/acceptance/client/OrderToCash.spec.js`). Its settings over the script's are in
[JS-Test-Client, Load and parallel](JS-Test-Client.md#load-and-parallel).

## Pitfalls

- A request at the top level throws. Put a login in `setup`, and what every iteration needs in `run`.
- `params` values have the types of their defaults: `{ pdf: { default: true } }` is a boolean in the script whatever
  text the run gave, and a parameter with no default is text.
- `iterations` is each user's count in a closed scenario and the scenario's starts in all with a rate.
- `think`, `pause` and `sleep` are in milliseconds; `duration`, `rampUp` and a stage's `duration` are in seconds.
- `expect: [...]` replaces the 400-and-up rule for that request: list every status that is not an error.
- A `step` is not a request: the totals and `rps` count the requests inside it.
- A load against a server with the demo data is answered 429 by its tarpits; run the server on
  `conf/MoquiLoadTestConf.xml` ([Load-Runner, Notes on measuring](../tools/Load-Runner.md#notes-on-measuring)).
- A REST logout ends every session of that user, and an account locks on its fourth failed login
  ([JS-Test-Client, Pitfalls](JS-Test-Client.md#pitfalls)): do not log out in `teardown` a user the others share.
