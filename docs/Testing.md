# Testing components with `moqui test`

`moqui test` runs the JavaScript spec files in a component's `test/` directory. They run in the same script engine as
the component's services, on a database loaded with every data type, with Spock's conveniences: spec and feature
setup and cleanup, ordered features with shared state, `given`/`when`/`then`/`expect` blocks whose conditions fail with
the values of their parts, `where` tables, a suite order, data checks, pinned sequence ids, a pinned clock, screen and
REST renders, and features that run on several threads.

The specs of `runtime/component/mr-mantle-usl/test` are the largest set, ported from mantle-usl's Spock specs; its
`README.md` maps Groovy to spec files line by line. The design record is `plans/archive/TestHarness.md`.

## Running

```sh
./moq test mantle-usl                                  # the release binary and the dev conf
moqui test mantle-usl                                  # every spec, fresh database
moqui test mantle-usl --db snapshot                    # load once, restore the saved dump on later runs
moqui test mantle-usl --spec OrderToCashBasicFlow      # one spec and the specs its dependsOn names
moqui test mantle-usl --spec OrderToCashBasicFlow --feature "ship Sales Order"
moqui test mantle-usl --junit build/test-results       # JUnit XML for CI
moqui test mantle-usl --coverage                       # the services no spec called, per service file
moqui test mantle-usl --keep --log-level info          # keep the scratch runtime, more on the console
```

With no component, every component that has `test/*.spec.js` runs, in load order. `./moq test --debug` uses the debug
binary.

| Option | |
|---|---|
| `--spec Name[,Name]` | The specs to run, by file name without `.spec.js`, with the specs their `dependsOn` names, in suite order. `dependsOn` is resolved within the spec's own component |
| `--feature text` | Only the features whose names contain the text |
| `--db fresh` | The default: load every data type into a scratch runtime, removed at the end unless `--keep` |
| `--db snapshot` | As fresh, but save the load under `runtime/save` and restore it while the entity and data files and the binary are unchanged |
| `--db existing` | The runtime's embedded database as it is. The specs change its data |
| `--junit dir` | `TEST-<component>.<Spec>.xml` per spec |
| `--coverage`, `--coverage-report file` | The services of the components' `service/*.xml` that no spec called, with the total; the report adds call counts |
| `--log-level level` | Console level, `warn` by default. Everything goes to `runtime/log/moqui.log`, whose path the run prints |

The console has a line per feature, and the failures in full at the end. Any failed feature exits 1.

## Files and order

A spec is `<component>/test/<Name>.spec.js`. Its name is the file name. `test/suite.js`, `suite(["SpecA", "SpecB"])`,
gives the order; the other spec files run after it, by name. Each spec runs in its own execution context, on the one
database of the run, so a spec sees what the specs before it wrote.

## A spec file

```js
const effectiveTime = Date.now()
let cartOrderId                                     // shared between features (Spock's @Shared)

spec({
    dependsOn: ["OrderProcureToPayBasicFlow"],      // run first when this spec is picked with --spec
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

### `spec({...})`

Once per file:

- `dependsOn`: the specs that run first when this one is picked with `--spec`.
- `requires`: build features the spec needs, such as `["git"]` (`moqui version` lists a binary's; see
  `docs/Packaging.md`). In a binary without one of them the spec is reported as skipped, not run.
- `stepwise`: after a failed feature, skip the rest (Spock's `@Stepwise`). Off by default.
- `tempSequences: { start, banks }`: sequenced ids from `start`, with the bank size for each entity, set before
  `setupSpec` and reset after `cleanupSpec`.
- `allowMessageErrors`, and `timeout` in seconds per block.
- `setupSpec`, `cleanupSpec`, `setup` and `cleanup`, as in Spock. Cleanups always run. These get no `ctx`.

### `feature(name, {...})`

A statement at the top level of the file; features run in the order they are written.

- Blocks: `given`, then `when` with `then`, or `expect` alone, and `cleanup`, which always runs. Each block gets `ctx`:
  the fields of the `where` row and what the earlier blocks returned.
- `where`: an array of row objects, or a function that returns one. Each row is its own case, and `#field` in the name is
  the row's value.
- `ignore: "reason"`, `timeout`, `allowMessageErrors`.
- `requires: ["git"]`: build features the feature needs, as on `spec()`; in a binary without one it is reported as
  skipped. A `where` row may have its own `requires`, for a row that needs a feature the others do not.
- A case that does not say `requires`, and fails only because the binary reports that a tool is not built in
  (`git is not built into this host`, `scheme git is not supported`, and the same messages for the other tools), is
  skipped the same way. A stepwise file that is entirely about that tool says so on `spec()`, so its later features
  are not started without their setup.
- A feature with `then` and no `when`, or a block name that is not one of these, is an error when the file loads.

## Conditions

Each expression statement directly in `then` or `expect` is a condition, as in a Spock `then:` block. A false one fails
the feature and shows the value of each part:

```
Condition not satisfied:

priceMap.price == 16.99
|        |     |
|        16.98 false
{"price":"16.98","priceUomId":"USD"}
```

- `==`, `!=`, `<`, `<=`, `>` and `>=` compare as Groovy does: decimals, numbers and number text by value
  (`16.990 == 16.99`), and lists and maps by their items. `===` stays strict.
- `!x` and a bare value use Groovy truth: an empty list, map or string, and zero, are false.
- An assignment is not a condition, and a call that returns `undefined` passes.
- Inside an `if`, a loop or a callback, use `assert(condition, message)`.
- A feature also fails when it leaves errors in `ec.message`, unless `allowMessageErrors` is set. Messages are cleared
  between features.

The source is rewritten for this before it runs (`crates/moqui-core/src/testkit/rewrite.rs`), with no new lines, so error
line numbers are the file's. A `then` or `expect` the rewrite cannot see, such as a feature object built in a helper
function, is refused when the file loads rather than run with conditions that cannot fail. Use `where` rows instead.

## Helpers

`ec` is the full script `ec`, as in a service. The spec engine adds:

| Helper | |
|---|---|
| `call(name, params)` | A service call. It throws, naming the service and its `ec.message` errors, where `ec.service.sync()` would return null |
| `checkData(xml)` | `ec.entity.makeDataLoader().xmlText(xml).check()`: the messages of what did not match, each logged |
| `login(username, password = "moqui")`, `logout()` | `login` throws when the login fails |
| `screenTest(baseScreenPath)` | `ec.screen.makeTest().baseScreenPath(...)`. `render(path, params, method)` gives `output`, `errorMessages`, `status`, `renderTime`, `headers` (the response's, and `header(name)` by name in any case) and `assertContains(text)`; `renderMode("html-print")` and the other builder methods set the mode and root screen, `header(name, value)` a request header of every render, and `clientModes("qvue,qjs")` the `X-Moqui-Client-Modes` header `moqui.loadComponent` sends, so a spec checks the best client mode (`render("Gallery/QuickTask.qvt").header("X-Moqui-Qvt") == "qvue"`) |
| `parallel(n, fn, args, opts)` | Run `fn(threadNum, args)` on `n` threads, each with its own `ec`, and return the results. `opts`: `transaction` (default true), `timeout` (default 120 s) |
| `uploadedFile(fileName, text, contentType = "text/plain")` | The `FileItem` that services taking an upload expect |
| `assert(condition, message)` | A condition outside the top level of a block |
| `sum(list, field)`, `dec(value)` | A decimal sum of a field, and a value as a decimal |
| `log` | `ec.logger` |

`ec.user.setEffectiveTime(millis)` pins the clock for `ec.user.nowTimestamp`, date filters with no date, and the
`fromDate` defaults; the runner clears it after the spec. `ec.entity.tempSetSequencedIdPrimary` and
`tempResetSequencedIdPrimary` are what `tempSequences` calls.

## Mail

A spec checks a send that goes, with no mail server, through the mail capture: while it is on, `send#EmailMessage`,
and so `send#EmailTemplate` and the mail client's `send#Draft`, does all of a real send (the `EmailMessage` row, its
`ES_SENT` status and `sentDate`, the mail client's Sent folder) except connect, and keeps the message in memory. The
`EmailServer` must still have an SMTP host, as for a real send (an empty one sends nothing); it is never connected. A
spec turns it on for itself, and the conf property `mail_capture_sends=true` turns it on for a whole run
(`mail_capture_sends=true ./moq test ...`) or a server that must not send mail; it is off otherwise, so a spec that
checks a send that cannot connect sees one. The tool reads and clears what was captured:

```js
const capture = ec.factory.getTool("MailCapture")
spec({ setupSpec() { capture.enable() }, cleanupSpec() { capture.reset() } })
// in a feature
capture.clear()
const sent = call("org.moqui.impl.EmailClientServices.send#Draft", { emailMessageId: draftId })
capture.last().toList            // ["customer@example.com"]
```

| Method | |
|---|---|
| `getMessages()` | The captured messages, oldest first, each `{subject, fromAddress, fromName, toList, ccList, bccList, replyToList, body, bodyText, messageId, inReplyTo, references, attachmentNames, headers, capturedDate}` (the 500 most recent; `headers` the added header fields, such as a list email's List-Unsubscribe, name to value) |
| `last()`, `count()`, `clear()` | The newest one (or null), how many, and forget them |
| `enable()`, `disable()`, `reset()` | Capture on or off for this server whatever the property says, and back to the property; reset it when the spec ends, as the switch outlives the spec |
| `isEnabled()` | Whether a send now is captured |

The capture is a tool of the framework (`moqui_core::service::mail_capture`, in `MoquiDefaultConf.xml`).

## Notifications

A `moqui test` run has no notification socket, and what is sent only to someone listening (the shell counts of
`org.moqui.impl.ShellServices.publish#ShellCounts`) is not sent at all. A spec that checks those notifications turns on
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

## Agent evaluation

`--agents` runs a component's agent tasks instead of its specs: the files `test/agent/*.agent.js`, each a set of tasks
for one agent, run as real runs of the agent on the test database, with the agent engine started and the screen
sessions installed as a server has them (`plans/LLM-A2A-Assist.md`, section 9). The model is a live one or a recording:

```sh
moqui test tools --agents --llm-profile local-qwen --runs 3 --seed 7   # a model of the llm-facade conf
moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay   # no model
moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay-small   # a small model in JSON tool mode
moqui test tools --agents --llm-profile local-qwen --feature "approval" --db snapshot
```

| Option | |
|---|---|
| `--llm-profile model` | The model the agent runs on: a `model` id of the `llm-facade` conf or an `LlmModel` row. The agent's feature (`assist`) is bound to it for the run and put back after. When the model's endpoint does not take a connection the run is skipped, not failed. An unknown id is an error |
| `--replay dir` | No model: the model replies recorded in `<dir>/<spec>/<task>-<n>.jsonl` (else `<task>-1.jsonl`) are played by the scripted fake provider, with the recorded model's settings, and the tools run live. The replay model makes no side call (`sideCalls` N: no intent hint, the deterministic compaction summary, no schema repair call), since a recording holds only the steps' replies. The directory is a profile's set: `replay/` is the profile `replay`, another `replay-<name>/` (or `<name>/`) the profile `replay-<name>`, with its own baseline; a task the set has no recording of is skipped |
| `--runs N` | Runs of each task, for pass@1 and pass^N. 1 by default |
| `--seed S` | Run n sends `seed` S + n - 1 to a Chat Completions model (in the model row's `apiConfigJson`; other protocols take none) |
| `--min-pass F` | The pass@1 a live run needs, 0.75 by default (6 of 8). A replay needs every task to pass |
| `--write-baseline` | Write `test/agent/baseline-<profile>.json` (`baseline-replay.json`, `baseline-replay-small.json` for the replay sets) from this run |

`--spec` picks agent spec files by name (without `.agent.js`), `--feature` tasks by name, and `--db` works as for the
specs. A live profile is a model of the conf, such as the commented `local-qwen` example of `MoquiDefaultConf.xml` (Qwen
in LM Studio at `http://127.0.0.1:1234`): copy it into the runtime's conf, or set the default endpoint's
`llm_openai_url` and `llm_openai_model` and use `--llm-profile default`.

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
            start: "/apps/system/dashboard",         // the page the user is on; also { path, parameters, title }
            say: "Change the description of the user group AGENT_EVAL_GROUP to Renamed.",
            setup() { /* rows the task needs, with ec and the spec helpers */ },
            cleanup() { /* remove them */ },
            approvals: ["approve"],                  // scripted answers: "approve", { edit: {...} }, { reject: "why" }
            expect: {
                path: "/apps/system/Security/UserGroup/UserGroupDetail",
                parameters: { userGroupId: "AGENT_EVAL_GROUP" },
                held: { transition: "updateUserGroup", text: /Renamed/, status: "executed" },
                answer: /saved/i,
            },
            verify(run) { assert(run.calls.length <= 6, "too many calls") },
        },
    ],
})
```

A task has `say` (one message, or an array of messages) or `turns`, `[{ say, expect }]` with checks per turn, and
`answers`, the answers to the agent's questions in order. Each turn is a run in one conversation, sent with the
`<context>` tail the drawer sends (the time and the page). The checks are deterministic; an LLM judge is not built:

| Check | |
|---|---|
| `path`, `parameters`, `values` | The screen session's path, its parameters, and form values (`{Form: {field: value}}`, or `{field: value}` in any form) when the turn or task ends |
| `answer`, `answerNot` | Patterns the final answer matches, or must not |
| `called`, `notCalled` | Tools called: a name, or `{tool, args, result, error}` with `args` matched in part |
| `results` | `{tool, result}`: some result of the tool matches |
| `held`, `noWrite` | The approvals the run asked for (a number, or a match of `{tool, transition, text, status, channel}`: `text` is the summary and the changes, `Label: current → proposed`; `status` what became of it, such as executed, rejected or expired); no approval and no write tool call |
| `maxWrongCalls`, `maxToolCalls`, `questions`, `state` | Limits on failed calls and on calls, the questions asked, the run's end state |
| `noInvented` | The answer links nothing no tool returned (no `invented_reference`) |
| `verify(run)` | A function over what the run did (`answer`, `calls`, `held`, `questions`, `screen`, `turns`); `assert` fails it |

A value in a check is a string (equal), a `RegExp`, `null` (empty), or an object or array matched in part. A run that
does not end Completed fails its task. A write pauses the run for its approval, and the task's `approvals` answer them
in order as the user would in the drawer: `"approve"`; `{ reject: "feedback" }`, which the agent reads as its next task;
or `{ edit: { field: value } }`, Edit in screen: the write goes to its screen and the harness saves the form there as
the user, the values (by field name or label) over the proposed ones, so the agent reads the user's own save. A run
that waits for an approval the task has no answer for fails it.

### Transcripts, measures and baselines

Each task run writes `runtime/log/agent-eval/<run>/<spec>/<task>-<n>.jsonl`: a header with the model's settings, then
the user's messages, each model reply (text, tool calls, usage) with its calls and their full results, the held writes,
the questions, the answer and the screen, and the result with its measures. The console has a line per task run, and a
summary per component, also written as `summary-<component>.json`: pass@1, pass^k, model and tool calls, wrong calls,
argument repairs, invented references, held writes against the expectation, the wrong calls' share of the tool calls, prompt, completion and cached tokens, the
cache-hit ratio, the prompt prefix size, compactions and wall time.

A transcript is also a recording: `--replay` takes its `model` lines (a line's `raw`, the text a model wrote in JSON tool
mode or a call it wrote as text, is played as that text) and compares its `tool` lines with what the tools
answer now, dates, clock times and long ids left out. A change shows as a `replay drift` note, so a change of a tool's
output is seen without a model; the checks decide whether it fails. To record a live run, copy its transcripts into
the replay directory.

A committed `test/agent/baseline-<profile>.json` holds each task's pass@1 and tokens a run, and the suite's wrong-call
share, invented references and cache-hit ratio. A run with lower success, or more than 20% more tokens, than the baseline
prints a `REGRESSION` line and fails; so does a task of the baseline that did not run, unless `--feature` or `--spec`
picked some. The tools component has `baseline-replay.json` (24 tasks) and `baseline-replay-small.json` (9, JSON tool
mode); the baselines of a live model (`baseline-local-qwen.json`, an 8B model's in JSON mode) are written by a live run
with `--write-baseline`.

The tools component's suite (`test/agent/`): `assist.agent.js` (21 tasks in the drawer) and `headless.agent.js` (3 on
the `a2a` surface, run through the run engine; the spec lets the assist agent serve that surface while it runs). The
surface of a spec is `drawer`, `chat`, `tui`, `job`, `a2a` or `mcp`; a headless one sends only the time in the tail.

## Pitfalls

- Outside a `then` or `expect` block, a line that starts with `[`, `(`, `/` or a template literal continues the line
  before it, since JavaScript inserts no semicolon there: `x = 1` then a line `[a, b].forEach(f)` is one expression.
  Start such a line with `;`, or put the list in a variable. In a `then` or `expect` block each line is its own
  condition, as in Spock: the rewrite ends the line before such a line when it ended an expression, so
  `/^Today · \d+$/.test(label)` on its own line is a regular expression, not a division.
- Decimals from services are objects: `+`, `<` and `Number()` on them throw outside a condition. Use their methods
  (`add`, `subtract`, `multiply`, `negate`, `compare`), or `dec`.
- `parallel` runs its function from its source text on other threads, so it cannot use the spec's variables. Pass what it
  needs in `args`, which is copied as JSON, and return values JSON can hold.
- Time stamps reach scripts and data as epoch millis text: write `entryDate="${effectiveTime}"`, and
  `new Date(effectiveTime + n)` converts to a time stamp.
- A data-dependent service on a product another spec uses can see that spec's inventory and reservations when it runs
  after it; give a new spec its own stock or its own product.
- Generic JavaScript linters report `no-unused-expressions` in `then` and `expect`; those expressions are the conditions.
