# Context objects

Which values QuickJS and the browser can see, and which Rust types those environments share. Server text goes through one of the template renderers in `plans/archive/Architecture-Proposal.md`. The macro modules that expand XML are the same runtime, with a narrower input.

This follows Templating in `plans/archive/Architecture-Proposal.md` and the browser host in `plans/archive/QvtServerStatic.md`. The method list on server `ec` facades and on `sri` is Server host methods, below.

## Decision

There are four places a value is visible. The three server places are one QuickJS runtime. The fourth is the browser.

| Environment | When it runs | What it is given |
|---|---|---|
| QuickJS scripts, `JsTemplateLiteral`, and the trusted fancy renderer | Every server evaluation: XML actions, `<script>`, attribute expressions, `ec.resource.expand()`, a trusted fancy template, and a server screen render (html, html-print, text, csv, xml, and qvt when it is not server-static; the name xsl-fo is html-print). | The live context stack, `ec`, and during a screen render `sri`. Entity values and lists stay host objects. |
| XML action and screen macro modules | Once, when an action or screen definition is compiled. The generated text is cached. | The element being compiled: attribute strings, child element text, and the already-rendered output of its children (or a `visit` function). No `ExecutionContext`, no context stack, no entity value. |
| Locked-down fancy renderer | When a template is rendered with the restricted renderer. | A plain snapshot of the map the caller passed. Format and localize functions. No `ec`, no `sri`, no live entity. |
| Browser | A server-static qvt screen, after the template is cached. | The JSON package from `plans/archive/QvtServerStatic.md`, plus the restricted browser `ec` that plan already lists. |

String expansion (`ResourceFacade.expand`) is `JsTemplateLiteral` on that runtime. A string with no `$`, or with `$` as the last character, is returned as itself. Localization of an input shorter than 256 characters still happens first, as `ResourceFacadeImpl.expand` does. The `${name}` expansion in `moqui-lib` stays string replacement against the environment and does not see the context stack. On the server, embedded expressions run in QuickJS. In the browser, `ec.resource.expand` uses the restricted host.

The macro module does not run on the request path. A screen function that today calls `sri` or `ec` while FreeMarker is rendering emits a JavaScript call instead. That call runs later, in QuickJS or in the browser. Short framework text uses `JsTemplateLiteral` and the live context. Structured component text uses the trusted fancy renderer. Text that must not be a script uses the locked-down renderer and the snapshot. The fancy dialect is `if`, `list`, and `assign`, compiled to one cached JavaScript function. The trusted renderer and the locked-down renderer are that same compiler. There is no Mustache library. `ScreenRenderEmail.xml` is the fixture.

## What Java does, and what changes

`MNode` implements the FreeMarker node interfaces and caches the wrappers on the node. That pays off because screen FreeMarker runs on every render and looks up `@attr` and child elements through those interfaces. Everything else reaches FreeMarker through `BeansWrapper` reflection. `ContextStack` is only a `Map`.

The two FreeMarker jobs already have different data models:

- `XmlActions.groovy.ftl` is compiled once. Its data model is one entry, `xmlActionsRoot`, the `MNode`. `<#visit>` walks the node. The names `ec`, `context`, and `result` appear in the generated Groovy. They are not in the FreeMarker data model. The generated script header says so.
- Screen macros run on every render. `ScreenWidgetRenderFtl` passes the whole `ContextStack` as the data model, after putting `sri` and the widgets node on it. `ec` is already on the stack. Macros both walk `.node` and call `sri` and `ec` while they write output.

The macro module is JavaScript on the QuickJS runtime, so a function can walk a node when the walker passes one. The Rust walker owns the default visit. For each element it calls `macros[elementName](node, children)`. `children` is the already-rendered output of the child elements, and the function places that string, including inside a loop it emits. Attribute strings are copied into that call. The copy happens once per definition, and the cached result is the generated script. That replaces the `MNode` wrapper cache. `Node` does not implement an engine trait. The function is not given the request context. Dispatch by element name is an ordinary property lookup, which is why the library is a JavaScript module rather than a template that cannot call a macro through a variable.

## One stack, three projections

`ContextStack` stays in `moqui-lib`. It is a stack of maps. It implements no QuickJS trait. `moqui-lib` does not depend on `rquickjs`. A type in lib cannot implement that trait anyway: the orphan rule requires the crate to own the type or the trait. The core runtime reads the stack through the proxy below.

QuickJS is the live view. Bare names in an expression (`orderId`, `statusId`) resolve on the stack, the way the Groovy binding and the FreeMarker data model do. `context` is that stack. `ec` is the host object for the execution context and is also the `ec` entry on the stack. Assignments write the top map. A service `result` map is present when the current actions provide one, as today. The QuickJS runtime is the one already owned by that `ExecutionContext`. Values created for a call die with it. They are not stored on a process-wide runtime.

### Codegen rule: bare names

This is the contract for macros and for migrated scripts. It is decided before macros are written.

Groovy returns null for an unset bare name. `ContextBinding.hasVariable` always returns true (`ContextBinding.java:41-45`), so the get falls through to `contextStack.getByString` and yields null instead of `MissingPropertyException`. `if (someUnsetParam)` is false. In JavaScript the same line throws `ReferenceError` at runtime. Identifier lookup calls `HasBinding` before `Get`, so a get-only trap never runs for an unset name.

The server runtime sets the prototype of `globalThis` to a `Proxy` for that `ExecutionContext`: `Object.setPrototypeOf(globalThis, new Proxy({}, { has, get, set }))`. The proxy closes over that thread's `ContextStack`. Assigning a `Proxy` to `globalThis` does not change identifier lookup, and an unset name still throws `ReferenceError`. The proxy is not installed on the browser `window`. `with` is not used. Strict mode rejects `with`. Evaluated scripts are strict, so an assignment goes through the `set` trap instead of creating an own global.

- `has` returns true for every name that is not already an own property of the real global. An unknown name still has a binding, so the read does not throw.
- `get` returns the stack value when any map on the stack contains the key, including an explicit null. Otherwise it returns a host helper (`ObjectUtilities`, `CollectionUtilities`, `StringUtilities`). Otherwise it returns null. Those helpers are not own properties of `globalThis`. A context key of the same name wins.
- Own properties of the real global (`Array`, `Math`, `Object`, and the other built-ins) are resolved by the engine before the proxy. A context key cannot hide those names.
- `set` writes the top map, as `ContextBinding.setVariable` does. It does not create a real global.
- An unset name is null, so `if (unset)` is false. `typeof unset` is `"object"`, because the value is null. A presence check is `== null`.

Macros emit bare names. They do not rewrite reads to `context.foo`. `context.foo` still works because `context` is on the stack. Server-static qvt expressions are Vue template expressions against the JSON package. A missing key renders empty there. A new client script that mentions an unset name can still throw. That is acceptable. The proxy is not how the browser works.

Expression hazards that change what a macro emits, including Elvis versus `??`, empty collections, `==`, and Groovy ranges, are the table in `plans/archive/Architecture-Proposal.md` (What changes in expressions). Macros follow that table. `??` is only for a null check. An empty string or a zero uses logical or. An empty list or map tests length. `==` stays `==`. A Groovy inclusive range becomes `slice` with an exclusive end.

The locked-down fancy renderer and the browser see a snapshot, not the stack. The snapshot is the same projection as `ContextJavaUtil.unwrapMap`:

- Strings, booleans, numbers, and timestamps pass through. Timestamps become ISO-8601 text in JSON and in the locked-down template. A decimal becomes a string in JSON and in the locked-down template. It does not become a JavaScript number. String order and `+` are then lexicographic and concatenation (`"10.00" < "9.00"`, `"1.00"+"2.00"`).
- An entity value becomes the plain field map. An entity list becomes a list of those maps.
- `EntityFind`, the execution context, `sri`, and the context stack object are dropped.
- Anything else is dropped and logged, as `unwrap` does today.

The locked-down template's plain values and the browser's JSON are two sinks of that one projection. The projection is the shared code. QuickJS host objects are not.

The browser package is that projection plus two filters from `plans/archive/QvtServerStatic.md`: credential parameters are stripped, and form-list lists and pagination keys are removed. A server-static expression that does decimal arithmetic, or that calls `ObjectUtilities` for it, is a compatibility-report violation. Formatting goes through `ec.l10n`. On the server, a decimal stays a host value.

A live entity value inside the locked-down template would let it walk a relationship and read through to the database. The snapshot is that boundary. Callers pass the map the template is allowed to see. Format and localize call the same functions as `ec.l10n` on the server. They do not take an `ExecutionContext`. `JsTemplateLiteral`, the trusted fancy renderer, and scripts are not this path. They see the live context, which is the same trust as a component-authored script.

## Host objects in QuickJS

These are the types a script can hold. Plain maps and lists that are already plain data are ordinary JavaScript objects, copied in at the boundary. The types below stay Rust values behind getters so a screen or a service does not clone the working set into the engine.

| Name | Visible as | Rust value |
|---|---|---|
| context keys | Bare names, and `context.name`. On the server an unset name is null, via the proxy above | The `ContextStack` |
| `ec` | Host object | The `ExecutionContext` and its facades |
| `sri` | Host object, only while a server screen render is on the stack | The screen renderer. Absent in actions that are not a render, and absent in the browser |
| entity value | Host object. Fields read and write as properties (`value.statusId`, `value['statusId']`, `value.statusId = x`, `value?.statusId`) and with `get` and `set` | The entity value. `unwrap` is what the locked-down template, JSON, and the browser receive instead. Property access follows Java `EntityValueBase.get` and `put` as Groovy map property syntax reaches them; see the rules below the table |
| entity list | Host object, index, length, iterable, `size()`, `isEmpty()`, `map`, `filter`, `forEach` | The list. `map` returns a JavaScript array so `list.map(x => x.field)` works without copying first. Each row is shared: an index, `get`, `for...of`, `map`/`filter`/`forEach`, `getPartialList`, `filterByDate` and an `<iterate>` entry give the row itself, so a field set on `list[0]` is kept in the list, as in Groovy. The rows are copied from the find's result when the list reaches a script, so the entity cache's list does not change. `list[0] === list[0]` is still false: each read is a new host object over the same row |
| decimal, timestamp | Host values | `rust_decimal` and `jiff` from `moqui-lib`. Not JavaScript numbers. Operators are not overloaded. `valueOf` throws an error naming the method to use, and `toString` prints, so `` `${decimal}` `` interpolation works. `+`, relational operators, and `==` against a number or a string throw. `==` between two decimals does not call `valueOf` and is reference equality, so it returns false. A decimal is always truthy, including zero, so `if (amount)` and `amount || fallback` do not match Groovy. The host methods are `add`, `subtract`, `multiply`, `divide`, `eq`, `compare`, `isZero`, and `toNumber`. The other side of `add` through `compare` is a decimal, a JavaScript number, or numeric text (`amount.compare(70)`); a number that is not finite or other text throws. `toNumber()` is the explicit way to a JavaScript number when float rounding is acceptable (`Math.round`, `Number` APIs); `Number(amount)` still throws. The global `toDecimal(x)` makes a decimal from the same inputs. `divide` uses scale `max(left, right, 10)`. Checked operations return a script error when the 28-digit coefficient overflows. The browser package and the locked-down template still receive a decimal as a string. A server-static expression that does decimal arithmetic is a compatibility-report violation (`plans/archive/QvtServerStatic.md`). That is the migration rule in `plans/archive/Architecture-Proposal.md` |
| `ObjectUtilities`, `CollectionUtilities`, `StringUtilities` | The same helpers the generated script header calls today | Functions in lib, bound once per runtime. The proxy returns them when the stack has no such key. They are not own properties of `globalThis` |

Entity value properties. The value is a QuickJS exotic class object (rquickjs `class(exotic)`), not a JavaScript `Proxy`, so it stays the same host object for `js_to_host` and costs one prototype lookup and one field map lookup for a field the value holds.

- A method wins over a field of the same name. Groovy can tell `value.update` (map get) from `value.update()` (method call); JavaScript reads the property before it calls it, so the method has to win. Such a field is read with `get`. No framework field has a method name.
- A field name reads the field with the same conversion `get` uses (a decimal is the host decimal, a timestamp is text). A field of the entity the value does not hold reads as null.
- A relationship name (full name, `title#entity`, or `short-alias`) that is not a field reads the related value for a `one` relationship and an entity list for `many`, as `EntityValueBase.get` does. It runs the find each time, as Java does.
- Any other name reads as undefined. Java throws there, but the engine and scripts look up names a value does not have (`toJSON`, `then`, `Symbol.toPrimitive`, `constructor`), and a throw would break `JSON.stringify`, `await`, and template text. `get` of an unknown name stays null.
- Writing a name that is not a field of the entity is an error, `The field name x is not valid for entity y`, as Java `put` throws. `set` checks the same way. When no entity definition can be reached every name is set.
- `name in value` is a prototype member or a field of the entity. `Object.keys`, `for...in`, spread and `JSON.stringify` see the fields the value holds, in name order. `delete value.field` does nothing.
- An entity list is exotic too, for `list[0]` and `length`. Assigning to a list property is ignored. `ec.context.get(name)` gives the same list or map a bare `name` reads, so its writes reach the stack; `context.name` is still a snapshot.

`ec.entity`, `ec.service`, and the rest of the facade surface are the Server host methods section. They exist only on this QuickJS object. The browser `ec` is the table in `plans/archive/QvtServerStatic.md` and does not grow to match the server object. `sri` is not added to the browser.

Generated screen JavaScript for a server render calls `sri` and writes to the render writer. Generated server-static qvt does not. It transcribes expressions into the Vue template, and the browser evaluates them against the package and the restricted `ec`.

## What is reused

- `Node` and `ContextStack` in lib. One tree, one stack. Engines do not own copies of them.
- The plain projection (`unwrap`). The locked-down template and browser JSON both consume it.
- Format and localize functions. QuickJS `ec.l10n`, the locked-down template, and the browser `ec.l10n` call them. The browser implementation is the subset already specified and talks to the locale catalog, not to the server function.

## What is not reused

- A separate Rust value trait per template syntax. The renderers and the macro modules share the QuickJS runtime. The locked-down renderer gets the unwrap snapshot as plain JavaScript values.
- `MNode`'s FreeMarker wrapper cache. Codegen runs once. The walker passes strings.
- A reflection wrapper in the style of `BeansWrapper`. Each host type above is written out.
- Live entity values, `ec`, or `sri` inside the locked-down template or inside the cached qvt template.

## Server host methods

Measured 2026-10-11 from `framework/template/XmlActions.groovy.ftl` and the kept screen macros in `runtime/template/screen-macro/` (`any`, `html`, `qvt`, `text`, `csv`, `xml`, `xsl-fo`, `plain`). `vuet` is excluded. The `xsl-fo` file is the one that was counted; the mode that replaces it is `html-print`. These are the calls the generated JavaScript makes. The browser `ec` keeps `l10n` format and localize, `resource.expand`, and the user allowlist from `plans/archive/QvtServerStatic.md`. It does not gain the rest of this list. `sri` stays off the browser.

### `sri`, during a server screen render

`doBoundaryComments`, `getFieldValueString`, `makeUrlByType`, `getScreenUrlInstance`, `getFieldOptions`, `getActiveScreenDef`, `popContext`, `buildUrl`, `getFieldEntityValue`, `getFieldValue`, `renderSection`, `pushSingleFormMapContext`, `renderSubscreen`, `getFieldValuePlainString`, `getFormInstance`, `getThemeIconClass`, `startFormListRow`, `endFormListRow`, `getFieldTransitionValue`, `renderSectionInclude`, `pushContext`, `setInContext`, `getFormHiddenParameters`, `getCurrentScreenUrl`, `safeCloseList`, `appendToAfterScreenWriter`, `renderIncludeScreen`, `makeFormListSingleMap`, `getRenderMode`, `getSingleFormMap`, `renderText`, `isAnchorLink`, `getFormListHeaderValues`, `startFormListSubRow`, `endFormListSubRow`, `getFieldValueClass`, `getThemeValues`, `appendToScriptWriter`, `getSectionIncludedNode`, `getFormFieldValues`, `makeFormListMultiMap`, `getNamedValuePlain`, `getFormNode`.

Property: `activeInCurrentMenu`.

The macros then read the objects those methods return. A screen URL has `path`, `url`, `getUrlWithParams`, `cloneUrlInstance`, `addParameter`, `removeParameter`, and `getPassThroughParameterMap`. A screen definition has `location`, `getMenuSubscreensItems`, `getDefaultMenuName`, and `getScreenName`. `makeUrlByType` returns `getUrlWithParams` and `addParameter`. `buildUrl` returns `path` and `url`.

### `ec` during a server screen render

- `resource` (`getResource`): `expand`, `expandNoL10n`, `expression`, `condition`.
- `l10n` (`getL10n`): `localize`, `format`, `formatCurrency`, `formatCurrencyNoSymbol`.
- `context` (`getContext`): `get`, plus the fields `formListFindId` and `orderByField`.
- `web` (`getWeb`): `sessionToken`, `parameters`, `requestParameters`, `getWebappRootUrl`.
- `user` (`getUser`): `locale`, `getLocale`, `getPeriodDescription`.
- `screen`: `isRenderModeValid`.

### Statics the screen macros call

- `WebUtilities.encodeHtmlJsSafe`, `WebUtilities.fieldValuesEncodeHtmlJsSafe`.
- `CollectionUtilities.paginateParameters`, `CollectionUtilities.getMapArrayListValues`.
- `StringUtilities.encodeForXmlAttribute`.
- `ScheduledJobRunner.getCronDescription`, one call.

### `ec` from XML actions

`XmlActions.groovy.ftl` emits these. There is no `sri` in an action run.

- `service.sync` and `service.async`, then `name`, `disableAuthz`, `multi`, `ignoreTransaction`, `requireNewTransaction`, `transactionTimeout`, `useTransactionCache`, `parameters`, `parameter`, `call`. The current template also emits `distribute(true)` for `async="distribute"`. That attribute stays rejected, so the host does not implement `distribute`.
- `entity.find`, then `useCache`, `forUpdate`, `useClone`, `distinct`, `offset`, `limit`, `selectField`, `orderBy`, `condition`, `havingCondition`, `requireSearchFormParameters`, `searchFormMap`, `one`, `list`, `count`, `iterator`, `deleteAll`, `getLimit`, `getPageIndex`, `getPageSize`.
- The list from a find: `getPartialList`, `filterByDate`, `filterByLimit`, `size`, `pageIndex`, `pageSize`.
- The value: `setSequencedIdPrimary`, `setSequencedIdSecondary`, `setFields`, `create`, `createOrUpdate`, `update`, `delete`, `deleteRelated`, `findRelated`, `findRelatedOne`.
- `entity.makeValue`.
- `entity.conditionFactory`: `makeConditionDate`, `makeActionConditionDirect`, `makeCondition`.
- `resource.expand`, `resource.expression`, `resource.script`.
- `message.addError`, `message.addPublic`, `message.addMessage`, `message.hasError`, `message.getErrorsString`, `message.clearErrors`, and the `errors` field.
- `logger.warn`, `logger.log`.
- `user.nowTimestamp`.
- `web.sendJsonResponse`, `web.requestParameters`.
- `context` as the default field map for `condition`, `parameters`, and `searchFormMap`.

`entity-data` is still a TODO in the Java template and adds no method here.

### Added by phase 8 slice 1

The framework screens call these before any widget renders, so they exist ahead of the rest of `sri`.

- `sri.sendRedirectAndStopRender(url)`: 302, or 205 with `X-Redirect-To` in an always-standalone mode, and the screen is not rendered. The rest of `sri` waits for slice 2.
- `ec.web.saveScreenLastInfo(path, parameters)`: pushes the path, or the screen being rendered, on the screen history. The parameters are not saved yet.
- `ec.user.getPreference(key)`: the `UserPreference` row for the logged-in user, else null. Preference groups are not read.
- `System.getProperty(name)`: `get_prop_or_env`, null when empty.
- A script `Set` kept in a context name round-trips between action blocks. The stack holds it as a one-key map `{"__set__": [items]}` and the next block gets a `Set` back. `html_scripts`, `footer_scripts`, and `html_stylesheets` depend on this.

### Added by phase 8 slice 2

The macro engine, `sri`, and the text, csv and xml modes. The macro modules are
`framework/template/screen/{ScreenSupport,ScreenText,ScreenCsv,ScreenXml}.js`. They are embedded in the binary,
and a `macro-template-location` that is a real file other than the built-in path replaces the mode's module.
A module is a function of the support object and `ScreenSupport` and returns `{macros, opaque, prelude}`. The
walker builds the whole node tree first (`name`, `attrs`, `text`, `children`, `parent`, and `inert` for the
descendants of an `opaque` element), then calls `macros[name](node, children)` in post-order. A macro that must
place a child in a loop of its own reads `node.children[i].source`, so there is no separate `visit` hook.

`sri`, added now. Methods that return text in Java return `""` here, so `${sri.renderSubscreen()}` in a string
template still works.

- Output and state: `write`, `getRenderMode`, `doBoundaryComments`, `getActiveScreenDef` (`location`, `screenName`,
  `getScreenName`, `getLocation`, `getDefaultMenuName`), `encodeHtml`, `appendToAfterScreenWriter`,
  `appendToScriptWriter`, `getAfterScreenWriterText`, `getScriptWriterText`.
- Rendering: `renderSection`, `renderSectionInclude` (also a name or location with `${}`), `renderSubscreen`,
  `renderIncludeScreen`, `renderText`, `getSectionIncludedNode` (the section name only).
- Context: `pushContext`, `popContext`, `setInContext`, `pushSingleFormMapContext`, `getSingleFormMap`. The map
  argument of the last two is the map or an expression string, as `form-single.@map` is.
- Fields: `getFieldValue`, `getFieldValueString` (one argument with a widget, or field, default, format and, for
  date-time, the type), `getFieldValuePlainString`, `getFieldValueClass`, `getNamedValuePlain`, `getFieldEntityValue`,
  `getFieldOptions` (a `Map`, so numeric-looking keys keep their order). The widget and field arguments are element
  objects `{name, attrs, text, children, parent}` that the generated code writes as literals.
- Forms: `getFormNode`, `getFormInstance` (`formNode`, `name`, `list` only), `getFormHiddenParameters`,
  `openFormList(name)` (runs the form's `entity-find` and `row-actions` and the fields' `from`, returns `{id, count}`),
  `startFormListRow(info, index)`, `endFormListRow`, `safeCloseList(info)`, `formListColumnCharWidths`. Java passes the
  row and the render info to `startFormListRow`. Here the rows stay on the host so dates and decimals keep their types.
- URLs: `buildUrl`, `buildUrlFromTarget`, `makeUrlByType`, `getScreenUrlInstance`, `getCurrentScreenUrl`,
  `isAnchorLink`, and the property `activeInCurrentMenu`. A URL object has `path`, `url`, `pathWithParams`, `readOnly`,
  `getPath`, `getUrl`, `getPathWithParams`, `getUrlWithParams(extension)`, `getMinimalPathUrl`, `isReadOnly`,
  `cloneUrlInstance`, `addParameter`, `addParameters`, `removeParameter` (these three return the URL),
  `getParameterMap`, `getPassThroughParameterMap`.
- Themes: `getThemeIconClass`, `getThemeValues`. The current theme is the user's `UserScreenTheme`, else the
  `Enumeration.enumCode` of the theme type, which is simpler than Java: user group themes and the `DEFAULT` lookup
  are not read.

`sri`, intentionally deferred to the html slices because only their macros call them: the server-side `dynamic-options`
(which runs a transition as a test render, as `getFieldTransitionValue` now does), `makeFormListSingleMap`,
`makeFormListMultiMap`, `getFormListHeaderValues`, `getFormFieldValues`, `startFormListSubRow`, `endFormListSubRow`,
`getFormInstance().makeFormListRenderInfo()`, and the screen definition's `getMenuSubscreensItems`.
Form-list `group-by`, `sub-list` and `show-total`, saved finds and `select-columns` are done (`sri.formListSubRowCount`,
`sri.startFormListSubRow`, `sri.endFormListSubRow`, `sri.getUserFormListFinds`, `sri.getUserDefaultFormListFindId`,
`sri.renderUserColumnsForm`).
`auto-fields-service`, `auto-widget-service`, `DbForm` forms and `dynamic="true"` forms are assembled (Phase 8 notes in
`plans/archive/Implementation-Phases.md`). A related entity's drop-down in `auto-fields-entity` is a text widget.

`ec` and statics, added now:

- `ec.resource`: `expand(text, location, map)` (localizes first when shorter than 256 characters, as Java does),
  `expandNoL10n`, `expression(expr, location, map)`, `condition(expr, location)`. The optional map is put in a
  context frame of its own for the call.
- `ec.l10n`: `localize` (the `LocalizedMessage` locale chain `en_US`, `en`, `default`, the text before `##` as a last
  try, an error over 255 characters, cached in `l10n.message`), `format(value, pattern)`, `formatCurrency(amount, uomId,
  digits)`, `formatCurrencyNoSymbol`, `parseTimestamp(text, format)`. The formatter is `moqui_lib::format`: the `#,##0.00`
  pattern subset with `HALF_EVEN`, a small separator table for the common languages, the currency symbol from the
  `Uom` row or a table of ISO codes, and the date patterns of `moqui_lib::datefmt`. Shared vectors:
  `crates/moqui-lib/testdata/format-vectors.json`.
- `ec.context`: `get(name)` and the properties `formListFindId` and `orderByField`.
- `ec.screen.isRenderModeValid(mode)`.
- `ec.web`: `requestParameters`, `getWebappRootUrl(...)` (the base URL; the four arguments are ignored).
- `ec.user`: `locale`, `getLocale()` (text such as `en_US`), `nowTimestamp` (a time stamp reaches a script as text).
- `WebUtilities`: `encodeHtml`, `encodeHtmlJsSafe`, `fieldValuesEncodeHtmlJsSafe`, `ipMatches`,
  `isSafeSinglePathSegment`, `isSameOriginRedirect(url, host)`, `lastPathSegment`.
- `StringUtilities`: `encodeForXmlAttribute`, `base64UrlEncode`, `base64UrlDecode`.
- `CollectionUtilities`: `paginateParameters`, `getMapArrayListValues`.
- `entity find .orderBy("a,-b,^c")` takes comma separated fields, as Java does.
- `ScheduledJobRunner.getCronDescription`, `ec.user.getPeriodDescription` and `ec.web.request` are not added.

### Added by phase 8 slice 3

The fancy renderers, the print mode, and the render calls services use. Dialect and registration are Template
renderers in `plans/archive/Architecture-Proposal.md`.

- The locked-down renderer runs in its own QuickJS runtime on the thread, with no `ec`, `sri`, context stack or host
  objects. The snapshot is JSON of the map passed in (`unwrap_json`: a decimal is a string, an entity value is its
  field map, an entity list is a list of maps, a host object is dropped and logged), parsed there and read through a
  `with` over a `Proxy` that returns null for an unset name. `format(value, pattern)` and `localize(text)` are its only
  functions besides the language built-ins. A run is cut off after five seconds and 32 MB.
- `sri.renderInlineText(body)`: a `<text template="true">` body as a trusted fancy template, returning the text. The
  function is compiled once per body and kept per engine. `sri.renderText(location, template)` picks the renderer by the
  extension of the location, and a location with no renderer is a trusted fancy template.
- `sri.once(key)`: true the first time a render sees the key. The print mode writes its document wrapper with it.
- `ec.resource.template(location, data)`: the file rendered by the renderer for its extension, as text.
- `ec.resource.xslFoTransform(...)`: throws "PDF is not available until phase 12".
- `ec.screen.render(options)` with `{screenPath, renderMode, rootScreen, baseLinkUrl, lastStandalone, parameters}`
  returns `{status, contentType, text}`, and `ec.screen.makeRender()` is the Java builder over it (`webappName`,
  `rootScreen`, `rootScreenFromHost` (ignored), `screenPath`, `renderMode`, `baseLinkUrl`, `lastStandalone`,
  `parameters`, `render()` returning text). `ec.screen.getMimeTypeByMode(mode)` and `ec.screen.isRenderModeText(mode)`.
  A render started inside a render keeps the outer render's output; `xsl-fo` means `html-print` in all of them.
- `sri.getFieldOptions` runs `dynamic-options` through `ScreenTest` (`screen::ScreenTest`, `ec.screen.makeTest()` in
  Java) when the other option sources give nothing: the transition is rendered with the `depends-on` values and the
  parameters, and its JSON (a list, or a map with `options`) is read with `value-field` and `label-field`.

### Added by phase 8 slice 4

The html mode, its page shell, static files, uploads, and the messages a redirect carries.

- Mode runtime: a macro module may export `runtime`, source that each script engine runs once before the first unit of
  that module (`$mqHtml` for `ScreenHtml.js`). `sri.getFieldValidation(subNode)` (`{classes, regexp, message}` from
  `validate-service`/`validate-parameter` or `validate-entity`/`validate-field`), `sri.getMenuItems(path?)` (the menu
  entries of the active screen, or of the screen at `path`: `name`, `title`, `url`, `disabled`, `inPath`, `image`,
  `imageType`, `hasParameters`; only items the user may view), `sri.getScreenUrlInfo()` (`targetScreen`, `parentScreen`,
  each with `screenName` and `getDefaultMenuName()`).
- `ec.web`: `getPageMessages()` (`{message, typeString}` for saved then current messages and errors), `savedMessages`,
  `savedErrors`, `savedValidationErrors`, `errorParameters` (what a failed submit posted, once), `getScreenHistory()`
  (`{name, url, urlNoParams}`), `getResourceDistinctValue()`, `getHostName(withPort)`, `multiPartParameters`.
- `ec.user.passwordRules` (`minLength`, `minDigits`, `minOthers`), `ec.factory.moquiVersion`, and `find.disableAuthz()` on
  an entity find.
- Uploaded file object: `name`, `fileName`, `fieldName`, `contentType`, `size`, `getString()`, `write(location)`
  (`dbresource://` only), `isInMemory()`. A script cannot hold the bytes.
- Error pages: `errorThrowable` has `message`, `getMessage()`, `getArtifactInfo()` (`getName()`,
  `getActionDescription()`, `getTypeDescription()`), `getArtifactStack()` (empty), `getStackTrace()` (the message lines),
  `pathFromLastScreen`, `fullPathNameList`, `resourceLocation`, `lastSd` (null).
- Host: `web::body_limit(request)`, `web::max_upload()` (`webapp_max_upload`), `webapp_notification_endpoint`,
  `ResourceRef::{read_bytes, size, file_path}`.
- Browser library paths the html mode references (vendored by the libs slice, same paths):
  `/libs/fontawesome/css/font-awesome.min.css` (with `/libs/fontawesome/fonts/`),
  `/libs/tom-select/tom-select.complete.min.js`, `/libs/tom-select/tom-select.css`, `/libs/sortablejs/Sortable.min.js`.
  `runtime/base-component/webroot/screen/webroot/README.md` lists them too.

### Added by phase 8 slice 6

The qvt mode, its shell and the menu data.

- `sri.getMenuData(pathNames)`: the `menuData` transition of `webroot.xml`. A list of `{name, title, subscreens, path, pathWithParams,
  hasTabMenu, renderModes, image, imageType}` for each screen on the path above the target and a last `{..., extraPathList, screenDocList,
  savedFinds}` for the target; null after a 401, 403 or 404 JSON error was sent. `savedFinds` and `screenDocList` are empty lists. It runs the
  `pre-actions` of the screens on the path from the third, and drops the subscreens the user may not view (the parents are on the artifact stack for
  the check), a subscreen with a missing required parameter, and one that is not `menu-include` and not on the path. `getMenuItems` is still the html
  navigation.
- `sri.getScreenUrlInfo().extraPathNameList`: the names after the transition. The url objects of `makeUrlByType` and `buildUrl` gain `isScreenUrl()`
  (not a plain URL), `isTransition()` and `getTransitionMethod()` (lower case, empty for a screen).
- `ec.user.timeZone` / `getTimeZone()` (the session zone name, else `UTC`) and `ec.user.userAccount` (the signed-in user's `UserAccount` as a map without
  `currentPassword`, `passwordSalt`, `passwordHashType`, `passwordSetDate`, `passwordHint`, `successiveFailedLogins`, `disabledDateTime` and
  `requirePasswordChange`).
- Mode runtime for qvt: `$mqQvt` (`ScreenQvt.js`), with the form values of `ScreenRenderImpl.addFormFieldValue` computed in the runtime from the widget
  elements and `sri.getFieldValue*`, `getFieldOptions`, `getFieldEntityValue`, `getFormHiddenParameters`, `getNamedValuePlain`. `T.options.mode`
  is the render mode for a module that serves more than one (`ScreenPlain.js`).
- Response headers of the qvt family: `Cache-Control: private, no-store`, and `X-Moqui-Qvt: merged-fragment` for `qvt`.
- `screen::vue_check::compile_errors(compiler_source, templates)`: the vendored Vue compiler in a QuickJS runtime of its own, for tests and the compatibility report.
- `sri.getFieldTransitionValue(transition, node, term, labelField, alwaysGet)` and `sri.getFieldValidationJsRules(sub)` (`[{expr, message}]`),
  added after Phase 11 for the `dynamic-options` label and the qvt field rules.
- For the `tui` mode ([Screen-TUI-Impl](Screen-TUI-Impl.md)): `sri.renderTextValue(location, template)` (as `renderText`, the
  text returned rather than written), `sri.uniqueId(id)` (an id no earlier node of the document has, `id~2` for a repeat),
  `sri.htmlText(html)` (the JSON of HTML as text blocks and numbered links) and `sri.getFieldValidationRules(sub)` (the same
  checks as `getFieldValidationJsRules`, as data: `[{kind, pattern, min, minEq, max, maxEq, message}]`).

### Added by phase 8 slice 7

Server-static qvt (`plans/archive/QvtServerStatic.md`).

- `sri.staticUrl(text, urlType)`: where a link of a static template goes, with no parameters: `{path, exists, isTransition, isScreen, readOnly, method}`. The
  name is resolved against the screen being rendered, and a name that is a transition of the active screen is found when the screen is not mounted. A
  text with `${` is not resolved.
- `sri.staticCheck(json)` and `sri.staticNote(kind, element, detail)`: the expressions a static macro wrote (`[{expr, element, attr}]`) and the server-only
  work it could not write. Both do nothing unless a compatibility report is being made.
- `ScreenQvtStatic.js` runtime `$mqQvtS` (`$s`): it writes the template. The generated unit reads no request data and runs no action.
- Server package (`GET {screen}/actions`): the unwrapped context with credential names removed at any depth, the lists and page keys of every form-list removed
  (`{list}`, `Count`, `PageIndex`, `PageSize`, `PageMaxIndex`, `PageRangeLow`, `PageRangeHigh`), and `mqExtra_{form}` for each form-single: `{field}_display`
  (display-entity and entity links), the fields its `set` elements made, and the option lists of its drop-downs under `mqOpt_{form}_{field}_{sub}_{widget}`, where the
  two indexes are the places of the field's sub-node among the field's children and of the widget in the sub-node. `GET {screen}/actions/{form}` answers the same
  kind of map for a form-list: the list under its name, the page keys, the values of the header fields, the option lists, and each row with `{field}_display` and the
  fields its `set` elements made.
- Browser `ec` (`qvt.makeEc`): the table in `plans/archive/QvtServerStatic.md`. `ec.resource.expand(text, table)` and `expandNoL10n` take the table of functions by
  expression text that the compiled template carries; there is no `eval`. `$mq` is the helper object (`str`, `dflt`, `nul`, `cur`, `fmt`, `merge`, `qs`, `url`,
  `shown`). `$scope` is the scope chain. The compatibility report's wrapped `ec` has the same members.
- Report: `screen::statics::static_report`, `static_reports_for_files`, `StaticReport`, `Violation`. `script::set_widget_wrap` turns the wrapped `ec` on for widget
  expressions (`eval_condition`, `eval_value`, `eval_map`, `expand`, `ec.resource.expand/condition/expression`), and `run_actions` turns it off for the actions.

### Added by phase 11 (tools)

The calls the tools screens and services make (`script/runtime/tools_host.rs`). They are outside the Server host methods list.

- `ec.cache`: `cacheExists`, `clearAllCaches`, `getAllCachesInfo(orderBy, filterRegexp)`, `makeElementInfoList(name, orderBy)`. The list is the conf caches,
  `screen.definition`, and the entity list caches (`entity.record.list.{entity}`); `getCache(name).clear()` clears any of them.
- `ec.entity`: `isEntityDefined`, `getAllEntityNames`, `getAllEntitiesInfo`, `getAllEntityInfo`, `getAllEntityNamesWithMaster`, `getAllEntityRelatedFields`,
  `getEntityDefinition(name)` (names, `entityInfo`, fields, relationships, dependents, and the Java getters), `getDatasourceGroupNames`, `getEecaRuleCount`,
  `eecaRulesByEntityName.get`, `runSqlStatement(group, sql, limit)` (the SQL runners), `makeDataWriter()` (`file`, `directory`, `zipFile`, `zipDirectory`,
  `text`), `makeDataLoader()`, `formatFieldString`. `getQueryStatsList` is empty: query statistics are off by default in Java and not kept here.
- `ec.service`: `getKnownServiceNames`, `getAllServiceInfo`, `getSecaRuleCount`, `secaRulesByServiceName.get`, `getServiceDefinition(name)` (with
  `serviceNode`), `getJobRunner()`, `jobWorkerPool`, `isLastRunBeforeLastSchedule(cron, lastRunTime)`, `restApi.getFreshRootResources()`.
- `ec.screen`: `getAllRootScreenLocations`, `getScreenInfoList(root, levels)`, `getFormNode(location)`. `ec.factory`: `getStatusMap`, `getComponentInfoList`,
  `getRuntimePath`, `getThreadList`, `workerPool`, `versionMap`. `ec.resource.getLocationReference(location)` and `getLocationText`. `ec.web`:
  `getRequestDetails`, `secureRequestParameters` (the parameters not in the URL query string). `ec.user.getPeriodRange`, `ec.message.messageInfos`,
  `ec.l10n.getAvailableLocales`, `getAvailableTimeZones`, `ec.context.putAll`, `ec.artifactExecution.isPermitted(name, type, action)`.
- Helpers: `CollectionUtilities.paginateList` and `orderMapList`, `StringUtilities.camelCaseToPretty`, `prettyToCamelCase`, `encodeAsciiFilename`,
  `WebUtilities.isRestrictedGenericEntity`. An entity value has `getPrimaryKeys()`. `deleteAll()` returns the count.
- The stack holds data, so a method does not survive a `set`. A tools object is a map with a `__wrap` name; reading it back from the stack gives it its methods
  again (`globalThis.__moquiWrap`). A method of any other object is left out when the object goes on the stack, as the unwrapped projection leaves out a function.

### Added by phase 12

- Binary values cross into a script as `Uint8Array`, and a `Uint8Array` or `ArrayBuffer` goes back as bytes. Before, a binary value could not be read by
  a script. `context` still leaves binary values out.
- `ec.resource.xslFoTransform(document, null, null, 'application/pdf')` returns the PDF bytes of an `html-print` (old name `xsl-fo`) document. Java
  wrote to an `OutputStream` argument, which a script has none of. A stylesheet or another content type is an error.
- A resource reference from `ec.resource.getLocationReference` has `putBytes(bytes)` beside `putText`.
- `SftpClient`, a global like `EdiHandler`: `new SftpClient(host, username, port?)` for the Groovy `new org.moqui.sftp.SftpClient(...)`, with the Java
  methods (`plans/archive/Ssh.md`). Bytes are `Uint8Array`; `openStream` and `putStream` return stream objects with `read`, `readAllBytes`, `readText`, `write`,
  and `close`.

## Out of scope

- QuickJS operator overloading for decimals. QuickJS-NG ties it to the built-in `BigDecimal` and `BigFloat` classes (`CONFIG_BIGNUM`). It does not extend to a host class, so the build flag does not give a host decimal `+`. The browser cannot share that flag either.
- Running the macro module per request so a screen function can read the context stack. Request data is read by the generated script.
- A second script or template engine. V8 is the later option in `plans/archive/Architecture-Proposal.md`, and only as a replacement for this runtime if a profile says the JavaScript is the heavy part.
