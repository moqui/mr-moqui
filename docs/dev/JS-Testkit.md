# Server specs: the `moqui test` API

`moqui test` (`./moq test`) runs the JavaScript spec files of a component's `test/` directory, and with
`--acceptance` those of its `acceptance/` directory. A spec runs inside the server's process, in the same script
engine as the component's services, in an execution context of its own on the run's database. It has the full
script `ec`, and Spock's conveniences: spec and feature setup and cleanup, ordered features with shared state,
`given`/`when`/`then`/`expect` blocks whose conditions fail with the values of their parts, `where` rows, data
checks, pinned sequence ids, a pinned clock, screen and REST renders, and features that run on several threads.

This page is the API a spec is written with. Running the specs (the options of `moqui test`, the files and their
order, the two suites and the regression budget) is in [Build-Test](../Build-Test.md#component-specs). The other
JavaScript test APIs are [JS-Test-Client](JS-Test-Client.md), the client specs of `moqui test-client`, which reach a
running server from outside over HTTP, WebSocket and Chromium and share `spec`, `feature`, the conditions and
`assert` with this page, and [JS-Test-Load](JS-Test-Load.md), the load scripts of `moqui test-load` and Tools ›
Load.

The code is `crates/moqui-devkit/src/testkit/`: `prelude.js` (`spec`, `feature`, the conditions, `skip`, `dec`,
`sum`), `assert.js`, `server.js` (the server helpers), `rewrite.rs` and `lines.rs` (the rewrite of conditions),
`run.rs` (running a spec), `host.rs` and `sandbox.rs`. The agent specs' globals are
`crates/moqui-llm/src/testkit/agents/prelude.js`. The design record is `plans/archive/TestHarness.md`.

## A spec file

```js
const effectiveTime = Date.now()
let cartOrderId                                     // shared between features (Spock's @Shared)

spec({
    dependsOn: ["OrderProcureToPayBasicFlow"],      // run first, and added when this spec is picked with --spec
    tempSequences: { start: 55500, banks: { "mantle.order.OrderHeader": 10 } },   // reset after cleanupSpec
    setupSpec() { ec.user.setEffectiveTime(effectiveTime) },                      // reset after the spec
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("create Sales Order", {
    when() {
        login("joe@public.com")
        const priceMap = call("mantle.product.PriceServices.get#ProductPrice", { productId: "DEMO_1_1" })
        cartOrderId = call("mantle.order.OrderServices.add#OrderProductQuantity", { productId: "DEMO_1_1", quantity: 1 }).orderId
        const dataCheckErrors = checkData(`<entity-facade-xml>
            <mantle.order.OrderHeader orderId="${cartOrderId}" entryDate="${effectiveTime}" grandTotal="200.68"/>
        </entity-facade-xml>`)
        return { priceMap, dataCheckErrors }         // what then() reads
    },
    then({ priceMap, dataCheckErrors }) {
        priceMap.price == 16.99
        dataCheckErrors.length == 0
    },
})

feature("call the REST API (#requestMethod, #screenPath)", {
    where: [
        { requestMethod: "post", screenPath: "s1/mantle/orders/55800/place", containsTextList: ['"statusChanged" : true'] },
    ],
    expect({ requestMethod, screenPath, containsTextList }) {
        const str = screenTest("rest").render(screenPath, null, requestMethod)
        containsTextList.every((text) => str.assertContains(text))
    },
})
```

The file is evaluated once, as the body of a function, so `const`, `let` and functions at its top level are the
file's own and are shared by its features. Evaluating it registers the spec and its features; then `setupSpec`
runs, each feature in the order written, and `cleanupSpec`. A file that does not parse, or registers something
wrong (an unknown field, a `then` with no `when`), fails as a whole before any feature runs, with every problem
listed.

The examples in the repository: `runtime/dev-component/example/test/Example.spec.js` (services and tools),
`runtime/base-component/framework/test/ScreenHtml.spec.js` (a spec with `database: false`), and
`runtime/component/mr-mantle/test`, the largest set, ported from the Java mantle-usl's Spock specs; its `README.md`
maps Groovy to spec files line by line.

### `spec({...})`

Once per file, and it may be left out. A second call, or a field not listed here, is an error when the file loads.

| Field | |
|---|---|
| `dependsOn` | The specs that run before this one in any run, and that `--spec` adds when it picks this one, from its own component or, where that has none of the name, another component. A spec that reads another spec's data names it here, rather than relying on file names or load order. It is read from the source as an array of string literals, without running the file |
| `requires` | Build features the spec needs, such as `["git"]` (`moqui version` lists a binary's; see [Packages](../Build-Test.md#packages)). In a binary without one of them the spec is reported as skipped, not run. Also string literals |
| `database: false` | The spec does not use the entity facade ([Specs with `database: false`](#specs-with-database-false)). A literal |
| `stepwise` | After a failed feature, skip the rest (Spock's `@Stepwise`). Off by default. A skipped feature is not a failure, so the rest still run after one |
| `tempSequences: { start, banks }` | Sequenced ids from `start`, with the bank size for each entity of `banks` (`{ "mantle.order.OrderHeader": 10 }`), set before `setupSpec` and reset after `cleanupSpec`. It calls `ec.entity.tempSetSequencedIdPrimary` and `tempResetSequencedIdPrimary` |
| `allowMessageErrors` | Features may leave errors in `ec.message` ([Conditions](#conditions)) |
| `timeout` | Seconds `setupSpec`, each feature and `cleanupSpec` may take; a feature's own `timeout` wins. No limit when it is left out |
| `setupSpec()`, `cleanupSpec()` | Once before the features and once after. `cleanupSpec` runs whatever happened, also after a failed `setupSpec` |
| `setup()`, `cleanup()` | Before and after each feature. `cleanup` runs whatever happened, after the feature's own `cleanup` |
| `server: false`, `allowPageProblems` | For client specs ([JS-Test-Client](JS-Test-Client.md#files-and-order)) |

`setupSpec`, `cleanupSpec`, `setup` and `cleanup` get no argument. A failed `setupSpec` fails the spec and skips
every feature. `ec.message` is cleared after `setupSpec`.

### `feature(name, {...})`

A statement at the top level of the file; features run in the order they are written. A field not listed here is
an error when the file loads.

| Field | |
|---|---|
| `given(ctx)` | Setup of the feature |
| `when(ctx)`, `then(ctx)` | The action, and its conditions. They come together: one without the other is an error |
| `expect(ctx)` | Conditions alone, in place of `when` and `then`. A feature has `then` or `expect`, not both and not neither |
| `cleanup(ctx)` | Runs whatever happened before it, then the spec's `cleanup` |
| `where` | Rows, each its own case ([`where` rows](#where-rows)) |
| `ignore` | `"reason"`, or `true`: the feature is reported as ignored and not run |
| `requires` | Build features the feature needs, as on `spec()`; in a binary without one it is reported as skipped. A `where` row may have its own `requires`, for a row that needs a feature the others do not |
| `timeout` | Seconds the feature may take, with its setup and cleanup; over the spec's |
| `allowMessageErrors` | This feature may leave errors in `ec.message` |
| `allowPageProblems` | For client specs |

**Blocks and `ctx`.** The blocks of a case run in this order: the spec's `setup`, `given`, `when`, `then` or
`expect`, the feature's `cleanup`, the spec's `cleanup`. After a block fails the later ones are left out, except
the two cleanups. Each block of the feature gets one argument, `ctx`: the fields of the `where` row, and what the
earlier blocks returned. When a block returns an object (not an array), its fields are copied onto `ctx`, so
`when() { return { priceMap } }` is read as `then({ priceMap })`, and a `when` that returns a service's result is
read as `then(out) { out.orderId != null }`. A returned array, number or text is dropped: wrap it in an object.
State that crosses features goes in a `let` at the top of the file.

**The rewrite must see the blocks.** Write `feature(name, { ... })` as a statement at the top level, with `then`
and `expect` as functions in the object literal: a method (`then() {`), `then: function () {` or `then: (...) => {`.
A `then` or `expect` the rewrite cannot see, such as a feature object built in a helper function, is refused when
the file loads rather than run with conditions that cannot fail. Use `where` rows instead of a loop that calls
`feature`.

### `where` rows

`where` is an array of row objects, or a function that returns one, called when the file has loaded (before
`setupSpec`). Each row is its own case, reported and timed on its own, and its fields are on `ctx`. `#field` in the
feature's name is the row's value (Spock's `@Unroll`), shown as a condition's part is, so text is in quotes
(`row "x" plus 2`), and cut at 60 characters; a `#name` the row lacks stays as written. A `where` that throws, or is not an array, is an error when the file loads.
An empty array gives no case.

```js
feature("#path shows #text", {
    where: [
        { path: "Order/FindOrder", text: "Find Order" },
        { path: "Order/OrderDetail", text: "Order", requires: ["pdf"] },
    ],
    expect({ path, text }) { screenTest("apps/marble").render(path).assertContains(text) },
})
```

### Skipped and ignored

| | |
|---|---|
| `ignore: "reason"` | The feature is not run and is reported as ignored |
| `requires: [...]` | On the spec, a feature or a row: skipped in a binary without the build feature, with `requires git, not built into this binary (moqui version)` |
| `skip(reason)` | Called in a block: the feature is reported as SKIPPED with the reason. In `setupSpec` it skips every feature of the spec, and `cleanupSpec` still runs |
| A missing tool | A case that does not say `requires`, and fails only because the binary reports that a tool is not built in (`git is not built into this host`, `scheme git is not supported`, and the same messages for the other tools), is skipped the same way. So is a `setupSpec` that fails that way: its features are skipped |
| No Chromium | A case that fails only because PDF is not available, on a machine with no Chromium (`./moq chromium-download`, or `moqui_chromium_path`), is skipped; the skip reason is that message |

A stepwise file that is entirely about one tool says so with `requires` on `spec()`, so its later features are not
started without their setup.

## Conditions

Each expression statement directly in the body of `then` or `expect` is a condition, as in a Spock `then:` block. A
false one fails the feature and shows the value of each part:

```
Condition not satisfied:

priceMap.price == 16.99
|        |     |
|        16.98 false
{"price":"16.98","priceUomId":"USD"}
```

- `==`, `!=`, `<`, `<=`, `>` and `>=` compare as Groovy does. When either side is a number or a decimal, both are
  compared by value as decimals, number text included (`16.990 == 16.99`, `"16.99" == 16.99`). Lists (arrays and
  entity lists) are equal when their items are, in order, and maps (plain objects and entity values) when their
  keys and values are. `null` and `undefined` are equal to each other and to nothing else. Values that cannot be
  ordered throw `cannot compare ...`.
- `===` and `!==` stay strict JavaScript.
- A bare value, `!x`, and the sides of `&&` and `||` use Groovy truth: `null`, `false`, zero (a number or a
  decimal), and an empty string, list, map or `Set` are false.
- A condition that is a call and returns `undefined` passes, as a void method call does in Spock. So an action in
  `then` that returns nothing (`capture.clear()`) passes without checking anything, while a call that returns a
  value is checked as any condition is: `map.delete(key)` on a missing key returns false and fails the feature. Do
  actions in `when`, and keep `then` and `expect` for conditions.
- An assignment is not a condition. Neither is a declaration (`const out = ...`), an `if`, a loop or a `return`:
  only expression statements are.
- Inside an `if`, a loop or a callback nothing is rewritten: use [`assert`](#assert) there.
- A feature also fails when it leaves errors in `ec.message`, unless `allowMessageErrors` is set on it or on the
  spec; the errors are listed in the failure. Messages are cleared between features.
- What `checkData` found is added to a failed feature's report, each as `data check: ...`.

The source is rewritten for this before it runs (`crates/moqui-devkit/src/testkit/rewrite.rs`), with no new lines,
so the line of an error is the file's line. A condition that spans lines, or has no parts to show, lists its parts
as `part  ->  value` under its text in place of the diagram.

## `assert`

`assert(condition, message)` is a condition anywhere: in a loop, an `if`, a callback or a helper function, where
statements are not rewritten. It uses Groovy truth, fails with the message and the value, and returns true, so a
call of it may also stand as a condition. Its methods follow `node:assert`, each returns true, and each takes a
last `message` that is added to the failure:

| Method | |
|---|---|
| `assert.ok(value, message)` | As `assert`, with Groovy truth |
| `assert.equal(actual, expected, message)`, `assert.notEqual(...)` | The same value by `Object.is`, or not: strict, with none of a condition's Groovy comparison, so a decimal does not equal a number here. For decimals use `assert(a.compare(b) === 0)` |
| `assert.deepEqual(actual, expected, message)` | Equal structure, strictly: arrays by item, objects by their own keys and the same prototype, dates by time, regular expressions by source and flags, everything else by `Object.is` |
| `assert.match(text, pattern, message)`, `assert.doesNotMatch(...)` | The text matches the regular expression, or does not. A value that is not a string, or a pattern that is not a `RegExp`, fails |
| `assert.throws(fn, expected, message)` | `fn` throws. With `expected`: a regular expression the error's message matches, an error constructor the error is an instance of, or a function that returns true for the error |
| `assert.rejects(fnOrPromise, expected, message)` | The promise, or the one the function returns, rejects, with `expected` as for `throws`. In a spec it runs the engine's jobs until the promise settles and returns true; a promise still pending when no job is left fails |
| `assert.fail(message)` | Fails |

A failed `assert` is an assertion failure, as a false condition is, not an error.

## Helpers

`ec` is the full script `ec`, as in a service. The spec engine adds:

| Helper | |
|---|---|
| `call(name, params)` | A service call, `ec.service.sync().name(name).parameters(params).call()`, that returns the service's result. It throws `service <name> failed: <errors>` when the call added errors to `ec.message` or returned null, where `ec.service.sync()` would return null |
| `checkData(xml)` | `ec.entity.makeDataLoader().xmlText(xml).check()`: the list of messages of what did not match, each logged at info. An empty list means the data matches |
| `login(username, password = "moqui")`, `logout()` | `ec.user.loginUser` and `logoutUser`. `login` throws `login of <username> failed: ...` when the login fails or needs a second factor |
| `screenTest(baseScreenPath)` | `ec.screen.makeTest()`, with the base screen path when given ([`screenTest`](#screentest)) |
| `parallel(n, fn, args, opts)` | Run `fn(threadNum, args)` on `n` threads, each with its own `ec` ([`parallel`](#parallel)) |
| `uploadedFile(fileName, text, contentType = "text/plain")` | The `FileItem` that services taking an upload expect, holding the text |
| `setProperty(name, value)` | A process property, as a conf property would set it (`System.getProperty` and `get_prop_or_env` read it); `null` clears it. It holds for the rest of the spec, for a policy switch its features share such as `call_recording_enabled` |
| `withProperty(name, value, fn)` | The property only while `fn` runs, then the value it had before; returns what `fn` returned. For one feature, such as `call_captions_enabled` off |
| `log` | `ec.logger`: `log.info(...)`, `log.warn(...)` |
| `skip(reason)` | Skip the feature, or in `setupSpec` the spec ([Skipped and ignored](#skipped-and-ignored)) |
| `assert(condition, message)` and its methods | [`assert`](#assert) |
| `dec(value)` | The value as a decimal, for arithmetic with its methods: `dec(a).add(b)`, `subtract`, `multiply`, `negate()`, `compare()` |
| `sum(list, field)` | The decimal sum of a field of a list's items, or of the items themselves with no `field`. An array or an entity list; a null or empty value counts as zero |
| `runInContext(source, sandbox)` | Evaluate script text with `sandbox` as its global ([`runInContext`](#runincontext)) |

Every property a spec changed, by `setProperty` or `withProperty`, is put back as it was before the spec after
`cleanupSpec`, and when the spec ends without reaching it.

`ec.user.setEffectiveTime(millis)` pins the clock for `ec.user.nowTimestamp`, date filters with no date, and the
`fromDate` defaults; the runner clears it after the spec.

### The context stack

A spec reads and writes a context name through the stack: `context.name` or `ec.context.get("name")`, and
`context.name = value` or `ec.context.put("name", value)`. `context` and `ec.context` are the same object, the
context stack, with Java's `Map` and frame methods (`get`, `put`, `containsKey`, `keySet`, `remove`, `push`, `pop`).
A bare name is a service's or a screen's script's, not a spec's: in a spec file a bare name is the file's own
variable, a helper of this page, or a global of the engine. The stack takes only data, so a function or a stub
object with methods does not go there: keep it in a variable of the file. The stack is cleared before and after
each spec. `runtime/base-component/framework/test/EcApi.spec.js` shows the calls.

### `screenTest`

`screenTest(baseScreenPath)` is Java's `ScreenTest`: it renders paths under a base screen path as the current user,
with the errors of each render collected on the result instead of thrown. As Java's `ScreenTestImpl`, a test render
is not counted against tarpits. The builder methods return the test:

| Builder | |
|---|---|
| `baseScreenPath(path)`, `rootScreen(location)` | Where the paths start, and the root screen |
| `renderMode(mode)` | The render mode, such as `"html-print"` |
| `baseLinkUrl(url)` | The base of the links the render writes |
| `header(name, value)` | A request header of every render |
| `clientModes("qvue,qjs")` | The `X-Moqui-Client-Modes` header `moqui.loadComponent` sends (a text, or an array), so a spec checks the best client mode: `render("Gallery/QuickTask.qvt").header("X-Moqui-Qvt") == "qvue"` |
| `withStaticData()` | A qvt render of a server-static screen answers its data-free template. With this the output also has the text of the screen's data (its `/actions` and each form-list's `/actions/<form>`), so a spec can look for both |
| `agent()` | Every render as an agent's screen session renders it (an agent's origin, fenced, writes held), so a spec sees what reaches a model: `framework/test/AiOptOut.spec.js` |
| `render(path, parameters, method)` | One render; the result below |
| `renderAll(paths, parameters, method)` | Each path in turn, for the counts |
| `getNoRequiredParameterPaths(screensToSkip)` | The paths below the base screen of the screens with no required parameters |
| `renderCount`, `errorCount`, `renderTotalChars`, `startTime` | Counts over the renders, also as `getRenderCount()` and the like |

| Render result | |
|---|---|
| `output`, `status`, `renderTime`, `screenPath` | The text, the HTTP status, the time and the path asked for |
| `errorMessages` | The errors of the render; a failed render has its failure here |
| `headers`, `header(name)` | The response headers, and one by name in any case, or null |
| `jsonObject` | The output parsed as JSON, or null when it is not JSON |
| `assertContains(text)`, `assertNotContains(text)`, `assertNoErrors()` | True or false, for a condition |
| `getOutput()`, `getErrorMessages()`, `getRenderTime()` | Java's getters |

The rules for renders in the regression suite are in
[Build-Test](../Build-Test.md#rendering-screens-in-tests).

### `parallel`

`parallel(n, fn, args, opts)` runs `fn(threadNum, args)` on `n` threads, `threadNum` from 0, each with its own
execution context and script engine, and returns the array of their results in thread order. `fn` runs from its
source text, so it cannot use the spec's variables: pass what it needs in `args`, which is copied as JSON, and
return values JSON can hold. The helpers of this page are there in each thread. A thread fails when it throws or
leaves errors in its `ec.message`; then `parallel` throws `parallel: 2 of 5 threads failed: ...`. Each thread is
logged out when it ends.

| Option | |
|---|---|
| `transaction` | True by default: the function runs in a transaction of the thread's own. `false` runs it with none |
| `timeout` | Seconds to wait for the threads; 120 by default |

### `runInContext`

`runInContext(source, sandbox)` evaluates script text with `sandbox` as its global and returns its completion
value, as `eval` would. A free name of the source is read from the sandbox, and one the sandbox lacks from the
real global; what the source assigns to a sandbox name, or defines on `globalThis`, is on the sandbox afterwards.
It is how a spec evaluates a module's text with stubs that hold functions (`sri.write`), which the spec's own
global does not take. The screen template specs read the template modules with `ec.resource.getLocationText` and
evaluate them this way. A source that does not parse throws a `SyntaxError`. On an engine without `with` the
source runs as the body of a function whose parameters are the sandbox's names: a function the source made sees
the names as they were passed, and its later assignments to them stay in it.

## Specs with `database: false`

`spec({ database: false })` says the spec does not use the entity facade. In it `ec.entity`, `call` and
`screenTest` throw, and the error names `database: false`, even when the run opened a database. `ec.resource`
stays. `checkData` and `tempSequences` use the entity facade, so they fail in one of these specs.

A run of only such specs uses `--db none`: the factory boots and Postgres is skipped, so a build without
`embedded-postgres` can run it ([Build-Test](../Build-Test.md#running)). With `--db none` every selected spec must
set `database: false`; one that does not is named and the run stops before boot.

The screen template specs `ScreenHtml`, `ScreenQvt`, `DatePeriod` and `FieldTitle` are this kind.

## What the runner resets

Each spec runs in its own execution context, on the one database of the run, so a spec sees what the specs before
it wrote. After a spec the runner puts back what the spec left on its context:

- the effective time (`ec.user.setEffectiveTime`);
- authorization a script disabled (`ec.artifactExecution.disableAuthz`);
- the login: the user is logged out;
- `ec.message`, and the context stack;
- the process properties the spec changed, and the temporary sequences of `tempSequences`;
- the masks of `database: false`.

It then waits up to 5 seconds for queued async service work to finish. The switches of the capture tools below are
not reset: they outlive the spec, so the spec resets them.

## Capture tools

Three tools of the framework let a spec check what the server would send, with nothing sent. A spec gets each with
`ec.factory.getTool(name)`.

### Mail

A spec checks a send that goes, with no mail server, through the mail capture: while it is on, `send#EmailMessage`,
and so `send#EmailTemplate` and the mail client's `send#Draft`, does all of a real send (the `EmailMessage` row, its
`ES_SENT` status and `sentDate`, the mail client's Sent folder) except connect, and keeps the message in memory. The
`EmailServer` must still have an SMTP host, as for a real send (an empty one sends nothing); it is never connected. A
spec turns it on for itself, and the conf property `mail_capture_sends=true` turns it on for a whole run
(`mail_capture_sends=true ./moq test ...`) or a server that must not send mail; it is off otherwise, so a spec that
checks a send that cannot connect sees one. `send#Draft` called in a transaction (as `send#Compose` calls it) only
claims the message there and sends once that commits, on a service worker: such a spec waits for the message's status
before it reads the capture. The tool reads and clears what was captured:

```js
const capture = ec.factory.getTool("MailCapture")
spec({ setupSpec() { capture.enable() }, cleanupSpec() { capture.reset() } })
// in a feature
capture.clear()
const sent = call("org.moqui.mail.EmailClientServices.send#Draft", { emailMessageId: draftId })
capture.last().toList            // ["customer@example.com"]
```

| Method | |
|---|---|
| `getMessages()` | The captured messages, oldest first, each `{subject, fromAddress, fromName, toList, ccList, bccList, replyToList, body, bodyText, messageId, inReplyTo, references, attachmentNames, headers, capturedDate}` (the 500 most recent; `headers` the added header fields, such as a list email's List-Unsubscribe, name to value) |
| `last()`, `count()`, `clear()` | The newest one (or null), how many, and forget them |
| `enable()`, `disable()`, `reset()` | Capture on or off for this server whatever the property says, and back to the property; reset it when the spec ends, as the switch outlives the spec |
| `isEnabled()` | Whether a send now is captured |

The capture is a tool of the framework (`moqui_core::service::mail_capture`, in `MoquiDefaultConf.xml`).

### HTTP fixtures

A spec exercises a provider adapter (a payment, shipping or EDI gateway) through its production code with no network:
while the HTTP fixtures are enabled, every outbound HTTP request of the run's server (`ec.service.rest()` `call()` and
`send()`, `remote-rest` services, `http` locations) is answered by the first fixture it matches, in the order added, and
recorded. A request that matches none fails as a transport error naming it, so nothing goes out. The adapter keeps its
endpoint from its gateway configuration (a sandbox URL), and the fixture matches that URL; the adapter has no test branch.

```js
const http = ec.factory.getTool("HttpFixtures")
spec({ setupSpec() { http.enable() }, cleanupSpec() { http.reset() } })
// in a feature: an answer once, then a lost answer (the request went out, its answer did not come back)
http.add({ method: "POST", url: "https://apitest.example.com/v1/charges", status: 200, contentType: "application/json",
    body: JSON.stringify({ id: "ch_1" }), times: 1 })
http.add({ url: "https://apitest.example.com/v1/*", error: "operation timed out" })
// ... call the Mantle service that calls the adapter, then
http.requests()[0].headers["Idempotency-Key"]
```

| Method | |
|---|---|
| `add({method, url, bodyContains, status, body, headers, contentType, error, times})` | A fixture. `url` is exact, or a prefix when it ends with `*`; `method` and `bodyContains` narrow it; `status` is 200 by default; `error` fails the call as a transport error after recording it; `times` is how many requests it answers (absent or 0, any number) |
| `requests()`, `last()`, `count()` | The recorded requests, oldest first, each `{method, url, headers, body, fixture, at}`, `fixture` the index of the fixture that answered or -1 (the 500 most recent). The values of Authorization, Proxy-Authorization, Cookie and `*Api-Key` headers are `[redacted]` |
| `enable()`, `disable()`, `isEnabled()` | Answer and record; off by default. `enable()` is refused outside a `moqui test` run |
| `allowNetwork(urlPrefixes)` | Let requests to these URL prefixes go to the network while the fixtures are off (see below); an empty list closes it again |
| `clear()`, `reset()` | Forget the fixtures and requests; `reset()` also disables and forgets `allowNetwork`. Reset when the spec ends, as the switch outlives the spec |

The fixtures are a tool of the framework (`moqui_core::service::http_fixtures`, in `MoquiDefaultConf.xml`), on the
transport under `moqui_core::outbound`. They are process-wide, so while they are on a request sent with no execution
context (a worker thread's) is answered or refused too, and only the `moqui test` runner allows enabling them.

A `moqui test` run is closed to the network while the fixtures are off too: a request to anything but a loopback address
(localhost, `*.localhost`, 127.0.0.0/8, ::1) fails as a transport error naming its redacted URL, with a warning in the
log. A spec's gateway records outlive it, and a later spec's job over them (a details poll) would otherwise send their
spec credentials to a provider's sandbox. A spec that needs a real host opens it with `allowNetwork`, and resets after.

### Notifications

A `moqui test` run has no notification socket, and what is sent only to someone listening (the shell counts of
`org.moqui.ui.ShellServices.publish#ShellCounts`) is not sent at all. A spec that checks those notifications turns on
the notification capture, which listens on the notification topic as the server's socket does and keeps each message in
memory:

```js
const capture = ec.factory.getTool("NotificationCapture")
capture.enable()
try {
    // ... what publishes, then (the shell counts go after the commit, on a thread of the server) wait for them
    capture.getMessages("shellCounts").filter((row) => row.userIds.includes(userId)).length
} finally { capture.disable(); capture.clear() }
```

| Method | |
|---|---|
| `getMessages(topic?)` | The captured messages, oldest first, all or of one topic, each `{topic, subTopic, userIds, title, link, message, capturedDate}` (the 500 most recent) |
| `last(topic?)`, `count(topic?)`, `clear()` | The newest one (or null), how many, and forget them |
| `enable()`, `disable()`, `isEnabled()` | Listen or stop; disable it when the spec ends, as the listener outlives the spec |

The capture is a tool of the framework (`moqui_core::notify::capture`, in `MoquiDefaultConf.xml`).

## Agent specs

`moqui test --agents` runs a component's agent tasks instead of its specs: the files `test/agent/*.agent.js`, each a
set of tasks for one agent, run as real runs of the agent on the test database. Running them (the model or a
recording, the options, transcripts, measures and baselines) is in
[Build-Test, Agent evaluation](../Build-Test.md#agent-evaluation). An agent file has the helpers of this page
(`ec`, `call`, `login`, `assert` and the rest) and one more global, `agentSpec`. It has no features: `spec` and
`feature` are not what the harness runs there.

### A task file

```js
agentSpec({
    agent: "assist",             // the agent (AgentDefinition), assist by default
    surface: "drawer",           // drawer (default), chat, tui or job
    user: "john.doe",            // the user the tasks run as, unless a task names its own
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
    tasks: [
        {
            name: "multi-step with approval",
            start: "/apps/system/Home",              // the page the user is on; also { path, parameters, title }
            say: "Change the description of the user group AGENT_EVAL_GROUP to Renamed.",
            setup() { /* rows the task needs, with ec and the spec helpers */ },
            cleanup() { /* remove them */ },
            approvals: ["approve"],                  // scripted answers: "approve", { edit: {...} }, { reject: "why" }
            expect: {
                path: "/apps/system/Access/GroupDetail",
                parameters: { userGroupId: "AGENT_EVAL_GROUP" },
                held: { transition: "updateUserGroup", text: /Renamed/, status: "executed" },
                answer: /saved/i,
            },
            verify(run) { assert(run.calls.length <= 6, "too many calls") },
        },
    ],
})
```

`agentSpec({...})` is called once per file. An unknown field of the spec, a task, a turn or an `expect` is an error
when the file loads.

| `agentSpec` field | |
|---|---|
| `agent` | The agent (`AgentDefinition`); `assist` by default |
| `surface` | `drawer` (the default), `chat`, `tui`, `job`, `a2a` or `mcp`. A headless one sends only the time in the tail |
| `user` | The user the tasks run as; `john.doe` by default |
| `tasks` | The tasks, at least one, each with its own name |
| `timeout`, `requires` | Seconds for a task, and build features the file needs |
| `setup()`, `cleanup()` | Around the file's tasks |

| Task field | |
|---|---|
| `name` | Required. Its lower-case form, words joined by `-`, names the task's transcript and recording |
| `start` | The page the user is on: a path, or `{ path, parameters, title }` |
| `user` | The user this task runs as, over the spec's |
| `say` | One message, or an array of messages, each a turn |
| `turns` | `[{ say, expect }]`, with checks per turn, in place of `say` |
| `approvals` | Scripted answers to the run's approvals, in order: `"approve"`, `{ edit: { field: value } }`, `{ reject: "feedback" }` (or `"reject"`) |
| `answers` | The answers to the agent's questions, in order |
| `expect` | The checks over the whole task (below) |
| `verify(run)` | A function over what the run did (`answer`, `calls`, `held`, `questions`, `screen`, `turns`); `assert` fails it |
| `setup()`, `cleanup()` | Around the task: the rows it needs, with `ec` and the spec helpers |
| `timeout`, `ignore` | Seconds for the task, and a reason not to run it |

Each turn is a run in one conversation, sent with the `<context>` tail the drawer sends (the time and the page). The
checks are deterministic; an LLM judge is not built, and a task with `judge` is refused:

| Check | |
|---|---|
| `path`, `parameters`, `values` | The screen session's path, its parameters, and form values (`{Form: {field: value}}`, or `{field: value}` in any form) when the turn or task ends |
| `answer`, `answerNot` | Patterns the final answer matches, or must not; one or an array |
| `called`, `notCalled` | Tools called: a name, or `{tool, args, result, error}` with `args` matched in part |
| `results` | A pattern, or `{tool, result}`: some result of the tool matches |
| `held`, `noWrite` | The approvals the run asked for (a number, or a match of `{tool, transition, text, status, channel}`: `text` is the summary and the changes, `Label: current → proposed`; `status` what became of it, such as executed, rejected or expired); no approval and no write tool call |
| `maxWrongCalls`, `maxToolCalls`, `questions`, `state` | Limits on failed calls and on calls, the questions asked (a number, or a match of one), the run's end state |
| `noInvented` | The answer links nothing no tool returned (no `invented_reference`) |

A value in a check is a string (equal as text), a `RegExp`, `null` (empty), an object matched in part, or an array
each item of which matches some item. A run that does not end Completed fails its task. A write pauses the run for
its approval, and the task's `approvals` answer them in order as the user would in the drawer: `"approve"`;
`{ reject: "feedback" }`, which the agent reads as its next task; or `{ edit: { field: value } }`, Edit in screen:
the write goes to its screen and the harness saves the form there as the user, the values (by field name or label)
over the proposed ones, so the agent reads the user's own save. A run that waits for an approval the task has no
answer for fails it.

## Porting a Spock spec

How a Moqui Java Spock specification maps to a spec file, as used to port mr-mantle's flows:

| Spock / Groovy | spec file |
|---|---|
| `class X extends Specification` | `X.spec.js` with `spec({...})` |
| `@Shared String cartOrderId` | `let cartOrderId` at the top of the file |
| `def setupSpec()`, `setup()`, `cleanup()`, `cleanupSpec()` | the same names in `spec({...})` |
| `@Stepwise`, `@Ignore("reason")`, `@Unroll` | `stepwise: true` on `spec()`, `ignore: "reason"` on the feature, `#field` in the name |
| `ec.entity.tempSetSequencedIdPrimary(name, 55500, 10)` in setupSpec, reset in cleanupSpec | `tempSequences: { start: 55500, banks: { name: 10 } }` |
| `ec.user.setEffectiveTime(new Timestamp(effectiveTime))` | `ec.user.setEffectiveTime(effectiveTime)` |
| `def "create Sales Order"() { when: ... then: ... }` | `feature("create Sales Order", { when() {...}, then(ctx) {...} })` |
| a local of `when:` read in `then:` | returned from `when()`, destructured in `then({ ... })` |
| `expect:` / `where:` table | `expect(row) {...}`, `where: [{...}, ...]` |
| `thrown(SomeException)` | `assert.throws(() => ..., /message/)` |
| `ec.service.sync().name(n).parameters([a:1]).call()` | `call(n, { a: 1 })` |
| `"""<entity-facade-xml>...${x}...</entity-facade-xml>"""` with `makeDataLoader().xmlText(..).check()` | `` checkData(`<entity-facade-xml>...${x}...`) `` |
| `dataCheckErrors.size() == 0` | `dataCheckErrors.length == 0` |
| `ec.user.loginUser("john.doe", "moqui")` | `login("john.doe")` |
| `logger.info(...)` | `log.info(...)` |
| `new Timestamp(effectiveTime)`, `"${effectiveTime}"` in data | `effectiveTime` (time stamps are epoch millis text in scripts and data) |
| `new Date(effectiveTime + n)` | `new Date(effectiveTime + n)` (converts to a time stamp) |
| `list.sum { it.availableToPromiseTotal }` | `sum(list, "availableToPromiseTotal")` |
| `a + b` on BigDecimal | `dec(a).add(b)`; `.subtract`, `.multiply`, `.negate()`, `.compare()` |
| `Thread.start {}` with a `CountDownLatch` | `parallel(5, (threadNum, args) => {...}, { effectiveTime })` |
| `ec.screen.makeTest().baseScreenPath("rest")` | `screenTest("rest")` |
| `for (x in list) assert ...` | `for (const x of list) assert(...)` |
| a bare context name (`orderId`) after a script ran | `context.orderId` |

## Pitfalls

- Outside a `then` or `expect` block, a line that starts with `[`, `(`, `/` or a template literal continues the line
  before it, since JavaScript inserts no semicolon there: `x = 1` then a line `[a, b].forEach(f)` is one expression.
  Start such a line with `;`, or put the list in a variable. In a `then` or `expect` block each line is its own
  condition, as in Spock: the rewrite ends the line before such a line when it ended an expression, so
  `/^Today · \d+$/.test(label)` on its own line is a regular expression, not a division. A line inside brackets of
  its own (a multi-line object, array or call) is not touched.
- Every expression statement of `then` and `expect` is checked. An action there passes only when it is a call that
  returns `undefined`; one that returns false, zero, null or an empty list fails the feature. Do actions in `when`.
- A condition inside an `if`, a loop or a callback of `then` is not checked: `list.forEach((x) => { x.ok == true })`
  checks nothing. Use `assert` there, or `list.every(...)` as the condition.
- A block's return value reaches `ctx` only when it is an object. `when() { return list }` gives `then` nothing.
- Decimals from services are objects: `+`, `<` and `Number()` on them throw outside a condition. Use their methods
  (`add`, `subtract`, `multiply`, `negate`, `compare`), or `dec`.
- `assert.equal` and `assert.deepEqual` are strict, without the Groovy comparison of a condition: a decimal is not
  equal to a number there.
- `parallel` runs its function from its source text on other threads, so it cannot use the spec's variables. Pass what it
  needs in `args`, which is copied as JSON, and return values JSON can hold.
- A context name is read through `context` or `ec.context`, not as a bare name, and the stack holds only data.
- Time stamps reach scripts and data as epoch millis text: write `entryDate="${effectiveTime}"`, and
  `new Date(effectiveTime + n)` converts to a time stamp.
- A data-dependent service on a product another spec uses can see that spec's inventory and reservations when it runs
  after it; give a new spec its own stock or its own product.
- The switches of `MailCapture`, `HttpFixtures` and `NotificationCapture` outlive the spec: reset or disable them in
  `cleanupSpec` or a `finally`.
- Generic JavaScript linters report `no-unused-expressions` in `then` and `expect`; those expressions are the conditions.
