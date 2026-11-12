# Testing a server from outside with `moqui test-client`

`moqui test-client` (`./moq test-client`) runs the JavaScript spec files in a component's `test/client/` directory
against a server that is already running. The specs have the format of `moqui test`
([JS-Testkit](JS-Testkit.md)): `spec`, `feature`, the blocks, `where` rows, conditions that fail with the values of
their parts, `suite.js`, the console report and JUnit. They reach the server only from outside: HTTP sessions with
cookies and the session token, raw requests over TCP or TLS, WebSocket, headless Chromium, and a load runner of
virtual users.

The other JavaScript test APIs are [JS-Testkit](JS-Testkit.md), the server specs of `moqui test`, which run inside
the server's process with `ec`, and [JS-Test-Load](JS-Test-Load.md), the load scripts of `moqui test-load` and
Tools › Load. Building, the two suites, the regression budget and the commands are in
[Build-Test](../Build-Test.md#component-specs).

It is a pure client. It opens no database, starts no execution context, reads no conf and no `runtime/.env`, opens no
`runtime/log/moqui.log`, and binds no listener of its own; the only port is the one Chromium opens on loopback for
DevTools, with a temporary profile removed after each spec. It writes only under `--out` and `--junit`. So it runs
beside `./moq run` in the same runtime directory, or on another machine against any server. `ec`, `call`, `checkData`,
`screenTest` and `uploadedFile` are not there: each throws an error that names the client equivalent.

It is in every package (the `test-client` feature, [Packaging](../Build-Test.md#packages)). `--serve`, which starts a server for the
suite, needs `moqui test` and so the `embedded-postgres` build (not Enterprise). The design record is
[ClientTestHarness](../../plans/archive/ClientTestHarness.md).

## Running

```sh
./moq run                                              # a dev server, in one terminal
./moq test-client framework                            # the framework's client specs against http://localhost:8080
./moq test-client framework -p 8081                    # the server at http://localhost:8081
moqui test-client framework --serve                    # start the server suite.js declares, run, stop it
moqui test-client webroot --offline                   # client specs with no server and no listening port
moqui test-client runtime/dev-component/dev-aide --offline
moqui test-client mr-marble --serve --junit build/test-results
moqui test-client framework --acceptance --spec LoadBasic --param users=50 --param duration=60
moqui test-client mr-marble --serve --spec FindScreens --param screen=Survey/FindSurvey --headed
moqui test-client mr-marble --serve --spec FindScreens --param phase=before --video --out /tmp/run
moqui test-client --server http://localhost:8081 --wait 120 --junit build/test-results --fail-on-skip
moqui test-client runtime/component/my-site/test/client --server https://test.example.com --insecure
```

With no component, every component under the runtime that has `test/client/*.spec.js` (in `test/client` or a
subdirectory of it other than `process`; a spec's name is its file name, unique in the component) runs: `base-component`, then
`component`, then `dev-component`, by directory name within each, and each after the components its `component.xml`
`depends-on` names. A target with a `/`, or ending in `.spec.js`, is a path: a `test/client` directory, a component
directory, or one spec file (its component is the directory above `test/`). A path serves a machine that has the test
files and the binary but no runtime.

The run first sends `GET /` with redirects off. Any HTTP answer means the server is up; a connection error ends the run
with "nothing answers at http://localhost:8080: start a server (./moq run), or pass --server or --serve". `--wait`
polls until it answers, for CI that starts the server in the background. Any failed feature exits 1.

| Option | |
|---|---|
| `--server URL` | `http://localhost:8080` by default, or `MOQUI_TEST_SERVER`. Plain `http` goes only to this machine (`localhost`, `127.0.0.0/8`, `::1`) unless `--allow-http`, since a login over it crosses the network in the clear. `./moq test-client -p N` gives `http://localhost:N` |
| `--ssh host:port` | Where `ssh.connect` goes when a spec names no host and port (`server.ssh`); a port alone is on the server's host. Or `MOQUI_TEST_SSH`. Not with `--serve`, which takes the started server's ([SSH](#ssh)) |
| `--serve` | Start each suite's declared server as a child process and run against it (below). Not with `--server`, `--wait` or `--offline` |
| `--offline` | Run only specs with `server: false`. Pages open at `https://moqui.test`. Nothing listens. Not with `--serve` |
| `--spec Name[,Name]` | The specs to run, by file name without `.spec.js`, exactly (no patterns), with the specs their `dependsOn` names |
| `--feature text` | Only the features whose names contain the text |
| `--acceptance` | The acceptance client specs (`acceptance/client/`) in place of `test/client/`; with `--serve` the server is the one `acceptance/client/suite.js` declares, and its specs resolve across `test/` and `acceptance/` ([The two suites](../Build-Test.md#the-two-suites)) |
| `--param name=value` | Repeatable; `params.name` in specs. `MOQUI_TEST_<NAME>` variables are params too, in camel case (`MOQUI_TEST_LOAD_PASSWORD` is `params.loadPassword`), for a password that should not show in `ps`. A `--param` wins over a variable |
| `--junit dir` | `TEST-<component>.client.<Spec>.xml` per spec. Files a case attaches are `[[ATTACHMENT\|path]]` lines in its `system-out`, after its load table |
| `--out dir` | Where saved bodies, screenshots, PDFs, failure files, load results and `--serve` logs go. The default is `runtime/tmp/test-client/<yyyyMMdd-HHmmss>-<pid>`, or `test-client-out/<run>` in the working directory without a runtime. The run prints it at the end when anything was written |
| `--wait seconds` | Poll until the server answers |
| `--timeout seconds` | What `waitUntil`, a raw request, a WebSocket connect and receive, and a browser action wait by default. 10 |
| `--chromium path` | The browser, else `moqui_chromium_path`, else `runtime/chromium` (`./moq chromium-download`), else an installed Chromium, then Google Chrome or Microsoft Edge (on the PATH or where the OS installs them) |
| `--headed` | A visible browser, for debugging |
| `--video` | Record each spec's pages into `<out>/video/<Spec>.webm`, with `<Spec>.timeline.json` ([Video](#video)). Needs `ffmpeg` |
| `--insecure` | Accept an invalid server certificate (a WARN says so); Chromium gets `--ignore-certificate-errors` too, and `ssh` takes any host key |
| `--allow-http` | Allow plain `http` to another machine |
| `--fail-on-skip` | A skipped spec or feature fails the run, so that a missing Chromium or fixture does not pass as green in CI |
| `--log-level level` | Console only: `trace`, `debug`, `info`, `warn` (the default) or `error`. `log.info` in a spec shows at `info` |

The console has the `moqui test` lines, one per case, with a load's table after its line, and the failures in full at
the end:

```
SecurityAuthn > A07 a POST runs with the session token and stores nothing without it (csrf: false)  PASSED (0.04s)
BrowserSmoke > "/apps/SimpleScreens/Order/OrderDetail?orderId=55401" shows "Picker Bot 2000"  FAILED (3.12s)
LoadBasic > mixed reads and writes  PASSED (10.31s)
    4 users, 10.0 s: 2,412 requests, 241.2/s, 0 errors
    name                count  errors    p50    p95    p99    max
    s1 enums              603       0  3.1ms  9.0ms   14ms   31ms
    ...
Output: runtime/tmp/test-client/20261024-153012-48211
```

### Against a server of your own

Against `./moq run` (the dev conf) the specs that pass on any conf pass, and two things differ from `--serve`:

- **Fixtures.** Users and grants a suite needs are made by a server spec of the same component (below), which
  `--serve` runs. For a server it did not start, run that spec on the server's database, such as
  `moqui test framework --spec ClientFixtures --db existing`. Without the fixtures the features that need them skip,
  naming the command.
- **Conf.** A spec that needs a conf says so. `SecurityCors` needs a server that allows no foreign origin, and fails in
  its `setupSpec` on the dev conf (`webapp_allow_origins=*`). A real load run needs the tarpits off
  (`conf/MoquiLoadTestConf.xml`; see Load). Start such a server with `./moq run -c conf/MoquiLoadTestConf.xml` or
  `MOQUI_CONF`.

From another machine, copy the binary and the component's `test/client` directory (a checkout is simplest) and pass
`--server https://<host>`, with `--insecure` for a self-signed certificate. The specs read only their own component's
files.

### `--serve`

A suite declares the server it needs in `test/client/suite.js`, beside the order:

```js
suite(["SecurityAccess", "SecurityHeaders", "SecurityCors", "SecurityAuthn", "SecurityScreens", "SecurityRest"], {
    server: { components: ["framework"], specs: ["ClientFixtures"], conf: "conf/MoquiLoadTestConf.xml" },
})
```

`components` are what the server loads (the suite's own component when empty), `specs` the server specs that run first
as fixtures (with their `dependsOn`, looked up across the named components), `conf` the runtime conf, relative to
the runtime as `MOQUI_CONF` is, and `properties` conf properties set in the server's environment (mr-boss's walks
send invoices and reminders, so its suite has `properties: { mail_capture_sends: "true" }`). The file is read as
literals, not run.

`moqui test-client --serve` groups the selected components by their declaration: components with the same one, or
with the same `conf` and `properties` and named `specs` on both, share a server, which loads the components of all of
them and runs all of their specs (an empty `specs` list, every spec of its components, stays on a server of its own). For each group it starts this binary as `moqui test <components> --spec <specs> --db snapshot --serve
127.0.0.1:0 --ready-file <file>`, with `MOQUI_CONF` set to the conf, the properties in its environment and a pipe on
the child's standard input. The
child's output goes to `server-<n>.log` in the output directory. Once the ready file names the URL, the group's specs
run against it; then the pipe closes, and the child stops and removes its scratch runtime. The child also stops when
the client dies without cleaning up, since its standard input closes. A component with no declaration, or a child that
exits or does not start within 30 minutes, fails that group's specs with the last lines of its log; the other groups
still run. With `--acceptance` the child gets `--acceptance` too, so its `specs` may name a regression fixture or an
acceptance one. When `--chromium` names the browser and `moqui_chromium_path` is not set, the child renders PDF with that
browser too.

The child is an ordinary `moqui test` run, so it writes what one writes: a scratch runtime, and the saved load under
`runtime/save` that `--db snapshot` restores on later runs while the components it loads, their entity and data files
and the crate version are unchanged (a relink keeps them). Servers on different confs or instance purposes load different components (a dev
component only on a dev instance), so each has a snapshot of its own. A first run with Mantle's data takes minutes;
later ones take seconds.

`moqui test --serve [address]` is the server side, also for use by hand. When its specs pass it shuts the test factory
down and serves the same scratch database on the address (`127.0.0.1:0`, a free port, by default; or a port alone, on
loopback), printing `Serving <url> (Ctrl-C to stop)`, until Ctrl-C, SIGTERM, or the end of standard input when it is a
pipe. The SSH, HTTPS and cluster ports move to free ones unless set, so it starts beside a running server; the ready
file names the moved SSH listener on a second line (`ssh 127.0.0.1:<port>`), which `--serve` gives the specs as
`server.ssh`. With
`--db existing` it serves the runtime's own database instead of a scratch one; when there are no specs to run (each
component named has none) no test factory boots first, and the server starts at once. Not with `--db none`, `--agents`
or `--skills`.

```sh
moqui test mr-mantle mr-marble --spec SimpleScreensRender,ClientAccess --serve 18081
moqui test-client mr-marble --server http://127.0.0.1:18081
```

A component's `test/MoquiConf.xml` merges after its `MoquiConf.xml` in `moqui test` runs only: the in-process specs
and the server of `--serve`. It is where a test-only mount or setting goes; `./moq run` never reads it. mr-marble
mounts `apps/SimpleScreens` there.

### `--offline`

`spec({ server: false })` is an offline spec. `moqui test-client <component> --offline` runs only those, and a run
without `--offline` runs them too. `--offline` with `--serve` is an error. The spec opens no server and binds no port.
Chromium's pages load at `https://moqui.test`. Nothing listens: the browser asks, and the answer is a file or a 404.

| Path | File |
|---|---|
| `/js`, `/css`, `/libs` | `runtime/base-component/webroot/screen/webroot` |
| `/files/<repo-relative>` | the repository, read-only; a `..` segment is a 404 |
| `/favicon.ico` | an empty 204, because the browser asks for one |

`files.read` stays inside the spec's component. Fetch runs only for `https://moqui.test`, and only on an offline page.
`hmac`, `files`, `sleep` and `log` stay available. The webroot and dev-aide offline specs are not part of the framework
security suite.

`browser.runInPage({ scripts, styles, test })` is called at the top of an offline spec, not inside a feature, where
it throws; in a spec that is not `server: false` it fails. It opens a fresh page, loads `styles` and then `scripts`,
each in order, then the `test` file, all paths on `https://moqui.test`. That file calls `test(name, fn)`, and each
one is a case; `--feature` picks them by name. The spec itself is the container, not an extra passing case. With no
Chromium the cases are skipped, and `--fail-on-skip` makes that a failure.

The test file runs in the page, not in the spec's engine, so it has the page's globals and these:

| In the page | |
|---|---|
| `test(name, fn)` | A case. `fn` may return a promise, which is awaited. The cases run in the order written |
| `assert` and its methods | The same `assert` as a spec's ([JS-Testkit](JS-Testkit.md#assert)). `assert.rejects` returns a promise there: return or await it |
| `harness.frame(scripts, globals)` | A promise of the window of a new same-origin iframe: `globals` are set on it, then each script loads in order. For code that must load into a clean global |
| `harness.allowProblems()` | Page problems until this test finishes do not fail it |

The spec's `timeout` is what the whole file may take, `--timeout` otherwise. A test that does not finish fails by
name.

## Files and order

A client spec is `<component>/test/client/<Name>.spec.js`. `moqui test` reads only the top level of `test/`, so the two
sets never mix, and a component can have both. `test/client/suite.js`, `suite(["SpecA", "SpecB"], { server })`, gives
the order; the other spec files run after it, by name. Components run after the components their `component.xml`
`depends-on` names. A run then takes the order of `moqui test`: a spec whose `dependsOn` names a spec later in that
order (in its own component or another) waits for it, with the specs after it in its `suite.js` chain, and the rest
keep their places; a `dependsOn` cycle fails the run before any spec runs. With `--serve` this holds among the specs of
each server, and the servers run in the order of their first components. Each spec gets a default session, and its
sessions, sockets, pages and browser are closed after `cleanupSpec`.

A process spec, `<component>/test/client/process/<businessProcessId>.spec.js`, tells one business process as a story
(Chisel's Process Story Diff records it with `--video`). It runs only when named: by `--spec <businessProcessId>` with
its component, or by its file path. A whole-component run leaves it out. It can `dependsOn` the component's other specs.

`spec({...})` and `feature(name, {...})` take what they take in `moqui test`
([JS-Testkit](JS-Testkit.md#spec)): `dependsOn`, `requires`, `stepwise`, `timeout`, the setup and cleanup blocks,
`given`, `when`, `then`, `expect`, `where`, `ignore`. Two fields are the client's: `server: false` on `spec()`
makes an offline spec ([`--offline`](#--offline)), and `allowPageProblems` takes the place of `allowMessageErrors`,
since there is no `ec.message` to check. `tempSequences` and `database: false` are for server specs: a client spec
has no `ec` for them. A regression client spec over 15 seconds fails the run: move
it to `acceptance/client/` or delete it ([The two suites](../Build-Test.md#the-two-suites)). An acceptance client spec
is `<component>/acceptance/client/<Name>.spec.js`, with `acceptance/client/suite.js` for its order and server. A
suite.js that declares no server holds offline specs (`server: false`), which `./moq accept` runs with `--offline`
(webroot's `ThemeLayout`, for one).

**Fixtures are server specs.** Data a client suite needs is made by a `test/*.spec.js` of the same component that also
asserts what it made, such as `framework/test/ClientFixtures.spec.js`, which makes the `sec.*` users and `test.load` as
Java's `SecurityTestSupport.ensureUsers` did. It runs in `--serve`, in a plain `moqui test` on its scratch database, or
with `--db existing` on a server's database. Nothing goes in `data/`, where a production load could reach it.

**`skip(reason)`** reports the feature as SKIPPED with the reason; in `setupSpec` it skips the whole spec. A client spec
skips when its fixtures are missing:

```js
spec({
    setupSpec() {
        try {
            login("test.load", params.loadPassword ?? "TestLoad1!!")
        } catch (e) {
            skip(`test.load is not on this server (${e.message}): ` +
                "run moqui test framework --spec ClientFixtures --db existing")
        }
    },
})
```

## The API

Every call blocks until it is done; there is no `await`. Conditions are plain values, as in server specs.

### Globals

| Global | |
|---|---|
| `server` | `{ url, host, port, secure, ssh }` of the server under test; `ssh` is `{ host, port }` of its SSH listener (`--ssh`, or the server `--serve` started), else null |
| `params` | The `--param` values and `MOQUI_TEST_*` variables, as text |
| `http` | The current session: the spec's default, or in `load` and `parallel` the thread's own |
| `session(opts)` | A new session with its own cookie jar and token. `opts.headers` go on each of its requests; `opts.user` (and `opts.password`, `moqui` by default) logs it in |
| `login(user, password = "moqui")`, `logout()` | `http.login` and `http.logout` |
| `hmac` | `sha256Base64` and `sha256Timestamp`, the System Message signers ([HTTP](#http)) |
| `ws` | WebSocket |
| `ssh` | SSH: `connect` to a server, then `exec`, `agentApps` and `shell` ([SSH](#ssh)) |
| `browser` | Headless Chromium: `newPage(opts)`, and `runInPage(opts)` at the top of an offline spec |
| `load(opts)`, `parallel(n, fn, args, opts)` | Virtual users, and threads |
| `step(name, fn)`, `pause(spec)`, `fail(message)`, `service.call(...)` | The load globals ([Load and parallel](#load-and-parallel)) |
| `files.list(dir, regex)`, `files.read(path)` | Read-only, within the spec's own component directory. `list` gives every file under `dir` with its path relative to the component (`test/client/Http.spec.js`), filtered by the pattern when given. A path with `..`, an absolute path, or a link out of the component is refused |
| `skip(reason)` | Skip the feature, or in `setupSpec` the spec |
| `waitUntil(fn, { timeout, interval })` | Call `fn` until its value is true by the truth of a condition (an empty list or text is false) and return that value; else throw with the last value. `timeout` in seconds (`--timeout` by default), `interval` in milliseconds (100) |
| `sleep(ms)` | Pause |
| `assert` and its methods, `dec`, `sum` | As in server specs ([JS-Testkit](JS-Testkit.md#assert)) |
| `log.trace`, `debug`, `info`, `warn`, `error` | One message to the console at that level; a value that is not text is shown as a condition's part is |

There is no process or shell API. The server's helpers are not there: `ec` throws when read, and `call`,
`checkData`, `screenTest` and `uploadedFile` throw when called, each naming what to use instead (`http.post` on the
service's REST path, `http.get` on `/rest/e1/...`, `http.get` on the screen path or `browser`, `http.post` with
`multipart`). `plan` and `scenario` are a load script's ([JS-Test-Load](JS-Test-Load.md)) and throw in a spec. A
response and a session are objects the calls return; their classes are not global names.

### HTTP

`http.get(path, opts)`, `post`, `put`, `patch`, `delete`, `options`, `head`, and `request(method, path, opts)`, on
`http` or any session. A path is relative to `server.url`; a full URL is taken as it is. Each returns a response.

| Request option | |
|---|---|
| `params` | Query parameters; an array value repeats the name |
| `form`, `json`, `body`, `multipart` | The body: URL-encoded, JSON, text, or parts (`{ name: value }`, or `{ name: { fileName, text, contentType } }` with `base64` in place of `text` for bytes) |
| `headers` | Request headers. A header given here wins over the session's; a `Cookie` given here replaces the jar's |
| `follow` | Follow redirects, up to 10. Off by default, so a test sees each 3xx and its `Location` |
| `csrf` | The session token on POST, PUT, PATCH and DELETE: sent when the session has one by default, `false` to leave it out, or a text to send instead |
| `basic: [user, password]`, `apiKey`, `loginKey` | A Basic header, `api_key`, `login_key` |
| `accept` | `"json"`, `"html"`, or any `Accept` value |
| `timeout` | Seconds; 60 by default |
| `name`, `expect` | For `load`: the name the request is grouped under, and the statuses that are not errors (below) |

`hmac.sha256Base64(secret, body)` returns the standard Base64 of HMAC-SHA256 of the body, and
`hmac.sha256Timestamp(secret, body, unixSeconds)` returns the `t=<unixSeconds>,v1=<hex>` header, using the current
unix second when `unixSeconds` is omitted. Both match System Message HMAC (`SmatHmacSha256` and
`SmatHmacSha256Timestamp`).

The response has `status`, `ok` (2xx), `url` (the last one, after redirects), `location`, `contentType`, `timeMs`,
`length` (the full size; the body is kept up to 16 MiB), `text`, and:

- `json()`: the body parsed, or an error with the status and the start of the body;
- `header(name)`, the first value in any case, or null; `headers(name)`, every value, as for `Set-Cookie`; `headers()`,
  every `[name, value]`;
- `cookies`, each `Set-Cookie` parsed (`name`, `value`, `domain`, `path`, `maxAge`, `expires`, `secure`, `httpOnly`,
  `sameSite`), and `cookie(name)`;
- `magic`: the first four bytes as text when printable, such as `%PDF` or `PK`, else as hex;
- `urlParam(name)`: a query parameter of `url`, else of `location` (a redirect not followed), decoded, or null;
- `assertContains(text)`, `assertNotContains(text)`: true, or a failure with the start of the body;
- `save(name)`: the body into the output directory, a plain file name; the path. The last 32 bodies are kept for it.

A session also has `raw(text, opts)` ([Raw requests](#raw-requests)), `login(user, password)`, `logout()`,
`formLogin(user, password, opts)` (below), `cookies()` (the jar: `name`, `value`, `domain`, `path`, `expires`,
`secure`, `httpOnly`, `sameSite`), `clearCookies()` and `token()`, the session token or null. The jar keeps cookies by name, domain and path, with expiry, and sends
them as a browser would.

**The session token** is taken from the `moquiSessionToken` and `X-CSRF-Token` response headers, which a server sends
when a session is new or its token changes, and from an HTML page's `<meta name="moquiSessionToken">` or a `/qapps`
page's `moqui-config` JSON. The session sends it as `X-CSRF-Token`.

**Logins.** `login(user, password)`, on `http` or any session, posts `/rest/login`, the API login, which needs no
session token, and returns that response. The password is `moqui` when left out. It throws when the login fails
(`login of no.such.user failed: HTTP 401: ...`) or needs a second factor (`login of <user> needs a second factor`).
`logout()` gets `/rest/logout`. `formLogin(user, password, { returnTo, headers })` gets `/Login` for its cookie, then posts `/Login/login` with
redirects off and returns that response, for the redirect proofs.

```js
spec({ setupSpec() { login(params.user ?? "john.doe", params.password ?? "moqui") } })

feature("a POST runs with the session token and stores nothing without it (csrf: #csrf)", {
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

feature("an anonymous session cookie is HttpOnly and SameSite=Lax", {
    expect() {
        const cookie = session().get("/Login").cookie("moqui.session")
        cookie.httpOnly
        cookie.sameSite == "Lax"
    },
})
```

### Raw requests

`http.raw(text, { cookies, timeout, name, expect })` sends the request text as it is over TCP, or TLS for an `https`
server, with the session's `Cookie` header added unless `cookies: false`, and parses the answer into a response. The
text has its own request line and `Host`. It serves paths that must not be normalized and malformed requests:

```js
http.raw("GET /js/../../Login.xml HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", { cookies: false })
```

### WebSocket

`ws.connect(path, { session, headers, origin, timeout })` opens a socket with the session's cookies (`http` by default)
and returns `{ status, headers, open, send(text), receive({ timeout }), close() }`. `headers` are the handshake
response's, by lower-case name. A refused handshake does not throw: `open` is false and `status` is the HTTP answer,
such as 401 or 403, because the security proofs check refusals. `receive` returns the next text frame, or null on
timeout or close; its `timeout` is in seconds, `--timeout` by default.

```js
const s = session({ user: "john.doe" })
const evil = ws.connect("/notws", { session: s, origin: "https://evil.example" })
!evil.open
evil.status == 403
```

In a load the handshake, a send given a `name` (`send(text, { name })`) and each receive are samples of kind `ws`,
and a refused handshake, a receive that times out and one whose socket closes are errors unless the call's `expect`
lists the status or `timeout` or `closed` ([JS-Test-Load, WebSocket and SSH](JS-Test-Load.md#websocket-and-ssh)).
Outside a load they behave as above.

### SSH

`ssh.connect({ user, password, key, host, port, fingerprint, timeout })` logs in to an SSH server and returns a
connection; `user` and a `password` or a private `key` (PEM text) are needed. With no `host` and `port` it goes to
`server.ssh`: `--ssh host:port` (a port alone is on the server's host), `MOQUI_TEST_SSH`, the listener of a server
`--serve` started, or in a load run from Tools the server's own; a `port` with no listener named is on 22. The host
key: a pinned `fingerprint` (`SHA256:...`) must match; a server on this machine is taken, and so is any with
`--insecure`; any other is refused, naming its fingerprint to pin. The calls on the connection:

| Call | |
| --- | --- |
| `exec(command, { input, timeout, name, expect })` | Runs one command and returns `{ status, stdout, stderr }`; `input` is its standard input. It does not throw for an exit status, only for a channel the server refused (a command the account may not run) or a timeout |
| `agentApps(path = "/apps", { size, timeout, name })` | A session of the server's `agent-apps` ([TUI, For agents](../ui/TUI.md#for-agents)) on `path`, as JSON; it throws when the session does not open. Its `command(line, { name, expect })` runs one command and returns the answer (`{ ok, reason, path, title, screen, elements, ... }`), `close()` ends it, and `answer` is the last one |
| `shell({ cols, rows, term })` | A pty (120x40, `xterm-256color` by default) and the shell the account gets: the `moqui>` prompt with `SSH_SHELL`, else the apps. `send(text)` writes to it (`"\r"` is Enter); `expect(pattern, { timeout, expect })` waits until the output, escape sequences taken out and a cursor move read as a line break, matches a RegExp or a string, and returns `{ match, groups, before }`, the output up to the match then read. A timeout throws, with the end of the output, unless `expect` lists `timeout` (then null). The pattern is the regex crate's, so a lookahead or a back reference is refused |
| `close()` | Logs out; the spec's connections and shells close after the spec in any case |

```js
const conn = ssh.connect({ user: "john.doe", password: "moqui" })
const apps = conn.agentApps("/apps/system")
apps.command("go /apps/system/Security/UserAccount/UserAccountList").ok
conn.exec("agent-apps nosuch look").status == 2
const shell = conn.shell()
shell.expect(/moqui> |System/, { timeout: 20 })
```

In a load each connect, exec, agent-apps command and expect is a sample of kind `ssh`
([JS-Test-Load, WebSocket and SSH](JS-Test-Load.md#websocket-and-ssh)). The code is `test_client/ssh.rs`, with the
login of the host's SFTP client (`crates/moqui-host/src/ssh/client.rs`); a build without the `ssh` feature (`lite`)
throws on each call.

### Browser

`browser.newPage({ user, password, viewport: { width, height }, colorScheme, reducedMotion, javaScript, timeout, problems })`
opens a page in a browser context of its own, so each page has its own cookies. With `user` it signs in through the form
at `/Login` (`password` is `moqui` by default) and fails when the page stays there. That is done once a spec for each
user and password: later pages of the user are given the cookies of that sign-in, so they share its server session, and
when the first `goto` of one lands on `/Login` (the session ended) it signs in again and goes on. A spec that tests
signing in opens a page with no `user` and fills the form itself. The viewport is 1440 × 1000 by default; `colorScheme`
is `light`, `dark` or `no-preference` and `reducedMotion` is `reduce` or `no-preference` (the media features
`prefers-color-scheme` and `prefers-reduced-motion`, emulated together); `javaScript: false` turns the page's own scripts
off once it has signed in (the form at `/Login` signs in with them on, also when it signs in again), for a check of what
a page does without JavaScript; `timeout` (seconds) is what the page's actions wait, `--timeout` by default. With scripts
off the harness still evaluates in the page: `evaluate`, `waitFor`, `text()` and the locators work, and their actions
are input events as before, so a link or a plain form submits as the browser does it; a page's handlers, its Vue client
and `settle`'s wait for that client are not there. A page opened in a feature closes after it; one opened in
`setupSpec` lives for the spec.

The spec has one Chromium, started on its first `newPage`, with a temporary profile removed after the spec. When no
Chromium is found (or `--chromium` names a file that is not there), `newPage` skips the feature, or in `setupSpec` the
spec, with the reason; `--fail-on-skip` makes that a failure. A Chromium that is found but does not start fails the
feature.

| Page | |
|---|---|
| `goto(path)` | Navigate, wait for the load event (for the page's timeout, and at least 30 s), then settle. `{ status, url }` of the document response. On an offline page a path is on `https://moqui.test` |
| `setContent(html)` | Offline only. The document at `https://moqui.test/`, so `/js`, `/css` and `/libs` resolve on that origin |
| `addScript(path \| { content })`, `addStyle(path \| { content })` | A path on the page's origin, waited on until it loads, or an inline script or stylesheet |
| `keyboard.type(text)`, `keyboard.press(key)` | `type` sends key events, one character at a time. `fill` inserts text and does not send keys |
| `url()`, `title()`, `text()`, `html()` | The current URL, the title, `document.body.innerText`, the HTML |
| `locator(css)`, `getByRole(role, { name, exact })`, `getByText(text, { exact })`, `getByLabel(text, { exact })` | Locators, resolved again on each use. A name or text is a substring ignoring case and spacing, the whole text with `exact`, or a regular expression |
| `fill(css, value, { timeout })`, `click(css, { timeout })`, `press(key)`, `keyboard.press(key)` | Shortcuts for the locator actions; `press` goes to the focused element (`"Escape"`, `"Control+A"`) |
| `evaluate(fn, arg)` | Run in the page: a function, called with `arg` (copied as JSON), or an expression text. A promise is awaited; the answer comes back as JSON |
| `waitFor(fn, { arg, timeout })` | Evaluate in the page until truthy; its value. Else throw with the last value, the page's problems and the start of its text |
| `waitForURL(match, { timeout })` | Until the URL is the text (the whole URL, or its path with or without the query), matches the regular expression, or the function of it is truthy; then settle. The current URL counts, so a navigation that already happened passes |
| `settle({ timeout })` | Wait for the Moqui client to go idle; throw if it does not |
| `screenshot(name, { fullPage })` | A PNG into the output directory (`.png` added); the path. `fullPage` up to 16,000 px |
| `pdf(name, { landscape })` | Print to PDF into the output directory; `{ path, size, magic }` |
| `problems()`, `takeProblems()`, `allowProblems(pattern)` | The page's problems so far; take them, so they do not fail the feature; ignore those matching, seen and to come |
| `setViewport(width, height)`, `close()` | |
| `setEmulation({ colorScheme, reducedMotion, javaScript })` | Change what `newPage` set; what is not named stays. `javaScript` takes effect for what runs next, so `goto` again for the document to load with or without its scripts |

| Locator | |
|---|---|
| `click()`, `fill(value)`, `press(key)`, `hover()`, `focus()` | Wait until the element is attached, visible, enabled (not for `hover` and `focus`), editable (for `fill`), in a stable box, and for `click` and `hover` not covered by another element; then act through input events and settle. `fill` replaces the value; `fill("")` clears it. Each takes `{ timeout }` and returns true |
| `text()`, `value()`, `attribute(name)` | Read at once, without waiting; throw when nothing matches |
| `isVisible()`, `isEnabled()`, `isEditable()`, `count()`, `boundingBox()` | Read at once; false, 0 or null when nothing matches |
| `waitFor({ state, timeout })` | `visible` (the default), `hidden`, `attached` or `detached`. True; on timeout it throws with the page text and its problems |
| `locator(css)`, `getByRole(...)`, `getByText(...)`, `getByLabel(...)` | Search within the elements found so far |
| `first()`, `last()`, `nth(i)` | One of them; a negative `i` counts from the end |
| `evaluate(fn, arg)` | `fn(element, arg)` in the page, on the first match. It takes a function, not an expression text |
| `toString()` | How the locator was made, such as `page.getByRole("button", { name: /Save/ }).first()`, as failures name it |

Locators are not strict: an action or a read uses the first match, in document order. Scope a locator, or check
`count()`, where more than one element could match.

**Roles** come from the `role` attribute, else from the tag: `button` (buttons, `[role=button]`, submit, reset and
image inputs; a Quasar `q-btn` renders a button), `link` (an `a` with `href`), `textbox`, `checkbox`, `radio`,
`combobox`, `listbox`, `dialog`, `heading`, `list`, `listitem`, `table`, `row`, `cell`, `columnheader`, `img` and the
rest of the common implicit roles. An element hidden from the accessibility tree is not found. The **accessible name**
is `aria-label`, then `aria-labelledby`, then for a field its labels, placeholder or title, for an input button its
value, else its text without what is `aria-hidden` (so an icon's ligature is left out). `getByText` finds the deepest
visible element with the text. `getByLabel` finds the control of a matching `label`, an element with a matching
`aria-label` or `aria-labelledby`, and the field of a Quasar `q-field` label.

**Settling.** The page is settled when the Moqui client is idle for 100 ms: the document loaded, no load pending in
`moqui.webrootVue`, no `.q-loading-bar--visible` or `.q-inner-loading`, and no `fetch` or `XMLHttpRequest` in flight (a
counter is injected into each new document). A page that is not the Moqui client settles when it is loaded and has no
request in flight. `goto` and the actions settle within the page's timeout and go on when it ends; `settle()` throws.

**Page problems** are:

- uncaught exceptions;
- console errors and failed asserts (warnings too, when the page is opened with `problems: { warnings: true }`);
- responses of 400 and up, and failed loads other than cancelled ones;
- after a settle: `Error rendering` or `Exception in render` in the text, and the client's error notifications
  (Quasar's negative notifications, the danger alerts of the HTML client).

A case fails when its pages have problems at its end, unless the feature or spec has `allowPageProblems: true`: the
browser counterpart of `allowMessageErrors`. The problems are listed in the failure. A sign-in's own problems are not
the feature's. `allowProblems` takes a regular expression (or a text, matched literally); it is matched in Rust, so
only the `i`, `m` and `s` flags carry over, and lookaround and back-references are not supported.

**On a failure**, each open page leaves `failed-<feature>-<n>.png` and `failed-<feature>-<n>.txt` (its URL, its problems
and its text) in the output directory, and the report and JUnit list them as attachments.

```js
spec({ setupSpec() { login("john.doe") } })           // the default session; browser pages sign in on their own

feature("#path shows #text", {
    where: [
        { path: "/apps/SimpleScreens/Customer/FindCustomer?pseudoId=CustJqp", text: "Joe" },
        { path: "/apps/my/User/Account", text: "Change Password" },
    ],
    expect({ path, text }) {
        const page = browser.newPage({ user: "john.doe" })
        page.goto(path).status == 200
        page.text().includes(text)
    },                                                 // page problems fail the case unless allowed
})

feature("New Survey opens and closes on Escape", {
    expect() {
        const page = browser.newPage({ user: "john.doe" })
        page.goto("/apps/SimpleScreens/Survey/FindSurvey")
        page.getByRole("button", { name: /New Survey/ }).click()
        const dialog = page.locator("#NewSurveyContainer")
        dialog.waitFor()
        dialog.locator("[name=description]").isEditable()
        page.keyboard.press("Escape")
        dialog.waitFor({ state: "hidden" })
    },
})
```

### Video

`--video` records each spec that opens a page: `<out>/video/<Spec>.webm` (VP8), and beside it
`<Spec>.timeline.json`, the spec's features on the video's clock. It needs `ffmpeg` on the `PATH` (`/usr/bin/ffmpeg`
from the distribution's package will do) as well as Chromium. Without either, an error names what is missing, the
specs still run, and only the timelines are written.

Each page is recorded with the DevTools screencast (`Page.startScreencast`, JPEG, every frame, each acknowledged). The
screencast sends a frame only when the page changes, so each frame lasts until the next one, from the time the browser
drew it, and the last frame lasts until the spec ends: a page that stays the same for ten seconds is ten seconds of
video. The video starts when the spec starts and is white until the first page draws. A spec with several pages is one
video: it shows the page opened last that is still open, and when that page closes, the one before it again. A spec
that opens no page has a timeline and no video. A failed feature does not stop the recording.

The timeline has every feature that was reported, in order, those that open no page and the skipped ones too:

```json
[
  { "feature": "a page changes and then stays the same", "start": 0.012, "end": 1.31, "status": "passed", "reason": null },
  { "feature": "a feature the before phase skips", "start": 1.31, "end": 1.311, "status": "skipped", "reason": "phase is before" }
]
```

`start` and `end` are seconds from the start of the video (the start of the spec), on the clock of its frames. `status`
is `passed`, `failed` or `skipped` (an `ignore` is `skipped`); `reason` is the skip reason or the failure message, or
`null`. With `--param`, one spec can serve several runs: `params` is there at the top of the file and in every block,
and a feature that does not belong to this run calls `skip("reason")`:

```js
spec({ server: false })
const phase = params.phase                       // --param phase=before

feature("the new layout", {
    expect() {
        if (phase === "before") skip(`phase is ${phase}`)
        browser.newPage().goto("/apps/my-app").status == 200
    },
})
```

### From Playwright

| Playwright | Here |
|---|---|
| `await page.goto(u)` | `page.goto(u)` (also settles) |
| `page.getByRole("button", { name: "Save" })` | The same; not strict, so `.first()` is implied |
| `await loc.click()`, `await loc.fill(v)` | `loc.click()`, `loc.fill(v)` |
| `await expect(loc).toBeVisible()` | `loc.waitFor()` as a condition |
| `await expect(loc).not.toBeVisible()` | `loc.waitFor({ state: "hidden" })` |
| `await expect(loc).toBeEditable()` | `loc.isEditable()` (reads at once; `waitFor()` first if it may not be there yet) |
| `await expect(loc).toHaveCount(n)` | `loc.count() == n` (at once) or `page.waitFor(...)` |
| `await expect(page).toHaveURL(re)` | `page.waitForURL(re)` |
| `await expect(loc).not.toContainText(re)` | `!re.test(loc.text())` |
| `page.evaluate(fn, arg)` | The same; `fn` runs from its source text |
| `page.on("pageerror" \| "console" \| "response")` collecting into an array | Built in: page problems |
| `Promise.all([page.waitForURL(f), form.requestSubmit()])` | `form.evaluate((el) => el.requestSubmit()); page.waitForURL(f)` |
| `testInfo.attach`, `screenshot: "only-on-failure"` | `page.screenshot(name)`; failures attach on their own |
| `request.newContext()` | `session()` |

### Load and parallel

`load(opts)` runs virtual users, each a thread with its own script engine, client globals and default session (`http`).
It returns the statistics, prints a table after the feature's line, and writes the whole result as JSON.

| Option | |
|---|---|
| `users` | The number of users, 1 to 10,000; 1 by default |
| `duration`, `iterations` | Seconds the scenario repeats for, and/or iterations per user; at least one. With both, a user stops at whichever comes first |
| `rampUp` | Seconds over which the users start, evenly; with `rate` and no `stages`, the seconds over which the rate rises from 0 |
| `rate` | Iterations started per second (an arrival rate, [JS-Test-Load](JS-Test-Load.md#rates-and-stages)), with `users` the most at once (by default the highest rate rounded up, which assumes an iteration takes a second or less: give more for longer ones). `iterations` is then the scenario's starts in all. Not with `stages` |
| `stages` | `[{ duration, users }]`, the users over time, or `[{ duration, rate }]`, the rate over time; with no `duration`, the scenario lasts as long as its stages |
| `think` | Between a user's iterations, not timed: milliseconds, `[min, max]` (uniform) or `{ gaussian: [mean, deviation] }` (clamped at 0) |
| `pause` | A wait of the same forms before each request of the scenario, not timed |
| `newSession` | `true` starts each iteration on a fresh `http` session (no cookies, no token) |
| `setup(user, args)`, `scenario(user, args, iteration)`, `teardown(user, args)` | `setup` runs once per user before the timed part, `scenario` repeats, `teardown` runs after. `user` is the user's number from 0 |
| `args` | Copied to each user as JSON |
| `thresholds` | `{ errorRate, p95, p99, names: { "<name>": { p95 } } }` (also `min`, `mean`, `p50`, `p90`, `max`), milliseconds; checked at the end into `thresholds` and `passed`. An unknown metric or a limit that is not a number is refused before the users start |
| `label` | Kept in the result's `run` block |
| `stopAfter`, `stopNowAfter` | Seconds after which the users stop after their current iteration, or before their next request (a refused request is not an error) |
| `timeout` | Seconds before the users are told to stop; `duration + rampUp + 120`, or with only `iterations` 600 (a rate scenario: the time of its last start and 120), by default. A user still running 10 s later is counted as an error |

The functions run from their source text on other threads, so they cannot use the spec's variables: pass what they need
in `args`, or put the load in a load script.

**Load scripts** ([JS-Test-Load](JS-Test-Load.md)). `load({ script: "test/load/Name.load.js",
params, scenarios: { name: { users, iterations, ... } } })` runs a `.load.js` file of the spec's component, whose
`plan` and `scenario` calls that page describes. With `scenarios` (`{ name: settings }`, or `[names]`), only those
named run, with their settings over the script's (null unsets one), and a name the script does not have is refused.
`users`, `iterations`, `duration`, `rampUp`, `think`, `pause`, `rate` and `stages` beside `script` apply to every
scenario; a `duration` given alone replaces the script's `iterations`, and `iterations` alone its `duration`; a `rate`
replaces the script's stages (and, for a closed scenario, its `iterations`, which were each user's), `stages` its
`rate`, and `stages` alone its `duration` and `iterations`. A secret param is left out of the result. `moqui
test-load` runs a script from the command line, and Tools › Load in a server
([Load-Runner](../tools/Load-Runner.md)).

Globals for any spec or load script:

| Global | |
| --- | --- |
| `step(name, fn)` | Runs `fn`, records its whole time as one sample `name` of kind `step` (a JMeter transaction controller), and returns what `fn` returned. The requests inside still count under their own names; a step is not a request in the totals |
| `pause(ms \| [min, max] \| { gaussian: [mean, deviation] })` | A wait that is not timed. A stop ends it |
| `fail(message)` | Ends the iteration as an error with the message, as a failed `assert` does |
| `response.urlParam(name)` | A query parameter of the response's final `url`, else of its `location` (a redirect not followed); null when neither has it. A transition's redirect carries the new record's id this way |
| `service.call(name, parameters, { name, expect })` | In a load run in a server only: the service in process, as the user who started the run ([Load-Runner, Security](../tools/Load-Runner.md#security)). Its out-parameters; the sample is named the service, or `name`. An error ends the iteration unless `expect` lists `"error"`; then the call answers `{ error }`. In a client it throws, naming `http.post` on the service's REST path |

A load user's requests carry `User-Agent: moqui-load/<version>`.

Each request of the scenario is timed in Rust from send to the last byte of the body, and grouped under its `name`
(`GET /rest/s1/moqui/basic/enums` by default: the method and the path without the query; `RAW <target>` for
`http.raw`). An error is a response of 400 and up, a transport error, or a throw in the scenario. With
`expect: [statuses]` on a request, any status outside the list is an error and those in it are not, so a list that
allows a 401 names 200 too if 200 is also fine. An error ends that user's iteration (the request throws in the scenario)
and is counted once; the next iteration starts after the think time.

The result has the totals, `latency`, a row per name, per-second `bins`, `dropped` for a rate scenario,
`thresholds` and `passed`, the `generator` block and the `run` block; its fields are in
[Load-Runner, Reading a result](../tools/Load-Runner.md#reading-a-result), where it also says how approximate its
percentiles are. The table, with the first three
error samples and any broken threshold under it, goes to the console and into the case's JUnit `system-out`. The JSON,
whose `run` block also has `component`, `spec`, `feature`, `server` and `finished`, goes to
`<out>/load/<component>.<Spec>.<feature>.json` (`-2` and on for another load in the same feature), for comparing runs.

```js
feature("mixed reads and writes", {
    when() {
        return load({
            users: Number(params.users ?? 4), duration: Number(params.duration ?? 10), rampUp: 2, think: [50, 200],
            args: { user: "test.load", password: params.loadPassword ?? "TestLoad1!!" },
            setup(user, args) { login(args.user, args.password) },          // once per user, not timed
            scenario(user, args) {                                           // repeated until the duration ends
                http.get("/rest/s1/moqui/basic/enums", { params: { enumTypeId: "UserPreferenceKey" },
                    name: "s1 enums" })
                http.get("/apps/system/Home.qvt", { name: "dashboard qvt" })
                http.post("/apps/setPreference", { form: { preferenceKey: `test.load.${user}`, preferenceValue: "x" },
                    name: "setPreference" })
            },
        })
    },
    then(stats) {
        stats.errors == 0
        stats.names["s1 enums"].p95 < Number(params.maxP95 ?? 2000)
    },
})
```

`parallel(n, fn, args, { timeout })` runs `fn(index, args)` once on each of `n` threads, each with its own engine and
default session, from its source text as in `load`, and returns `{ results, errors }`: a thread that threw leaves null
in `results` and `"thread <i>: <message>"` in `errors`. `timeout` is 120 s by default. It is meant for races such as
simultaneous logins.

A user's thread and engine start in about 7 ms and take about 1 MB, so a few hundred users per process is the scale;
beyond that, run several client processes. A request costs the generator 33 to 45 µs of CPU, about 25,000 a second
from a core, and one process made 77,000 a second of a one-request script on a 24-thread machine
([Load-Runner, Notes on measuring](../tools/Load-Runner.md#notes-on-measuring)). Without `rate` it is a closed model: each user waits for its answer before
the next request.

`stages` and `rate` ([JS-Test-Load, Rates and stages](JS-Test-Load.md#rates-and-stages)) give users over
time and an arrival rate, which keeps the wait for a stalled server in the times (coordinated omission).

**Tarpits.** A load against a server with the demo data is answered 429 by its tarpits. Run it against
`conf/MoquiLoadTestConf.xml`, the production conf with the screen, transition and service tarpits off, as the
framework suite's `--serve` does ([Load-Runner, Notes on measuring](../tools/Load-Runner.md#notes-on-measuring)). As
in Java, REST paths, services, the target screen of each request (when it requires authentication) and every
transition count against tarpits; an in-process `screenTest` render does not.

## Output

The output directory has what the specs wrote and what failures left:

- `response.save(name)` bodies, `page.screenshot(name)` PNGs and `page.pdf(name)` PDFs, under their names;
- `failed-<feature>-<n>.png` and `.txt` for each page open at a failure;
- `load/<component>.<Spec>.<feature>.json` for each load;
- `video/<Spec>.webm` and `video/<Spec>.timeline.json` with `--video`;
- `server-<n>.log`, the output of each `--serve` child.

## Examples in the repository

- `runtime/base-component/framework/test/client/`: the security proofs ported from Moqui Java's pytest suite
  (`SecurityAccess`, `SecurityHeaders`, `SecurityCors`, `SecurityAuthn`, `SecurityScreens`, `SecurityRest`; each
  feature named with its OWASP id and a comment naming the Python test), and `Java-Port-Catalog.md`, which catalogs
  all 143 Python tests as ported, pending or not applicable; `LoadBasic` and `SecurityTarpit` are in
  `framework/acceptance/client/`. The fixtures are `framework/test/ClientFixtures.spec.js`.
- `runtime/component/mr-marble/test/client/`: `BrowserSmoke` (pages of the flow data with their text, PrintOrder's
  barcodes and a PDF), with `test/ClientAccess.spec.js` and `test/MoquiConf.xml` for the server; and in
  `acceptance/client/`, `FindScreens` (a `where` row for each Find screen, found with `files`; `--param screen=` runs
  one).
- `crates/moqui-host/tests/fixtures/test-component/test/client/`: the harness's own specs (HTTP, sessions and the
  token, raw, WebSocket, load and parallel, skip, and a served spec), and `browser-client/Browser.spec.js` over the
  gallery screens and `browser-client/Video.spec.js` with `--video` (`cargo test -p moqui-host --test client_browser`,
  opt-in because it needs Chromium).

## Pitfalls

- `load`, `parallel`, `page.evaluate`, `page.waitFor` and `locator.evaluate` run their function from its source text,
  on another thread or in the page, so it cannot use the spec's variables. Pass what it needs as `args` or `arg`, which
  are copied as JSON, and return values JSON can hold.
- A REST logout marks the account logged out, which ends every session of that user. Do not log out a user other
  features or load users share; a feature that must log out goes last, or uses a user of its own.
- An account locks on its fourth failed login. Prove a refused login with an unknown user, not a wrong password for a
  shared one.
- Redirects are off by default: a screen that needs a login answers 302 to `/Login`, not the login page. Pass
  `follow: true` to follow.
- A POST on a session that has not yet received a token has none to send and is refused (401). `login()` and any GET
  of a page give the session its token.
- `expect: [...]` replaces the 400-and-up rule for that request: list every status that is not an error.
- Locator reads (`text`, `count`, `isVisible`, ...) do not wait. Wait for the element first (`waitFor()`), or poll
  with `waitUntil`.
- Locators are not strict; an action on a locator that matches several elements uses the first.
- `--spec` takes exact names, comma-separated; there are no patterns.
- Timeouts are in seconds everywhere; `think`, `sleep` and `waitUntil`'s `interval` are in milliseconds.
- `--serve` fails a component whose `test/client/suite.js` declares no server, or that has no `suite.js`. A component whose specs all set `server: false` is skipped; those run with `--offline`.
- `test/MoquiConf.xml` merges only in `moqui test` runs (and so in `--serve`); a spec that needs it skips or fails
  against `./moq run`.
- A spec that needs Chromium passes as skipped on a machine without one. Use `--fail-on-skip` in CI.
- The rules of [JS-Testkit](JS-Testkit.md#pitfalls) apply: a line outside `then` and `expect` that starts with `[`, `(`, `/`
  or a template literal continues the line before it.
