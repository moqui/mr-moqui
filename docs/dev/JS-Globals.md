# Globals and host values in server scripts

What a server script has besides `ec`: the names it can use without declaring them, and the host value types that
are not plain JavaScript (decimals, timestamps, XML elements, uploaded files). It is for someone writing or porting a
service or screen script. The `ec.*` API is in [JS-Core-API](JS-Core-API.md); what a service body sees and returns is
in [JS-Service-Scripts](JS-Service-Scripts.md); the XML actions are in [JS-XML-Actions](JS-XML-Actions.md); screen
scripts are in [JS-Screen-Scripts](JS-Screen-Scripts.md); the declarations an editor reads are in
[JS-Types-Tooling](JS-Types-Tooling.md).

Contents: [Bare names](#bare-names) · [Decimals](#decimals) · [Dates and times](#dates-and-times) ·
[Converting and comparing](#converting-and-comparing) · [List helpers](#list-helpers) ·
[Text helpers](#text-helpers) · [Helper objects](#helper-objects) · [MNode](#mnode) ·
[System, ResourceReference, EdiHandler, SftpClient](#system-resourcereference-edihandler-sftpclient) ·
[Uploaded files and streams](#uploaded-files-and-streams) · [sri](#sri) ·
[Differences from Moqui Java and Groovy](#differences-from-moqui-java-and-groovy)

## Bare names

A name a script does not declare is looked up in this order: the script's own `const`, `let`, `var` and `function`
declarations, the global object (the JavaScript built-ins and the host globals below), then the context stack
(`script/oxide/names.rs` `GlobalNames`).

- **A bare read is the context stack.** `orderId` is the value the stack holds for it, the top map first.
- **An unset name is null.** It does not throw a `ReferenceError`. `typeof unset` is `"object"`. Test with `== null`.
- **A bare assignment writes the top map.** `total = 5` sets `total` in the context. A declared name is a local and
  is not in the context.
- **A list or map read by name can be changed in place.** `items.push(x)` and `map.key = value` are written back to
  the stack.
- **`context` is the stack itself**, the same object as `ec.context`, with Java's Map methods and `push`/`pop`
  ([JS-Core-API](JS-Core-API.md#eccontext)). `context.name` and the bare `name` read the same value. The stack
  refuses the name `context`.
- **A global wins over a context name.** A parameter named `compare` or `Map` is read as `context.compare`. A
  script's own declaration of the name hides the global for that script only.

### Host globals

| Global | |
|---|---|
| `ec` | The execution context ([JS-Core-API](JS-Core-API.md)) |
| `context` | The context stack |
| `sri` | The screen renderer ([sri](#sri)) |
| `toDecimal`, `toNumber` | [Decimals](#decimals) |
| `basicConvert`, `compare` | [Converting and comparing](#converting-and-comparing) |
| `orderMapList`, `filterMapList`, `filterMapListByDate` | [List helpers](#list-helpers) |
| `numberToWords`, `numberToWordsWithDecimal`, `elasticQueryAutoWildcard` | [Text helpers](#text-helpers) |
| `StringUtilities`, `ObjectUtilities`, `CollectionUtilities`, `WebUtilities`, `UserSecurityChecks` | [Helper objects](#helper-objects) |
| `MNode` | [MNode](#mnode) |
| `System`, `ResourceReference`, `EdiHandler`, `SftpClient` | [Below](#system-resourcereference-edihandler-sftpclient) |

A host global cannot be assigned (`script/oxide/engine.rs` `FREEZE_HOST_GLOBALS_JS`). One runtime serves every
script on a thread, so a replaced global would stand for the scripts after it.

- `compare = 1`, `StringUtilities = {}`, `MNode = null` and an assignment to any other host global throw
  `TypeError: compare is a host global and cannot be assigned`, in sloppy code too.
- An assignment to `ec`, `sri`, `toDecimal` or `basicConvert` does nothing in sloppy code and throws in strict code.
- The helper objects, their functions and `System` are frozen: `StringUtilities.cleanHtml = f` does nothing in
  sloppy code and throws in strict code.
- Names that begin `__` (`__moquiWrap`, `__compareOrder`, `__inDateRange`, `__isEmpty`, `__isGroovyFalse`) are the
  framework's own. Do not call them.

### JavaScript built-ins

The engine is quickjs-oxide. It has the ECMAScript built-ins: `Object`, `Array`, `Map`, `Set`, `WeakMap`, `WeakRef`,
`JSON` (with `JSON.rawJSON`), `Math`, `Date`, `RegExp`, `Promise`, `Proxy`, `Reflect`, `Symbol`, `BigInt`,
`Iterator`, the typed arrays, `ArrayBuffer`, `DataView`, the error classes, `encodeURIComponent` and the rest, and
explicit resource management (`using`, `DisposableStack`, `AsyncDisposableStack`, `SuppressedError`,
`Symbol.dispose`). The framework adds these (`script/oxide/builtins.rs`), which a script may replace as it may any
built-in:

| Built-in | |
|---|---|
| `queueMicrotask(callback)` | Runs the callback one job later |
| `btoa(text)`, `atob(text)` | Base64 of Latin-1 text, as in a browser. An invalid character throws a `DOMException` named `InvalidCharacterError`. For UTF-8 text and bytes use `StringUtilities.base64Encode` |
| `DOMException` | `new DOMException(message, name)` with `name`, `message` and the legacy `code` |
| `performance` | `performance.now()` in fractional milliseconds on a monotonic clock, and `performance.timeOrigin` |

It does not have:

| Not there | Use |
|---|---|
| `console` | `ec.logger` ([JS-Core-API](JS-Core-API.md#eclogger)) |
| `setTimeout`, `setInterval` | A service job or an async service call. A script runs to its end on one thread |
| `fetch`, `XMLHttpRequest` | `ec.service.rest()` ([JS-Core-API](JS-Core-API.md#ecservice)), `WebUtilities.simpleHttpStringRequest` |
| `require`, `process`, `Buffer`, `fs` and the other Node APIs | `ec.resource` for files, `System.getProperty` for the environment, `Uint8Array` for bytes |
| `Intl` | `ec.l10n` ([JS-Core-API](JS-Core-API.md#ecl10n)) |
| `TextEncoder`, `URL`, `structuredClone`, `crypto` | The host helpers below |

A bare read of one of these is null, since it is an unset context name: `console.log(x)` fails with a `TypeError`
on null, not a `ReferenceError`.

### Engine limits

Each thread has one runtime with a 64 MiB heap. A script is stopped by the time left in its transaction or request,
64 scripts may nest, and a value that crosses to the framework may nest 200 levels. A whole number past 2^53 cannot
reach a script. The table is in [JS-Service-Scripts](JS-Service-Scripts.md#engine-limits).

## Decimals

A decimal is a host value, not a JavaScript number (`script/oxide/decimal.rs`, typed as `Decimal`). It holds up to
28 digits exactly and keeps its scale, as Java's `BigDecimal` does: `1.50` stays `1.50`.

**How one arrives.** A service parameter of type `BigDecimal`, an entity field of a decimal type
(`number-decimal`, `currency-amount`, `currency-precise`), `basicConvert(x, "BigDecimal")`, a `<set>` with
`type="BigDecimal"`, and `toDecimal(x)`. A number literal such as `1.5` is a JavaScript number.

| Global | |
|---|---|
| `toDecimal(value)` | A decimal from a decimal, a number or numeric text. Null, a boolean, `NaN` or other text throws "toDecimal takes a decimal, a number or numeric text" |
| `toNumber(value)` | A JavaScript number from a decimal, a number or numeric text. Null and `""` give null |

Every method that takes another value takes a decimal, a number or numeric text. Anything else throws "a decimal
method takes a decimal, a number or a numeric string". A method returns a new decimal: a decimal does not change.

| Method | |
|---|---|
| `add(other)`, `subtract(other)` | The scale is the larger of the two. Overflow throws |
| `multiply(other)` | The scale is the sum of the two |
| `divide(other)` | Rounded half up to a scale of 10, or the larger scale of the two when that is more. Division by zero throws |
| `divide(other, scale, roundingMode)`, `divideScaled(other, scale, roundingMode)` | Java's `divide`: the quotient at `scale` places. The mode is `HALF_EVEN` when none is given |
| `setScale(scale, roundingMode)` | The value at `scale` places. With no mode, a value that needs rounding throws "rounding necessary" |
| `abs()`, `negate()` | |
| `eq(other)` | Numeric equality: `1.0` equals `1.00` |
| `compare(other)` | -1, 0 or 1 |
| `isZero()` | |
| `scale()` | The digits after the decimal point |
| `toNumber()` | A JavaScript number, with float rounding |
| `toString()` | The plain digits with the scale: `"20.50"` |
| `toJSON()` | What `JSON.stringify` writes: a JSON number, with all its digits when a JavaScript number would round them |
| `valueOf()` | Throws "decimal valueOf is not supported; use add, subtract, multiply, divide, eq, compare, isZero, or toNumber" |

A rounding mode is a Java `RoundingMode` name (`UP`, `DOWN`, `CEILING`, `FLOOR`, `HALF_UP`, `HALF_DOWN`,
`HALF_EVEN`, `UNNECESSARY`) or the old `ROUND_*` number `0` to `7`.

**Operators do not work on a decimal.** JavaScript has no operator overloading, and a silent conversion to a float
would lose the exactness a decimal is for, so `valueOf` throws.

| Do not write | It does | Write |
|---|---|---|
| `a + b`, `a * 2`, `a > 0`, `a == 0`, `Number(a)` | Throws: `valueOf` is not supported | `a.add(b)`, `a.multiply(2)`, `a.compare(0) > 0`, `a.isZero()`, `a.toNumber()` |
| `a == b`, `a === b` on two decimals | False unless they are the same object | `a.eq(b)` |
| `if (a)`, `a \|\| fallback`, `!a` | Always true: an object is truthy, zero included | `a != null && !a.isZero()` |
| `` `${a}` ``, `String(a)` | Works: the plain digits | |

```js
const lineTotal = toDecimal(quantity).multiply(unitAmount).setScale(2, "HALF_UP");
if (lineTotal.compare(creditLimit) > 0) ec.message.addError("Over the credit limit");
grandTotal = (grandTotal ?? toDecimal(0)).add(lineTotal);
const shown = ec.l10n.format(lineTotal, "#,##0.00");
```

**Where one goes.** Assigned to an entity field or passed as a service parameter it is the same decimal.
`JSON.stringify` writes it as a JSON number (`{"price":10.25}`). In a JSON answer of a transition, in a server-static
template and in the browser it is a string. `ec.l10n.format(value, "#,##0.00")` and `ec.l10n.formatCurrency` take
one.

## Dates and times

**A timestamp, a date and a time reach a script as text of epoch milliseconds** (`script/runtime/convert.rs`
`temporal_text`): an entity field, a service parameter or result, and a context value alike.

| Type | In the script |
|---|---|
| `Timestamp` (`date-time`) | The epoch milliseconds: `"1709647629000"` |
| `Date` (`date`) | The epoch milliseconds of midnight UTC of that day |
| `Time` (`time`) | The milliseconds since midnight |

The text converts back where a type is known: an entity field on a write, a declared service parameter, a find
condition on a date field, `basicConvert(text, "Timestamp")`. Besides the text of epoch milliseconds, those places
take a number of milliseconds, a JavaScript `Date` (it crosses as a timestamp), and text such as
`"2026-10-16 08:30:00"` or ISO-8601. Text with no offset is read in the session's time zone.

| Call | |
|---|---|
| `ec.user.nowTimestamp` | Now, or the effective time set with `ec.user.setEffectiveTime`, as text of epoch milliseconds |
| `ec.user.timestampMillis(value)` | The epoch milliseconds as a number, from the text, a number, a `Date` or date text. Null for null, `""` and what is not a time |
| `ec.user.getNowCalendar()` | A JavaScript `Date` of the effective time |
| `ec.l10n.format(value, pattern, locale, timeZone)` | Text in a Java date pattern. With no pattern, text of epoch milliseconds is formatted as `yyyy-MM-dd HH:mm`. The zone is an IANA name, the session's when left out |
| `ec.l10n.parseTimestamp(text, pattern, locale, timeZone)` | The timestamp, or null. Text with no offset is read in the zone named, else the session's |
| `ec.l10n.parseDate(text, pattern)`, `ec.l10n.parseTime(text, pattern)` | The date or time, or null |

```js
const day = 24 * 60 * 60 * 1000;
const dueMillis = ec.user.timestampMillis(invoice.invoiceDate) + 30 * day;
invoice.dueDate = dueMillis;                                  // a number of milliseconds converts on the write
const overdue = ec.user.timestampMillis(ec.user.nowTimestamp) > dueMillis;
const label = ec.l10n.format(invoice.invoiceDate, "MMM d, yyyy");
const when = new Date(ec.user.timestampMillis(invoice.invoiceDate));   // for the Date methods
```

- **Compare and add as numbers.** `a < b` on two texts compares characters, which goes wrong when the lengths
  differ. Use `ec.user.timestampMillis(a) < ec.user.timestampMillis(b)`, `Number(a) < Number(b)`, or
  `compare(a, "less", b, null, null, "Timestamp")`.
- **`+` on the text joins text.** `invoice.invoiceDate + day` is `"170964762900086400000"`.
- **A `date` field is midnight UTC.** `ec.l10n.format` formats text of epoch milliseconds as a timestamp in the
  session's time zone, so pass `"UTC"` as the zone for a date: `ec.l10n.format(value, "yyyy-MM-dd", null, "UTC")`.
- **A screen field shows it formatted.** A form field whose `from` is a plain path to a temporal value gets the
  value itself, and the display formats it.

## Converting and comparing

`basicConvert(value, type)` converts a value to a type by its Java name, as the `type` of `<set>` does. The short
name and the full one (`java.math.BigDecimal`) are both taken.

| Type | Result in the script |
|---|---|
| `String` | Text. A decimal gives its plain digits, a timestamp its epoch milliseconds |
| `Boolean` | `true` for the text `true` or `y` in any case and for a number that is not zero, else `false` |
| `Integer`, `Long` | A whole number. A decimal is truncated; a float with a fraction does not convert |
| `Double`, `Float` | A number |
| `BigDecimal` | A decimal |
| `Timestamp`, `Date`, `Time` | Text of epoch milliseconds. Timestamp text with no offset is read in the session's time zone |
| `byte[]`, `Blob` | A `Uint8Array` |

Null stays null. A value that does not convert, and any other type name, gives the value back unchanged: it does
not throw. Test the result when the input is not trusted.

`compare(field, operator, value, toField, format, type)` is Java's `ObjectUtilities.compare`, the compare of
`<if-compare>` and `<compare>`. The right side is `value`, or `toField` when `value` is null. Both sides are
converted to `type`; with no `type` (or `"Object"`) to the type of `field`. `format` is accepted and not used.

| Operator | |
|---|---|
| `equals`, `not-equals` | Equal by value after the conversion. Numbers are equal when their values are: a decimal `10.0` equals `10.00`, and a decimal `10.00` equals the whole number `10`. Any operator not in this table is `equals` |
| `less`, `greater`, `less-equals`, `greater-equals` | Ordered. A null `field` sorts after a value. A null on the right, or two values of different types, throws |
| `contains`, `not-contains` | The text of `field` contains the text of the right side |
| `matches`, `not-matches` | The whole text of `field` matches the regular expression |
| `empty`, `not-empty` | `field` is null, `""`, an empty list or an empty map |

```js
compare(amount, "greater", "100")                         // amount is a decimal: "100" converts to one
compare(statusId, "matches", "Order(Placed|Approved)")
compare(fromDate, "less", thruDate, null, null, "Timestamp")
```

A timestamp is text in a script, so pass `"Timestamp"` as the type to compare two of them as times.

## List helpers

They change the list in place and return nothing. Each works on an array of plain objects and on an entity list,
reading a field with `get(name)` when the item has one. A null list is left alone (`script/runtime/helpers.rs`).

| Function | |
|---|---|
| `orderMapList(list, fields)` | Sorts by the field names in the array. A `-` before a name is descending, a `+` ascending. A null sorts last |
| `filterMapList(list, values, exclude)` | Keeps the items whose fields equal every entry of `values`, as `compare` has `equals`. With `exclude` true it removes those items. Empty `values` changes nothing |
| `filterMapListByDate(list, fromName, thruName, valid, ignoreIfEmpty)` | Keeps the items in effect at `valid`: from date null or not after it, thru date null or after it. A null or empty `valid` is `ec.user.nowTimestamp`, or with `ignoreIfEmpty` true the list is left as it is |

```js
const members = ec.entity.find("moqui.security.UserGroupMember").condition("userId", userId).list();
filterMapListByDate(members, "fromDate", "thruDate", null);
orderMapList(members, ["-fromDate", "userGroupId"]);
```

## Text helpers

| Function | |
|---|---|
| `numberToWords(value, useAnd)` | A whole number in English words, the first letter capital: `numberToWords(21)` is `"Twenty one"`. Zero is `""`. `useAnd` puts `and` before the last part |
| `numberToWordsWithDecimal(amount)` | The whole part in words, then the cents: `"Five and 00/100"` |
| `elasticQueryAutoWildcard(query, allFieldPrefix)` | A `*` after each plain word so partial words match, and with `allFieldPrefix` true `*:` before it. `AND`, `OR`, `NOT` and words with query syntax are left alone. Null or blank is `"*"` |

## Helper objects

Java's static helper classes, as objects of functions. They are frozen ([Host globals](#host-globals)). The bare
helper functions above are also on the class Java has them on. Sources: `script/host/globals.rs`,
`script/host/utilities.rs`, `script/host/tools.rs`.

### StringUtilities

| Member | |
|---|---|
| `base64Encode(value)` | Standard base64 of text (its UTF-8) or of a `Uint8Array`. Null is `""` |
| `base64Decode(text)` | The bytes, a `Uint8Array`. White space is skipped and padding is optional. Other text throws |
| `base64UrlEncode(text)`, `base64UrlDecode(text)` | URL-safe base64 with no padding, text to text |
| `camelCaseToPretty(text)` | `"productStoreId"` to `"Product Store ID"` |
| `prettyToCamelCase(text, firstUpper)` | The reverse. Characters that are not letters or digits start a new word |
| `cleanHtml(text)` | The HTML cleaned as an `allow-html="safe"` parameter is |
| `encodeForXmlAttribute(text, addZeroWidthSpaces)` | Text escaped for an XML attribute, characters above ASCII as character references |
| `decodeFromXml(text)` | `&lt;`, `&gt;`, `&quot;`, `&apos;` and `&amp;` put back. Null for null |
| `urlEncodeIfNeeded(text)` | URL-encoded unless it is already safe. A space is `+` |
| `encodeAsciiFilename(filename)` | A path with the characters a URI does not allow escaped |
| `paddedNumber(number, length)` | The whole number with zeros before it: `paddedNumber(42, 5)` is `"00042"` |
| `paddedString(text, length, rightPad)`, `paddedString(text, length, padChar, rightPad)` | Cut or padded to the length, on the left unless `rightPad` |
| `removeNonAlphaNumeric(text)`, `replaceNonAlphaNumeric(text, replacement)` | Null for null |
| `isAlphaNumeric(text, also)` | Every character is a letter, a digit or one of the characters of `also` |
| `findFirstNumber(text)` | The first run of digits, or null |
| `getRandomString(length)` | Random text, as `ec.user.randomString` |
| `pathStringToList(path, skipSegments)` | The segments between `/`, without the first `skipSegments` |
| `escapeElasticQueryString(query)` | The query syntax characters escaped with `\` |
| `toStringCleanBom(text)` | The text without a leading byte order mark |
| `masker(mask, placeholder)` | A formatter for a mask such as `"###-###-###"`, or null for no mask. `valueToString(value)` fills the mask (`#` digit, `U` and `L` a letter made upper or lower case, `A` letter or digit, `?` letter, `H` hex digit, `*` any, `'` quotes a literal); a short value is filled with the placeholder and an invalid character throws |
| `getYearList(years)` | This year and the ones after it, as text |
| `elasticQueryAutoWildcard`, `numberToWords`, `numberToWordsWithDecimal` | The [text helpers](#text-helpers) |

### ObjectUtilities

| Member | |
|---|---|
| `isEmpty(value)` | Null, `""`, an empty array, `Set`, `Map`, entity list or plain object. `0` and `false` are not empty |
| `compare(...)`, `basicConvert(value, type)` | The [globals](#converting-and-comparing) |
| `compareLike(value, pattern)` | SQL `LIKE`: `%` is any run and `_` any one character. Case matters |
| `countChars(text, digits, letters, other)` | How many characters are of the kinds asked for |
| `toPlainString(value)` | A decimal's plain digits, else `String(value)`. Null for null |
| `addNumbers(left, right)` | A number for two numbers, else a decimal. A null side gives the other |

### CollectionUtilities

| Member | |
|---|---|
| `orderMapList(list, fields)` | The [list helper](#list-helpers); returns the list |
| `filterMapList(list, values, exclude)`, `filterMapListByDate(...)` | The [list helpers](#list-helpers) |
| `getMapArrayListValues(list, key, excludeNull)` | The value under `key` of each map in the list, null for a map without it unless `excludeNull`. Null when `list` is not an array |
| `flattenNestedMap(map)` | One map of the entries of the map, its nested maps and the maps in its nested lists. A later key replaces an earlier one. Null for what is not a map |
| `findFieldNestedMap(key, map)` | The first value under `key` in the map or a map or list nested in it, or null |
| `mapMatchesFields(map, fields)` | Each entry of `fields` equals the map's, as `compare` has `equals` |
| `mergeNestedMap(base, overrides, overrideEmpty)` | Puts the overrides into `base`: nested maps are merged, lists joined. An empty override value replaces only with `overrideEmpty` |
| `removeNullsFromMap(map)` | Removes the null entries, in nested maps and lists too; returns the map |
| `toHashMap(map)` | A shallow copy as a plain object |
| `addToListInMap(key, value, map)` | Pushes onto the array under `key`, made when missing |
| `addToSetInMap(key, value, map)` | Adds to the `Set` under `key` |
| `addToMapInMap(keyOuter, keyInner, value, map)` | Sets `map[keyOuter][keyInner]` |
| `addToBigDecimalInMap(key, value, map)` | Adds `value` to the decimal under `key`. A null value changes nothing |
| `addToBigDecimalInMapInMap(keyOuter, keyInner, value, map)` | The same in the map under `keyOuter` |
| `paginateParameters(count, listName, context)` | Sets `{listName}Count`, `PageIndex`, `PageSize`, `PageMaxIndex`, `PageRangeLow` and `PageRangeHigh` on the map given, from its `pageIndex` (0) and `pageSize` (20); returns them. When `{listName}Count` is already set it only sets `{listName}AlreadyPaginated` |
| `paginateList(listName, pageListName, context)` | Sets the page of the list `context[listName]` as the bare name `pageListName` (the list's own name when null), with the page values beside it. A list that is already a page is left as it is |

### WebUtilities

| Member | |
|---|---|
| `encodeHtml(text)` | HTML-escaped text. Null is `""` |
| `encodeHtmlJsSafe(text)` | HTML escapes plus those a single-quoted JavaScript string needs |
| `fieldValuesEncodeHtmlJsSafe(map)` | The map as a JavaScript object literal with its values escaped that way; `"null"` for what is not a map |
| `ipMatches(patterns, address)` | The address matches one of the patterns. No patterns matches every address |
| `isSafeSinglePathSegment(text)` | The text is one path segment of URL-safe characters |
| `isSameOriginRedirect(url, configuredHost)` | The URL is a path on this server, or an `http(s)` URL whose host is `configuredHost` (else the webapp's `http-host`). The request's `Host` header does not count |
| `lastPathSegment(path)` | The part after the last `/` or `\`. Null for null |
| `isRestrictedGenericEntity(entityName)` | The entity, or a view over it, is one the generic tools (auto screens, data edit, data import) do not take from a request |
| `simpleHttpStringRequest(location, body, contentType)` | The response text of a `POST` of the body, or of a `GET` with no body |
| `simpleHttpMapRequest(location, parameters)` | The response text of a `POST` of the parameters as a form |

### UserSecurityChecks

The checks `Security.eecas.xml` runs before a write of a group membership or a permission grant. A denied check
throws, so the write fails.

| Member | |
|---|---|
| `assertPrivilegedGroupMembership(userGroupId)` | Throws when the group is privileged and the user may not write a membership in it |
| `assertSealedPermissionGrant(userPermissionId)` | Throws when the permission is sealed and the user may not grant it |
| `assertSealedGroupMembership(userGroupId, thruDate, hasStored, storedThruDate)` | Throws for a membership in a group that grants a sealed permission the user does not hold. `hasStored` is true for an update that found the stored row, so a change that only ends the membership sooner passes |
| `assertDocumentExpression(fieldPath)` | Throws for a data document field path that is an expression (it has a `(`) unless the user has `DATABASE_ADMIN` |
| `privilegedGroups()` | The effective privileged group ids, sorted |
| `sealedPermissions()` | The effective sealed permission ids, sorted |
| `endUserSessions(userId, visitIds)` | Ends the web sessions of the user for the visits named, or all of them with none; the number ended. It checks no authority: the caller does |

## MNode

An XML element as data with methods, Java's `MNode` (`script/host/tools.rs` `wrappers.Node`, typed as `MoquiNode`).

| Static | |
|---|---|
| `MNode.parseText(location, text)` | The root element of the XML text. Null when the text does not parse. `location` is not used |
| `MNode.parse(location)` | The root element of the XML at a resource location. Null when there is none or it does not parse |
| `MNode.make(name, attributes)` | A new element |

| Member | |
|---|---|
| `name`, `getName()` | The element name |
| `attributes`, `getAttributes()` | A plain object of the attributes, all text |
| `attribute(name)` | The attribute, or null |
| `setAttribute(name, value)` | Sets it as text, or removes it for null; returns the node |
| `text`, `getText()` | The text content, or null |
| `setText(text)` | Returns the node |
| `childNodes` | The array of child elements |
| `children(name)` | The children of that name, or all with no name |
| `first(name)` | The first child of that name, or the first child; null for none |
| `hasChild(name)` | |
| `append(name, attributes)` | Makes a child and returns it |
| `append(node)` | Adds a node and returns it |
| `remove(name)`, `remove(index)` | Removes the children of that name, or the one at the index; whether any went |
| `deepCopy()` | A copy of the element and all below it |
| `depthFirst(matches)`, `breadthFirst(matches)` | The elements below this one that the function holds for |
| `descendants(name)` | The elements of that name at any depth |
| `descendants(names)` | An object of those arrays by name |
| `toString()` | The XML text |

```js
const root = MNode.parseText(null, '<root kind="a"><item name="one"/></root>');
root.append("item", {name: "two"});
root.setAttribute("kind", "b");
const names = root.children("item").map((item) => item.attribute("name"));   // ["one", "two"]
const xml = String(root);
```

A node kept in the context keeps its methods. The form, service and entity definitions `ec` hands out
(`getFormNode`, `serviceNode`, `getEntityNode`, a REST response's `xmlNode()`) are nodes of this kind.

## System, ResourceReference, EdiHandler, SftpClient

| Global | |
|---|---|
| `System.getProperty(name)` | A system property or environment value, null when it is not set or empty |
| `ResourceReference.isTextFilename(filename)` | Whether the content type of the file name is a text type. It is the only member: get a reference with `ec.resource.getLocationReference(location)` ([JS-Core-API](JS-Core-API.md#ecresource)) |

### EdiHandler

`new EdiHandler(ec)` reads and writes X12, EDIFACT and iTrade text (`script/host/edi.rs`). The argument is accepted
and not used. The setters and loaders return the handler, so calls chain.

| Member | |
|---|---|
| `loadEnvelope(location)`, `loadBody(location)` | Loads a structure: a script file at the location that evaluates to the structure list |
| `setChars(segmentTerminator, elementSeparator, componentDelimiter, escapeCharacter)` | The separators. A null takes the X12 default: `~`, `*`, `:`, `?` |
| `setX12DefaultChars()`, `setITradeDefaultChars()`, `setEdifactDefaultChars()` | |
| `setSegmentSuffix(character)`, `setPaddingChar(character)` | |
| `parseText(text)` | The message as a map of segments |
| `segmentErrors`, `getSegmentErrors()` | The errors of the last parse, or null |
| `generateText(map)` | The text of a map of segments |
| `splitMessage(headerId, trailerId, text)` | The text split into messages at the header and trailer segment ids, as an array of text |
| `combineSegments(segments)` | The segments as one text, the segment terminator (and suffix) after each |
| `countSegments(map)` | The segments under the map, not counting the map itself |
| `EdiHandler.makeAk3(segmentError)` | Static: the AK3 segment map for a segment error, Java's `segmentError.makeAk3()` |

### SftpClient

`new SftpClient(host, username, port)` is the `moqui-sftp` client (`script/host/sftp.rs`). Bytes are `Uint8Array`s.
Close it in a `finally`: a client left open is closed when it is collected, with an error in the log.

| Member | |
|---|---|
| `SftpClient.SFTP_PORT`, `SftpClient.SSH_PORT` | 115 and 22 |
| `password(password)`, `publicKeyPkcs8(privateKeyPem, publicKey)` | The login. Return the client |
| `hostKeyFingerprint(fingerprint)` | Pins the server's host key, `SHA256:...`. Without it the key is trusted on first use |
| `preserveAttributes(preserve)` | Returns the client |
| `connect()`, `isConnected()`, `close()` | `connect` returns the client |
| `lsFiles(path)` | The file names |
| `ls(path)` | Maps of `name`, `path`, `isFile`, `size`, `lastAccess`, `lastModified` (seconds since the epoch) |
| `getBinary(path)`, `getText(path, charset)` | The file as bytes or text |
| `put(path, filename, data, charset, createDir)` | Writes text, a `Uint8Array`, an `ArrayBuffer`, an uploaded file or a resource reference. Returns the client |
| `openStream(path)` | A read stream: `read(max)` (65536 bytes when left out), `readAllBytes()`, `readText(charset)`, `close()` |
| `putStream(path)` | A write stream: `write(data, charset)`, `close()` |
| `moveFile(filePath, destDirPath, createDir)`, `renameFile(filePath, newFileName)`, `rename(filePath, newFilePath)` | Return the new path |
| `rm(path)` | Returns the client |
| `publicKey(...)`, `getInternalClient()` | Throw: they take Java objects. Use `publicKeyPkcs8` |

```js
const sftp = new SftpClient("files.example.com", "acme").password(secret).connect();
try {
    for (const name of sftp.lsFiles("/outbound")) importFile(name, sftp.getText("/outbound/" + name));
} finally {
    sftp.close();
}
```

## Uploaded files and streams

A script cannot hold a Java stream. An uploaded file and a stream of bytes are host objects that read the bytes
whole, a window at a time, or write them to a resource location as they are read (`script/host/upload.rs`).

An uploaded file is the value of a file field in `ec.web.parameters`, and of a service parameter that takes one.

| Uploaded file | |
|---|---|
| `name`, `fileName`, `getName()` | The file name the browser sent. `toString()` is the same |
| `fieldName`, `getFieldName()` | The form field |
| `contentType`, `getContentType()` | |
| `size`, `getSize()` | Bytes |
| `isInMemory()` | |
| `getString(charset)` | The bytes as text, UTF-8 when no charset is named |
| `getInputStream()` | A stream of the bytes |
| `write(location)` | Stores the file at a resource location as it is read, so a large upload is not held in memory |

A stream comes from `getInputStream()` of an uploaded file and `openStream()` of a resource reference (typed as
`MoquiResourceStream`).

| Stream | |
|---|---|
| `size`, `available()` | Bytes |
| `read(offset, max)` | Up to `max` bytes from `offset` as a `Uint8Array`, at most 8 MiB in one call. Fewer only at the end, none past it. Only the window is read |
| `readAllBytes()` | The whole content as a `Uint8Array` |
| `getText(charset)` | The whole content as text |
| `write(location)` | Writes the content to a resource location a window at a time |
| `close()` | Holds nothing open; for a `try`/`finally` ported from Java |

```js
const file = ec.web.parameters.contentFile;
if (file.size > 20 * 1024 * 1024) return ec.message.addError("The file is too large");
file.write(`dbresource://uploads/${StringUtilities.encodeAsciiFilename(file.fileName)}`);
```

## sri

`sri`, the screen renderer, is a global during a screen render. Its members are in
[JS-Templates-Macros](JS-Templates-Macros.md#the-sri-global); what a screen script sees is in
[JS-Screen-Scripts](JS-Screen-Scripts.md).

## Differences from Moqui Java and Groovy

- **No `BigDecimal` operators.** Groovy's `a + b` and `a > b` on decimals are the [decimal methods](#decimals), and
  a literal `1.5` is a number, not a decimal.
- **No Groovy truth in plain JavaScript.** An empty list, an empty map and a zero decimal are true. `==` on two
  decimals or two entity values is identity ([JS-XML-Actions](JS-XML-Actions.md#truth-and-comparison)).
- **A `List` is an array and a `Map` is a plain object.** `list.size()` is `list.length`, `map.keySet()` is
  `Object.keys(map)`, `map.containsKey(k)` is `k in map`. An entity list keeps Java's list methods, and `context`
  keeps the Map methods. A Java `Set` is a `Set`, and `byte[]` is a `Uint8Array`.
- **A `Timestamp` is text of epoch milliseconds**, not an object with `getTime()` ([Dates and times](#dates-and-times)).
- **An unset name is null**, where Groovy throws `MissingPropertyException` for a name outside the binding.
- **No Java classes.** `new java.util.Date()`, `java.util.UUID` and a static call on a Java class have no form
  here. The classes above are the ones that do, as globals with no package: `new EdiHandler(ec)`, not
  `new org.moqui.impl.util.EdiHandler(ec)`.
- **`ObjectUtilities.compare` ignores `format`.**

For those who come from JavaScript: the Moqui calls compare as a business rule would, not as `===` does. `compare`
with `equals`, and what is built on it (`filterMapList`, `CollectionUtilities.mapMatchesFields`, an entity value's
`mapMatches`, an entity list's `filterByAnd`, `findByAnd`, `removeByAnd` and `indexMatching`, an XML `<compare>`),
converts both sides to one type first and takes numbers by value: `"10"` and `10` are equal, a decimal `10.0` equals
`10.00`, and two timestamps of the same moment are equal. In plain JavaScript `toDecimal("10.0") === toDecimal("10.00")`
is false, because two decimals are two objects: compare them with `eq`, or with `compare`.
- **`MNode.parseText` returns null** for text that does not parse, where Java throws.

These Java statics are not available:

| Java | Use |
|---|---|
| `StringUtilities.calculateCrc16` | No equivalent |
| `ObjectUtilities.objectToMap` | A plain object is already a map; an entity value has `getMap()` |
| `ObjectUtilities.getStreamText` | `getText(charset)` of a [stream](#uploaded-files-and-streams), `ec.resource.getLocationText` |
| `CollectionUtilities.flattenNestedMapWithKeys` | `flattenNestedMap`, which does not prefix the keys |
| `CollectionUtilities.stdDevMaxFromMapField` | Compute it in the script |
| The `WebUtilities` HMAC and redaction helpers | No equivalent |
