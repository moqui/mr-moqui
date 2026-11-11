# The `ec` API of server scripts

`ec` is the execution context of the thread of work a script runs in: the user, the messages, the transaction, the
context stack, and the facades for entities, services, resources, screens and the web request. It is Moqui Java's
`ExecutionContext` with the same member names, read from JavaScript. This page lists every facade of `ec` and every
object a facade returns, member by member, and then how they differ from Moqui Java.

`ec` is a global of the server's script engine. It is there in a service body (inline actions, a `.js` file, a script
module), in XML actions compiled to a script, in a screen's actions, transitions and widget expressions, in a trusted
template, and in a server spec of `moqui test`. A locked template has only the data it was given
([JS-Fancy-Templates](JS-Fancy-Templates.md#locked-templates)), and a server-static screen in the browser has a small
`ec` of its own ([JS-Screen-Scripts](JS-Screen-Scripts.md#in-the-browser)).

Related pages: [JS-Globals](JS-Globals.md) (`Decimal`, timestamps, the utility objects, `MNode`),
[JS-Service-Scripts](JS-Service-Scripts.md), [JS-XML-Actions](JS-XML-Actions.md),
[JS-Screen-Scripts](JS-Screen-Scripts.md), [JS-Templates-Macros](JS-Templates-Macros.md) (the `sri` object),
[JS-Fancy-Templates](JS-Fancy-Templates.md), [JS-Framework-Services](JS-Framework-Services.md),
[JS-Testkit](JS-Testkit.md), [JS-Types-Tooling](JS-Types-Tooling.md) (the typed declarations of everything here),
[Web-Scripts](../interfaces/Web-Scripts.md) (the request and response of a web script),
[Services](../logic/Services.md) (errors and transactions of a service call),
[Attack-Surface](../security/Attack-Surface.md) (`runAsSystem`) and [Native-Components](Native-Components.md) (tools).

Contents: [Conventions](#conventions) · [`ec`](#ec) · [`ec.context`](#eccontext) · [`ec.user`](#ecuser) ·
[`ec.message`](#ecmessage) · [`ec.l10n`](#ecl10n) · [`ec.resource`](#ecresource) · [`ec.logger`](#eclogger) ·
[`ec.cache`](#eccache) · [`ec.transaction`](#ectransaction) · [`ec.artifactExecution`](#ecartifactexecution) ·
[`ec.entity`](#ecentity) · [`ec.service`](#ecservice) · [`ec.screen`](#ecscreen) · [`ec.web`](#ecweb) ·
[`ec.search`](#ecsearch) · [`ec.factory`](#ecfactory) · [`ec.makeNotificationMessage()`](#ecmakenotificationmessage) ·
[`ec.llm`](#ecllm) · [Differences from Moqui Java](#differences-from-moqui-java)

## Conventions

**Two forms.** Groovy reads a Java getter as a property or as a method, and scripts ported from it use both. Each
facade of `ec` has both forms of each of its members: a getter or value `x` also has `getX()`, and a `getX()` or
`isX()` that needs no argument also reads as the property `x` (`ec.transaction.transactionInPlace`). So `ec.user.userId` and `ec.user.getUserId()` are the same, as are
`ec.message.errors` and `ec.message.getErrors()`, `ec.factory.moquiVersion` and `ec.factory.getMoquiVersion()`. `ec`
itself has `ec.getUser()`, `ec.getMessage()` and the rest, which return `ec.user`, `ec.message` and the rest. A `getX`
that needs an argument (`ec.user.getPreference(key)`) is a method only. The tables below list one form; the other is
there. The rule covers the facades, not the objects they return (a find, an entity value, a REST response), whose
tables list what they have.

**The context.** `ec.context` is the same object as the bare name `context`, and a name in it is also a bare name:
`context.orderId`, `ec.context.get("orderId")` and `orderId` read the same value.

**Values.** A decimal field or parameter is a `Decimal` host value, not a JavaScript number, and a timestamp is
ISO-8601 text; [JS-Globals](JS-Globals.md) has both. A Java `Map` is a plain object, a `List` an array, a `Set` a
`Set`. Bytes are a `Uint8Array`.

**Errors.** A facade call that fails throws an `Error` with the message. A service call does not throw for a failed
service: the failure is in `ec.message` ([`ec.service`](#ecservice)).

## `ec`

The top-level members.

| Member | |
|---|---|
| `context` | The context stack ([`ec.context`](#eccontext)); the same object as the name `context` |
| `contextRoot` | The entries of the root map of the context stack |
| `user` | [`ec.user`](#ecuser) |
| `message` | [`ec.message`](#ecmessage) |
| `l10n` | [`ec.l10n`](#ecl10n) |
| `resource` | [`ec.resource`](#ecresource) |
| `logger` | [`ec.logger`](#eclogger) |
| `cache` | [`ec.cache`](#eccache) |
| `transaction` | [`ec.transaction`](#ectransaction) |
| `artifactExecution` | [`ec.artifactExecution`](#ecartifactexecution) |
| `entity` | [`ec.entity`](#ecentity) |
| `service` | [`ec.service`](#ecservice) |
| `screen` | [`ec.screen`](#ecscreen) |
| `web` | [`ec.web`](#ecweb) |
| `search` | [`ec.search`](#ecsearch) |
| `factory` | [`ec.factory`](#ecfactory) |
| `llm` | [`ec.llm`](#ecllm); only in a build with the `llm` feature |
| `getTool(name, ...parameters)` | The object of a component's Rust tool factory; the same as `ec.factory.getTool` ([tools](#tools)) |
| `makeNotificationMessage()` | A notification builder ([below](#ecmakenotificationmessage)) |
| `getNotificationMessages(topic)` | The signed-in user's notification messages, of one topic or of all, as plain objects; empty with nobody signed in |
| `getContext()`, `getContextRoot()`, `getUser()`, `getMessage()`, `getL10n()`, `getResource()`, `getLogger()`, `getCache()`, `getTransaction()`, `getArtifactExecution()`, `getEntity()`, `getService()`, `getScreen()`, `getWeb()`, `getFactory()` | Each member by its Java getter |

## `ec.context`

The context stack, Java's `ContextStack`: a stack of maps read as one map, the top map first. A service call, a
screen and an included section each push a map, so what they set does not leak to the caller. A name no map holds
reads as null. `context.x = v` sets `x` in the top map, `delete context.x` removes it from the top map, `"x" in
context` tells whether any map holds it, and `Object.keys(context)` and a spread give the visible names.

A name in the context wins over a method of the same name: with a context name `size`, `context.size` reads that
name, so read such a name with `context.get("size")`.

| Member | |
|---|---|
| `get(name)`, `getByString(name)` | The value of the name, null when no map holds it |
| `put(name, value)` | Sets the name in the top map; returns the value it had |
| `putAll(map)` | Sets each entry in the top map |
| `remove(name)` | Removes the name from the top map; returns the value it had |
| `containsKey(name)` | Whether any map holds the name |
| `containsValue(value)` | Whether a visible name has the value (by `===`) |
| `keySet()` | The visible names |
| `values()` | The values of the visible names |
| `entrySet()` | Each visible name as `{key, value, getKey(), getValue()}` |
| `size()`, `isEmpty()` | How many names are visible; whether none is |
| `getCombinedMap()` | One plain object of the visible names |
| `push()`, `push(map)` | A new top map, empty or with the entries given; returns the stack |
| `pop()` | Removes the top map and returns its entries. The last map stays |
| `pushContext()` | Sets the whole stack aside for an empty one, until `popContext()`; returns the stack |
| `popContext()` | Brings back the stack `pushContext()` set aside; returns the stack |
| `getRootMap()` | The entries of the root map (also `ec.contextRoot`) |
| `addRootMap(map)` | Adds a map below every other as the root map |
| `clear()` | Empties the top map |
| `sharedMap`, `getSharedMap()` | One map for the execution context, whatever the stack holds, shared by every service call in it. A value is copied in and out, so change an object in it by assigning its key again |
| `toString()` | `ContextStack[name, name, ...]` |

```js
ec.context.push({ orderId: "100" })       // a frame of its own
try {
    total = ec.resource.expression("orderId + '-A'", null)
} finally {
    ec.context.pop()                       // what the frame set is gone
}

ec.context.pushContext()                   // nothing of the caller is visible
try { /* ... */ } finally { ec.context.popContext() }
```

Passed to a host call, `context` is the combined map of the visible names. `parameters(ec.context)` on a service call
reads the service's in-parameters from the stack.

## `ec.user`

The user of the session, with the locale, the time zone and the effective time, and the login and credential helpers
the user services call.

| Member | |
|---|---|
| `userId`, `username` | The signed-in user's id and username; null with nobody signed in |
| `userAccount` | The signed-in `UserAccount` as a map, without its credential and lockout columns; null with nobody |
| `locale`, `setLocale(locale)` | The session's locale (`en_US`); setting saves it on the signed-in user's account. Null or empty clears it |
| `timeZone`, `setTimeZone(zone)` | The session's time zone (an IANA name), the user's own, else the server's; setting saves it on the account. An unknown zone is an error |
| `machineTimeZone` | The machine's time zone, the zone a ServiceJob cron is read in |
| `currencyUomId`, `setCurrencyUomId(uomId)` | The currency of the signed-in user's account; the set does nothing with nobody signed in |
| `nowTimestamp` | The effective time when one is set, else the clock |
| `getNowCalendar()` | The effective time as a `Date` |
| `setEffectiveTime(time)` | `nowTimestamp`, date filters with no date, and the entity-auto `fromDate` and audit stamps use `time` (epoch millis, a `Date` or timestamp text) in this execution context; null goes back to the clock |
| `timestampMillis(value)` | Epoch milliseconds of a timestamp, null when the value is not one |
| `getPreference(key)` | The user's preference: a property, the user's `UserPreference`, a group's, or null |
| `getPreferences(keyRegexp)` | The preferences as a map; with a pattern, those whose whole key matches it |
| `setPreference(key, value)` | Saves the user's preference |
| `context` | A map for the user of this execution context, read by the entity filters. `get`, `put`, `putAll`, `remove`, `containsKey`, and names as properties. A list read from it is a copy |
| `hasPermission(permissionId)` | Whether the user has the permission |
| `isInGroup(userGroupId)` | Whether the user is in the group |
| `getUserGroupIdSet(userId)` | The group ids of the current user, or of the user given, as a list (so it works as the value of an `in` condition) |
| `visitId`, `visit` | The `moqui.server.Visit` of the session, its id and the record as a map; null outside one |
| `getVisitUserId()` | The user of the visit, which a later login in the same visit does not change |
| `visitorId` | The visitor id of the session, or null |
| `endVisit(visitId)` | Ends another session of the signed-in user; how many web sessions ended. An error for the current visit or another user's |
| `getClientIp()` | The request's client address; empty outside a request |
| `loginUser(username, password)` | Whether the user is logged in. A failure adds its message to `ec.message` and returns false; false with no message when a second factor is still needed |
| `internalLoginUser(username)` | Logs the user in without a password; a failure is a message and false |
| `loginUserKey(loginKey)` | Whether the login key logged its user in; a key that does not adds an error message |
| `loginAnonymousIfNoUser()` | With nobody logged in the session becomes anonymous; whether this call did that |
| `logoutUser()` | Logs out; true unless it failed |
| `getLoginKey(expireHours)` | A new login key (an API key) of the signed-in user; an error with nobody signed in |
| `passwordRules` | The password policy of the conf: `{minLength, minDigits, minOthers}` |
| `loginRules` | The login policy of the conf: `{maxFailures, disableMinutes, historyLimit, emailExpireHours, emailRequireChange}` |
| `hashPassword(password)` | A new argon2 password hash |
| `passwordsMatch(plain, stored, hashType, salt, base64)` | Whether the plain text matches a stored hash, argon2 or a legacy digest; an empty stored hash never matches |
| `singleUseCodeHash(code)` | The stored form of a single-use code |
| `randomCode(width)` | A random number of `width` digits (6) as text, zero padded |
| `randomString(length)` | Random URL-safe text of the length |
| `totpSecret()` | A new TOTP shared secret (base32) |
| `totpCodeValid(secret, code)` | Whether the code is the authenticator code of the secret now, or one period either side |
| `totpUri(label, issuer, secret)` | The `otpauth://totp/` URI an authenticator app scans; null when the secret or label is not valid |
| `getPeriodRange(period, poffset, pdate)` | `[from, thru]` of a period (`day`, `week`, `month`, `quarter`, `year`, `7d`, `30d`, ...) offset by `poffset` from `pdate` or today |
| `getPeriodRange(baseName, inputs)` | The range of `{baseName}_period`, `_poffset` and `_pdate` of the inputs map, else its `{baseName}_from` and `_thru` |
| `getPeriodDescription(period, poffset, pdate)` | Such as `This Month (2026-11-01 to 2026-11-30)`, the end date inclusive |

## `ec.message`

The messages and errors of the execution context. An error stops the service calls after it and rolls back the
transaction ([Services](../logic/Services.md)). The lists are arrays, empty when there is nothing.

| Member | |
|---|---|
| `addMessage(message, type)` | A message for the user; the type is `info` (the default), `success`, `warning` or `danger` |
| `addPublic(message, type)` | A message that a public site or portal may also show; it is in `messages` and `publicMessages` |
| `addError(message)` | An error |
| `addValidationError(form, field, serviceName, message)` | An error for one field of a form or service; its message is also in `errors` |
| `hasError()` | Whether there is an error |
| `errors` | The error texts, a validation error's message among them |
| `errorsString`, `getErrorsString()` | The errors as one text, each on a line |
| `validationErrors` | Each as `{form, field, serviceName, message, formPretty, fieldPretty, serviceNamePretty, stringPretty}` |
| `messages` | The message texts |
| `messagesString` | Each message and a newline |
| `messageInfos` | Each message as `{message, typeString}` |
| `publicMessages`, `publicMessageInfos` | The same for the messages added with `addPublic` |
| `clearErrors()` | Clears the errors and validation errors; the messages stay |
| `clearAll()` | Clears the errors, the messages and the public messages |
| `pushErrors()` | Sets the current errors aside, so a call that follows starts with none |
| `popErrors()` | Adds back the errors `pushErrors()` set aside |

```js
if (!order) {
    ec.message.addError(ec.resource.expand("Order ${orderId} not found", null))
    return
}
ec.message.addMessage("Order approved", "success")

// a call whose failure the caller reports itself
ec.message.pushErrors()
ec.service.sync().name("my.Services.check#Thing").parameters({ thingId }).call()
const problems = ec.message.errors
ec.message.clearErrors()
ec.message.popErrors()
```

Taking the errors back does not take a rollback-only mark off the transaction; [Services](../logic/Services.md) has
the pattern for a call that may fail.

## `ec.l10n`

Localized text, and formatting and parsing in the session's locale and time zone.

| Member | |
|---|---|
| `localize(text, locale)` | The text in the locale given, else the session's; the text itself when there is no `LocalizedMessage` |
| `format(value, pattern, locale, timeZone)` | A number, date, time or timestamp as text; a timestamp in the zone named (an IANA name), else the session's. `ec.entity.databaseTimeZone` as the zone is ignored |
| `formatNumber(value, pattern)`, `formatValue(value, pattern)` | `format` |
| `formatCurrency(amount, uomId, fractionDigits, locale, hideSymbol)` | An amount with the currency's symbol and digits |
| `formatCurrencyNoSymbol(amount, uomId, fractionDigits)` | The same without the symbol |
| `roundCurrency(amount, uomId, precise, roundingMode)` | The amount rounded to the currency's digits, a `Decimal`; the mode is a `RoundingMode` name or an old `ROUND_*` number |
| `parseTimestamp(text, format, locale, timeZone)` | A timestamp, or null. Text that names no offset is read in `timeZone`, else the session's. The locale is not used |
| `parseDateTime(text, format)` | `parseTimestamp` |
| `parseDate(text, format)`, `parseTime(text, format)` | A date or a time, or null |
| `parseNumber(text, format)` | A `Decimal`, or null |
| `getAvailableTimeZones()` | The time zone names |
| `getAvailableLocales()` | A fixed list of common locales, each `{locale, name}` with an English name |

## `ec.resource`

Script text, templates and files by location (`component://`, `runtime://`, `dbresource://`, `git://`, `classpath://`,
`file:`, `content://`).

### Scripts and expansion

These run in the calling script's own names: the text reads and writes the caller's context. With a `map`, its entries
are in a context frame of their own while the text runs; an entity value may be the map.

| Member | |
|---|---|
| `expand(text, location, map, localize)` | Localizes text shorter than 256 characters, then expands `${}` against the context and `map`. A fourth argument `false` skips the localization. Null text expands to nothing |
| `expandNoL10n(text, location, map)` | `expand` without the localization |
| `expression(expression, location, map)` | The value of the expression; null for an empty one |
| `condition(expression, location, map)` | Whether the expression holds, by JavaScript's truth; false for an empty one |
| `script(location)` | The value of the script file: what it returns, or its last expression |
| `script(location, method)` | Calls the function of that name on the file's value, with no arguments, and gives its result (the file's value when it has no such function) |
| `script(location, method, additionalContext)` | The same, with the map's entries in a context frame of their own |
| `template(location, data)` | The file rendered by the template renderer for its extension, as text; a string second argument is the default extension instead |
| `getTemplateRendererExtensionSet()` | The file extensions that have a template renderer |

```js
const title = ec.resource.expand("Order ${orderId} for ${customerName}", null, { customerName: party.organizationName })
if (ec.resource.condition(rule.conditionExpression, null, { order })) { /* ... */ }
const helpers = ec.resource.script("component://my-app/script/OrderHelpers.js")
helpers.recalculate(orderId)
```

### Locations and references

| Member | |
|---|---|
| `getLocationReference(location)` | A resource reference (below). `location#versionName` is a reference of that version |
| `getUriReference(uri)` | The same; a URI is a location here |
| `getLocationText(location, cacheOrCharset)` | The file's text. A second argument that is a charset name decodes the bytes in it; a cache flag is ignored |
| `getLocationStream(location)` | The file as a stream, null for a file that is not there |
| `getContentType(filename)`, `contentTypeOf(filename)` | The content type of a file name, or null |
| `isTextFilename(filename)` | Whether the file name's content type is text; also `ResourceReference.isTextFilename` |

A resource reference is data (`location`, `fileName`, `exists`, `directory`, `file`, `contentType`, `text`, `binary`,
and `size` and `lastModified`, read when asked for) with Java's `ResourceReference` methods:

| Member | |
|---|---|
| `getLocation()`, `getFileName()`, `getName()`, `getExists()`, `isDirectory()`, `isFile()`, `isText()`, `isBinary()`, `getContentType()`, `getSize()`, `length()`, `getLastModified()` | The data by Java's names |
| `getText(versionName)` | The file's text, or that version's; null for a file that is not there |
| `openStream()`, `getInputStream()` | The bytes as a stream; null for a file that is not there |
| `putText(text, note)`, `putBytes(bytes, note)` | Writes the file; a note is kept with the version the write makes |
| `putStream(stream, note)` | Writes a stream (of `openStream()` or an upload's `getInputStream()`) or a `Uint8Array` |
| `putUpload(fileItem)` | Writes an uploaded file |
| `copyFrom(source)` | Copies from a location or another reference |
| `move(newLocation)` | Moves the file; nothing when it is not there |
| `delete()` | Deletes it |
| `getDirectoryEntries()`, `directoryEntries` | The references of a directory's entries |
| `getChild(name)` | The reference of a name under the matching directory |
| `getChildren()` | The files of the matching directory; null when there is none |
| `findMatchingDirectory()` | The directory named as this file without its extensions, or this directory |
| `findChildFile(relativePath)`, `findChildDirectory(relativePath)` | A file or directory under this one, by path, else searched for by name; a reference that does not exist when nothing is found |
| `getActualChildPath()` | The path of a file `findChildFile` found, under the directory it was found in |
| `walkChildTree(flatList, treeList)` | Adds `{path, name, location}` of every file under this one to the flat list, and as a tree with `childResourceList` to the other; either may be null |
| `supportsVersion()` | Whether the location keeps versions (`dbresource://`, `git://`) |
| `getVersion(versionName)`, `getCurrentVersion()`, `currentVersion`, `getRootVersion()` | A version (below), or null |
| `getVersionHistory()` | The versions |
| `getNextVersions(versionName)` | The versions made from that one |
| `getVersionDiff(versionNameLhs, versionNameRhs)` | The line diff between two versions (an empty name is the current text): `{fromName, toName, unifiedDiff, html, stats: {insertions, deletions, hunks}, hunks}` |

A version is `{versionName, previousVersionName, versionDate, userId, note}` with `getVersionName()`,
`getPreviousVersionName()`, `getVersionDate()`, `getUserId()`, `getNote()`, `getText()`, `getPreviousVersion()`,
`getNextVersions()` and `getMap()`.

A stream has `readAllBytes()`, `getText(charset)`, `read(offset, max)` (a window of the bytes), `write(location)`,
`available()`, `size` and `close()`. A script does not read it a byte at a time.

### Files, versions, archives and digests

Change sets, version notes and change logs are described in
[Revision-Management](../data/Revision-Management.md).

| Member | |
|---|---|
| `putContent(location, content, note)` | Writes text, a `Uint8Array`, an uploaded file or a stream, with a note for the version (the commit message on `git://`, `versionNote` on `dbresource://`) |
| `makeChangeSet(root)` | A change set under one root: `write(path, content)`, `delete(path)`, `move(from, to)`, `mkdir(path)`, `author(userId, date)`, `size()`, then `commit(message)` gives `{versionName, applied, failures: [{path, error}]}`. One commit on `git://`, the changes in order elsewhere |
| `getLastChange(location)` | `{isFile, size, versionName, date, userId, note}` of a file or directory; null when nothing is there |
| `getChildChanges(location)` | The same for each entry of a directory, with `name` |
| `getChangeLog(location, limit)` | The versions under a location, newest first (100): `{versionName, previousVersionName, versionDate, userId, note, paths}` |
| `getTreeSize(location)` | The bytes under a location |
| `restoreVersion(location, versionName, note)` | Writes a version back as the newest; the new version's name |
| `getVersionBytes(location, versionName)` | The bytes of a version, or null |
| `textDiff(oldText, newText, fromName, toName)` | The line diff of two texts, as `getVersionDiff` gives it |
| `readLines(location, offset, maxLines, maxBytes)` | A window of a text file's lines, from line `offset`, at most `maxLines` (1000) and `maxBytes` (16 MB) |
| `digest(location, algorithm)` | The digest of a file's bytes, read as a stream (`sha256` by default) |
| `digestText(text, algorithm)` | The digest of text |
| `gunzip(source, target, maxBytes)` | Unpacks a gzip file to a location, refusing output over `maxBytes` (4 GB) |
| `extractArchive(source, target, limits)` | Unpacks an archive under a location, within the limits |
| `writeZip(target, entries)` | A zip of the files at the entries' locations (`[{path, location}]`): `{count, size, failures: [{path, error}]}` |
| `writeTarGz(target, entries)` | A `.tar.gz` of entries |
| `extractText(location, cacheKey, maxChars, cacheOnly)` | The text of a PDF, spreadsheet or text file (100000 characters), kept under `runtime://tmp/file-text` by the key; `""` when it has none; null with `cacheOnly` when not extracted yet |
| `readSheetPreview(location, sheetName, maxRows, maxCols)` | The first rows of a sheet (100 by 26): `{sheetNames, sheetName, rows, truncated}`; null when it is not a spreadsheet |
| `makeGrant(key, subject, ttlSeconds)` | A signed grant for the subject, valid `ttlSeconds` (3600) |
| `checkGrant(key, subject, token)` | Whether the token is a valid grant of the same key and subject |
| `runTokenCommand(command)` | The first output line of a command run without a shell (a registry token) |

### Barcodes and rendering

| Member | |
|---|---|
| `code128Svg(text, options)` | The Code 128 barcode as an inline `<svg>` sized in millimetres, for a print document. Options: `height`, `moduleWidth`, `fontSize`, `fontFamily`, `humanReadable`, `quietZone`. No text, or text Code 128 cannot hold, gives `""` |
| `barcodeSvg(symbology, text, options)` | The barcode of a named symbology; text it cannot hold is an error ([Barcodes](../data/Barcodes.md)) |
| `parseGs1(text)` | The application identifiers of a GS1 barcode's text |
| `isValidGs1CheckDigit(text)` | Whether the last digit is the GS1 check digit of the rest |
| `xslFoTransform(document, xslt, out, contentType)` | The PDF bytes of an `html-print` document (old name `xsl-fo`), a `Uint8Array`. `xslt` must be null and `out` is ignored. Throws when this server has no PDF engine |
| `markdownHtml(text)` | CommonMark as HTML, not sanitized |
| `makeThumbnail(source, target, maxSize)` | Writes a PNG thumbnail of an image, no side over `maxSize` (256): `{width, height, contentType}`; null when the bytes are not an image |
| `makePdfPreview(source, target, maxSize)` | Writes page 1 of a PDF as a PNG (1024): `{width, height, pages, contentType}`; null with no PDF engine or a PDF that does not open |
| `imageAvailable` | Whether this build can read images |
| `theme` | The theme composition adapter: `validate`, `render`, `engines`, `engineInfo`, `limits`, `sanitizeSvg`, `sanitizeRichText` ([Themes](../ui/Themes.md)) |

## `ec.logger`

The server log, by level. The last argument of each call may be a caught error, logged after the message as
`message: name: text`.

| Member | |
|---|---|
| `trace(message, thrown)`, `debug(message, thrown)`, `info(message, thrown)`, `warn(message, thrown)`, `error(message, thrown)` | A line at the level |
| `log(level, message, thrown)` | The level is a name (`trace`, `debug`, `info`, `warn`, `error`, `off`) or one of the level numbers; `off` and 0 log nothing |
| `logEnabled(level)` | Whether a message at the level would be written |
| `OFF_INT`, `FATAL_INT`, `ERROR_INT`, `WARN_INT`, `INFO_INT`, `DEBUG_INT`, `TRACE_INT`, `ALL_INT` | Java's level numbers: 0, 100, 200, 300, 400, 500, 600 and 2147483647. A number between two levels is the more severe one |

```js
try { send(order) } catch (err) { ec.logger.warn(`Could not send order ${orderId}`, err) }
```

## `ec.cache`

The named framework caches. A name the conf does not define is made with the default settings on first use.

| Member | |
|---|---|
| `getCache(name)` | The cache of the name |
| `getLocalCache(name)`, `getDistributedCache(name)` | `getCache(name)` |
| `cacheExists(name)` | Whether a cache of the name exists |
| `getCacheNames()` | The names of the caches |
| `clearAllCaches()` | Empties every cache |
| `clearCachesByPrefix(prefix)` | Empties the caches whose names begin with the prefix |
| `getAllCachesInfo(orderByField, filterRegexp)` | A row of statistics for each cache, for Tools |
| `makeElementInfoList(cacheName, orderByField)` | A row for each entry of a cache, for Tools |

A cache. Keys are text.

| Member | |
|---|---|
| `get(key)` | The value, or null |
| `put(key, value)` | Sets the value |
| `remove(key)` | Removes the key; whether it was there |
| `containsKey(key)` | Whether the key is there |
| `getAndPut(key, value)` | Sets the value; returns the one it had |
| `getAndRemove(key)` | Removes the key; returns the value it had |
| `putIfAbsent(key, value)` | Puts only when the key is not there; whether it put |
| `replace(key, value)` | Puts only when the key is there; whether it was |
| `getAll(keys)` | A map of those keys' values |
| `putAll(entries)` | Puts each entry of a map |
| `removeAll(keys)` | Removes those keys, or every entry with no argument |
| `clear()` | Removes every entry |
| `keySet()` | The keys |
| `size()` | How many entries |
| `getName()` | The cache's name |

## `ec.transaction`

The transaction of the default entity group, as Java's `TransactionFacade`. A service call manages its own
transaction ([JS-Service-Scripts](JS-Service-Scripts.md#transactions)); these are for a script that needs one of its
own.

| Member | |
|---|---|
| `isTransactionInPlace()` | Whether a transaction is in place |
| `begin(timeout)` | Begins a transaction (timeout in seconds, null for the default); whether this call began one. With one in place it joins it and returns false |
| `commit(began)` | Commits when `began` is true or not given; nothing when it is false. A transaction marked rollback-only is rolled back, which is an error |
| `rollback(began, message, error)` | Rolls back when `began`, else marks the transaction in place rollback-only |
| `rollback(message, error)` | Rolls back the transaction in place |
| `setRollbackOnly(message, error)` | Marks the transaction in place rollback-only |
| `suspend()` | Suspends the transaction in place; whether there was one |
| `resume()` | Resumes the suspended transaction |
| `getStatus()`, `status` | 0 active, 1 marked rollback-only, 6 no transaction (the numbers of `javax.transaction.Status`) |
| `getStatusString()`, `statusString` | Such as `Active (0)` |
| `runUseOrBegin(timeout, rollbackMessage, body)` | `body()` in the transaction in place, or in one of its own: committed when it returns, rolled back (or, joined, marked rollback-only) when it throws. Returns what `body` returns |
| `runRequireNew(timeout, rollbackMessage, body)` | `body()` in a new transaction, the one in place suspended until it ends |
| `isTransactionCacheActive()` | Whether the transaction cache is on |
| `initTransactionCache(readOnly)` | `ec.entity.startTxCacheDb(readOnly)` |
| `flushAndDisableTransactionCache()` | `ec.entity.stopTxCache()` |

```js
const began = ec.transaction.begin(null)
try {
    order.set("statusId", "OrderApproved").update()
    ec.transaction.commit(began)
} catch (err) {
    ec.transaction.rollback(began, "Error approving order", err)
    throw err
}

// the same, and a unit of work that commits whatever the caller's transaction does
ec.transaction.runUseOrBegin(null, "Error approving order", () => order.set("statusId", "OrderApproved").update())
ec.transaction.runRequireNew(60, "Error writing the log row", () => log.create())
```

A `begin` that joins takes nothing that must be given back, so `begin`, `try`, `commit(began)` is balanced.

## `ec.artifactExecution`

The stack of artifacts being run (screens, services, entities) and the authorization switches of the execution
context.

| Member | |
|---|---|
| `getStack()`, `getStackArray()` | The frames of the artifact stack, top first, each `{name, typeEnumId, actionEnumId}` |
| `peek()` | The top frame, or null |
| `push(name, typeEnumId, actionEnumId, requiresAuthz)` | Pushes a frame, checked as any is; throws when the user is not authorized. The type and action are Java's enum ids (`AT_OTHER`, `AUTHZA_VIEW`) or their short forms. The caller pops it |
| `pop()` | Removes the top frame |
| `getStackNameString()` | The artifact names on the stack, top first, joined with `, ` |
| `isPermitted(name, artifactType, action)` | Whether the user may reach one artifact |
| `isPermittedOwn(name, artifactType, action)` | The same question of the user's own authorization records: nothing is inherited from the frames on the stack, so a screen mounted under another app's inheritable always-allow can ask whether the user holds an artifact's grant themselves (the System screens' instance administrator sections, `tools/template/AdminMount.js`) |
| `disableAuthz()` | Authorization is not checked in this execution context until `enableAuthz()`. Returns whether it was already disabled |
| `enableAuthz()` | Checks authorization again |
| `authzDisabled()` | Whether authorization is disabled, by a script or by the caller (a call with `disableAuthz()`, a job); changes nothing |
| `disableEntityEca()` | Entity ECA rules do not run for this execution context's writes. Returns whether they were already disabled |
| `enableEntityEca()`, `entityEcaDisabled()` | Runs them again; whether they are disabled |
| `disableTarpit()` | No hit of this execution context is counted against a tarpit until `enableTarpit()`. Returns whether it was already disabled |
| `enableTarpit()` | Counts hits again |
| `setAnonymousAuthorizedAll()`, `setAnonymousAuthorizedView()` | What runs under the top frame is authorized for an anonymous user, for every action or for view |
| `runAsSystem(body)` | `body()` with no user and authorization off, in a fresh execution context that shares the transaction; the caller's user and authorization come back after. Only a service's own code may call it ([Attack-Surface](../security/Attack-Surface.md)) |
| `runAsUser(location, body)` | `body()` as the current user with authorization on, whatever disabled it, with the screen at `location` checked for all actions and pushed under it |
| `getFindFiltersForUser(entityName)` | The `EntityFilter` records that apply to a find on the entity; null when none do |

```js
const already = ec.artifactExecution.disableAuthz()
try {
    account = ec.entity.find("moqui.security.UserAccount").condition("userId", userId).one()
} finally {
    if (!already) ec.artifactExecution.enableAuthz()
}
```

## `ec.entity`

Records by entity name: finds, values, lists, conditions, and the data loader and writer.

### The facade

| Member | |
|---|---|
| `find(entityName)` | A find builder ([EntityFind](#entityfind)) |
| `makeValue(entityName)` | A new value that is not in the database ([EntityValue](#entityvalue)) |
| `fastFindOne(entityName, useCache, disableAuthz, ...pkValues)` | The value of the primary key values, in the entity's key order (also as one array); null with fewer values than key fields |
| `createBulk(values)` | Creates each value, in order |
| `sequencedIdPrimary(seqName, staggerMax, bankSize)` | The next id of a sequence name, as text. No stagger is 0, no bank size 50 |
| `tempSetSequencedIdPrimary(seqName, nextSeqNum, bankSize)` | Pins the next ids of an entity or sequence name to `nextSeqNum` through `nextSeqNum + bankSize`, for the whole server. For tests |
| `tempResetSequencedIdPrimary(seqName)` | The next id of the sequence comes from the database again |
| `conditionFactory`, `getConditionFactory()` | The condition factory ([Conditions](#conditions)) |
| `getDataDocuments(dataDocumentId, condition, fromUpdateStamp, thruUpdateStamp)` | The documents of a `DataDocument`, as plain objects; the condition is one of the factory or a map of alias to value |
| `getEntityDataDocument()` | Java's `EntityDataDocument`: `getDataDocuments(id, condition, from, thru)`, `writeDocumentsToFile(path, ids, condition, from, thru, pretty)`, `writeDocumentsToDirectory(...)`, `writeDocumentsToWriter(writer, ...)`; each write gives the count |
| `getValueListFromPlainMap(graph, entityName)` | A graph of plain maps flattened to a list of unpersisted values, inheriting keys and skipping read-only relationships |
| `makeDataLoader()`, `makeDataWriter()` | The builders [below](#data-loader-and-writer) |
| `startTxCacheDb(hold)` | Enters the transaction cache on this thread; `hold` must be true (writes never reach the database). A second start while one is active is an error |
| `stopTxCache()` | Leaves it; nothing when none is active |
| `isTxCacheActive()` | Whether one is active on this thread |
| `databaseTimeZone` | `UTC`, the time zone of the database timestamps |
| `getDatabaseConfName(entityName)` | The `database-list` name of the database the entity's group is on (`postgres` for the embedded cluster) |
| `getEntityGroupName(entityName)` | The entity's group, null for no such entity |

### EntityFind

A find builder. Each chain method changes the builder and returns it. A run (`one()`, `list()`, `count()`) leaves
the find as it was, so it can run again.

| Member | |
|---|---|
| `condition(field, value)` | The field equals the value; a null value is `IS NULL` |
| `condition(field, operator, value)` | A comparison; the operators are [below](#conditions). An unknown one is an error |
| `condition(map)` | Each entry as an equals condition |
| `condition(condition)` | A condition of the factory; null is skipped |
| `conditionToField(field, operator, toField)` | One field against another |
| `conditionDate(fromField, thruField, moment)` | Rows in effect at the moment (now when empty): from is null or not after it, thru is null or after it |
| `havingCondition(condition)` | A condition on an aggregate alias of a view entity; null is skipped |
| `searchFormMap(inputs, defaultParameters, skipFields, defaultOrderBy, alwaysPaginate)` | The conditions, order and page a search form's inputs give (`name`, `name_op`, `name_not`, `name_ic`, `name_from`, `name_thru`, `orderByField`, `pageIndex`, `pageSize`); what it applied is put in the context |
| `searchFormInputs(inputFieldsMapName, defaultOrderBy, alwaysPaginate)` | `searchFormMap` over the context, or over the map of that name in it |
| `searchFormInputs(inputFieldsMapName, defaultParameters, defaultOrderBy, alwaysPaginate)` | The same with default parameters |
| `requireSearchFormParameters(require)` | The find gives nothing unless the search form inputs give a condition |
| `selectField(fields)`, `selectFields(fields)` | The fields to read: comma separated text or a list |
| `orderBy(fields)` | Comma separated text or a list; `-` before a field is descending, `^` ignores case |
| `limit(limit)`, `offset(offset)` | The page; null is none |
| `offset(pageIndex, pageSize)` | The offset `pageIndex * pageSize`; does not set the limit |
| `distinct(flag)` | Distinct rows |
| `useCache(flag)` | Through the entity cache; null leaves the entity's own setting |
| `useClone(flag)` | Reads from a clone datasource of the entity's group when one is open |
| `forUpdate(flag)` | Locks the rows for update |
| `forNoKeyUpdate(flag)` | Locks with Postgres's `FOR NO KEY UPDATE`, which does not block another transaction's foreign key check on the rows; other databases lock for update |
| `disableAuthz()` | The find is not checked or filtered by authorization |
| `resultSetType(type)` | 1003 forward only (the default), 1004 or 1005 scrolling, for `iterator()` |
| `resultSetConcurrency(concurrency)` | Accepted; an iterator is read only |
| `fetchSize(size)`, `maxRows(rows)` | Accepted and not used |
| `copy()` | A copy of the find, so a page's `list()` and the `count()` of all rows can both run |
| `one()` | The one value, or null |
| `oneMaster(master)` | The value with the related records of a master definition (`default`), as a plain object; null for none |
| `list()` | The rows ([EntityList](#entitylist)) |
| `listMaster(master)` | Each row with the related records of a master definition, as plain objects |
| `iterator()` | The rows through a database cursor ([EntityListIterator](#entitylistiterator)) |
| `count()` | How many rows match |
| `updateAll(fieldsToSet)` | Sets the fields on each row found and updates the ones that change; how many were updated. Each row's ECA rules run |
| `deleteAll()` | Deletes the rows; how many |
| `purge(maxSeconds, chunkSize)` | Deletes the rows a chunk at a time by primary key, each chunk committed on its own; a failed chunk is logged and passed over, and no chunk begins after `maxSeconds`. How many rows the committed chunks deleted |
| `getEntityName()` | The entity of the find |
| `getSelectFields()` | The fields selected, empty for all |
| `getLimit()`, `getOffset()` | The limit and offset, or null |
| `getPageIndex()`, `getPageSize()` | `offset / pageSize`, 0 with no offset; the limit, 20 with none |
| `getResultSetType()` | The type set, 1003 by default |
| `getFetchSize()`, `getMaxRows()` | null |
| `getQueryTextList()` | The SQL the find ran, without its bound values |
| `selectFormFields(formName)` | The fields of a form-list, for its `entity-find`; generated code only |

```js
const orders = ec.entity.find("mantle.order.OrderHeader")
    .condition("statusId", "in", ["OrderPlaced", "OrderApproved"])
    .condition("placedDate", "greater-equals", fromDate)
    .conditionDate("fromDate", "thruDate", null)
    .orderBy("-placedDate")
    .limit(20)
    .list()
for (const order of orders) { /* order.orderId, order.get("grandTotal") */ }

const header = ec.entity.find("mantle.order.OrderHeader").condition("orderId", orderId).useCache(true).one()
if (header == null) { ec.message.addError(`Order ${orderId} not found`); return }
```

### EntityValue

One record. A field reads and writes as a property (`value.statusId`, `value["statusId"]`) and with `get` and `set`.
A method wins over a field of the same name. A relationship name also reads as a property: the related value for a
`one` relationship, the list for a `many`. `Object.keys(value)` gives the fields the value holds. `==` on two values
is identity; `equals` compares them.

| Member | |
|---|---|
| `get(field)` | The field, a localized field in the user's locale; a relationship name gives the related value or list; null for any other name |
| `set(field, value)` | Sets the field, converting text to the field's type; returns the value |
| `setString(field, value)`, `setBytes(field, value)` | `set` |
| `setFields(fields, setIfEmpty, namePrefix, pks)` | Sets the entity's fields from a map or another value: empty values too with `setIfEmpty`; from names with the prefix; `pks` true for the key fields only, false for the others, null for all |
| `setAll(fields)` | `setFields(fields, true, null, null)` |
| `setSequencedIdPrimary()` | Sets the single primary key field to the entity's next sequenced id |
| `setSequencedIdSecondary()` | Sets the last key field to the next number within the other key fields |
| `create()` | Inserts the record |
| `update()` | Writes the fields that changed since the value was read or last written |
| `createOrUpdate()`, `store()` | Updates when the record is there, else creates |
| `delete()` | Deletes the record |
| `refresh()` | Reads the row of the primary key again and replaces the fields. False, and unchanged, when there is no row |
| `getEntityName()`, `resolveEntityName()` | The full entity name |
| `getEntityNamePretty()`, `resolveEntityNamePretty()` | The short entity name in words (`OrderPart` is `Order Part`) |
| `isField(name)` | Whether the entity has a field of the name |
| `isFieldSet(name)` | Whether the value holds the field, null or not |
| `isFieldModified(name)`, `isModified()` | Whether the field, or any field, changed since the value was read or last written |
| `getOriginalDbValue(name)` | What the field held when the value was read or last written |
| `getPrimaryKeys()` | The primary key fields as a map |
| `getPrimaryKeysString()` | The primary key values joined by `::` |
| `containsPrimaryKey()` | Whether every primary key field has a value |
| `getMap()` | The fields as a plain object |
| `getPlainValueMap(dependentLevels)` | The fields without the nulls, with `_entity`, and the dependent relationships' values down `dependentLevels` |
| `getMasterValueMap(master)` | The value with the related records of a master definition (`default`), as plain objects |
| `entrySet()` | Each field as `{key, value, getKey(), getValue()}` |
| `mapMatches(fields)` | Whether each entry of the map equals the value's field |
| `equals(other)` | The same entity with the same field values |
| `compareTo(other)` | By entity name, then the primary key fields, then the others: negative, zero or positive |
| `cloneValue()` | A separate value with the same entity and fields |
| `isMutable()` | true |
| `getNoCheckSimple(field)` | `get` |
| `getString(field)`, `getBoolean(field)`, `getLong(field)`, `getDouble(field)`, `getBigDecimal(field)`, `getTimestamp(field)`, `getDate(field)`, `getTime(field)`, `getBytes(field)` | The field converted to the type, null for none |
| `findRelated(relationship, byAndFields, orderBy, useCache, forUpdate, forNoKeyUpdate)` | The related rows, as a list |
| `findRelatedOne(relationship, useCache, forUpdate, forNoKeyUpdate)` | The related value, or null |
| `findRelatedCount(relationship, byAndFields, useCache)` | How many related rows |
| `findRelatedFk(skipEntities)` | The rows of every entity that holds a foreign key to this value, as one list; skip by full name, short alias or entity name, in an array or a `Set` |
| `deleteRelated(relationship)` | Deletes the related rows |
| `deleteWithRelated(relationships)` | Deletes the rows of the named `many` relationships, then this value. False, with nothing deleted, when rows of another entity still reference it |

The setters and the writes return the value, so they chain:

```js
const item = ec.entity.makeValue("mantle.order.OrderItem")
    .setAll({ orderId, orderPartSeqId: "01", productId, quantity: 2 })
    .setSequencedIdSecondary()
    .create()

const order = ec.entity.find("mantle.order.OrderHeader").condition("orderId", orderId).forUpdate(true).one()
order.statusId = "OrderApproved"
order.update()
```

### EntityList

The rows of a find. It reads as an array: `list[0]`, `list.length`, `for (const row of list)`, a spread. A field set
on a row is kept in the list.

The methods that filter or order change the list itself and return it. The Array methods that give a new array in
JavaScript (`map`, `filter`, `slice`, `concat`, `toSorted`, `toReversed`, `flat`, `flatMap`) give a plain array of the
rows and leave the list as it was.

| Member | |
|---|---|
| `length`, `size()`, `isEmpty()` | How many rows; whether none |
| `get(index)`, `[index]` | The row; null outside the rows |
| `first()`, `getFirst()`, `last()` | The first or last row, null for an empty list |
| `filterByAnd(fields)`, `filterByAnd(name, value, ...)` | The list keeps the rows that match every field |
| `removeByAnd(fields)` | The list drops the rows that match |
| `filterByCondition(condition, include)` | The list keeps the rows the condition matches, or with `include` false those it does not |
| `filterByDate(fromField, thruField, moment, ignoreIfEmpty)` | The list keeps the rows in effect at the moment (now for none); `fromDate` and `thruDate` by default |
| `filterByLimit(offset, limit)` | The list keeps `limit` rows from `offset` |
| `filterByLimit(inputFieldsMapName, alwaysPaginate)` | The same by the `pageIndex` and `pageSize` of the context, or of the map of that name in it |
| `orderByFields(fields)` | The list ordered by each field in turn (a list or comma separated text), `-` for descending |
| `sort(compare)`, `reverse()` | The list sorted or reversed in place |
| `findByAnd(fields)`, `findByAnd(name, value, ...)` | A new list of the rows that match |
| `getPartialList(offset, limit)` | A new list of `limit` rows from `offset` (0-based), sharing the rows |
| `cloneList()` | A new list of copies of the rows |
| `indexMatching(fields)` | The index of the first row that matches, -1 for none |
| `add(value)`, `add(index, value)`, `addAll(values)` | Adds rows; true |
| `addIfMissing(value)`, `addAllIfMissing(values)` | Adds a value no row equals |
| `remove(index)`, `remove(value)` | By index: the row removed, or null. By value: whether one was removed |
| `set(index, value)` | Replaces a row; the row it had |
| `move(fromIndex, toIndex)` | Moves a row |
| `clear()` | Removes every row |
| `push(...)`, `pop()`, `shift()`, `unshift(...)`, `splice(...)` | As on an array; they change the list |
| `getValueMapList()` | Each row's `getMap()` |
| `getPlainValueList(dependentLevels)` | Each row's `getPlainValueMap(dependentLevels)` |
| `getMasterValueList(master)` | Each row's `getMasterValueMap(master)` |
| `forEach`, `find`, `findIndex`, `findLast`, `findLastIndex`, `some`, `every`, `reduce`, `reduceRight`, `includes`, `indexOf`, `lastIndexOf`, `join`, `at`, `entries`, `keys`, `values` | As on an array of the rows |
| `map`, `filter`, `slice`, `concat`, `flatMap`, `flat`, `toSorted`, `toReversed` | As on an array; the result is a plain array |
| `toJSON()` | The rows, so `JSON.stringify(list)` writes them |

```js
const active = ec.entity.find("mantle.party.PartyRole").condition("partyId", partyId).list()
    .filterByDate("fromDate", "thruDate", null)       // the list itself, filtered
const roleIds = active.map((row) => row.roleTypeId)   // a plain array
```

### EntityListIterator

A find's rows through a database cursor, so a long result is never held whole. It reads forward only unless the
find's `resultSetType` is 1004 or 1005, or the find is for update; a move back or a jump on a forward-only iterator
throws. `for (const row of iterator)` reads it. Close it when done; it closes itself after its last row and at the
end of the script.

| Member | |
|---|---|
| `next()` | The next row; null after the last |
| `hasNext()` | Whether a row follows |
| `previous()`, `hasPrevious()` | The row before, null before the first; whether there is one |
| `nextIndex()`, `previousIndex()` | The indexes of the rows `next()` and `previous()` would give |
| `first()`, `last()` | Moves to the first or last row; false when there is none |
| `beforeFirst()`, `afterLast()` | Moves before the first row or after the last |
| `absolute(rowNum)` | Moves to row `rowNum`, 1-based, from the end when negative; true when on a row |
| `relative(rows)` | Moves by a number of rows; true when on a row |
| `currentIndex()` | The current row, 1-based; 0 before the first |
| `currentEntityValue()` | The current row, or null |
| `setFetchSize(rows)` | How many rows the cursor fetches at a time |
| `getCompleteList(closeAfter)` | Every row as a list: from the first when the find scrolls, else from here on |
| `getPartialList(offset, limit, closeAfter)` | Up to `limit` rows from row `offset`, 1-based from the first |
| `writeXmlText(writer, prefix, dependentLevels)` | Writes each row as its XML element to an object with a `write` method; the records written |
| `writeXmlTextMaster(writer, prefix, masterName)` | The same with the records of a master definition |
| `close()` | Closes the cursor |

### Conditions

`ec.entity.conditionFactory` makes condition objects for `EntityFind.condition`, `havingCondition`,
`filterByCondition` and `getDataDocuments`.

Operators are names: `equals` (`=`), `not-equals` (`!=`, `<>`), `less` (`<`), `greater` (`>`), `less-equals` (`<=`),
`greater-equals` (`>=`), `like`, `not-like`, `in`, `not-in`, `between`, `not-between`, `is-null`, `is-not-null`. Java's other
spellings are taken too: `not-equal`, `less-than`, `greater-than`, `less-than-equal-to`, `greater-than-equal-to`,
`IN`, `NOT IN`, `BETWEEN`, `LIKE`, `NOT LIKE`, `IS NULL`, `IS NOT NULL`. The value of `in` and `not-in` is a list or
comma separated text; of `between` and `not-between` a list of the low and the high value. The join operators are `and` and `or`.

| Member | |
|---|---|
| `makeCondition(field, operator, value, orNull, ignoreCase)` | A comparison; with `orNull` the field may also be null; with `ignoreCase` text compares without regard to case |
| `makeCondition(left, joinOperator, right)` | Two conditions joined: `makeCondition(a, "or", b)` |
| `makeCondition(conditions, joinOperator, mapOperator, mapJoinOperator)` | A list joined (`and`). A map in the list is its entries compared with `mapOperator` (`equals`) and joined by `mapJoinOperator` (`and`) |
| `makeCondition(fields, operator, joinOperator)` | The entries of a map compared with `operator` (`equals`) and joined by `joinOperator` (`and`) |
| `makeConditionToField(field, operator, toField)` | One field against another |
| `makeConditionDate(fromField, thruField, moment, ignoreIfEmpty, ignore)` | From and thru date fields that cover the moment, or now; null when ignored |
| `makeConditionWhere(sql)` | A SQL fragment used as written; null for empty text |
| `getTrueCondition()` | A condition every row meets |
| `comparisonOperatorFromEnumId(enumId)` | The operator name of a `ComparisonOperator` enumeration id (`ENTCO_LESS` is `less`), or null |
| `makeActionConditionDirect(field, operator, from, value, toField, ignoreCase, ignoreIfEmpty, orNull, ignore)` | The condition of one `econdition` element, null when skipped; generated code only |

A condition:

| Member | |
|---|---|
| `ignoreCase()` | The comparison ignores case; returns the condition |
| `mapMatches(fields)` | Whether a map or entity value meets the condition. A SQL fragment cannot be checked |
| `mapMatchesAny(fields)` | Whether it meets any one comparison of the condition |
| `mapKeysNotContained(fields)` | Whether the condition names none of the map's keys |
| `populateMap(map)` | Puts the fields the condition requires to equal a value in the map given (or a new one), and returns it |
| `toString()` | The condition as text, for a log line |

```js
const cf = ec.entity.conditionFactory
const open = cf.makeCondition("statusId", "in", ["OrderPlaced", "OrderApproved"])
const mine = cf.makeCondition([{ customerPartyId: partyId }, { salesRepPartyId: partyId }], "or")
const orders = ec.entity.find("mantle.order.OrderHeader").condition(cf.makeCondition(open, "and", mine)).list()
```

### Data loader and writer

`ec.entity.makeDataLoader()` loads entity data text or files. Each setter returns the loader.

| Member | |
|---|---|
| `location(location)`, `locationList(locations)` | Files to load |
| `xmlText(text)`, `csvText(text)`, `jsonText(text)` | Text to load |
| `dataTypes(types)` | The data file types to load (`seed`, `demo`, ...) |
| `componentNameList(names)` | Only the data files of these components |
| `defaultValues(values)` | Field values for records that do not set them |
| `csvEntityName(name)`, `csvFieldNames(names)`, `csvDelimiter(char)`, `csvCommentStart(char)`, `csvQuoteChar(char)` | How CSV text is read; the field names are for text with no header line |
| `transactionTimeout(seconds)` | The timeout of the load's transaction |
| `useTryInsert(flag)` | Accepted and not used |
| `onlyCreate(flag)` | Leave records that are there as they are |
| `dummyFks(flag)` | Create a dummy record for a foreign key that points nowhere |
| `disableEntityEca(flag)`, `disableAuditLog(flag)`, `disableFkCreate(flag)`, `disableDataFeed(flag)` | Switch each off for the load |
| `messageNoActionFiles(flag)` | With false, no message for a file that had nothing to load |
| `restrictSensitiveEntities(flag)` | Refuse records of the entities a request may not name |
| `load(messages)` | Loads; the number of records. The messages are added to an array given |
| `check()`, `check(messages)` | Compares the data with the database and writes nothing: the messages of what did not match, or with an array the fields checked |
| `checkInfo()`, `checkInfo(diffInfoList, messages)` | A row for each record not found and each field that did not match, or with arrays the fields checked |
| `list()` | The values of the records the data has, with nothing written |

`ec.entity.makeDataWriter()` writes records as entity data. Each setter returns the writer.

| Member | |
|---|---|
| `fileType(type)` | `XML`, `JSON` or `CSV` |
| `entityName(name)`, `entityNames(names)`, `allEntities()` | The entities to write |
| `skipEntityName(name)`, `skipEntityNames(names)` | Entities to leave out |
| `dependentRecords(flag)`, `dependentLevels(levels)` | The dependent records with each record |
| `master(name)` | The records of a master definition with each record |
| `prefix(prefix)` | A prefix of the element names |
| `filterMap(fields)` | Only the records that match |
| `orderBy(fields)` | The order of the records |
| `fromDate(date)`, `thruDate(date)` | Only the records last updated in the range |
| `isoDateTime(flag)` | Timestamps as ISO-8601 text |
| `tableColumnNames(flag)` | Table and column names in place of entity and field names |
| `file(filename)` | Writes one file; the number of records |
| `directory(path)` | Writes a file for each entity into a directory; the number of records |
| `zipFile(filenameWithinZip, zipFilename)`, `zipDirectory(pathWithinZip, zipFilename)` | The same into a zip file |
| `text()` | The data as text |

```js
const messages = []
const loaded = ec.entity.makeDataLoader().location("component://my-app/data/MyDemoData.xml").load(messages)
const xml = ec.entity.makeDataWriter().entityName("moqui.basic.Enumeration").filterMap({ enumTypeId: "MyType" }).text()
```

### Entity definitions and admin calls

These serve the Tools screens and generic services.

| Member | |
|---|---|
| `isEntityDefined(entityName)` | Whether the entity exists |
| `getEntityDefinition(entityName)` | The definition (below), or null |
| `getAllEntityNames(filterRegexp)` | The entity names |
| `getAllEntityNamesWithMaster()` | The entities that have a master definition, sorted |
| `getAllEntitiesInfo(orderByField, filterRegexp, masterEntitiesOnly, excludeViewEntities)` | A row for each entity |
| `getAllEntityInfo(levels, excludeViewEntities)` | A row for each package down a number of name levels (2) |
| `getAllEntityRelatedFields(entityName, orderByField, dbViewEntityName)` | The fields of an entity and of each related entity, with whether a data view has them |
| `getDatasourceGroupNames()` | The entity group names |
| `clearEntityDefinitionFromCache(entityName)` | The definitions built from rows (data views) are read again on their next use |
| `createAllAutoReverseManyRelationships()` | Nothing: the reverse relationships load with the definitions |
| `checkTables(entityNames, groups)` | Checks, and creates or extends, the tables of the entities (of the groups, when given): `{ddl, errors, failed: [{entityName, error}]}` |
| `getEecaRuleCount()`, `eecaRulesByEntityName.get(entityName)` | How many entity ECA rules there are; the rules of an entity |
| `getQueryStatsList(orderByField, entityFilter, sqlFilter)` | The statement statistics of this server, when `entity-facade@query-stats` is on |
| `clearQueryStats()` | Clears them |
| `runSqlStatement(groupName, sql, limit)` | Runs one statement on a group's database: `{columns, records, limitReached, rowsAffected}`, values as text (500 rows) |
| `testConfConnection(conf)` | Whether a connection opens to another database, named by a map of `entity_ds_db_conf`, `entity_ds_host`, `entity_ds_port`, `entity_ds_database`, `entity_ds_user` and `entity_ds_password` |
| `runSqlCountConf(table, where, conf)` | `SELECT COUNT(*)` on that database |
| `runSqlUpdateConf(sql, conf)` | One statement outside a transaction on that database; the rows it changed |
| `formatFieldString(entityName, fieldName, text)` | The text as stored |
| `findOneXml(...)` | A generated `<entity-find-one>` in one call; generated code only |

An entity definition is data (`fullEntityName`, `entityName`, `isViewEntity`, `tableName`, `groupName`,
`pkFieldNames`, `allFieldNames`, `prettyName`, `entityInfo`) with these methods:

| Member | |
|---|---|
| `getFullEntityName()`, `getPkFieldNames()`, `getAllFieldNames()` | The names |
| `getFieldNames(pks, nonPks)` | The key fields, the others, or both |
| `isField(name)`, `isPkField(name)` | Whether the name is a field, a key field |
| `getFieldInfo(name)` | `{name, type, columnName, isPk, fieldNode, ...}` of a field, or null |
| `getColumnName(name)` | The column of a field |
| `getPrettyName(title, baseName)` | The entity's name in words |
| `getPrimaryKeys(values)` | The key fields of a map or value that have a value |
| `getRelationshipsInfo(dependentsOnly)` | The relationships, each with `relatedEntityName`, `prettyName`, `keyMap`, `riRelatedEntityName()`, `riPrettyName()` and `getTargetParameterMap(values)` |
| `getRelationshipInfo(name)` | One relationship by name, short alias or related entity name; or null |
| `getMasterDefinitionMap()` | The master definitions by name |
| `getEntityNode()` | The `entity` element as an `MNode` |
| `getDependentsTree()` | The tree of dependent entities; `toString()` is its text |

## `ec.service`

Service calls, jobs and the HTTP client.

### Calling a service

`ec.service.sync()` and `ec.service.async()` give a call builder. Each option changes the builder and returns it.

| Member | |
|---|---|
| `name(serviceName)` | The service: `path.verb#noun`, or an entity-auto `create#Entity`, `update#Entity`, `store#Entity`, `delete#Entity` |
| `name(verb, noun)`, `name(path, verb, noun)` | `verb#noun` and `path.verb#noun` |
| `parameters(map)` | Adds the entries of a map or an entity value. `parameters(ec.context)` reads the service's in-parameters from the context |
| `parameter(name, value)` | Adds one parameter |
| `disableAuthz()` | The call and what it runs are not checked by authorization |
| `requireNewTransaction(flag)` | The service runs in a transaction of its own, the caller's suspended |
| `ignoreTransaction(flag)` | The call begins no transaction |
| `transactionTimeout(seconds)` | The timeout of a transaction the call begins |
| `useTransactionCache(flag)` | The call runs with the transaction cache |
| `ignorePreviousError(flag)` | The call runs though the caller has error messages; they come back after |
| `softValidate(flag)` | Input that fails validation is danger messages, not errors, and the call returns no results |
| `multi(flag)` | One call for each row of `name_N` parameters, in one transaction |
| `throwOnError(flag)` | A failed call throws an error with its kind, in place of error messages |
| `noRememberParameters()` | The artifact hit of the call does not keep its parameters |
| `distribute(flag)` | Accepted; the call runs on this server |
| `getServiceName()` | The name set |
| `getCurrentParameters()` | The parameters set so far |
| `call()` | Runs the service. Sync: the out-parameters as a map. Async: starts it on a service worker and returns null |
| `callFuture()` | Starts the service on a service worker and returns a future |

A sync `call()` does not throw for a failed service. The failure is in `ec.message` and the result is the results as
they were, or null for an error of the call itself (an unknown service). An authorization failure is thrown. Check
`ec.message.hasError()` after a call whose failure matters ([Services](../logic/Services.md)).

```js
const out = ec.service.sync().name("mantle.order.OrderServices.place#Order").parameters({ orderId }).call()
if (ec.message.hasError()) return
const created = ec.service.sync().name("create#mantle.party.Party").parameters({ partyTypeEnumId: "PtyPerson" }).call()
partyId = created.partyId

ec.service.async().name("my.Services.send#Notice").parameters({ orderId }).call()

const future = ec.service.async().name("my.Services.calculate#Totals").parameter("orderId", orderId).callFuture()
const totals = future.get(5000)
```

A future:

| Member | |
|---|---|
| `isDone()` | Whether the service ended |
| `get(timeoutMillis)` | The out-parameters. A call not done within the timeout, or with none within the time the script has left, throws |

`ec.service.special()` is a service run when the transaction in place ends:

| Member | |
|---|---|
| `name(serviceName)`, `name(verb, noun)`, `name(path, verb, noun)` | The service |
| `parameters(map)`, `parameter(name, value)` | Its parameters |
| `registerOnCommit()` | The service runs asynchronously when the transaction commits |
| `registerOnRollback()` | The service runs asynchronously when the transaction rolls back |

`ec.service.job(jobName)` runs a `ServiceJob` outside its schedule:

| Member | |
|---|---|
| `parameters(map)`, `parameter(name, value)` | Parameters over the job's own |
| `localOnly(flag)` | Accepted; a job runs on this server |
| `run()` | Starts the job; its job run id. A job that cannot start is an error message and null |

### RestClient

`ec.service.rest()` is an HTTP client, Java's `RestClient`. Each setter returns the client.

| Member | |
|---|---|
| `method(method)`, `getMethod()` | The HTTP method, `GET` by default |
| `uri(uri)` | The URL |
| `uri()` | A URI builder (below) |
| `getUri()`, `getUriString()` | The URL set |
| `addHeader(name, value)`, `addHeaders(map)` | Request headers |
| `basicAuth(username, password)` | The `Authorization` header of HTTP basic authentication |
| `contentType(contentType)` | The content type of a text body, and the `Accept` header unless one is set; `application/json` by default |
| `acceptContentType(contentType)` | The `Accept` header |
| `encoding(characterEncoding)` | Named as the charset on the `Content-Type` of a text body; the text is sent as UTF-8 |
| `text(body)` | A text body |
| `jsonObject(body)` | A body of the value as JSON; text is sent as it is |
| `bodyBytes(bytes)` | A binary body, a `Uint8Array`; the `Content-Type` is the caller's header |
| `multipart(parts)` | A multipart body of uploaded files and text fields |
| `streamFrom()` | The body of the web request being handled, sent on without passing through the script |
| `addBodyParameter(name, value)`, `addBodyParameters(map)` | Parameters, sent in the query string |
| `timeout(seconds)` | The timeout, 30 by default |
| `followRedirects(follow)` | With false a redirect comes back as the 3xx response; `send()` only |
| `denyInternal()` | Refuses a host that is or resolves to an internal address, redirects included; `call()` only |
| `call()` | Sends the request; a response |
| `send()` | As `call()`, with the headers only as given and the body of the response left on the server until `text()` or `bytes()` reads it or `ec.web.response.forward()` passes it on |
| `callFuture()` | Makes the call and returns `{get(), isDone(), isCancelled(), cancel()}` holding the response |

The URI builder of `uri()`:

| Member | |
|---|---|
| `protocol(protocol)` | `http` by default |
| `host(host)` | The host; required |
| `port(port)` | 80 by default |
| `path(path)` | Adds a path element |
| `parameter(name, value)`, `parameters(map)` | Query parameters |
| `fragment(fragment)` | The fragment |
| `build()` | Sets the URL on the client and returns the client |

A response:

| Member | |
|---|---|
| `statusCode`, `getStatusCode()` | The HTTP status |
| `getReasonPhrase()` | The reason phrase, or null |
| `checkError()` | Throws for a status outside 200 to 299; else returns the response |
| `text()`, `getBodyText()`, `getResponseText()` | The body as text |
| `bytes()` | The body as a `Uint8Array` |
| `jsonObject()` | The body parsed as JSON; throws when it is not JSON |
| `xmlNode()` | The body parsed as XML, an `MNode` |
| `headerFirst(name)` | The first value of a header, or null |
| `headerList(name)` | The values of a header |
| `headers()`, `getHeaders()` | The headers by lower case name, each with the list of its values |
| `getContentType()` | The `Content-Type` header |
| `getUri()`, `getClient()` | The URL of the request; the client |

```js
const response = ec.service.rest().method("POST").uri("https://api.example.com/v1/orders")
    .addHeader("Authorization", `Bearer ${token}`).jsonObject({ orderId, total: total.toString() }).timeout(20).call()
if (response.statusCode !== 200) { ec.message.addError(`Order service answered ${response.statusCode}`); return }
const remoteId = response.jsonObject().id

const page = ec.service.rest().uri().protocol("https").host("example.com").port(443).path("search")
    .parameter("q", term).build().call().checkError().text()
```

### Cron, REST API and definitions

| Member | |
|---|---|
| `nextCronExecution(cron, after)` | Epoch millis of the next time a Quartz cron expression (seconds first) is due after the moment (now for null); null when never |
| `getCronDescription(cron)` | The expression in English words |
| `isLastRunBeforeLastSchedule(cron, lastRun, description, now)` | Whether there is no last run, or it is before the latest time the expression came due |
| `restApi.resolveService(path, method)` | The service of a Service REST API path (without `/rest/s1`): `{serviceName, path, method, pathParameters}`; not an authorization check |
| `restApi.getRootResources()`, `restApi.getFreshRootResources()` | The root resources of the Service REST API |
| `getServiceDefinition(serviceName)` | The definition with its auto-parameters expanded, or null: `inParameterNames`, `inParameterInfoMap`, `outParameterInfoMap` (each `{name, type, required, parameterNode}`), `serviceNode`, `getInParameter(name)`, `convertValidateCleanParameters(parameters)`. It does not run the service |
| `getKnownServiceNames()` | The names of the defined services |
| `getAllServiceInfo(levels)` | A row for each service package down a number of name levels (2) |
| `getSecaRuleCount()`, `secaRulesByServiceName.get(serviceName)` | How many service ECA rules there are; the rules of a service, or null |
| `getJobRunner()` | The scheduled job runner's counts: `getLastExecuteTime()`, `getExecuteCount()`, `getTotalJobsRun()`, `getLastJobsActive()`, `getLastJobsPaused()` |
| `jobWorkerPool` | The service worker pool: `getActiveCount()`, `getPoolSize()`, `getMaximumPoolSize()`, `getCorePoolSize()`, `queue.size()`, `queue.remainingCapacity()` |
| `ssoAvailable` | Whether this build has single sign-on |
| `passAuthority(endpoint)` | A PASS frontend's client of its authority over gRPC: `call(call)`, `upload(call, file)`, `download(call)`. Throws in a build without gRPC |
| `callXml(...)` | A generated `<service-call>` in one call; generated code only |

## `ec.screen`

Screen renders from a script: a report as text or bytes, an email body, a test of a screen.

| Member | |
|---|---|
| `render(options)` | Renders `{screenPath, renderMode, rootScreen, baseLinkUrl, lastStandalone, parameters}` in this execution context: `{status, contentType, text, bytes}` |
| `makeRender()` | A builder over `render` (below) |
| `makeTest()` | Java's `ScreenTest` (below) |
| `isRenderModeValid(mode)` | Whether the render mode is configured |
| `isRenderModeText(mode)` | Whether the mode writes text |
| `isRenderModeBinary(mode)` | Whether it is a screen-output mode such as `xlsx`, whose render is bytes |
| `getMimeTypeByMode(mode)` | The content type of a mode, or null |
| `getAllRootScreenLocations()` | The locations of the root screens of the webapps |
| `getScreenInfoList(rootLocation, levels)` | A row for each screen under a root, down a number of levels (2), each with `getIndentedName()` |
| `getTransitionInfo(root, location, name, method)` | Metadata of a transition: `{location, name, method, path, serviceName, pathParameters, parameterMap}`; throws for a missing or ambiguous mount |
| `getFormNode(location, asOfDate, includeDrafts)` | A form's element as an `MNode`; `DbForm#id` is a database form as of a date |
| `staticReport(options)` | The server-static qvt report of `{rootScreen, screenPath, parameters}` with the full check: `{location, path, verdict, marked, eligible, fullCheck, reasons, violations}` |
| `testRender(options)`, `testNoRequiredParameterPaths(options)` | The host calls under `makeTest()` |

The builder of `makeRender()`. A render mode named `xsl-fo` is `html-print`.

| Member | |
|---|---|
| `rootScreen(location)` | The root screen |
| `rootScreenFromHost(host)`, `webappName(name)` | The root screen the webapp (`webroot` by default) has for a host, when no root screen is set |
| `screenPath(path)` | The path under the root: text, or a list of names |
| `renderMode(mode)` | The render mode |
| `parameters(map)` | The request parameters |
| `baseLinkUrl(url)` | The base of the links in the output |
| `lastStandalone(value)` | The `lastStandalone` parameter |
| `render()` | The output as text |
| `renderBytes()` | The document of a binary mode such as `xlsx`, or the UTF-8 of a text mode |

```js
const body = ec.screen.makeRender().rootScreen("component://my-app/screen/MyReports.xml")
    .screenPath("OrderSummary").renderMode("html").parameters({ orderId }).render()
```

The test of `makeTest()` renders as the current user. A failed render does not throw: its errors are on the result.

| Member | |
|---|---|
| `rootScreen(location)`, `baseScreenPath(path)`, `renderMode(mode)`, `baseLinkUrl(url)`, `webappName(name)` | Where and how the renders run; render paths are under the base path |
| `header(name, value)` | A request header of every render |
| `clientModes(modes)` | The `X-Moqui-Client-Modes` header, such as `"qvue,qjs"` |
| `withStaticData()` | A qvt render of a server-static screen also gets the text of the screen's data in its output |
| `render(screenPath, parameters, requestMethod)` | One render; a result (below) |
| `renderAll(screenPaths, parameters, requestMethod)` | A render of each path |
| `getNoRequiredParameterPaths(screensToSkip)` | The paths below the base of the screens with no required parameters |
| `renderCount`, `errorCount`, `renderTotalChars`, `startTime` | Counts over the renders, and epoch millis when the test was made; each also as `getRenderCount()` and the rest |

A render result:

| Member | |
|---|---|
| `screenPath` | The path rendered |
| `output`, `getOutput()` | The response text; null when the render raised an error |
| `errorMessages`, `getErrorMessages()` | The errors of the render |
| `renderTime`, `getRenderTime()` | Milliseconds |
| `status` | The HTTP status, or null |
| `headers`, `header(name)` | The response headers; one by name in any case |
| `jsonObject`, `getJsonObject()` | The output parsed as JSON, null when it is not JSON |
| `assertContains(text)`, `assertNotContains(text)` | Whether the output has the text, or does not |
| `assertRegex(regex)` | Whether the output has a match of the pattern, text or a `RegExp` |
| `assertNoErrors()` | Whether the render had no error |

In a spec, `screenTest` of [JS-Testkit](JS-Testkit.md) wraps this.

## `ec.web`

The web request of the thread. `ec.web` is there in every script. Outside a request (a job, an async service, a spec)
`request`, `response`, `requestUrl`, `pathInfo`, `requestBodyText` and `session` are null, the maps and lists are
empty, and a send needs a request.

| Member | |
|---|---|
| `parameters`, `requestParameters` | The parameters of the request, as a map |
| `secureRequestParameters` | The parameters that came in the body or path, not in the query string |
| `multiPartParameters` | The fields of a multipart body; a file is an uploaded file (below) |
| `requestAttributes` | The request attributes as a map |
| `sessionAttributes` | The session attributes: read a name, assign it, `get`, `put`, `remove`, `containsKey`. Values are text |
| `sessionToken` | The session token a form posts back |
| `session` | The session: `id`, `getId()`, `getAttribute(name)`, `setAttribute(name, value)`, `removeAttribute(name)`, `getAttributeNames()` |
| `requestUrl` | The URL of the request without the query string |
| `pathInfo`, `pathInfoList` | The path after the servlet path, and its parts |
| `requestBodyText` | The body as text; empty for a multipart body |
| `request`, `response` | The request and response (below) |
| `getRequestDetails()` | The request and its parameters as text, without credential parameters; empty with no request |
| `getHostName(withPort)` | The host the request was for; with `true`, with its port |
| `getWebappRootUrl(requireFullUrl, useEncryption)` | Empty when a path-only link will do, else the scheme and host of the conf webapp; never the request's `Host` header |
| `getWebappRootUrl(webappName, servletContextPath, requireFullUrl, useEncryption)` | The same for a named webapp |
| `getResourceDistinctValue()` | A value that changes when the server starts, for the `?v=` of asset links |
| `rotateSession()` | Gives the browser a new session id after authentication, keeping the session's state |
| `declineSession()` | Sends this response no session: no session or visitor cookie and no session token ([Attack-Surface](../security/Attack-Surface.md)) |
| `getPageMessages()` | The messages, errors and validation errors for the page, the saved ones first: each `{message, typeString}`, a validation error also with `field` and `form` |
| `savedMessages`, `savedErrors`, `savedValidationErrors` | What the response before this one saved for the page |
| `errorParameters` | The parameters of the submit that led here with an error, or null |
| `getFieldValidationErrors(fieldName)` | The validation errors of one field, of the submit that led here and of this request |
| `saveScreenLastInfo(path)` | Remembers the current screen path, or the one given, so a login can go back to it |
| `getScreenHistory()` | The pages the user got in `html`, newest first: `[{name, url, urlNoParams}]` |
| `sendJsonResponse(value)` | The value as the JSON response; entity values and lists as plain maps and lists |
| `sendJsonError(status, message)` | A JSON error response |
| `sendError(errorCode, message, throwable)` | An error response as HTML, JSON or plain text, by the request's `Accept` |
| `sendTextResponse(text, contentType, filename)` | A text response, `text/plain` by default; a file name sends it as an attachment |
| `sendResourceResponse(location, inline, filename)` | The bytes at a resource location (`location#versionName` for a version), inline or (the default) as an attachment; 404 when nothing is there |
| `sendBytesResponse(bytes, contentType, inline, filename)` | Bytes as the response. With no content type PDF, PNG, GIF and JPEG are told from the first bytes. Null bytes are a 404 |
| `sendQzSignedResponse(message)` | The QZ Tray signature of the message, as `text/plain` |
| `viewEmailMessage()` | The tracking pixel of `/email/{emailMessageId}` |

An uploaded file has `name`, `fileName`, `fieldName`, `contentType`, `size` (each also `getName()` and the rest),
`getString(charset)`, `getInputStream()`, `write(location)` and `isInMemory()`.

### `ec.web.request`

A subset of `HttpServletRequest`. [Web-Scripts](../interfaces/Web-Scripts.md) has the handlers that work with it.

| Member | |
|---|---|
| `method`, `requestURI`, `queryString`, `contextPath`, `servletPath`, `pathInfo`, `scheme`, `serverName`, `serverPort`, `secure`, `remoteAddr`, `contentType`, `characterEncoding`, `contentLength` | The parts of the request; each also `getMethod()` and the rest, `secure` as `isSecure()`. `contextPath` is empty |
| `getRequestURL()` | The URL without the query string |
| `headerPairs` | The headers as sent, `[name, value]` with names in lower case |
| `getHeader(name)`, `getHeaders(name)`, `getHeaderNames()` | A header's first value, its values, the header names |
| `getIntHeader(name)` | A header as a number, -1 for none |
| `getCookies()` | `[{name, value}]` |
| `getParameter(name)`, `getParameterValues(name)`, `getParameterMap()`, `getParameterNames()` | The parameters of `ec.web.requestParameters` |
| `getAttribute(name)`, `setAttribute(name, value)`, `removeAttribute(name)`, `getAttributeNames()` | The request attributes |
| `getSession()` | The session of `ec.web.session`, or null |
| `bodyText()`, `bodyBytes()` | The raw body as text and as a `Uint8Array`; empty for a multipart body |

### `ec.web.response`

A subset of `HttpServletResponse`.

| Member | |
|---|---|
| `setStatus(status)`, `getStatus()` | The HTTP status |
| `setHeader(name, value)`, `addHeader(name, value)`, `setIntHeader(name, value)`, `setDateHeader(name, millis)` | Response headers; a null value removes the header |
| `getHeader(name)`, `getHeaders(name)`, `getHeaderNames()`, `containsHeader(name)` | The headers set |
| `setContentType(contentType)`, `getContentType()`, `setCharacterEncoding(charset)` | The `Content-Type` |
| `setContentLength(length)` | Accepted; the server sets the length |
| `write(content)` | Adds text or a `Uint8Array` to the body |
| `getWriter()`, `writer` | `write(text)`, `print(text)`, `println(text)`, `flush()`, `close()` |
| `getOutputStream()`, `outputStream` | `write(bytes)`, `flush()`, `close()` |
| `flushBuffer()` | Sends what is written so far |
| `isCommitted()` | Whether the response has begun |
| `sendError(status, message)` | `ec.web.sendError` |
| `sendRedirect(location)` | A redirect |
| `forward(upstream, options)` | The status, headers and body of an `ec.service.rest()` response as this response; a body `send()` left on the server streams to the client. `options.skipHeaders` names headers not to pass on |

## `ec.search`

Document search over the data documents, on the configured search backend. Each call takes one options map; a target
is `documentType` (one id, a comma separated list or a list) or `indexName`.

| Member | |
|---|---|
| `search(options)` | `{indexName, documentType, query, queryString, filter, sort, offset, limit, highlight, rank, consistency, freshTimeout}` gives `{hits: [{documentId, id, score, body, highlights}], total, totalRelation}`. `retrieval: "hybrid"` or `"semantic"` with `text`, `k`, `weights`, `fuzzyBelow` and `qualityBoost` fuses branches |
| `aggregate(options)` | `{documentType, filter, aggs, consistency, freshTimeout}` gives a result by aggregation name |
| `parseQuery(text)` | A query tree from a query string |
| `elasticQuery(mapOrJson)` | A query tree from an Elasticsearch query |
| `indexDocuments(documents)` | Indexes a list of documents; the count |
| `getDocument(documentType, id)` | A document, or null |
| `deleteDocument(documentType, id)` | Whether a document was deleted |
| `deleteMatching(options)` | Deletes the documents `{indexName or documentType, filter}` match; the count |
| `ensureIndex(target)`, `dropIndex(target)` | Makes or drops the index of a target; how many document types |
| `indexExists(target)` | Whether the index is there |
| `documentTypes(target)` | The document ids of a type name or an options map |
| `embedPending(options)` | Stores embeddings for `{indexName or documentType, batch, maxChars}`; how many |
| `processFeedPending(ids, stuckMinutes)` | Delivers the pending data feed rows of the ids, or every one: `{documentsIndexed, documentsDeleted, rowsDone, rowsLeft, rowsHeld, rowsStuck, rowsPending, oldestPendingSeconds, stuckError}` |
| `rebuildFeed(options)` | Indexes the documents of `{dataFeedId, dataDocumentId, fromUpdateStamp, thruUpdateStamp, batchSize}` again; the count |
| `flatten(map)` | A map with dotted keys |
| `isAvailable()` | Whether search is available on this server |
| `backendName()` | The backend's name |
| `capabilities()` | The names of what the backend can do: `proximity`, `fuzzy`, `autocomplete`, `weightedRank`, `accent`, `semantic`, `hybrid` |
| `hasElasticApi()` | Whether the backend answers the Elasticsearch REST API (OpenSearch) |

`consistency: "readYourWrites"` waits up to `freshTimeout` milliseconds (5000, at most 60000) for this session's
writes to be indexed.

## `ec.factory`

The server the execution context belongs to.

| Member | |
|---|---|
| `moquiVersion` | The framework version |
| `versionMap` | `{moqui: version}` |
| `hasFeature(name)` | Whether this binary has a build feature, as `moqui version` lists them (`git`, `imap`, `smtp`, `llm`; `mail` is `smtp` and `imap`) |
| `getRuntimePath()` | The runtime directory |
| `getInstanceId()` | This server's instance id, as its telemetry names it |
| `getComponentBaseLocations()` | Each component's location by its name, in load order |
| `getComponentInfoList()` | A row for each component |
| `getStatusMap(includeSensitive)` | The server status, for Tools and the status endpoint |
| `getStartupReport()` | What the server reported when it became ready: listeners, integrations, tools |
| `getThreadList()` | A row for each thread |
| `workerPool` | The service worker pool, as `ec.service.jobWorkerPool` |
| `waitWorkerPoolEmpty(retryLimit)` | Waits up to `retryLimit` (50) times 100 ms for async service work to finish; whether the pool is empty |
| `isDestroyed()` | false |
| `entity`, `service`, `l10n`, `resource`, `cache`, `screen`, `logger` | The same objects as `ec.entity` and the rest |
| `getTool(name, ...parameters)` | A tool (below) |
| `getToolFactory(name)` | The tool factory a conf `tool-factory` element selected: `name` and `getInstance(...parameters)`; null when there is none of that name |

### Tools

A tool is the object of a component's Rust tool factory ([Native-Components](Native-Components.md)). `ec.getTool(name,
...parameters)` and `ec.factory.getTool` take up to eight parameters and give an object with the methods its Rust
`Tool` exposes, which take and return script values. A name with no tool factory is an error. A tool that declares
its type is typed in `tools.d.ts` ([JS-Types-Tooling](JS-Types-Tooling.md)).

## `ec.makeNotificationMessage()`

A notification to users, by topic. Each setter returns the builder.

| Member | |
|---|---|
| `topic(topic)`, `subTopic(subTopic)` | The topic, whose `NotificationTopic` record gives the defaults |
| `userId(userId)`, `userIds(userIds)`, `userGroupId(userGroupId)` | Who is notified: a user, an array or `Set` of users, the members of a group |
| `message(message)` | The message, a map or its JSON text |
| `title(title)`, `link(link)` | The title and link, expanded with the message map |
| `type(type)` | `info`, `success`, `warning` or `danger` |
| `showAlert(flag)`, `alertNoAutoHide(flag)` | Whether the client shows an alert, and one that stays |
| `persistOnSend(flag)` | Whether the send saves the notification |
| `emailTemplateId(id)`, `emailMessageSave(flag)` | An email of the notification, and whether the email is saved |
| `reason(reason)`, `reasonEnumId(enumId)` | A `NotificationReason` enumeration id, or the short name of a seeded one (`mention`, `assignment`, `approval`, `dateAlert`, `reminder`, `comment`, `system`) |
| `record(entityName, recordId)`, `recordEntityName(name)`, `recordId(id)` | The record the notification is about |
| `actorUserId(userId)` | Who did what the notification tells of; the current user when not set |
| `appName(name)` | The app root the notification comes from |
| `send(persist)` | Sends it; returns the builder |
| `getTopic()` | The topic |
| `getNotificationTopic()` | The `NotificationTopic` value of the topic, or null |
| `getTitle()`, `getLink()` | As expanded by the last send; null before |
| `getNotificationMessageId()` | The id of the saved notification, or null |
| `getNotifyUserIds()` | The users it goes to |

```js
ec.makeNotificationMessage().topic("OrderApproved").userIds([salesRepUserId])
    .message({ orderId, customerName }).title("Order ${orderId} approved").link("/apps/order/OrderDetail?orderId=${orderId}")
    .type("success").send()
```

## `ec.llm`

Model calls of app code. `ec.llm` is there only in a build with the `llm` feature (`ec.factory.hasFeature("llm")`).
A model call is refused while a transaction is open, unless `complete` is given `allowInTransaction: true`, which
suspends the transaction for the call.

| Member | |
|---|---|
| `isEnabled()` | Whether model calls are enabled |
| `complete(options)` | One completion: `{prompt, system, messages, model, feature, tier, schema, schemaName, reasoning, maxOutput, timeoutSeconds, allowInTransaction}` gives `{text, json, modelId, finish, inputTokens, cachedInputTokens, outputTokens, cost, callIds}`. The model is `model`, else the binding of `feature`, else a model of `tier`. With `schema` (a JSON schema) the reply is parsed and checked into `json`. A call that ends without an answer throws |
| `summarize(text, options)` | A summary; `options.maxWords` |
| `classify(text, labels)` | One of the labels |
| `extract(text, fields)` | A value for each field of `{name: description}` |
| `startRun(agentId, text)` | Starts a background agent run as the current user; its run id |

## Differences from Moqui Java

A script written for Moqui Java ports with the same member names. These are the places where what a member does or
returns is not what Java's does. [JS-XML-Actions](JS-XML-Actions.md#groovy-that-does-not-carry-over) has the Groovy
that does not carry over.

### Behaviour

- **Values.** A decimal is a `Decimal` host value with methods, not a number with operators, and a timestamp is
  ISO-8601 text ([JS-Globals](JS-Globals.md)). A Java `Map` is a plain object and a `List` an array, so
  `ec.message.errors`, `messages`, `messageInfos`, `publicMessages` and `validationErrors` are arrays, empty when there
  is nothing, and a validation error is a plain object with `stringPretty` for Java's `toStringPretty()`.
- **Entity lists.** The filters and orders (`filterByAnd`, `filterByDate`, `filterByCondition`, `filterByLimit`,
  `orderByFields`, `removeByAnd`, `sort`, `reverse`) change the list in place and return it, as Java's do. The Array
  methods `map`, `filter`, `slice`, `concat` and `toSorted` return plain arrays, not entity lists.
- **Entity values.** `==` on two values is identity; use `equals`. `update()` writes only the fields that changed since
  the value was read or last written.
- **Finds.** A find can run more than once. `fetchSize` and `maxRows` are accepted and not used; `resultSetConcurrency`
  is accepted and an iterator is read only. Operators are names, not enum constants. The data loader's `useTryInsert`
  is accepted and not used.
- **`ec.web`** exists outside a web request, with null and empty reads, where Java's `ec.web` is null. Test
  `ec.web.request`, not `ec.web`, to tell whether there is a request.
- **`ec.context`.** A name reads and writes as a property, and a context name wins over a method of the same name, so
  read a name such as `size` with `get`.
- **Service calls.** `distribute` and a job's `localOnly` have no effect: a call runs on this server. `callFuture` is
  on every call builder. A sync call that fails gives an error message and null, not an exception, except an
  authorization failure and a call with `throwOnError(true)`.
- **RestClient.** `callFuture()` makes the call at once, on the calling thread. Text bodies are sent as UTF-8 whatever
  `encoding` names. `xmlNode()` gives an `MNode`.
- **`ec.user.getNowCalendar()`** gives a `Date`. `ec.l10n.parseDateTime` gives a timestamp.
- **`ec.logger`** has no fatal level: `FATAL_INT` logs as an error and `ALL_INT` as a trace.
- **`ec.search`** replaces `ec.elastic`. It takes query trees and Elasticsearch queries (`elasticQuery`) and runs on
  the configured backend.
- **`ec.transaction`** is the transaction of the default entity group. `getStatus()` gives 0, 1 or 6 only.
- **`ec.entity.startTxCacheDb`** takes `hold` true only: a transaction cache that flushes to the database is refused.
  `clearEntityDefinitionFromCache` drops every definition built from rows, not the one named.
- **`ec.resource`.** `getUriReference` takes a location. `xslFoTransform` returns the PDF bytes and ignores `out`.
  `getLocationText`'s second argument is a charset name. A stream is read whole, in a window, or written to a
  location.
- **`ec.l10n`.** `getAvailableLocales()` is a fixed list. `ec.service.getCronDescription` is English whatever the
  locale. `ec.entity.formatFieldString` gives the text as stored.
- **`ec.artifactExecution`** keeps no history: what ran is in the telemetry traces
  ([Monitoring](../tools/Monitoring.md)).
- **`ec.factory.isDestroyed()`** is always false, and `ec.factory.entity` and the rest are `ec`'s own facades.

### Additions

Members Moqui Java does not have.

| Facade | |
|---|---|
| `ec` | `search`, `llm` |
| `ec.context` | Property reads and writes, `in`, `delete`, `Object.keys` |
| `ec.user` | `machineTimeZone`, `passwordRules`, `loginRules`, `endVisit`, `hashPassword`, `passwordsMatch`, `singleUseCodeHash`, `randomCode`, `randomString`, `totpSecret`, `totpCodeValid`, `totpUri`, `timestampMillis` |
| `ec.l10n` | `getAvailableTimeZones`, `getAvailableLocales`, the time zone argument of `format` |
| `ec.resource` | `makeChangeSet`, `putContent`, `getLastChange`, `getChildChanges`, `getChangeLog`, `getTreeSize`, `restoreVersion`, `getVersionBytes`, `textDiff`, `readLines`, `digest`, `digestText`, `gunzip`, `extractArchive`, `writeZip`, `writeTarGz`, `extractText`, `readSheetPreview`, `makeThumbnail`, `makePdfPreview`, `imageAvailable`, `makeGrant`, `checkGrant`, `runTokenCommand`, `markdownHtml`, `code128Svg`, `barcodeSvg`, `parseGs1`, `isValidGs1CheckDigit`, `theme`; version notes on a reference's `putText`, `putBytes` and `putStream`, and `getVersionDiff` |
| `ec.artifactExecution` | `runAsSystem`, `runAsUser`, `isPermitted`, `isPermittedOwn` |
| `ec.entity` | `checkTables`, `runSqlStatement`, `testConfConnection`, `runSqlCountConf`, `runSqlUpdateConf`; EntityFind `forNoKeyUpdate`, `purge`; the `forNoKeyUpdate` argument of `findRelated` and `findRelatedOne`; EntityValue `getEntityNamePretty`, and fields and relationships as properties; EntityList as an array with the Array methods |
| `ec.service` | `throwOnError`, `nextCronExecution`, `restApi.resolveService`, `passAuthority`, `ssoAvailable`; RestClient `bodyBytes`, `multipart`, `streamFrom`, `followRedirects`, `denyInternal`, `send`; response `bytes` |
| `ec.screen` | `render(options)`, `renderBytes`, `isRenderModeBinary`, `staticReport`, `getTransitionInfo`; a screen test's `header`, `clientModes`, `withStaticData`, and a result's `status`, `headers`, `header` |
| `ec.web` | `rotateSession`, `declineSession`, `getPageMessages`, `request.bodyText`, `request.bodyBytes`, `request.headerPairs`, `response.write`, `response.forward` |
| `ec.factory` | `hasFeature`, `getStartupReport`, `getInstanceId`, `getThreadList` |
| `ec.makeNotificationMessage()` | `reason`, `reasonEnumId`, `record`, `recordEntityName`, `recordId`, `actorUserId`, `appName` |

### Not available

Java members that are not there, and what to use.

| Java member | |
|---|---|
| `ec.runAsync(closure)` | `ec.service.async()` |
| `ec.isSimSession` | Not there |
| `ec.getElastic()`, `ec.elastic` | [`ec.search`](#ecsearch) |
| `ec.initWebFacade`, `ec.destroy`, `ec.getContextBinding` | Not there: the host makes and ends the execution context, and the context is the script's names |
| `ec.context.clone` | `ec.context.getCombinedMap()` for a copy of the visible names |
| `ec.message.copyMessages` | Not there |
| `ec.message.addError(ValidationError)` | `addValidationError(form, field, serviceName, message)` |
| `ec.l10n.formatDateTime(Calendar, ...)` | `ec.l10n.format(timestamp, pattern, locale, timeZone)` |
| `ec.l10n.parseTimestamp` locale argument | Accepted and not used |
| `ec.cache.registerCache`, `ec.cache.getCache(name, K, V)` | `getCache(name)`: a cache is made on first use and its keys are text |
| `ec.transaction` XA resource, enlist and synchronization calls | `ec.service.special()` for work at commit or rollback |
| `ec.artifactExecution.getHistory`, `printHistory` | The telemetry traces ([Monitoring](../tools/Monitoring.md)) |
| `ec.web.getServletContext`, `applicationAttributes` | Not there; `ec.cache` for server-wide state |
| `ec.web.savedPublicMessages` | Not there |
| `ec.web.handleJsonRpcServiceCall`, `handleEntityRestCall`, `handleServiceRestCall`, `handleSystemMessage` | Not there: the host routes REST and system messages itself |
| `ec.resource.getLocationDataSource` | `getLocationStream(location)`, or a reference's `openStream()` |
| `ec.resource.template(location, Writer)` | `template(location, data)` returns the text |
| `ec.factory.getExecutionContext`, `destroy`, class loaders, listener registration, `checkEmptyDb` | Not there |
| `ec.entity.sqlFind`, `getConnection` | `ec.entity.runSqlStatement(groupName, sql, limit)` |
| `ec.entity.rest` | Not there: the host routes entity REST itself |
| `ec.entity.makeEtlLoader` | `makeDataLoader()` |
| `ec.entity.getDataFeedDocuments` | `getDataDocuments(dataDocumentId, condition, from, thru)` |
| `ec.entity.find(MNode)` | `find(entityName)` and the chain methods |
| EntityFind `makeEntityDynamicView`, `EntityDynamicView` | A view entity, or a `DbViewEntity` |
| EntityFind option getters other than `getSelectFields`, `getEntityName`, `getLimit`, `getOffset`, `getPageIndex`, `getPageSize`, `getResultSetType`, `getFetchSize`, `getMaxRows`, `getQueryTextList` | Not there |
| EntityFind `extract` | Not there |
| EntityValue `touchField`, `checkFks`, `checkAgainstDatabase`, `checkAgainstDatabaseInfo` | The data loader's `check()` and `checkInfo()` compare data with the database |
| EntityValue `deleteWithCascade` | `deleteWithRelated(relationships)` |
| EntityValue `writeXmlText`, `makeXmlElement` | `makeDataWriter()`, or an iterator's `writeXmlText` |
| EntityList `getOffset`, `getLimit`, `getPageIndex`, `getPageSize`, `isFromCache` | The find's `getOffset()`, `getLimit()`, `getPageIndex()`, `getPageSize()` |
| EntityList `writeXmlText` | An iterator's `writeXmlText`, or `makeDataWriter()` |
| `ComparisonOperator`, `JoinOperator` enums | Operator names: `"equals"`, `"in"`, `"and"`, `"or"` |
| `ec.service.registerCallback` | Not there |
| `ec.service.callJsonRpc` | `ec.service.rest()` |
| `ec.service` `getRunnable`, `getCallable` | `ec.service.async()` |
| RestClient `addFieldPart`, `addFilePart` | `multipart(parts)` |
| RestClient `retry`, `timeoutRetry`, `maxResponseSize`, the pool settings | Not there |
| RestClient `stream`, `streamSse` | `send()`, with the body read by `bytes()` or passed on by `ec.web.response.forward()` |
| ScreenRender `encoding`, `macroTemplate`, `servletContextPath`, `saveHistory`, `getPostRenderContext` | Not there |
| ScreenRender `render(Writer)` | `render()` returns the text, `renderBytes()` the bytes |
