# JavaScript in screens

A screen has JavaScript in several places, and they do not all run in the same place or see the same names. On the
server: the actions of the screen, its sections and its transitions, and the expressions in widget attributes. In the
browser: the expressions of a server-static template, which see a data package and a small `ec`, and the script of a
`.qvue` component, which is ordinary Vue code. This guide says what runs where, in what order, and with which names.

The action elements are in [JS-XML-Actions](JS-XML-Actions.md), the members of `ec` and `ec.web` in
[JS-Core-API](JS-Core-API.md), `sri` and the macro modules in [JS-Templates-Macros](JS-Templates-Macros.md), and the
browser client in [Qvt-Client](../ui/Qvt-Client.md). Screen practices are in
[Practices-App-Dev](Practices-App-Dev.md#5-screens).

## Server contexts

| Place | Runs | As |
|---|---|---|
| `<always-actions>` | For every screen on the path, on a render and on a request to a transition | XML actions |
| `<pre-actions>` | Before a render, for the screens that are rendered | XML actions |
| `<actions>` of the screen or of a `<section>` | When that screen or section renders, after its `<condition>` holds | XML actions |
| `<condition>` of a section, a transition or a response | When it is reached | A condition group, or one expression |
| `<transition>` `<service-call>` and `<actions>` | On a request to the transition | XML actions, two scripts |
| `<row-actions>` of a form-list | Once for each row | XML actions |
| An attribute of a widget or a form field | When the widget renders | An expression or a `${}` template |
| `<parameter from>` and `<parameter value>` | When a transition's or response's parameters are read | An expression, a `${}` template |

All of these run on the server's script engine with the context stack as their variables
(`crates/moqui-core/src/screen/render.rs`, `script/runtime.rs`).

### Names in scope

| Name | |
|---|---|
| Request parameters | Every parameter of the request is a context name, in a frame for the request. A parameter given inline in the path is one too |
| `ec` | The execution context. `ec.web` has the request |
| `context` | The context stack, the same object as `ec.context` |
| `sri` | The screen renderer. It is a global, and its methods work only while a render is in progress ([JS-Templates-Macros](JS-Templates-Macros.md)) |
| What actions set | A name an `always-actions`, `pre-actions` or `actions` block set stays in the context for the screens and widgets under it |
| `<entry>`, `<entry>_index`, `<entry>_has_next`, `sectionEntryIndex`, and the `key` name | In a `section-iterate`, in a frame for each entry |
| The names of a form-list row | See [Form expressions](#form-expressions) |
| `formListFindId`, `orderByField`, `pageIndex`, `pageSize` | Request parameters of a form-list: its saved find, its sort and its page |

An unset name reads as null, and a bare assignment writes the top frame of the stack
([JS-XML-Actions](JS-XML-Actions.md#names)). Screen actions are not isolated as a service body is: they see the
names of the screens above them.

The context holds data, not functions. Do not set a name to an arrow function; compute the value.

### The order of a render

For a request to a screen path (`render.rs` `render_screens`):

1. The request parameters are put in a context frame, and the saved find the request names (or the user's default
   one for the target screen) is applied.
2. `always-actions` run for every screen on the path, the root first.
3. `pre-actions` run for each screen from the first one rendered down. In a render mode the conf marks
   `always-standalone` (`qvt`, `qvue`, `qjs`, `csv`, `html-print`), the parents' `pre-actions` run too, from the third
   screen of the path on.
4. A `required` screen parameter that is still empty is an error, "Required parameter missing".
5. The first screen to render renders: its root section's `condition`, then its `actions`, then its `widgets`, or
   `fail-widgets` when the condition is false.
6. Each screen below renders where its parent's widgets have `subscreens-active` or `subscreens-panel`, the same
   way: condition, actions, widgets. A parent's `actions` therefore run before its child's.

- A section is the same three steps (`sri/sections.rs` `render_single`). When its actions leave an error, the errors
  are written in place of its widgets.
- A subscreen rendered on its own (a tab loaded by the client, a `lastStandalone` render) runs its parents'
  `always-actions` and not their `actions`.
- A screen stopped at step 4 runs none of its actions; the errors are written in place of the first rendered screen's
  widgets (the stopped screen's own, when it is the first).
- A screen's own `condition` (its root section's, before `actions`) with no `fail-widgets` says whether the screen is
  offered here. When it is false the screen is left out of the menu data (the browser's tabs and menus) and of
  `sri.getMenuItems` (the terminal's and html's tab bars), and a request that names the screen by its path shows
  "... is not offered here." in place of its widgets, with no actions run. It is evaluated in the context of the
  parents: the menu data runs the path's `always-actions` first when a listed subscreen has a condition, and a tab bar
  is drawn inside its parent's render. Keep such a condition to what `always-actions` set (an app root's `appRoot`, a
  360's tab list), and give the parent a default subscreen, or a `conditional-default`, that is offered in each
  context: the default is not checked. A condition that fails to evaluate leaves the screen offered (logged). With
  `fail-widgets`, the condition only chooses between `widgets` and `fail-widgets`, and the screen is always offered.
- A widget that fails does not fail the page: "Error rendering ..." is written where its output would have been. A
  security error (not logged in, not authorized, a tarpit) does stop the render.
- A screen of the path that fails (its actions or its widgets, other than a section's or an included screen's) is
  written in place the same way, inside the shell above it, but in an html or text render the page then answers the
  error's status, 500 (503 when the database was unavailable), with `Cache-Control: no-store`: the screen the request
  addresses is that screen or under it, so the page lost its own content. An error with a status of its own (a
  `NotFound` thrown in a path screen's `actions`) answers with its error page instead. A qvt or tui subscreen's error is
  shown inside the parents the client keeps, with the request's status as it was. The first screen's failure is the
  render's: its error page.
- `sri.sendRedirectAndStopRender(url)` in `pre-actions` or `always-actions` answers with a redirect and renders
  nothing.

### The order of a transition

For a request to a transition (`render.rs` `run_transition_steps`), in one transaction unless the transition has
`begin-transaction="false"`:

1. `always-actions` of every screen on the path.
2. The transition's `path-parameter` names take the extra path segments, and the `parameter` elements of the screen
   and of the transition are set in the context: `from` as an expression, `value` as text, else the context or
   request value. A missing `required` parameter answers 400.
3. The transition's `<condition>`. When it fails, an error is added, nothing runs, and the `error-response` (or the
   `default-response`) is used.
4. The `<service-call>`, only when there is no error so far.
5. The `<actions>`, always, so they can handle an error.
6. The response.

The `service-call` of a transition has defaults a `service-call` in actions does not: `in-map="true"` (the whole
context, of which the service takes its declared parameters) and `out-map="context"` (each result is a context name
after the call). Java also defaults it to `multi="parameter"` (a multi call when the request has `_isMulti=true`); here
that is the default only when a multi form of the screen posts to the transition (a `form-list` with `multi="true"`, or
a `row-selection` action not in `list-mode`; its own, a `section-include`'s or a `screen-extend`'s), so any other
shortcut is one call whatever the request says, and a posted `<name>_0` cannot stand in for an id the screen pinned
(SEC-RTE-81). Write `multi="parameter"` on a shortcut that another screen's multi form posts to. It is compiled apart
from the `<actions>`, since the call returns when it leaves an error and that must not skip the actions.

```xml
<transition name="openEntity">
    <actions><set field="openDefinition" from="jumpTo == 'definition'"/></actions>
    <conditional-response url="../Schema/Entity"><condition><expression>entityName &amp;&amp; openDefinition</expression></condition>
        <parameter name="selectedEntity" from="entityName"/></conditional-response>
    <conditional-response url="../Data/Grid"><condition><expression>entityName &amp;&amp; !openDefinition</expression></condition>
        <parameter name="entityName"/></conditional-response>
    <default-response url="."/>
</transition>
```

The response is chosen after the actions (`pick_response`):

1. With an error or a validation error in `ec.message`, the `error-response` when there is one.
2. Else the first `conditional-response`, in the order of the file, whose condition holds: its `condition`
   attribute (an expression), else its `condition` element; one with neither always holds. The conditions after it
   are not evaluated. (Moqui Java takes the last that holds.)
3. Else the `default-response`.

The response's `url` is a `${}` template, and its `parameter` elements and `parameter-map` are evaluated in the
context the actions left. A `type="none"` response sends nothing: the actions answered.

A transition with no `service-call` and no `actions` is read-only: a link, not a post.

### Answering from a transition

```xml
<transition name="saveTask">
    <actions><script><![CDATA[
        const text = 'Gallery: the task "' + (taskName || '') + '" would be saved'
        ec.web.sendJsonResponse({messageInfos: [{message: text, type: 'success'}]})
    ]]></script></actions>
    <default-response type="none"/>
</transition>
```

| Pattern | |
|---|---|
| `ec.web.parameters` | The request's parameters as a map. `requestParameters` is the same map |
| `ec.web.sendJsonResponse(value)` | Sends the value as JSON. Entity values and lists in it are sent as plain maps and lists. Use `default-response type="none"` |
| `ec.web.sendJsonError(status, message)`, `sendError`, `sendTextResponse`, `sendResourceResponse` | Other answers a script sends itself |
| `ec.web.sessionAttributes` | The session: read a name, assign one, `put`, `remove` |
| `ec.web.sessionToken` | The CSRF token of the session |
| `ec.web.savedMessages`, `savedErrors`, `savedValidationErrors`, `errorParameters` | What the last request left for this one |

- **Messages cross a redirect.** When a transition answers with a redirect, the messages and errors in `ec.message`
  are saved in the session and shown by the next page. With an error, the request's parameters are saved too
  (passwords and the token left out), so the form shows what was typed (`errorParameters`).
- **A JSON client gets the messages in the answer.** A request to `<transition>.json`, or with `Accept:
  application/json`, is answered with JSON in place of a redirect: `screenUrl`, `screenPathList` and
  `screenParameters` for a screen, the messages and the errors. The messages are then not saved for the next page.
- `ec.artifactExecution.runAsSystem` is refused in screen actions, transitions and widget expressions. Put the work
  in a service ([JS-Service-Scripts](JS-Service-Scripts.md#authorization)).

## Widget expressions

A widget's attributes are compiled with the screen: the macro module of the render mode writes a JavaScript function
for each section, and that function evaluates the attributes when it runs (`screen/generate.rs`).

| Attribute | |
|---|---|
| `condition` | A JavaScript expression, by JavaScript truth: an empty list is true |
| `from`, `in-map`, `parameter-map`, `text-map`, `map` of a form-single, `list` of a `section-iterate`, a form-list, a `list-options` or a `tree-sub-node` | A JavaScript expression |
| Any other attribute with `${...}`, such as `text`, `url`, `title`, `style` | Text, expanded as a template literal. `ec.resource.expand` localizes text shorter than 256 characters before it expands it |
| `<text template="true">` body | A trusted fancy template ([JS-Fancy-Templates](JS-Fancy-Templates.md)) |

```xml
<section name="NotFound" condition="!enumeration"><widgets>
    <container-box type="warning"><box-header title="Not found"/><box-body>
        <label type="p" text="There is no enumeration ${enumId}. The real screen goes back to the list with a notification."/>
        <link url="list" text="Back to the list"/>
    </box-body></container-box>
</widgets></section>
```

- An expression sees the context as it is when the widget renders, the row or entry frame included.
- A `container-box` `condition` is evaluated as a section's is (`sri.checkCondition`): when it is false nothing of
  the box renders (header, toolbar, body), in every render mode; one that fails is logged and its error shown in place
  of the box, as for a section; in qvt-static it is a `v-if` over the box. Unlike a section, a box has no actions and
  no fail-widgets.
- A decimal in a `${}` prints its digits. Format with `ec.l10n.format(amount, '#,##0.00')` or
  `ec.l10n.formatCurrency(amount, uomId)`.
- A `tui` text is JSON: build it with `${JSON.stringify(...)}`, never by pasting `${}` values into JSON.
- A `render-mode` text for `qvt` is a Vue template. `${}` in it is evaluated on the server before the browser sees
  it; `{{ }}` is Vue's and runs in the browser.

### Form expressions

A form field's `from` is an expression, and its widgets' attributes are templates as above. In a `form-single` the
values come from the form's `map` (an expression, or a context name) and fall back to the context.

A `form-list` makes a context frame for each row (`screen/sri/forms.rs` `form_row`):

1. The list is the result of the form's `entity-find`, or the value of its `list` expression.
2. For each row: the row's fields are names in the frame, with the row itself as `<list>_entry`, its index as
   `<list>_index` and `<list>_has_next`. With `list-entry="item"` the names are `item`, `item_index` and
   `item_has_next`.
3. `row-actions` run in that frame.
4. Each field with a `from` is set under the field's name.

So a field named `statusId` reads the row's `statusId` with no `from`, and `row-actions` can set a name a later
field reads:

```xml
<form-list name="EnumList" list="enumList" ...>
    <entity-find entity-name="moqui.basic.Enumeration" list="enumList">
        <search-form-inputs default-order-by="description,enumId"/>
    </entity-find>
    <row-actions>
        <set field="state" from="optionIndicator ? 'Flagged' : (parentEnumId ? 'Child' : (sequenceNum != null ? 'Ordered' : 'Plain'))"/>
    </row-actions>
    ...
    <field name="state"><header-field title="State"/>
        <conditional-field condition="state == 'Flagged'">
            <link url="." link-type="anchor-button" btn-type="warning" text="${state}" parameter-map="{optionIndicator:'Y'}"/></conditional-field>
        ...
    </field>
</form-list>
```

Here `optionIndicator`, `parentEnumId` and `sequenceNum` are the row's fields, and `state` is a name the
`row-actions` set for the row (`runtime/components/dev/example/screen/Gallery/List.xml`).

`row-actions` run once for each row of the page, so a find or a service call there runs that many times. Read the
related rows for the whole list in the screen's `actions`.

## In a preview of proposed text

A screen that stands at a preview's proposed text (a dev-aide change set or a `screen-xml` canvas, `screen::overlay`)
runs its code on an allow-list host (`crates/moqui-core/src/script/preview.rs`, plans/Security-Review-Low.md
SEC-AI-102). That is its actions (always-, pre-, transition and form row actions, a form's or a drop-down's
`entity-find`, a tree node's), its widget, field and condition expressions, and the forms that `extends` one of its
forms. The screen is told by the nearest screen frame above the last service frame, so a transition, the menu data and
a form's or tree's data are covered as the render is. The code may call:

| Group | Members |
|---|---|
| Entities | `ec.entity.find`, `findOneXml`, `makeValue`, `conditionFactory`; `EntityFind`, `EntityList`, `EntityValue`, `EntityListIterator`, `EntityCondition`; authorization and entity filters apply, and no `disableAuthz()` |
| Services | `ec.service.sync()` and its builder, `callXml`; the service's authorization applies, and no `disableAuthz()` |
| Messages and text | `ec.message`, `ec.l10n`, `ec.logger`; `ec.resource.expand`, `expandNoL10n`, `expression` and `condition`, which run more of the same text on the same host, their value copied out as data; `ec.resource.script` of a `component://` location, which runs inside the same region on the same list |
| Settings | `ec.factory.getConfValue` (a secret one reads as null) |
| Context | names read and written, `ec.context`, its shared map, `ec.user.context` reads |
| The user | `ec.user` `userId`, `username`, `hasPermission`, `isInGroup`, `getUserGroupIdSet`, `getPreference`, `getPreferences`, `nowTimestamp`, `timestampMillis`, `locale`, `timeZone`, `getLocale`, `getTimeZone`, `userAccount` |
| Helpers | `StringUtilities`, `CollectionUtilities`, the pure `WebUtilities` encoders, `compare`, the decimal and date globals |
| `sri` | the reads of the render, its field values, options and validations, and the URL builders (`buildUrl`, `makeUrlByType`, `UrlInstance`); no render, write or redirect |
| Authorization | `ec.artifactExecution.authzLocked()`, `isPermitted`, `isPermittedOwn` |

Anything else throws "... is not available in a preview of proposed screen text": every `ec.web` member (the
response's writes, headers, redirects, errors and status, cookies, the session and the request's parameters), the rest
of `ec.resource` (files, locations, a `script` of another location, `template`), `ec.cache`, the rest of `ec.factory`,
the rest of
`ec.artifactExecution`, raw SQL and the other `tools`, the user's switches (sign in and out, `runAsUser`) and the
effective time, `ec.screen`, the search index and notifications. A service the code calls, and the entity filters and
ECA rules its finds and writes reach, run their own code on the whole host, and an entity filter's map reads only
`ec.user.context`, never a name the screen set.

The preview renders on a script runtime of its own, let go after it, and the allow-list holds for all of it. Its built-in
objects are frozen, and so are `ec`, each facade under it, `sri`, the helpers, the host classes' prototypes and a mode's
render functions, each once made, so the proposed text replaces no function the framework's code calls after it. Every
expansion, expression, condition, value and map, and every actions snippet of a definition (a drop-down's
`entity-options` find, a form-list's count find), runs on the allow-list above while a preview holds the runtime,
whoever's frame is on top: the trusted shell's expansion of a proposed screen's menu title among them. The
framework's render units and the screens' code around the proposed text run on a list of their own, the members a render
needs (`sri`, its writes to the page, the reads of the request, the settings, `component://` files and
`ec.llm.isEnabled()` for the shell's client config), and no response write of `ec.web`. An expression a render unit
would write into its code (a `condition`, a field's `hide`, a form-single's `map`, a `text-map`, a
`currency-unit-field`, a `disable-condition`, a report's `variables`, `frame` or `panel`) is never spliced in: it must be
one JavaScript expression alone, or the screen does not load, and it is evaluated from a string literal by
`ec.resource.expression`, as the proposed text's own, its value copied out as data. A name, id, type or size the units
write into the page as it is loses any markup character, and a `default-menu-title` or `menu-title` is made literal (no
markup, no `$`). The proposed text reads a copy of the page's script and
stylesheet lists (`footer_scripts`, `html_scripts`, `footer_stylesheets`, `html_stylesheets`), so it adds no
`<script src>` or `<link>` to the shell's page, and may not set one. A parent screen whose subscreen is proposed is not
proposed for that alone. The response keeps the preview's `Content-Security-Policy`, drops a `Refresh` header and
redirects only to this site.

## Server-static screens

A screen whose `server-static` names `qvt` is sent to the browser as a cached template with no request data in it,
and the browser fills it from `GET <screen>/actions` ([Qvt-Client, Server-static
screens](../ui/Qvt-Client.md#server-static-screens)). This changes where its JavaScript runs:

| | Runs |
|---|---|
| `always-actions`, `pre-actions`, `actions`, `row-actions`, `entity-find` of a form | On the server, for each data request. `always-actions` and `pre-actions` run again for each form-list request |
| Widget attribute expressions | In the browser, as Vue expressions over the data package |

The data package is the context after the actions, as plain data (`screen/statics.rs`): a decimal is a string, a
timestamp is text, an entity value is a map of its fields, credential names are removed, and the lists and page keys
of each form-list are left out and fetched by `GET <screen>/actions/<form>`.

**Only the names the template reads.** When the template is built, the names its compiled render function reads
(`_ctx.name`, `$scope['name']`) are recorded, and the package holds only those, `hideNav`, and the names listed in
`client-context` on the actions that ran: the `always-actions` of every screen on the path (a parent's included), the
`pre-actions` of the screens whose pre-actions ran (from the third screen on), and the screen's own `actions` and the
actions of its sections that ran (a parent's `actions` and sections do not run for a subscreen's request). A name kept is sent whole: `item` read as `item.name` sends the whole `item` map, so a value the browser must not
see is kept out of a map the template reads, not only out of the template. The rows of `GET <screen>/actions/<form>`
keep the names the template reads, the form's fields (with their `_display` texts), the fields its `row-selection` and
views name, and the `client-context` of its `row-actions`; a key of a list entry that no column shows is left out. The
rows of a screen that is not server-static are limited the same way, with no template names: only its list views (a
board's rows, a calendar's range, a tree's children) fetch them, and they read the fields the form and its views name. An
`actions` request that comes before any `.qvt` request (after a restart, on another node) builds the template first;
if it cannot be built, the package holds only the `client-context` names and `hideNav`.

```xml
<actions client-context="chartData"><set field="chartData" from="..."/></actions>
```

`client-context` (a comma or space separated list) is for a name the template reads in a way the build cannot see,
such as by a computed name (`$scope['total_' + period]`, logged as a warning when the template is built), or one a
client component reads by itself. On a `section-iterate`'s actions the names are kept inside each entry; on a
`screen-extend`'s `actions-extend` they are added to the screen's. `moqui lint --screens` lists a `client-context` name
that its actions never set.

**A static expression may use:**

- the names of the package, and of the row or the form's map inside a form;
- `ec.l10n.localize`, `format`, `formatCurrency`, `formatCurrencyNoSymbol`;
- `ec.user.userId`, `username`, `locale`, `timeZone`, `userAccount`, `getPreference(key)`;
- `ec.resource.expand` and `expandNoL10n`, which the template calls for a `${}` text with a table of functions, so
  nothing is evaluated from text;
- `ec.factory.moquiVersion`;
- `StringUtilities.camelCaseToPretty`;
- plain JavaScript: operators, `Math`, `JSON`, string and array methods.

**It may not use** (`screen/static_ast.rs`, the kinds of the compatibility report):

| Kind | |
|---|---|
| `ec` | Any other `ec` area or member: `ec.entity`, `ec.service`, `ec.web`, `ec.message`. In the browser another area throws "ec.X is not available in a server-static qvt screen" |
| `sri` | Any `sri` call. The renderer is not in the browser |
| `context` | `context.x` or `context.get(...)`. Use the bare name |
| `utilities` | `ObjectUtilities`, `CollectionUtilities`, `WebUtilities`, and the rest of `StringUtilities` |
| `server-global` | `System` and other server names |
| `decimal` | `add`, `subtract`, `multiply`, `divide`, `compare`, `isZero`, `setScale`: the package holds a decimal as a string |

Compute such a value in the screen's `actions` and put the result in the context: a formatted amount, a name for an
id, a flag for a condition.

A screen cannot be static at all when (`statics.rs` `structure_reasons`): its qvt macro module is not the built-in
one; its `render-modes` leave out `qvt`; it has `allow-extra-path`; a form is `dynamic="true"`, extends an
expression, takes its fields from an entity or service named by an expression, or says a `server-static` without
`qvt`; a `section-include` or `include-screen` names its target with an expression; or it includes a screen that has
forms.

`moqui lint --screens --qvt-static [report-file]` writes the compatibility report of every screen: whether it is
marked and eligible, the reasons, and each violation with its element. It also renders each screen with a wrapped
`ec` that records a use the static scan could not see.

## In the browser

### The browser `ec`

A static template's expressions see `ec` as the object `qvt.makeEc` makes
(`webroot/screen/webroot/js/WebrootVue.qvt.js`; `MoquiBrowserEc` in `moqui-host.d.ts`). It holds the session's values
and the formatter, and does not reach the server.

| Member | |
|---|---|
| `ec.l10n.localize(text)`, `format(value, pattern)`, `formatCurrency(amount, uomId, digits)`, `formatCurrencyNoSymbol(...)` | From the locale catalog and the client's formatter |
| `ec.user.userId`, `username`, `locale`, `timeZone` | The session's |
| `ec.user.userAccount` | `partyId`, `emailAddress`, `userFullName`, `locale`, `timeZone`; no password fields |
| `ec.user.getPreference(key)` | From the preferences the page was given; null when not there |
| `ec.resource.expand(text, table)`, `expandNoL10n(text, table)` | `table` is the functions the compiled template carries, by expression text. An expression not in the table throws |
| `ec.factory.moquiVersion` | |

Beside `ec`, the template scope has `$mq` (the template's helpers) and `$scope` (the scope itself).

### `.qvue` files

A `.qvue` file is a Vue component served by an XML screen in the `qvue` render mode
([Qvt-Client, Loading a screen](../ui/Qvt-Client.md#loading-a-screen)). Its script runs only in the browser:

```html
<template><div>
    <m-form name="GalleryTask" :action="mScreen.path + '/saveTask'" :fields-initial="initial" v-slot:default="formProps">
        ...
    </m-form>
</div></template>

<script>
module.exports = {
    inject: { mScreen: { default: null } },
    computed: {
        initial() { return { taskName: '' }; }
    }
};
</script>
```

- The `<script>` gives the component as `module.exports = {...}`, `export default {...}` or `define(...)`.
- There is no server `ec`, no context and no `sri` in it, and no `ec` is put in its scope. It talks to the server
  through the transitions of its XML screen, which keep the parameters, the authorization and the actions.
- `inject: ["mScreen"]` gives the screen's own `path` and `parameters`, for building a transition's URL.
- With `template="false"` on its `<text>` element the file is sent as it is: a `${}` in it is not evaluated on the
  server. Such a file (and a `.qvue` file under a screen) is also answered compiled, for a page on the runtime build of
  Vue with no `unsafe-eval` ([Qvt-Client, Bundled qvue](../ui/Qvt-Client.md#loading-a-screen)): then its script runs once
  on the server, with no `ec` and a stand-in `moqui`, to find the string templates of its components, so it must not need
  the browser when it loads (`window`, `document`, `Vue` at the top level; inside methods they are fine), and it cannot be
  a `<script setup>` or a module.
- Use the `m-*` components and the shell's feedback ([Practices-App-Dev](Practices-App-Dev.md#6-vue-single-file-screens-qvue)).

The XML screen beside it declares the mode and the file:

```xml
<screen render-modes="qvue,tui">
    <transition name="saveTask">...</transition>
    <widgets><render-mode>
        <text type="qvue" location="component://example/screen/Gallery/QuickTask.qvue" template="false"/>
        <text type="tui"><![CDATA[...]]></text>
    </render-mode></widgets>
</screen>
```

Such a screen has no html mode. Asked for in html under Standard's `/apps` (its menu, a link, a bookmark), it answers a
redirect to the same path and query under `/qapps`, where Material renders it; elsewhere an html request is a 404.

## `moqui lint --screens`

`moqui lint --screens [report-file]` checks every screen and form XML file under the runtime's components
(`crates/moqui-devkit/src/screen_lint.rs`): the files under a `screen` directory, the screen parts under a `template`
directory (the targets of `section-include` and `transition-include`, by their `screen` root) and `screen-extend` files. The argument is the file the report is written to, not a screen to check:
naming a screen there overwrites that screen. With no file the report goes to standard output.

| It checks | |
|---|---|
| Expression attributes | `condition`, `from`, `in-map`, `parameter-map`, `text-map`, `map` of a form-single and the `list` attributes must parse as JavaScript |
| `${}` in other attributes | Each embedded expression must parse |
| `<script>` and `<expression>` bodies | Must parse |
| `actions`, `always-actions`, `pre-actions`, `row-actions`, and a transition's `service-call` | The generated script must parse |
| `<text template="true">` bodies | Must compile as a fancy template |
| Widget elements | An element no macro names is reported: it would render as a placeholder |
| Attributes | An attribute the XSD does not define on its element (`<container condition>`, `<display align>`) is listed as a warning: the renderers ignore it, or the XSD has not caught up with one they read. It does not change the exit code, as `moqui check`'s schema warnings do not |

A parse failure, an unknown element or a template that does not compile is printed and makes the command exit 1.

The report also has, as information:

- the idiom totals by class (Groovy, hazard, valid JavaScript) and the sites of each file, with the idiom names of
  [JS-XML-Actions](JS-XML-Actions.md#moqui-lint);
- the TUI worklist: a `qvt` or `qvue` text with no `tui` text beside it, a `qvue` screen without `tui` in its modes,
  and a `tui` text with a `${` outside `JSON.stringify(`;
- containers, boxes, rows and sections with no `name` or `id`, which a `screen-extend` cannot anchor to;
- `.ftl` template locations, which are listed and not compiled.

Keep a changed screen's idiom count from growing: a hazard such as `empty-collection-truthiness` parses and then
does something other than the Groovy it came from.

## Pitfalls

- `condition="list"` is true for an empty list. Write `list.length`.
- A read in `always-actions` runs on every request to a transition of the screen, a drop-down's search included.
  Read the record in `actions`.
- A transition's `<actions>` run even when its `service-call` failed. Test `ec.message.hasError()` first when they
  must not.
- Two `conditional-response` conditions that both hold give the first response, in document order; the conditions
  after it are not evaluated. Moqui Java takes the last that holds.
- A widget that fails renders its error into the page, with status 200. A test checks the output for
  `Error rendering`.
- In a static screen `context.x` and `ec.web.parameters.x` are violations: a request parameter is a bare name of the
  package.
- A `.qvue` reads its query from `mScreen.parameters`, not from the page's URL: it may be loaded in a dialog or a
  tray.

## Component libraries

A service or screen action can use `ec.resource.script("component://my-app/script/libs/round.js").default(value)`.
The properties of the returned object are the exports of the selected npm entry point.

A screen (or its site's `screen-extend`) selects a client library with
`<client-library library="my-app/round" render-mode="qvt"/>` alongside its extension declarations before the
screen's actions and widgets. Qvt waits for each declared bundle before using the screen. Its client JavaScript
can then call `moqui.libraries["my-app/round"].default(value)`. The same declaration works with server-static
templates and a qvue served as its bundle (`?_qvt=bundle`), whose registration names the libraries. For such a qvue,
put markup in the file's template or a literal component template: the compiler can then generate render functions
without running the library on the server. The desktop shell's bundled screens (`precompiled_qvue`) do not load client
libraries. See [component build and review](../Build-Test.md#add-a-component-library).
