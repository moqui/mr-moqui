# Service calls: errors and transactions

What a script calling services needs to know about errors, and the pattern for a call that may fail without failing
the caller.

## An error marks the transaction

A service call that ends with an error in `ec.message` (an `addError`, a parameter that fails validation, a script that
throws, or an error left by a service it called) rolls back the transaction it ran in, as Java's `callSingle` does: one
it began is rolled back, and one it joined (the caller's, as `transaction="use-or-begin"`, the default, joins) is marked
rollback-only (`service/call.rs`).

## What the client sees of a failure

A service whose body fails (a script that throws, an error from Rust) puts the failure's message in `ec.message`, and so
in the JSON answer of a transition, the errors saved for the next page and a toast: the message text only
(`context::client_text`), never the script's stack, the generated line, the service it ran in or the Rust source location
(`Rust: crates/...rs:line:col`). Those are in the server log, with the service's name (`Service ... failed: ...`). The
same holds for a service a script calls that is not there, the error answer of a transition that fails, and the error
page's message.

Taking the messages back does not take the mark back. After

```js
ec.message.pushErrors()
ec.service.sync().name('my.Services.check#Thing').parameters({thingId}).call()   // adds an error
ec.message.clearErrors()
ec.message.popErrors()
```

the caller has no error, but its transaction is still rollback-only: each later service call in it is refused with
"Transaction marked for rollback, not running service ...", and what it wrote is rolled back at the end. The same holds
for a Rust caller that takes the errors back (`service::shell::call_tolerant`).

## A probe call

A call whose failure the caller expects and gets past (one item of many, a check the screen only shows, a refresh that
may fail) runs in a transaction of its own:

```js
ec.message.pushErrors()
let info = null
try {
    info = ec.service.sync().name('org.moqui.impl.FileServices.get#ResourceInfo').parameters({driveId, path})
            .requireNewTransaction(true).call()
    if (ec.message.hasError()) { ec.logger.info(`${path} not read: ${ec.message.errorsString}`); info = null }
} finally {
    ec.message.clearErrors()
    ec.message.popErrors()
}
```

The caller's transaction waits while the probe runs and is untouched by its failure. The probe takes a second database
connection and does not see what the caller wrote and has not committed (a record made earlier in the same transaction
is not there for it), so a probe about the caller's own new work cannot be made this way.

## A check that answers instead

A service whose job is to be asked, such as an access or visibility check, answers with an out-parameter and adds no
error, so a caller asks it in its own transaction, sees its own new work, and has no mark to fear:

| Service | Answer |
|---|---|
| `mantle.party.CommunicationServices.get#RecordLinkInfo` with `checkOnly` | `recordVisible`, and `notVisibleReason` for a log |
| `org.moqui.impl.ChatServices.check#RecordVisible` | `recordVisible` from the theme's `STRT_RECORD_VISIBLE_SERVICE` hook |
| `org.moqui.impl.FileServices.get#DriveAccess`, `check#RecordPaths` | `level`; `visible` of each path |
| `org.moqui.impl.WikiServices.get#WikiPageAccess` | `canView`, `canUpdate`, `canAdmin` |

Write such a service the same way: every refusal an answer, nothing in `ec.message`, and only services that add no error
either on the paths it takes (`get#RecordLinkInfo` with `checkOnly` calls `setup#UserOrganizationInfo`,
`get#UserMailboxAccess`, `check#CalendarAccess` with a list and no permission, and `get#WikiPageAccess`, none of which
adds one there). A hook such as `STRT_RECORD_VISIBLE_SERVICE` is called in the caller's transaction for this reason: it
must see records the caller made, so it must answer with `checkOnly` and not fail.

## Reading a parameter's declaration

`ec.service.getServiceDefinition(name)` gives each `inParameterInfoMap` and `outParameterInfoMap` entry a `parameterNode`,
the declared `<parameter>` element as Java's `ParameterInfo` holds it: its attributes, `<description>`, nested
parameters and validations (`first('matches')`, `children()`). `getInParameter(name)` returns the same node. The
transition metadata of `ec.screen.getTransitionInfo` gives each `parameterMap` entry a `hasDefault`, true when the
transition's `<parameter>` sets a `from` or a `value`.
