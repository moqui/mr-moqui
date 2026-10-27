# Client test harness: `moqui test-client`

> Archived 2026-10-27: the client test harness and HTTP security-test migration are complete (125 Java proofs ported, 18 not applicable, none pending). Current usage is in [Client-Testing](../../docs/Client-Testing.md); coverage is catalogued in [Security proof tests](../../docs/SECURITY_TESTS.md). Optional extensions are tracked in [Remaining-Work, Tests](../Remaining-Work.md#tests).

Oct 24, 2026 · Built the same day: P1 to P6 (§11), with the implementation notes in "Built so far" at the end of §11.
The guide is [docs/Client-Testing.md](../../docs/Client-Testing.md). The user answered the first questions the same day: the server a suite needs comes from a `--serve` child process (D9), the command is in every
package (D12), and all of mr-marble's Node and Python tests move to Moqui (§9).

`moqui test` ([docs/Testing.md](../../docs/Testing.md), [TestHarness](TestHarness.md)) runs a component's specs
inside the framework: the services' engine, a scratch database, `ec`. Tests from outside, over HTTP and in a browser,
have no place there, so they were written on other stacks. mr-marble has a Playwright test with npm and a lock
file, and two Node scripts that drive Chromium over CDP. Moqui Java's security proofs over HTTP are a pytest suite (143
tests; requests and websocket-client in a venv; `J/pytest.sh`). Each is a second toolchain beside the framework's own,
and none of them runs from `./moq`.

This plan adds the client side to the testkit. `moqui test-client` runs the spec files in a component's `test/client/`
against a server that is already running. They use the same format as `moqui test`: `spec`, `feature`, the blocks,
`where`, power-asserted conditions, the suite order, the console report and JUnit. They reach the server over HTTP,
WebSocket and headless Chromium, and there is a load runner. The command opens no database, binds no port and writes
nothing the server owns, so it can run beside a server in the same runtime directory or on another machine. The server
is `http://localhost:8080` unless `--server` names another.

Three first users set the requirements:

1. **mr-marble** (§9): the Playwright `random-screen.spec.mjs`, the CDP page smoke, the `node:test`
   `order-role-filters.test.mjs` and the Python `screen-report.py`. Afterwards the component has no npm, Node or Python.
2. **The Java security suite** (§7): a first portion of `J/framework/test` (33 of its 143 tests). It covers every
   pattern the rest needs (anonymous access, sessions and the session token, headers and cookies, CORS, redirects,
   Basic and key headers, raw paths, WebSocket, fixture users), with a catalog of the rest for the full migration.
3. **A basic load test of the framework** (§8): the framework's own screens and REST, and nothing from add-on
   components.

Reviewed for it:

- the testkit (`core/testkit/`), `moqui test` (`host/test_cmd/`) and its guide;
- the script engine's setup (`core/script/runtime.rs`);
- `moqui tui --server`'s HTTP and WebSocket client (`host/tui_remote.rs`);
- the PDF engine's Chromium (`host/pdf.rs`, `host/pdf/cdp.rs`);
- the browser test driver (`host-tests/common/cdp.rs`) and `host-tests/gate3_browser.rs`;
- the speed tests (`host-tests/speed/`, [Profiling](../Profiling.md));
- the session token, CORS and login code (`core/screen/mod.rs`, `core/screen/render.rs`, `webroot/rest.xml`,
  `webroot/Login.xml`);
- every file of `MRSS/test/`;
- `J/framework/test/` with `J/SECURITY_TESTS.md` and `SecurityTestSupport.groovy`.

Paths: `core/` is `crates/moqui-core/src/`, `host/` is `crates/moqui-host/src/`, `host-tests/` is
`crates/moqui-host/tests/`, `fw/` is `runtime/base-component/framework/`, `webroot/` is
`runtime/base-component/webroot/screen/webroot/`, `MRSS/` is `runtime/component/mr-marble/`, and `J/` is the
Moqui Java checkout (`~/work/moquiwork/moqui`). Every other path is relative to this repository.

---

## 0. Decisions

| # | Decision | Why |
| --- | --- | --- |
| D1 | The same spec format, runner and report as `moqui test`. Client specs live in `<component>/test/client/*.spec.js`, with an optional `test/client/suite.js`. `moqui test` reads only the top level of `test/` (`host/test_cmd/mod.rs:209`), so the two sets never mix. | One way to write a test. The rewrite, the `where` rows, the `dependsOn` closure, the suite order, the console lines and JUnit are already built. |
| D2 | A pure client. No execution context factory, no database, no listener, no conf, no `runtime/.env`, no `runtime/log/moqui.log`, no service workers. The script engine starts on a thread with no execution context entered. `ec`, `call`, `checkData`, `screenTest` and `uploadedFile` are absent, and using one is an error that names the client equivalent. It reads spec files and its components' files, and writes only its output directory. | Asked for: it runs beside a server in the same directory and from another machine. Any boot reads and may write `.env`, appends to `moqui.log`, and starts or adopts the embedded postmaster (`host/db/embedded.rs` `start_with`). |
| D3 | A synchronous API with Playwright's names and no `await`: `page.goto`, `locator`, `getByRole`, `fill`, `click`, `waitForURL`, `screenshot`. A call blocks until done, and conditions are plain values, power-asserted as in server specs. | The engine drains no job queue, so an async result never settles in a spec. Blocking calls are how services are written. Playwright's names make porting easy for people and agents. |
| D4 | HTTP is a blocking `reqwest` client shared by the run (pooled; no redirects unless asked), with a cookie jar and session-token tracking of its own per session. It does not use `moqui_lib::http::RestClient`, whose outbound policy refuses internal addresses. Raw requests go over a TCP or TLS stream. WebSocket is `tungstenite`, as in `tui_remote`. | No new dependencies. Security tests need the raw `Set-Cookie` attributes, `Location` with redirects off, OPTIONS, Basic, paths that are not normalized, and a forged `Host`. |
| D5 | The browser is headless Chromium over CDP. The synchronous driver of `host-tests/common/cdp.rs` moves into the host as `host/test_client/cdp.rs`, returning errors instead of panicking, and `gate3_browser` uses it from there. Chromium is found as the PDF engine finds it (`pdf::find_chromium`, moved to a module both use), or by `--chromium`. There is one Chromium per spec file and a browser context per page. No Playwright, no WebDriver. | The driver exists (493 lines), and `gate3_browser` (4,515 lines) proves it on the `/qapps` shell. Chromium is already the framework's PDF engine, with `./moq chromium-download`. |
| D6 | Moqui-aware waits and problems. Actions wait for their element to be attached, visible, enabled and stable. `goto` and the actions then wait for the Moqui client to settle. A feature fails on *page problems*, as a server feature fails on `ec.message` errors, unless `allowPageProblems` is set. Page problems are uncaught exceptions, console errors, responses of 400 and up, failed loads, render-error text and error notifications. | Playwright tests write all of this by hand each time: `random-screen.spec.mjs:28-30` has three `page.on` handlers, then regular expressions over the body. `gate3_browser`'s `take_problems` and `assert_clean` do the same. |
| D7 | Load is `load({users, duration, rampUp, think, setup, scenario, args})` in a feature block. Each virtual user is a thread with its own engine and session, running the scenario's source as `parallel` runs its function. Rust times each request and returns statistics for `then` to check. The numbers are also printed and written as JSON. It is a closed model in one process. | A load test is a feature whose thresholds are conditions. There is no k6, locust or oha stack. `parallel` already proves functions copied as source onto threads (`core/testkit/run.rs:511`). |
| D8 | A host-call bridge. The testkit in moqui-core gets a `SpecHost`: a prelude of its own, named host calls with JSON in and out, and hooks after each case and spec. The drivers live in moqui-host (`host/test_client/`). The JavaScript API is a client prelude over the bridge. | rquickjs stays in moqui-core, and the network and browser code stays in the host beside `tui_remote` and the PDF engine. The JSON of one call is noise next to an HTTP round trip. |
| D9 | `--serve`. A client suite may declare the server it needs in `test/client/suite.js`: components, server specs as fixtures, and a conf. `moqui test-client --serve` starts that as a child `moqui test ... --serve` on a scratch runtime, waits for it, runs the specs, then stops it. `moqui test --serve [address]` runs its specs, then serves the scratch database until it is stopped. `<component>/test/MoquiConf.xml` merges into test runs only. | Answered (question 1). mr-marble needs Mantle flow data, a mount and its user's access; the security suite needs fixture users. The client process itself still touches no database and no port. |
| D10 | Fixtures are server specs. Data a client suite needs (the `sec.*` users, the load user, grants) is made by a `test/*.spec.js` of the same component, which `--serve` runs. Java's `SecurityTestSupport.ensureUsers` ran in a Spock `setupSpec` the same way. For a server that `--serve` did not start, the operator runs that spec on the server's database (`moqui test <c> --spec <S> --db existing`), and client specs `skip()` when the fixtures are missing, as the Python suite skips. | No test users in `data/`, where a production load could reach them, and no endpoint on the server that loads fixtures. |
| D11 | Parameters come from `--param name=value` and the `MOQUI_TEST_<NAME>` environment variables; the defaults are the demo data's (`john.doe`/`moqui`). Plain `http` is allowed only to loopback unless `--allow-http`. `--insecure` accepts an invalid certificate, with a WARN. | The Playwright tests read `MRSS_*` variables. A password on the command line shows in `ps`. The loopback rule is `tui --server`'s (`host/tui_remote.rs:88-107`). |
| D12 | In every package, as the `test-client` feature in Lite and Enterprise (Dev and Maximal include those). | Answered (question 2). It adds no dependency: `reqwest` is always in the host, and `tungstenite` comes with `tui`. A CI box with any binary can test any server. |
| D13 | Specs get no process or shell API. `files.list` and `files.read` are read-only and confined to the spec's own component directory. `random-screen.spec.mjs`'s `git diff` sample becomes every candidate Find screen as a `where` row, and `--param screen=` picks one. | A component's test should not run programs on the tester's machine. A random sample makes a failure unrepeatable. |
| D14 | Failure tolerance as in [AGENTS.md](../../AGENTS.md). These failures are each reported, naming what failed, and the run goes on: a spec that does not load, a feature that fails, a virtual user whose iteration throws, a page whose screenshot fails, and a `--serve` child that does not start (its suite only). | The repository's rule. |

## 1. What it looks like

```sh
./moq run                                            # a dev server, in one terminal
./moq test-client framework                          # the framework's client specs against http://localhost:8080
moqui test-client framework --serve                  # start the server the suite declares, run, stop it
moqui test-client mr-marble --serve --junit build/test-results
moqui test-client framework --spec LoadBasic --param users=50 --param duration=60
moqui test-client mr-marble --spec FindScreens --param screen=Survey/FindSurvey --headed
moqui test-client framework --server https://test.example.com --insecure    # from another machine
```

An HTTP spec (`fw/test/client/SecurityAuthn.spec.js`, part of §7):

```js
spec({ setupSpec() { login(params.user ?? "john.doe", params.password ?? "moqui") } })

// test_a07_authn.py: post_with_csrf_token_executes_preference_round_trip, post_without_csrf_does_not_store_preference
feature("A07 a POST runs with the session token and stores nothing without it (csrf: #csrf)", {
    where: [{ csrf: true, status: 200, stored: true }, { csrf: false, status: 401, stored: false }],
    when({ csrf }) {
        const key = `sec.test.${Date.now()}`
        const res = http.post("/apps/setPreference", { form: { preferenceKey: key, preferenceValue: "v1" }, csrf })
        const prefs = http.get("/apps/getPreferences", { params: { keyRegexp: key } }).json()
        return { res, prefs, key }
    },
    then({ res, status, prefs, key, stored }) {
        res.status == status
        !!prefs[key] == stored
    },
})

feature("A07 an anonymous session gets a session cookie that is HttpOnly and SameSite=Lax", {
    expect() {
        const cookie = session().get("/Login").cookie("moqui.session")
        cookie.httpOnly
        cookie.sameSite == "Lax"
    },
})
```

A browser spec (`MRSS/test/client/BrowserSmoke.spec.js`, §9):

```js
spec({ setupSpec() { login("john.doe") } })        // the default session; browser pages log in on their own

feature("#path shows #text", {
    where: [
        { path: "/apps/SimpleScreens/Customer/FindCustomer?pseudoId=CustJqp", text: "Joe" },
        { path: "/apps/SimpleScreens/Order/OrderDetail?orderId=55401", text: "Picker Bot 2000" },
        { path: "/apps/my/User/Account", text: "Change Password" },
    ],
    expect({ path, text }) {
        const page = browser.newPage({ user: "john.doe" })
        page.goto(path).status == 200
        page.text().includes(text)
    },                                               // page problems fail the feature unless allowed
})

feature("PrintOrder renders barcodes and prints a PDF", {
    expect() {
        const page = browser.newPage({ user: "john.doe" })
        page.goto("/apps/SimpleScreens/Order/PrintOrder.html-print?orderId=55400")
        page.text().includes("23,795.00")
        page.locator("svg rect").count() > 0
        page.pdf("PrintOrder-55400").magic == "%PDF"
    },
})
```

A load feature (`fw/test/client/LoadBasic.spec.js`, §8):

```js
feature("mixed reads and writes", {
    when() {
        return load({
            users: Number(params.users ?? 4), duration: Number(params.duration ?? 10), rampUp: 2, think: [50, 200],
            args: { password: params.loadPassword ?? "TestLoad1!!" },
            setup(user, args) { login("test.load", args.password) },             // once per user, not timed
            scenario(user, args) {                                                // repeated until the duration ends
                http.get("/rest/s1/moqui/basic/enums", { params: { enumTypeId: "UserPreferenceKey" }, name: "s1 enums" })
                http.get("/apps/system/dashboard.qvt", { name: "dashboard qvt" })
                http.post("/apps/setPreference", { form: { preferenceKey: `test.load.${user}`, preferenceValue: "x" },
                    name: "setPreference" })
            },
        })
    },
    then(stats) {
        stats.errors == 0
        stats.requests > 0
        stats.names["s1 enums"].p95 < Number(params.maxP95 ?? 2000)
    },
})
```

The console has the `moqui test` lines, with a table after a load feature:

```
SecurityAuthn > A07 a POST runs with the session token and stores nothing without it (csrf: false)  PASSED (0.04s)
BrowserSmoke > /apps/SimpleScreens/Order/OrderDetail?orderId=55401 shows Picker Bot 2000  FAILED (3.12s)
LoadBasic > mixed reads and writes  PASSED (10.31s)
    4 users, 10.0 s: 2,412 requests, 241.2/s, 0 errors
    name             count  errors    p50    p95    p99    max
    s1 enums           804       0    3ms    9ms   14ms   31ms
    dashboard qvt      804       0    6ms   18ms   25ms   52ms
    setPreference      804       0    4ms   11ms   17ms   40ms
Output: runtime/tmp/test-client/20261024-153012-48211
```

## 2. Where it starts

| Need | Existing piece |
| --- | --- |
| Spec DSL, rewrite, plan and case driver | `core/testkit/run.rs` (`run_spec` `:154`, `load` `:394`, `block`, `reset_context` `:492`), `core/testkit/rewrite.rs`, `core/testkit/prelude.js` |
| Threads running a function copied as source | `core/testkit/run.rs` `parallel_host` `:511`, `run_parallel`, `run_thread` |
| Missing tool becomes SKIPPED | `core/testkit/run.rs` `missing_tool` `:318` |
| Discovery, suite order, `dependsOn` closure | `host/test_cmd/mod.rs` `discover` `:168`, `component_specs` `:209`, `suite_order`, `select_specs` `:253` |
| Console lines, summary, JUnit | `host/test_cmd/report.rs` `case_line` `:36`, `summary` `:78`, `write_junit` `:224` |
| Scratch runtime, data load, snapshot, boot | `host/test_cmd/fixture.rs` `open` `:85`, `prepare_database` `:193`, `boot` `:279` |
| A server on an embedded cluster at an address | `host/lib.rs` `start_server` `:239` (`start_server_in`, `ListenBind::Address`) |
| Component conf merge | `core/context/conf.rs` `merge_component_confs` `:71` |
| Blocking HTTP client, REST login, session token from response headers | `host/tui_remote.rs` (client `:65-70`, login `:225-267`, cookie and `X-CSRF-Token` `:270-320`) |
| WebSocket client | `host/tui_remote.rs` `/notws` `:358-431` (tungstenite, rustls, webpki-roots) |
| Raw HTTP/1.1 over TCP, cookies, `<meta name="moquiSessionToken">` | `host-tests/common/mod.rs` `:739-833` (`http_request`, `cookies_of`, `session_token`), `sign_in` `:692` |
| CDP driver | `host-tests/common/cdp.rs`: `Browser::launch`, `open_tab`, `navigate`, `eval`, `wait_for`, `click`, `type_text`, `press`, `take_problems` |
| Browser test helpers | `host-tests/gate3_browser.rs` `FETCH_SPY` `:44`, `by_text` `:63`, `assert_clean` `:71`, `sign_in` `:85` |
| Chromium | `host/pdf.rs` `find_chromium` `:83`, `launch_args` `:770`, `devtools_url` `:796`; `./moq chromium-download` |
| Session token check and delivery | `core/screen/render.rs` `csrf_rejected` `:2408`; `core/screen/mod.rs` `attach_session` `:610-634` (headers `moquiSessionToken`, `X-CSRF-Token`); `runtime/base-component/webroot/screen/includes/Header.html.fancy:11` (meta) |
| JSON without a browser | `renderMode=tui` (`crates/moqui-tui/src/model.rs`), each screen's `actions` transition (`core/screen/definition.rs:310`), `/rest/s1`, `/rest/e1` |
| Settle polling for the Moqui client | `runtime/base-component/example/test/client/GalleryShots.spec.js` (`.q-page`, loading bars, `moqui.webrootVue.loading`) |

## 3. The command

```
moqui test-client [component | path ...] [--server URL | --serve] [--spec A,B] [--feature text]
    [--param name=value ...] [--junit dir] [--out dir] [--wait seconds] [--timeout seconds]
    [--chromium path] [--headed] [--insecure] [--allow-http] [--fail-on-skip] [--log-level level]
```

| Option | |
| --- | --- |
| `component` | Every component with `test/client/*.spec.js` runs when none is named. Order: `base-component`, then `component`, then `dev-component`, by directory name within each, following `component.xml` `depends-on` (no factory, so no load order of a booted server). |
| `path` | A `test/client` directory or a `.spec.js` file. Its component is the directory above `test/`. Use it on a machine that has the test files but no runtime. |
| `--server URL` | `http://localhost:8080` by default, or `MOQUI_TEST_SERVER`. `./moq test-client -p 8081` gives `http://localhost:8081`. |
| `--serve` | Start each suite's declared server as a child process (§6) and run against it. |
| `--spec`, `--feature` | As for `moqui test`: `--spec` takes exact names, comma-separated. |
| `--param name=value` | Repeatable. Read in specs as `params.name`, also from `MOQUI_TEST_<NAME>`. |
| `--junit dir` | `TEST-<component>.client.<Spec>.xml`, with attachments as `[[ATTACHMENT\|path]]` lines in `system-out`. |
| `--out dir` | Screenshots, PDFs, failure dumps and load JSON. The default is `runtime/tmp/test-client/<yyyyMMdd-HHmmss>-<pid>/`, or `./test-client-out/<run>/` without a runtime. |
| `--wait seconds` | Poll until the server answers, for CI that starts the server in the background. |
| `--timeout seconds` | The default time an action waits (10). The spec and feature `timeout` still bound each block. |
| `--chromium path` | Also `MOQUI_CHROMIUM`. Otherwise `pdf::find_chromium`'s order (`runtime/chromium` from `./moq chromium-download`, then `chromium` and `chromium-browser` on the PATH), then `google-chrome`, as the browser tests do. |
| `--headed` | A visible browser, for debugging. |
| `--insecure`, `--allow-http` | D11. |
| `--fail-on-skip` | A skipped spec or feature fails the run. For CI, so that a missing Chromium or fixture does not pass as green. |
| `--log-level` | Console only, `warn` by default. |

Steps:

1. Install console logging and parse the options.
2. Discover the specs as above.
3. Check the server: `GET /` with redirects off. Any HTTP answer means it is up. A connection error, after `--wait`,
   ends the run with "nothing answers at http://localhost:8080: start a server (./moq run), or pass --server or
   --serve".
4. Run each spec through `run_spec` with the client host (§4). Each spec gets a default session, a browser launched
   on first use, and its sessions, pages and browser closed after `cleanupSpec`.
5. Print the summary and the output directory, and write JUnit. Any failure exits 1.

What it never does: start or adopt a postmaster, connect to a datasource, bind a listener, read the conf, read or
write `runtime/.env`, open `runtime/log/moqui.log`, start service workers, or take a cluster or search role. Its only
writes are under `--out` and `--junit`. Its only listener is the one Chromium opens on an ephemeral loopback port for
DevTools, with a temporary profile that is removed afterwards. The CLI test (§12) checks that `.env`, the log and the
postmaster lock of the runtime are unchanged after a run.

## 4. The runner and the bridge (`core/testkit/`)

- **Prelude split.** `prelude.js` keeps the DSL: `spec`, `feature`, the plan and case driver, the conditions (`__cmp`,
  `__truthy`, `__pa`), `assert`, `dec`, `sum` and `skip`. A new `server.js` takes the server helpers: `call`,
  `checkData`, `login`, `logout`, `screenTest`, `parallel`, `uploadedFile` and `log` over `ec.logger`. The server mode
  installs both, so `moqui test` does not change. The client prelude comes from the host.
- **`SpecHost`.**

  ```rust
  pub trait SpecHost: Send + Sync {
      /// JavaScript evaluated after the core prelude: the mode's globals.
      fn prelude(&self) -> &str;
      /// `__moquiTestHost.call(name, argsJson)`: one named call, JSON in and out, on the calling thread.
      fn call(&self, name: &str, args: &str) -> Result<String, String>;
      /// After each case: problems that fail it (page problems), and files to attach (failure screenshots).
      fn after_case(&self, case: &str, failed: bool, allow_problems: bool) -> CaseExtras;
      /// After each spec: close what it opened.
      fn after_spec(&self);
  }
  ```

  `RunOptions` gets `host: Option<Arc<dyn SpecHost>>`. With a host there is no `ec.message` check and no
  `reset_context`; the host's hooks run instead, and `Failure.details` and a new `attachments` field carry what
  `after_case` returns.
- **`skip(reason)`**, in both modes, throws a marked error that the runner reports as SKIPPED with the reason. In
  `setupSpec` it skips the whole spec. The Python suite skips when its fixtures are missing; Spock has `Assume`.
- **Threads.** `testkit::run_on_thread(host, fn_source, args_json)` starts an engine on a new thread, installs the core
  and client preludes, and calls the function. The client's `parallel` and `load` use it. Server `parallel` stays as it
  is.
- **No execution context.** `with_context` (`core/script/runtime.rs:686`) starts the engine without one, and `ec` is
  installed but answers nothing. The client prelude redefines `ec` (it is configurable) as a getter that throws "`ec`
  is the server's: a client test (`test/client`) runs outside it; use http, ws or browser". The first unit test of P1
  runs a spec under a fake host with no context entered.

## 5. The client API

### 5.1 Globals

| Global | |
| --- | --- |
| `server` | `{ url, host, port, secure }` of the server under test |
| `params` | The `--param` values and `MOQUI_TEST_*` variables, as text |
| `http` | The current session (the spec's default; in `load` and `parallel`, the thread's own) |
| `session(opts)` | A new session with its own cookie jar and token. `opts`: `headers`, `user` and `password` (logs in) |
| `login(user, password = "moqui")`, `logout()` | On `http`. `login` throws when the login fails or needs a second factor |
| `ws` | WebSocket client (§5.4) |
| `browser` | Headless Chromium (§5.5) |
| `load(opts)`, `parallel(n, fn, args)` | §5.6 |
| `files` | `list(dir, regex)` and `read(path)`, within the spec's component directory, read-only |
| `skip(reason)`, `waitUntil(fn, { timeout, interval })`, `sleep(ms)` | Skip; poll a function until it is truthy, else throw (`timeout` in seconds, `interval` in milliseconds); pause |
| `assert`, `dec`, `sum`, `log` | As in server specs; `log` goes to the console at its level |

### 5.2 HTTP

`http.get(path, opts)`, `post`, `put`, `patch`, `delete`, `options`, `head` and `request(method, path, opts)`. A
`path` is relative to `server.url`; a full URL is taken as it is.

| Request option | |
| --- | --- |
| `params` | Query parameters |
| `form`, `json`, `body`, `multipart` | The body: URL-encoded, JSON, text, or parts (`{ name: value }` or `{ name: { fileName, text, contentType } }`; `base64` for bytes) |
| `headers` | Request headers; a `Host` given here is sent |
| `follow` | Follow redirects (up to 10). Off by default: a test sees each 3xx and its `Location` |
| `csrf` | Session token: automatic by default, `false` to leave it out, or a text to send instead |
| `basic: [user, password]`, `apiKey`, `loginKey` | The header logins of `core/screen/mod.rs:646-697` |
| `accept: "json" \| "html"` | `Accept` shortcuts |
| `timeout`, `name` | Seconds; and the name a load statistic groups under (default: method and path without the query) |

The response has `status`, `ok`, `url`, `location`, `contentType`, `timeMs`, `length`, `text`, `json()`,
`header(name)` and `headers(name)` (every value, as for `Set-Cookie`), `cookies` (each `Set-Cookie` parsed: `name`,
`value`, `httpOnly`, `secure`, `sameSite`, `path`, `maxAge`, `expires`), `cookie(name)`, `magic` (the first bytes as
text, such as `%PDF`, or hex), `assertContains(text)` and `assertNotContains(text)`, and `save(name)` (the body into the
output directory).

The **session token** is taken from the `moquiSessionToken` and `X-CSRF-Token` response headers, which are sent when a
session is new or its token changes. It is also read from an HTML page's `<meta name="moquiSessionToken">` or its
`/qapps` `moqui-config` JSON. The session sends `X-CSRF-Token` on POST, PUT, PATCH and DELETE when it has a token.

The **cookie jar** keeps cookies by name, domain and path, with expiry: `http.cookies()`, `http.clearCookies()`.

**Logins.** `login` posts `/rest/login`, the API login, which needs no session token. `http.formLogin(user, password,
{ returnTo, headers })` gets `/Login` for its cookie, then posts `/Login/login` with redirects off and returns that
response, for the redirect proofs.

The body is kept up to 16 MiB; `length` is the full size.

### 5.3 Raw requests

`http.raw(text, { timeout })` sends the request text over TCP, or TLS for `https`, as it is, with the session's
`Cookie` header added unless `cookies: false`, and parses the answer into a response. It serves paths that must not be
normalized (`/js/../../Login.xml`) and malformed requests. Built from the test client in `host-tests/common/mod.rs`.

### 5.4 WebSocket

`ws.connect(path, { session, headers, origin, timeout })` returns `{ status, headers, open, send(text),
receive({ timeout }), close() }`. `receive` returns the next text frame, or `null` on timeout (seconds) or close. A refused
handshake does not throw: `open` is false and `status` is the HTTP answer, such as 401 or 403, because the security
proofs check refusals. The session's cookies go in the handshake.

### 5.5 Browser

`browser.newPage({ user, password, viewport: { width, height }, colorScheme, timeout })` opens a page in a browser
context of its own, so each page has its own cookies. With `user`, it logs in through the form at `/Login` (the
`sign_in` of `gate3_browser`). The default viewport is 1440 × 1000.

| Page | |
| --- | --- |
| `goto(path)` | Navigates, waits for the load event, then settles. Returns `{ status, url }` of the document response |
| `url()`, `title()`, `text()`, `html()` | The current URL, the title, `document.body.innerText`, the HTML |
| `locator(css)`, `getByRole(role, { name, exact })`, `getByText(text \| regex)`, `getByLabel(text)` | Locators, resolved again on each use |
| `fill(css, value)`, `click(css)`, `press(key)`, `keyboard.press(key)` | Shortcuts for the locator actions |
| `evaluate(fnOrSource, arg)` | Runs in the page; the answer comes back as JSON. A function goes as its source, so it cannot use the spec's variables |
| `waitFor(fnOrSource, { timeout })`, `waitForURL(text \| regex \| fn)` | Poll in the page until truthy, or until the URL matches. Both look at the current state first, so a navigation that already happened counts |
| `settle()` | Wait for the Moqui client to go idle (below). `goto` and the actions call it |
| `screenshot(name, { fullPage })`, `pdf(name, { landscape })` | Into the output directory. They return the path, and `{ path, size, magic }` |
| `problems()`, `takeProblems()`, `allowProblems(regex)` | The page's problems so far; take them so they do not fail the feature; ignore the matching ones |
| `setViewport(width, height)`, `close()` | |

| Locator | |
| --- | --- |
| `click()`, `fill(value)`, `press(key)`, `hover()`, `focus()` | These wait (attached, visible, enabled, a stable box), scroll into view, act through `Input` events as the driver's `click` and `type_text` do, then settle |
| `text()`, `value()`, `attribute(name)`, `isVisible()`, `isEnabled()`, `isEditable()`, `count()`, `boundingBox()` | Read at once, without waiting, as in Playwright |
| `waitFor({ state, timeout })` | `visible` (the default), `hidden`, `attached` or `detached`. Returns true; on timeout it throws with the page text and its problems |
| `locator(css)`, `getByRole(...)`, `getByText(...)`, `first()`, `last()`, `nth(i)`, `evaluate(fn)` | Scoping, and a function of the element |

Roles cover what the Moqui screens use: `button` (`button`, `[role=button]`, submit inputs; a Quasar `q-btn` renders
a button), `link`, `dialog`, `textbox`, `checkbox`, `tab`, `menuitem` and `heading`. The accessible name is
`aria-label`, then `aria-labelledby`, then the text. The locate script is injected into each new document.

**Settling.** The page is settled when the Moqui client is idle for 100 ms: no load pending in `moqui.webrootVue`, no
`.q-loading-bar--visible` or `.q-inner-loading`, and no fetch in flight. A fetch counter is installed on each new
document, as `FETCH_SPY` is. A page that is not the Moqui client is settled at its load event.

**Page problems** are:

- uncaught exceptions (`Runtime.exceptionThrown`);
- console errors and failed asserts (`Runtime.consoleAPICalled`, `Log.entryAdded`);
- responses of 400 and up, and failed loads other than cancelled ones (`Network.responseReceived`, `loadingFailed`);
- after a settle: `Error rendering` or `Exception in render` in the text, and the client's error notifications.

Warnings are not problems unless a page is opened with `problems: { warnings: true }`. A feature fails when a page has
problems at its end, unless the feature or spec has `allowPageProblems: true`. This is the browser counterpart of
`allowMessageErrors`.

**On a failure**, each open page leaves `failed-<feature>-<n>.png` and `.txt` (its text and its problems) in the
output directory, and the failure report and JUnit list them.

From Playwright:

| Playwright | Here |
| --- | --- |
| `await page.goto(u)` | `page.goto(u)` (also settles) |
| `await expect(loc).toBeVisible()` | `loc.waitFor()` as a condition |
| `await expect(loc).not.toBeVisible()` | `loc.waitFor({ state: "hidden" })` |
| `await expect(loc).toBeEditable()` | `loc.isEditable()` |
| `await expect(page).toHaveURL(re)` | `page.waitForURL(re)` |
| `await expect(loc).not.toContainText(re)` | `!re.test(loc.text())` |
| `page.on("pageerror" \| "console" \| "response")` collecting into an array | Built in: page problems |
| `Promise.all([page.waitForURL(f), form.requestSubmit()])` | `form.evaluate(f => f.requestSubmit()); page.waitForURL(f)` |
| `testInfo.attach`, `screenshot: "only-on-failure"` | `page.screenshot(name)`; failures attach on their own |

### 5.6 Load and parallel

`load(opts)` runs `users` virtual users, each a thread with its own engine, client prelude and default session.

| Option | |
| --- | --- |
| `users` | The number of virtual users |
| `duration` or `iterations` | Seconds, or iterations per user |
| `rampUp` | Seconds over which the users start, evenly |
| `think` | Milliseconds between a user's iterations, or `[min, max]` for a random pause; not timed |
| `setup(user, args)`, `scenario(user, args, iteration)`, `teardown(user, args)` | Each runs from its source text, so it cannot use the spec's variables, as with `parallel` (Testing.md Pitfalls) |
| `args` | Copied as JSON |
| `timeout` | Seconds before the users are stopped (`duration + rampUp + 120` by default) |

Each request is timed in Rust from send to the last byte of the body. Requests are grouped under their `name`. Errors
are counted against the request they happen on: a response of 400 or more (or, when the request has `expect: [...]`,
any status outside that list), a transport error, or a throw in the scenario. The user's iteration ends there and the next one starts.

The result has `users`, `duration`, `iterations`, `requests`, `errors`, `errorRate` and `rps`, plus:

- `latency`: `min`, `mean`, `p50`, `p90`, `p95`, `p99` and `max`, in milliseconds;
- `names`: for each name, those values with `count`, `errors` and `statuses`;
- `errorSamples`: the first 20 errors, with name, status and message.

Percentiles come from every sample, kept and sorted at the end; a million requests is 8 MB. The table goes to the
console after the feature line and into JUnit `system-out`. The whole result goes to
`<out>/load/<component>.<Spec>.<feature>.json`, for comparing runs.

`parallel(n, fn, args, { timeout })` is the same machinery run once per thread, returning `{ results, errors }`. It is meant for
races such as simultaneous logins.

Scale: a thread and an engine per user, so a few hundred users per process. Beyond that, run several client processes
(D7; merged reports later, §15).

## 6. Serving a suite: `--serve`

A suite declares its server in `test/client/suite.js`, read without running it as `suite_order` reads the order (an
`oxc` parse of literals):

```js
// MRSS/test/client/suite.js
suite(["BrowserSmoke", "FindScreens"], {
    server: { components: ["mr-mantle", "mr-marble"], specs: ["SimpleScreensRender", "ClientAccess"] },
})
// fw/test/client/suite.js
suite(["SecurityAccess", "SecurityHeaders", "SecurityCors", "SecurityAuthn", "SecurityErrors", "SecurityWebSocket",
    "LoadBasic"], {
    server: { components: ["framework"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
```

- **`moqui test --serve [address]`** (the server side, so the `embedded-postgres` build, as `moqui test` is). It runs
  the specs as usual on a scratch runtime (`--db fresh` or `snapshot`). If any fail, it stops there with the list.
  Otherwise it shuts the test factory down and starts a server on the same scratch runtime and cluster with
  `start_server_in` (`host/lib.rs`), on the address (`127.0.0.1:0` by default, with the cluster's HTTPS port 0 as the
  tests use). It prints `Serving http://127.0.0.1:41733 (Ctrl-C to stop)`, writes the URL to `--ready-file` when
  given, and serves until SIGINT or SIGTERM, or until its standard input closes. It then stops and removes the scratch
  runtime unless `--keep`. It is also for use by hand: `moqui test mr-mantle mr-marble --spec SimpleScreensRender
  --serve 127.0.0.1:18081`.
- **`moqui test-client --serve`** groups the selected components by their declared server. For each group it starts
  the current executable as `moqui test <components> --spec <specs> --db snapshot --serve 127.0.0.1:0 --ready-file
  <tmp>`, with `MOQUI_CONF` set to the declared conf, and a pipe as the child's standard input. That way the child
  also stops when the client dies without cleaning up. It waits for the ready file, then runs that group's specs with
  `--server` set to the URL, then closes the pipe and waits for the child to exit. A child that fails to start fails
  its group's specs with its last log lines, and the other groups still run (D14).
- **`<component>/test/MoquiConf.xml`** is merged after the component's own `MoquiConf.xml`, in `moqui test` runs only
  (in-server specs and `--serve`), through a boot option of `merge_component_confs`. mr-marble mounts
  `apps/SimpleScreens` there; its `MoquiConf.xml:11-14` has the mount commented out.
- **`--spec` across components.** A `dependsOn` name that the spec's own component lacks is looked up in the run's
  other components. Today `select_specs` errors on it (`host/test_cmd/mod.rs:253`), and `SimpleScreensRender` depends
  on eight mr-mantle specs. This makes `server.specs` and the hand-run command above work.
- **Fixtures** (D10) are server specs that also assert what they made. One example is `fw/test/ClientFixtures.spec.js`
  (§7, §8): it creates the users with `create#UserAccount`, `UserGroupMember` and `ArtifactAuthz` as
  `SecurityTestSupport.ensureUsers` did, then checks that each can log in. They also run in a plain `moqui test`, on its
  scratch database.

## 7. The security suite: a first portion

The specs go in `fw/test/client/`, beside where Java keeps them (`J/framework/test`), with a `README.md` catalog: each
of the 143 Python tests with its spec feature, `pending`, or `not applicable` with the reason. Feature names start with
the OWASP id, and a comment names the Python test, so the catalog can be checked by grep.

**Fixtures** (`fw/test/ClientFixtures.spec.js`): `sec.view.only` (VIEW on `TOOLS_APP` and `SYSTEM_APP`),
`sec.none.only` (no authz), and the load user `test.load` (§8), with Java's passwords. Further `sec.*` users are added
as their tests are ported.

**The conf gate.** The CORS proofs need a server that allows no foreign origin. The dev conf sets
`webapp_allow_origins=*`, and production is the default conf's empty list. `SecurityCors`'s `setupSpec` sends an
`Origin: https://evil.example` and *fails* the spec (as `require_production_conf` fails the Python suite) with "this
server allows every origin (the dev conf): run against MoquiProductionConf.xml, as --serve does". The other specs pass
on any conf, so they run against `./moq run` too.

| Spec | Features (Python test in `J/framework/test`) | Needs |
| --- | --- | --- |
| `SecurityAccess` (A01) | Tools without a session is not the tool (`test_a01_access` `tools_without_session`). `/rest/e1` and `/rest/s1` anonymous are 401 (`rest_e1_…`, `rest_s1_…`). `/rest/api_key`, `/rest/moquiSessionToken` and `/rest/userInfo` are 404, as a `where` table (`removed_rest_api_key`, `removed_rest_moqui_session_token`, `test_api_rest` `rest_userInfo_transition_is_gone`). `/apps/getPreferences` anonymous is 401 (`test_a01_more` `apps_get_preferences_…`). `/js/../../Login.xml` does not serve screen XML (`static_path_dotdot_…`). `sec.none.only` cannot open Tools (`none_user_cannot_open_tools`). `sec.view.only` cannot open SQL Runner (`view_only_cannot_open_sql_runner`). `returnTo` off-site is refused and same-origin is allowed (`test_a01_redirect` ×2) | redirects off, raw path, fixtures, form login |
| `SecurityHeaders` (A02) | CSP with `frame-ancestors` and `form-action`, `X-Frame-Options`, `nosniff`. The session cookie is HttpOnly and SameSite=Lax. Login HTML is not publicly cacheable. No `Origin` gives no `Access-Control-Allow-Origin` (`test_a02_headers` ×5) | `Set-Cookie` attributes |
| `SecurityCors` (A02) | An unknown origin is 401, and so is its preflight. The same origin gets `Access-Control-Allow-Origin` with credentials. A same-origin preflight is allowed (`test_a02_headers` ×4) | OPTIONS, the production conf |
| `SecurityAuthn` (A07) | REST login needs no session token, and an unknown user is 401 with the shared text (`test_a07_authn` `rest_login_…`, `test_a07_more` `rest_login_unknown_user_is_401`). A POST with the session token runs and without it stores nothing (×2). Login issues a new session id. `api_key` in the query, or a garbage `api_key` header, does not log in (×2). Basic on `/rest/e1`: an unknown user is 401, a valid one is not (×2) | session token, Basic, `api_key` |
| `SecurityErrors` (A10) | A REST 401 in JSON has no stack frames (`test_a10_errors` `rest_unauth_json_…`) | `Accept` |
| `SecurityWebSocket` (A01) | `/notws` with a foreign `Origin` is refused (`test_a01_ws` `websocket_evil_origin_is_rejected`). An anonymous upgrade is 401 (replaces `notws_anonymous_upgrade_accepts_or_closes`) | WebSocket |

That is 33 Python tests in about 28 features, and it covers each pattern the remaining 110 need, except multipart, HMAC and
timing. Those three are named in the catalog and arrive with their tests.

**Differences from Java** (in the catalog; each one recorded in [Parity-Diffs](../Parity-Diffs.md) if it is not already
there):

- A preflight answers 204, not 200.
- The session cookie is `moqui.session`, not `JSESSIONID`.
- The session-token failure is 401 "Session token is missing or is not valid" (`core/screen/render.rs:1424`).
- An anonymous `/notws` upgrade is 401; Java accepts it and ignores the null user.
- MR-Moqui locks an account on the fourth failed login. No proof uses a wrong password for `john.doe`: they use an
  unknown user.

**Before porting `test_login_returnTo_does_not_follow_host_header`:** `webroot/Login.xml:135` compares `returnTo` with
`ec.web.getHostName(true)`, which comes from the request's `Host`. If the proof fails, it is a finding: fix it first,
following the process of `J/SECURITY_TESTS.md` (findings stay private until fixed), and land the proof with the fix.

**Not applicable here** (the catalog says why):

- the RPC tests: JSON-RPC was left out on purpose, and `/rpc/json` is 404;
- `/groovysh`: excluded;
- `/elastic` and `/kibana`: their proofs port to `/search` and `/dashboard`;
- the swagger schema tests: they answer 404 before any auth check;
- the H2 console: there is none (a "not mounted" 404 proof is kept).

The Selenium smokes beside the suite (`openui_*`, `llm_admin_*`, `vue_sfc_*`) and the A2A interop script are Java
Assist checks and are out of scope.

## 8. The load test

`fw/test/client/LoadBasic.spec.js`, on the framework's own screens and REST only. It runs as user `test.load`, which
is made by `ClientFixtures`:

- VIEW on the System app;
- the `MOQUI_API` group (`runtime/base-component/tools/data/ToolsSecurityData.xml:31`) for `/rest/s1/moqui/...`;
- VIEW on `moqui.basic.Enumeration` for `/rest/e1`;
- no tarpit of its own.

**The tarpit is off for the load test.** The demo data's `ALL_SCREENS` tarpit of 120 hits in 60 s
(`runtime/base-component/webroot/data/WebrootSecurityDemoData.xml:36`) is on `ALL_USERS`, not only `john.doe`. Every
user is in `ALL_USERS` and every matching tarpit applies (the longest lock wins, as in Java), so no row of `test.load`'s
own can exempt it. The framework suite's server therefore runs `conf/MoquiLoadTestConf.xml`: the production conf with
`tarpit-enabled="false"` for `AT_XML_SCREEN`, `AT_XML_SCREEN_TRANS` and `AT_SERVICE`, as Java's load tests do. It still
allows no foreign origin, so the security specs pass on it. A load run against a server of your own needs the same
conf. The tarpit proofs (`test_z_tarpit`), when ported, need a server with the tarpits on.

**Screen and transition hits count against tarpits.** LoadBasic showed in P4 that they did not; they now do, as
Java counts them: the render of a screen (`ScreenDefinition.render`, the target screen when it requires
authentication) and the run of a transition (`TransitionItem.run`, every transition) count, and the frames around
them (the path above the target, always- and pre-actions, menus and URL checks, included screens) do not. A screen
test render does not count (`disableTarpit()`, as Java's `ScreenTestImpl`). The load test's tarpit-off conf is what
keeps it from answering 429.

| Feature | What |
| --- | --- |
| each endpoint answers | One user, once each, with exact checks. `GET /Login` (anonymous), `GET /qapps/system/dashboard` (the shell HTML), `GET /apps/system/dashboard.qvt` (what `moqui.loadComponent` fetches), a System list screen as `renderMode=tui` JSON, `GET /rest/s1/moqui/basic/enums`, `GET /rest/e1/moqui.basic.Enumeration`, and `POST /apps/setPreference` then `GET /apps/getPreferences` |
| logins at once | `parallel(users, ...)`: each thread logs in on its own session and reads one page. All succeed, and the session ids are distinct |
| mixed reads and writes | `load` over the same mix (§1). Zero errors, and each name's p95 under `params.maxP95` (2,000 ms by default, loose so that CI does not flake; the numbers are what matter) |

The defaults (4 users, 10 s) make it a smoke test that every `--serve` run includes. A real run is
`--param users=50 --param duration=120`, against `./moq run` or another server, with the JSON kept for comparison. It
adds the HTTP-level numbers [Profiling](../Profiling.md) lacks (line 195: the speed tests do not cover screen renders).

## 9. mr-marble

| Now | Becomes |
| --- | --- |
| `test/random-screen.spec.mjs` (Playwright, one random `git diff` Find screen) | `test/client/FindScreens.spec.js`. A `where` row for each Find screen with a `header-dialog="true"` form-list and no required parameter (`files.list` and `files.read`, the filters of `random-screen.spec.mjs:13-16`): 32 today, or the one `--param screen=` names. Each row: status 200; the URL; the list's `table[id^=<form-list name>]` with a visible `th`; "Find Options" visible; at most 2 px of horizontal overflow; the table wider than 300 px; a screenshot. A Survey feature opens and closes New Survey (Escape) and submits Find Options' Find |
| the raw CDP page smoke | `test/client/BrowserSmoke.spec.js` (§1): its five pages as `where` rows with their text, page problems failing each row; the organization menu selects Ziziwork Retail and clears it; PrintOrder with SVG barcode bars and a PDF. Its `pdftotext` step was manual and is not ported |
| `test/order-role-filters.test.mjs` (`node:test`, `node:vm`) | `test/OrderRoleFilters.spec.js`, a server spec. It reads `FindOrder.xml` through `ec.resource`, takes the two `econdition ignore` expressions, and evaluates each in the real engine for the ten `orderType` values: 20 `where` rows, with the same expectations |
| `test/screen-report.py` | `test/ScreenReport.spec.js`, a server spec ignored unless the property `mrss_screen_report` names an output file (`./moq test mr-marble --spec ScreenReport -D mrss_screen_report=/tmp/mrss-report.md`). It builds the same table from `getNoRequiredParameterPaths`, the screen files (a directory listing instead of `git ls-files`) and the spec texts, and writes it with `ec.resource`. No more throwaway spec file |
| `package.json`, `package-lock.json`, `playwright.config.mjs`, the `.gitignore` lines | Removed |
| `test/README.md` | Rewritten: `moqui test-client mr-marble --serve`, `--param screen=`, the output directory |

The suite's server (§6): mr-mantle and mr-marble with `SimpleScreensRender`, which brings in the eight Mantle
flows that make orders 55400 and 55401, invoice 55400 and the rest. `test/MoquiConf.xml` mounts `apps/SimpleScreens`.
If `john.doe`'s ADMIN membership does not reach the mr-marble screens, the grant goes in a fixture spec; check when
building (the README's "test-only access"). It did not (ADMIN alone gets 403): the grant is
`test/ClientAccess.spec.js`.

The example component's gallery screenshots are `runtime/base-component/example/test/client/GalleryShots.spec.js` (`moqui test-client example --serve --spec GalleryShots`).
The Marble page smoke is `test/client/BrowserSmoke.spec.js`.

## 10. Code layout

| Where | What |
| --- | --- |
| `core/testkit/prelude.js`, `server.js` (new), `run.rs`, `mod.rs` | The split (§4); `SpecHost`, `RunOptions.host`, `skip`, `run_on_thread`, `attachments` |
| `host/test_client/mod.rs` | `dispatch_test_client`: options, discovery (shared with `test_cmd` through a `test/` or `test/client` parameter), server check, run loop, report |
| `host/test_client/prelude.js` | The client JavaScript API over `__moquiTestHost.call` |
| `host/test_client/http.rs`, `raw.rs`, `ws.rs` | Sessions, cookie jar, session token, requests and responses; raw requests; WebSocket |
| `host/test_client/cdp.rs`, `browser.rs`, `locate.js` | The CDP driver moved from `host-tests/common/cdp.rs` (which then re-exports it); pages, locators, settle, problems, artifacts; the locate and settle script injected into pages |
| `host/test_client/load.rs` | Virtual users, ramp-up, think time, samples, statistics, the table and JSON |
| `host/test_client/serve.rs`; `host/test_cmd/mod.rs` `--serve`, `--ready-file` | The child process and its pipe; the scratch server |
| `host/chromium.rs` | `find_chromium`, `launch_args` and `devtools_url` from `pdf.rs`, without the `pdf` feature gate |
| `host/lib.rs`, `host/usage.rs` | The `test-client` command arm and its usage text |
| `crates/moqui-host/Cargo.toml` | Feature `test-client` (tungstenite, webpki-roots), in `lite` and `enterprise` |
| `moq` | Task `test-client` (`-p` gives `--server`; `--serve` passes through), its help, and component completion from `*/test/client/*.spec.js` |
| `docs/Client-Testing.md` (new), `docs/Testing.md`, `docs/Packaging.md`, `README.md` | The guide; a pointer from Testing; the feature |

## 11. Phases

| Phase | What | Done when |
| --- | --- | --- |
| P1 Runner and HTTP | §4; the command (§3) with discovery, the server check, `params`, `skip` and the report; HTTP, sessions, cookies, the session token, `login`, `formLogin`, `raw`, `ws`; the `moq` task; the feature in every package | The test component's client specs (§12) pass against a test server; a failing one exits 1 with its line; JUnit written; `.env`, the log and the postmaster lock untouched |
| P2 Serving | `moqui test --serve`, `--ready-file`, the `test/MoquiConf.xml` overlay, `dependsOn` across components, `test-client --serve` | `moqui test-client <test component> --serve` starts, runs and stops its server; killing the client stops the child |
| P3 Security portion | `ClientFixtures`, the six specs of §7, the catalog README, the Parity-Diffs lines | `moqui test-client framework --serve` passes the six Security specs; `SecurityCors` fails clearly against `./moq run` (the dev conf) |
| P4 Load | `load`, client `parallel`, statistics, the table and JSON; `LoadBasic` | It passes in `--serve`; a 50-user run against `./moq run` in the same runtime directory completes with numbers |
| P5 Browser and mr-marble | The driver moved (with `gate3_browser` still passing on it), pages, locators, settle, problems, artifacts; §9 | `moqui test-client mr-marble --serve` passes with screenshots; the npm, Playwright and Python files are gone; `OrderRoleFilters` and `ScreenReport` run in `moqui test` |
| P6 Docs | `docs/Client-Testing.md`, the pointers, `plans/README.md`, this plan's "Built so far" | Written from what was built |

P1 to P4 need no browser, so the security and load users arrive before the largest piece.

**Built so far** (Oct 24, 2026): P1 to P6, with these differences from the text above.

- **P1, the runner and HTTP** (b2026c76). The testkit prelude is split into `prelude.js` and `server.js`; `SpecHost`,
  `skip`, `run_on_thread` and `Failure.attachments` are in the testkit, and `after_case` takes the case's name and
  whether its problems are allowed. `moqui test-client` with discovery, `--spec`, `--feature`, the server check with
  `--wait`, params, the report, JUnit with the `.client` infix, `--out`, `--fail-on-skip`, the loopback rule with
  `--allow-http` and `--insecure`; HTTP sessions, the jar, the session token, `login`, `logout`, `formLogin`, `raw`,
  `ws`, `files`, `waitUntil` and `sleep`; the `moq` task.
- **P2, serving** (f46cb285). `moqui test --serve [address]` and `--ready-file` (the SSH, HTTPS and cluster ports move
  to free ones unless set; refused with `--db existing`, `--agents` or `--skills`); the `test/MoquiConf.xml` overlay;
  `dependsOn` across components; `suite.js` server declarations; `moqui test-client --serve` with `server-<n>.log` in
  the output directory.
- **P3, the security portion** (c93dfae6). `ClientFixtures` and the six specs: 33 features porting 34 Python tests (the
  33 of §7 and the H2 console 404). The catalog README: 34 ported, 90 pending, 19 not applicable. Parity-Diffs has the
  session cookie name, the session-token text and the anonymous `/notws` 401.
- **P4, load** (ce2da5fb, then 267f152f). `load`, the client's `parallel`, the statistics, the table and the JSON;
  `LoadBasic`. A user's thread and engine start in about 7 ms and take about 1 MB (§14, 2); 50 users for 60 s ran
  86,092 requests at 1,435/s with no errors. The follow-up moved the framework suite's server from
  `MoquiProductionConf.xml` to `MoquiLoadTestConf.xml`, the production conf with the tarpits off (§8).
- **P5, the browser** (1fdf4ddb; mr-marble 0ad5a9f). The CDP driver in `host/test_client/cdp.rs` with
  `host-tests/common/cdp.rs` a panicking wrapper over it; `host/chromium.rs`; pages, locators, settle, page problems,
  failure files; `--chromium` and `--headed`; the opt-in `host-tests/client_browser.rs`. mr-marble has no npm,
  Node or Python: `FindScreens`, `BrowserSmoke`, `OrderRoleFilters`, `ScreenReport`, and `ClientAccess`, the test-only
  grant of §9. `moqui test-client mr-marble --serve` passes 40 of 40, and FindScreens passed 20 runs in a row
  (§14, 1).
- **P6, the docs.** [Client-Testing](../../docs/Client-Testing.md), pointers from Testing, Packaging and the READMEs.
- **Differences.**
  - `--spec` takes exact names, comma-separated, as `moqui test` does; there are no patterns.
  - Timeouts are seconds throughout: `waitUntil`'s `timeout`, a request's, a WebSocket `receive({ timeout })`, and the
    `timeout` that `load` and `parallel` also take (`duration + rampUp + 120` and 120 by default). `--timeout` also
    covers raw requests and the WebSocket connect.
  - `expect: [...]` on a request makes any status outside the list an error, not only 400 and up.
  - Locators are not strict: an action or read uses the first match.
  - With no Chromium found, or a `--chromium` that is not a file, `newPage` skips its feature (or spec, in
    `setupSpec`); `--fail-on-skip` fails the run. A Chromium that is found and does not start fails the feature.
  - A REST logout marks the account logged out, which ends every session of that user, so no spec or scenario logs out
    a user others share.
  - A component whose `suite.js` declares no server (or that has none) fails its specs under `--serve`, naming what to
    add.
  - `spec_depends_on` reads the source as the runner does, so a `then` line that starts with a regular expression no
    longer hides a spec's `dependsOn`.
- **Fixed after:** `gate3_browser` passes 22 of 22 on the moved driver. The wizard steps test had failed on the old
  driver too: its review check read `innerText`, and a `form-single` `m-display` is a readonly field (since
  `44a78fca`), so the check now reads the review fields' values. General JSON-RPC is gone: `webroot/rpc.xml` is
  removed, so `POST /rpc/json` is 404 (SecurityAccess), not a 500. Screen and transition hits count against tarpits
  as in Java (§8): `render_def` and the transition's run pass `count_tarpit`, a screen test disables the tarpit
  (`ec.artifactExecution.disableTarpit()` for scripts too), and a refusal under a template render keeps its kind and
  `Retry-After`. The mr-marble suite's server runs the dev conf, with the screen tarpits off, so it is not
  affected.
- **Follow-up complete (2026-10-27):** the HTTP security catalog now records 125 ported proofs and 18 not applicable, with none pending. Screen and transition tarpit counting is implemented; `SecurityTarpit` runs against the production conf. See [Security proof tests](../../docs/SECURITY_TESTS.md). Optional extensions from §15 are tracked in [Remaining-Work, Tests](../Remaining-Work.md#tests).

## 12. Tests of the harness

- **moqui-core** (`core/testkit/tests.rs`): a spec under a fake `SpecHost` with no execution context entered; host
  calls; `skip` in `setupSpec` and in a feature; `ec` and `call` in a client spec giving the named error; attachments
  in a failure; `run_on_thread`.
- **moqui-host units:** `Set-Cookie` parsing and jar matching; session token from headers and from meta; URL joining
  and the loopback rule; percentiles and the stats JSON; reading a `suite.js` server declaration; the locate script's
  role and name rules (on a fixture HTML in the browser test below).
- **Integration** (`host-tests/it/test_client.rs`): a server from `common::start`, then the binary's `test-client` on
  `host-tests/fixtures/test-component/test/client/`. Those specs cover HTTP, login, the session token both ways,
  redirects off, raw, WebSocket, a tiny `load` and `parallel`, a failing spec, a skipped one and JUnit. It also checks
  the untouched runtime files. `--serve` is checked on the same test component.
- **Browser** (opt-in like `gate3_browser`, because it needs Chromium): a client spec over the gallery screens with
  `goto`, roles, `fill`, `click`, dialogs, settle, a provoked console error failing the feature, `allowPageProblems`, a
  screenshot and a PDF.
- **CLI** (`crates/moqui/tests/cli/`): option errors, nothing answering, `--wait`.

## 13. Verification, end to end

1. `./moq build`, then `./moq run` in one terminal and `./moq test-client framework` in another, on the same runtime
   directory. `SecurityCors` fails with its conf message, and the features that need fixtures skip with theirs. After
   `./moq test framework --spec ClientFixtures --db existing` (on the running server's database), those pass too. `runtime/.env`,
   `runtime/log/moqui.log` (no lines from the client) and `runtime/db/embedded/data/postmaster.pid` keep their
   contents and times, and `ss -ltnp` shows no listener of the client's.
2. `moqui test-client framework --serve --junit /tmp/ct`: all pass, under the load test conf (the production conf with
   the tarpits off).
3. `moqui test-client framework --spec LoadBasic --param users=50 --param duration=60` against `./moq run`: the table
   and the JSON.
4. `moqui test-client mr-marble --serve`: BrowserSmoke and FindScreens pass, with screenshots and the PDF in the
   output directory. Break a Find screen on purpose and see the failure's screenshot and text.
5. `./moq test mr-marble --spec OrderRoleFilters`; `--spec ScreenReport -D mrss_screen_report=/tmp/r.md`.
6. From a second machine (or a container) with the binary and a checkout: `--server https://<host> --insecure` against
   a server started with `--conf conf/MoquiProductionConf.xml`.
7. `cargo test -p moqui-core testkit`, `cargo test -p moqui-host --test it`, the opt-in browser test, and
   `gate3_browser` on the moved driver.

## 14. Risks and open questions

1. **Settling on the Vue shell.** The idle signals (load pending, loading bars, fetches) may miss a component that
   renders after a fetch ends, such as a lazy dialog. Mitigation: actions wait on their own element anyway, and
   `waitFor` is there. Measure the flakiness over 20 runs of FindScreens before P5 ends.
2. **Engines per thread.** Each virtual user starts the full engine, `ec` objects included (64 MiB limit each). Measure
   the start time and memory at 100 users in P4; if they matter, a client engine without the `ec` install is a small
   change to `Engine::start`.
3. **The `--serve` time** for mr-marble is the eight Mantle flows on each run. A snapshot taken after the specs
   (beside the load snapshot) would make it seconds. Later, if it hurts.
4. **The `Host` header and `returnTo`** (§7): possibly a real finding; it is handled privately if so.
5. **Windows.** The child's stop through its standard input and the raw TLS stream are portable. Chromium discovery on
   Windows is not tested (as for PDF).
6. **Coverage of the remaining suite.** Completed after the initial build: the catalog has no pending HTTP proofs, including multipart, HMAC and production-conf tarpit checks. See the archive status and follow-up above.

## 15. Later

The HTTP security migration is complete. These optional extensions remain in [Remaining-Work, Tests](../Remaining-Work.md#tests):

- moving `gate3_browser` flows into client specs where that is clearer;
- browser users in `load`, an arrival-rate model, server `/metrics` before and after a load, merged reports from
  several clients;
- visual comparison of screenshots;
- CDP over a pipe instead of a port;
- TypeScript declarations for the spec globals (shared with `moqui test`'s open item).
