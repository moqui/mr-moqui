# JavaScript in services

A service's logic is JavaScript on the thread's script engine: inline XML actions that are compiled to a script, a
`.js` file, or a script module the actions call. This guide covers what such a body sees, how it returns results, how
it calls other services, and what its errors, transaction and authorization do. How to organize services and when to
use a module is in [Practices-App-Dev](Practices-App-Dev.md#4-services-and-scripts). The XML action elements are in
[JS-XML-Actions](JS-XML-Actions.md), and the members of `ec` in [JS-Core-API](JS-Core-API.md).

## Implementation types

The `type` attribute of `<service>` chooses the runner (`service/define.rs` `load_service` and `kind_of`,
`service/call.rs` `execute`). It is `inline` when left out.

| `type` | |
|---|---|
| `inline` | The `<actions>` of the definition, compiled to one script (`run_inline`) |
| `script` | The `.js` file at `location` (`run_script`). A `.groovy` location loads, and a call fails with "runs ..., a script language this port does not run" |
| `entity-auto` | `create`, `update`, `store` or `delete` of the noun's entity, no script. An undefined `create#Entity` name runs the same way |
| `interface` | Parameters only, for `<implements service="..."/>`. A call is an error |
| `remote-rest` | An HTTP call to `location` with the service's `method`, else its verb when that is an HTTP method, else POST |
| `rust` | A function in the Rust service registry, or of a linked native component, named by `location` ([Native-Components](Native-Components.md)) |
| a native runner's type | A type that a linked component registers a runner for |

`java`, `camel` and `remote-json-rpc` are refused when the file loads. A `rust` or native service with `<actions>` is
refused too: the registered function is the body.

The JavaScript of a service is checked when its file loads: each `from`, `condition`, `default` and `in-map`
attribute, each `<script>` body, and the script generated from the `<actions>`. A service that fails is not
registered, and the failure names it. `moqui lint [service-file]` runs the same check.

A module is not a service type. It is a `.js` file an inline service calls from a `<script>` element
([Script modules](#script-modules)).

## What the body sees

An inline or script body runs in an isolated context: a new context on the stack that hides the caller's names, so a
service sees its own parameters and nothing of its caller (`IsolatedContext`, as Java's `ContextStack.pushContext`).

| Name | |
|---|---|
| each in-parameter | A bare name, and `context.name`. Both read the same stack |
| `context` | The context stack itself, the same object as `ec.context`, with Java's Map methods and `push`/`pop` |
| `ec` | The execution context ([JS-Core-API](JS-Core-API.md)) |
| helper globals | `toDecimal`, `toNumber`, `basicConvert`, `compare`, `orderMapList`, `filterMapList`, `filterMapListByDate`, the utility objects ([JS-Globals](JS-Globals.md)) |

- **Undeclared in-parameters are dropped.** The call keeps only the declared in-parameters (and, for an entity-auto
  service, a declared field's `<field>_changeReason`). A service with `validate="false"` keeps every name it was
  given, and still converts the declared ones.
- **An unset name is null.** Reading a name nothing set gives `null`, not a `ReferenceError`. `typeof unset` is
  `"object"`. Test presence with `== null`.
- **A bare assignment sets the context.** `total = 5` writes the top frame of the stack. A `const`, `let`, `var` or
  `function` declaration is a local of the script and is not in the context.
- **A list or map read by name can be changed in place.** `items.push(x)` on a bare name, or on `context.items`, is
  written back to the stack.
- **A built-in wins over a context name.** `Array`, `Math`, `JSON`, `ec` and the helper globals are found before the
  stack, so a parameter named after one is read as `context.name`. The stack refuses the name `context`.
- **There is no `sri`** outside a screen render, and no `result` map ([Results](#results)).

A `type="script"` file runs as the body of a function, as a `<script>` element does: `return` ends it, and its last
expression statement is its value (`script/parse.rs` `file_function`). A line number in an error is the file's.

## Results

When the body ends, each declared out-parameter is read by name from the body's context (`call.rs`
`collect_outputs`). That is the only way an inline or script body returns a value: assign the bare name.

```xml
<service verb="get" noun="GalleryCallContext">
    <implements service="org.moqui.comms.ProviderInterfaces.call#Context"/>
    <actions><script><![CDATA[
        contactList = []
        recordList = []
        const n = String(number || '')
        if (!n.startsWith('+15550100')) return
        contactList = [{name: 'Gallery caller ' + n.slice(-4), kind: 'person', url: '/qapps/gallery/Widgets', detail: 'Example contact'}]
        recordList = [{title: 'Example record for ' + n, status: 'Open', url: '/qapps/gallery/Widgets'}]
    ]]></script></actions>
</service>
```

`number` is an in-parameter and `contactList` and `recordList` are out-parameters, all from the interface the service
implements; `n` is a local (`runtime/components/dev/example/service/example/ExampleServices.xml`).

- An out-parameter the body did not set is left out of the results.
- A name the body set that is not a declared out-parameter is not returned. An inline or script service with no
  out-parameters returns an empty map.
- `const contactList = ...` in a `<script>` would return nothing: it is a local. Assign the bare name.
- There is no `result` map. `result` is an ordinary context name, null unless something set it, and setting
  `result.x` returns nothing unless the service declares an out-parameter named `result`.
- A Rust, native, entity-auto or remote service returns what its runner returns, filtered to the declared
  out-parameters when there are any (`project`). With none declared, everything the runner returned is passed on.
- A `post-service` SECA rule may add to or change the results before they are filtered.

## Script modules

A module is a script file whose last expression is its value. `ec.resource.script(location)` runs the file and returns
that value (`script/oxide/resource.rs`). By convention the value is an object of functions:

```js
function available(scope) { ... }
function issue(scope, request = {}, operationKey = null) { ... }
;({available, issue})
```

The leading `;` keeps the object literal from continuing the line above it. A service calls a module from its
actions:

```xml
<actions><script>ec.context.putAll(ec.resource.script('component://mr-mantle/service/mantle/account/GatewayConnections.js').execute('runGatewayConnectionAdapter', context))</script></actions>
```

`execute(op, context)` here is the module's own function: it picks an operation, reads the parameters it is given and
returns a map, which `putAll` sets in the context so that the declared out-parameters are found there.

- The file runs in the caller's context: its bare names are the caller's, and its errors are the caller's to catch.
- **An inert module is kept.** A module whose top level only declares gives the same value for the rest of the
  outermost script run on the thread (the request, job or call), made once (`script/parse.rs` `module_is_inert`). Inert
  means each top-level statement is a function declaration, a `const` of a function, a literal, a table, `Set` or `Map`
  of literals, another module by `ec.resource.script("...")` with a literal location, or names read from those; and
  the last statement gives back an object of them, a name or a function.
- A `let`, a `var`, a loop or any other call at the top level keeps the file out: it is evaluated again on each
  `ec.resource.script` call. So module-level state never lasts past the run, and a per-call cache belongs in the call.
- The `script_module_cache` property set to `false` turns the keeping off.
- A file's text is read again when the file changes (checked every two seconds; never in production).

`<script location="..."/>` in actions runs a file the same way and discards its value.

## Calling other services

A script calls a service through a builder on `ec.service`; an XML `service-call` compiles to the same call. The
member tables are in [JS-Core-API](JS-Core-API.md).

| Pattern | |
|---|---|
| `ec.service.sync().name(n).parameters(map).call()` | Run now, on this thread. Returns the results map |
| `ec.service.async().name(n).parameters(map).call()` | Run on a service worker. `callFuture()` returns a future with `isDone()` and `get()` |
| `ec.service.special().name(n).parameters(map).registerOnCommit()` | Run when the current transaction commits, or `registerOnRollback()` when it rolls back |
| `ec.service.job(jobName).parameters(map).run()` | Run a `ServiceJob` now |

```js
const info = ec.service.sync().name('org.moqui.resource.FileServices.get#ResourceInfo')
        .parameters({driveId, path}).requireNewTransaction(true).call()
```

Options on the sync builder set the call's transaction (`ignoreTransaction`, `requireNewTransaction`,
`useTransactionCache`, `transactionTimeout`), its authorization (`disableAuthz`), and its error handling
(`ignorePreviousError`, `softValidate`, `throwOnError`). `multi(true)` runs the service once for each row of
`name_0`, `name_1` parameters, in one transaction.

`parameters(ec.context)` passes the whole context. The service still receives only what it declares.

## Errors and messages

A service fails in one of three ways, and in each the caller gets a results map back and the errors in `ec.message`
([Services](../logic/Services.md)).

| The body | The call |
|---|---|
| adds an error (`ec.message.addError(text)`, `<message error="true">`, `<return error="true" message="..."/>`) | returns the results the body set; the transaction is rolled back or marked rollback-only |
| throws | returns an empty map; the error's message is added to `ec.message`; the transaction is rolled back or marked |
| is not run because an in-parameter failed validation | returns an empty map; each failure is an error and a validation error with its field |

- **A failure names its service in the log.** A script error has "in service `<name>`" added after its message
  (`call.rs` `in_service`) and is logged as `Service <name> failed: ...` with the generated line and the stack. The
  client and `ec.message` get the message text only.
- **A service does not run when there is already an error.** A call made while `ec.message` has an error returns an
  empty map at once, with a warning in the log. `.ignorePreviousError(true)` sets the earlier errors aside for the
  call and puts them back after it.
- **An XML `service-call` returns from the actions** when the call leaves an error (`if (ec.message.hasError())
  return;`), unless it has `ignore-error="true"`, which logs and clears the errors.
- **`throwOnError(true)`** makes a failed body throw in the caller instead of only recording the error.
- **An authorization failure** is always thrown on to the caller.
- **`softValidate(true)`** turns the validation errors of that call into messages of type `danger`.
- `ec.message.errors`, `messages` and `validationErrors` are arrays. `ec.message.hasError()` is true for an error or a
  validation error, since a validation error is also in `errors`.

Clearing the errors does not clear the transaction's rollback mark; see [A probe
call](../logic/Services.md#a-probe-call) for a call whose failure the caller gets past.

## Transactions

The `transaction` attribute of the service says what its call does with the transaction
([Services](../logic/Services.md)).

| `transaction` | |
|---|---|
| `use-or-begin` | The default: join the caller's transaction, or begin one |
| `ignore` | Neither begin nor join one |
| `force-new` | Suspend the caller's and run in a new one |
| `cache` | As `use-or-begin`, holding writes in the [transaction cache](../logic/Services.md#transaction-cache) |
| `force-cache` | As `force-new`, with the transaction cache |

`transaction-timeout` is in seconds. `no-tx-cache="true"` writes the held rows and turns the cache off for the rest
of the transaction. A `transaction` attribute on a `service-call`, or the builder's options, overrides the
definition for that call.

A service that refuses with an error and is called from a `pre-service` SECA rule needs `transaction="force-new"`.
Otherwise it joins the guarded service's transaction and marks it rollback-only, and the user sees "Transaction marked
for rollback" beside the refusal ([JS-XML-Actions](JS-XML-Actions.md#rule-actions)).

## Authorization

| | |
|---|---|
| `authenticate` on the service | `true` (the default): a user must be logged in, and the service is checked as an artifact. `false`, `anonymous-all` and `anonymous-view` need no user |
| `.disableAuthz()` on a call, `disable-authz="true"` on an action | Authorization is off for that call or that entity operation |
| `ec.artifactExecution.disableAuthz()` / `enableAuthz()` | Off until turned on again. `disableAuthz()` returns whether it was already off, so restore only when it was not |
| `ec.artifactExecution.runAsSystem(() => ...)` | The function runs with no user and authorization off, in the same transaction |

A running service is on the artifact stack, so the entity operations and service calls it makes inherit its
authorization. A call with authorization disabled needs no logged-in user.

`runAsSystem` works only in a service's own code: the top artifact frame must be the service's. A screen's actions,
widget expressions and transitions, a REST resource and a web script are refused ([The system
scope](../security/Attack-Surface.md#the-system-scope)).

## Parameters: validation and types

Before the body runs, each declared in-parameter is given its default, checked and converted (`service/validate.rs`
`clean`):

1. A blank value takes the parameter's `default` (a JavaScript expression over the parameters so far) or
   `default-value` (text with `${}` expanded).
2. `required="true"` with nothing left is "Field cannot be empty".
3. The value is converted to the parameter's `type`. Text for a number or a date is parsed with the parameter's
   `format`, in the user's locale and time zone. A value that does not convert is an error naming the type.
4. An empty string with no default, for a type other than `String`, becomes null.
5. Text with `<` is an error unless `allow-html` permits it; the declared validations run (`matches`,
   `number-range`, `text-length` and the rest).

Every parameter is checked, and each failure is a validation error for its field, so a form shows them all. With
`validate="false"` only steps 1 and 3 run. A `Map` parameter with nested `<parameter>` elements is checked entry by
entry.

The converted value reaches the script as:

| Parameter `type` | In the script |
|---|---|
| `String` | A string |
| `Boolean` | `true` or `false` |
| `Integer`, `Long`, `Float`, `Double` | A number. A `Long` past 2^53 cannot be passed: "integer is too large for a script number" |
| `BigDecimal` | A host decimal, not a number ([JS-Globals](JS-Globals.md)). Use `add`, `subtract`, `multiply`, `divide`, `compare`, `eq`, `isZero`, `toNumber`; operators and `valueOf` throw, and a zero decimal is truthy |
| `Timestamp`, `Date`, `Time` | Text of the epoch milliseconds (`"1265198400000"`). Format it with `ec.l10n.format(value, pattern)`; an entity field converts it back on a write |
| `List`, `Set`, `Collection` | An array. Text given for one is split into a list |
| `Map` | A plain object |
| an entity value or list | A host object: fields as properties, the entity list's `size()`, `isEmpty()`, `map`, `filter` |
| bytes | A `Uint8Array` |

The same conversions apply to what a service returns to a script that called it.

## Engine limits

Each thread has its own script runtime (`script/oxide/engine.rs`, `script/run.rs`). A service a script calls runs in
the same runtime, nested.

| Limit | | When passed |
|---|---|---|
| Heap | 64 MiB for each thread's runtime | `Error: out of memory` |
| Time | The transaction's time remaining, or the service's `transaction-timeout`. Outside a transaction: `script-timeout` of `resource-facade` in the conf, else on a request thread the request timeout (`webapp_request_timeout`, 300 s), else none (a job or service worker) | `Error: interrupted` |
| Nesting | 64 scripts deep | "scripts are nested too deeply" |
| Native stack | Half the thread's stack, as the framework declares it to the engine (`script::declare_thread_stack`), for the outermost script and the scripts it nests: 4 MiB on a request worker (8 MiB), 1 MiB on a service or job worker (2 MiB; 4 MiB in Debug, whose workers have 8 MiB for its larger frames), on the main thread of `moqui load` and `moqui test`, half the stack limit (`ulimit -s`) less the arguments, the environment and 1 MiB, so about 3.5 MiB at the default 8 MiB (not declared when the limit is `unlimited` or under 3 MiB). 1 MiB on a thread not declared | "scripts are nested too deeply for the thread's stack", or the engine's "stack overflow" |
| Value nesting | 200 levels of arrays and objects in a value that crosses to the framework | "value is cyclic or nested too deeply" |

There are no instruction or iteration counts: a loop is stopped only by the time limit. The engine interprets
bytecode with no JIT, so heavy loops belong in the framework's helpers or in Rust ([Heavy
work](Practices-App-Dev.md#heavy-work-scripts-or-rust)).

Generated scripts are cached by their XML text. `MOQUI_ACTIONS_DEBUG=1` adds a comment naming the XML element before
each generated statement, and `MOQUI_ACTIONS_DEBUG=dump` also prints each generated script with line numbers.

## Typed services

`moqui types [--out dir]` writes TypeScript declarations for the loaded definitions, by default into `types/` in the
runtime directory. `services.d.ts` has the input and output map of each defined service: an input key is required when
the parameter is `required="true"` with no default, and `auto-parameters` are expanded against the entity model. The
implicit entity-auto services are mapped types over the entity map. See [JS-Types-Tooling](JS-Types-Tooling.md).

## Pitfalls

- A declaration (`const x`) in a `<script>` hides the context name `x` for the rest of the actions: a later
  `<set field="x">` assigns the local, and the out-parameter `x` is never set.
- Two `<script>` elements directly under `<actions>` that declare the same `const` or `let` do not parse: they share
  one function.
- `if (amount)` on a decimal is always true, and `amount == 0` throws. Use `amount.isZero()` or `amount.compare(0)`.
- An empty array or object is truthy. Test `list.length`.
- The context holds data, not functions: do not `set` a name to an arrow function. Keep a function in a local
  (`const`) or in a module.
- Matching on an error's wording breaks when the wording changes. Return a code and test that.
