# Screen macros

A screen's widgets are written by JavaScript macro modules, in the place the FreeMarker macro files
(`DefaultScreenMacros.*.ftl`) have in Moqui Java. A macro does not write output. It returns JavaScript: the code of
its element. The code of a section's widgets is one function, a **unit**, made once for each render mode and kept
with the screen. A request runs the unit, which writes through the global `sri` and reads the request through `sri`,
`ec` and the context. This is for someone adding a widget macro or a render mode, or reading what a screen runs.

The modules are in `runtime/base-component/framework/template/`: `TemplateSupport.js`, `screen/ScreenSupport.js` and
one module per mode in `screen/`. The Rust side is `crates/moqui-core/src/screen/generate.rs` (units),
`crates/moqui-core/src/script/walker.rs` and `script/oxide/walker.rs` (the walk), `script/oxide/units.rs` (running a
unit) and `script/host/sri.rs` with `host/urls.rs` (the `sri` global). Text templates are
[JS-Fancy-Templates](JS-Fancy-Templates.md); a screen's actions and transitions are
[JS-Screen-Scripts](JS-Screen-Scripts.md); `ec` is [JS-Core-API](JS-Core-API.md).

## Render modes

`MoquiDefaultConf.xml` names every mode in `screen-facade`, each with its module. There are no modes built into the
server beside these.

| Mode | Mime type | Module | Output |
|---|---|---|---|
| `html` | `text/html` | `ScreenHtml.js` | Terse semantic HTML with data attributes, bound by `/js/moqui-html.js` |
| `qvt` | `text/html` | `ScreenQvt.js` | The Vue 3 and Quasar template of the screen with the request's values merged in ([Qvt-Client](../ui/Qvt-Client.md)) |
| `qjs`, `qvue` | `application/javascript`, `text/html` | `ScreenPlain.js` | Only the screen's own `render-mode` text of that type: a component object, or a component file |
| `tui` | `application/json` | `ScreenTui.js` | The terminal's JSON node document ([TUI](../ui/TUI.md)) |
| `html-print`, `xsl-fo` | `text/html` | `ScreenHtmlPrint.js` | The print document, which becomes a PDF. `xsl-fo` is an alias: the macros see `html-print` |
| `text` | `text/plain` | `ScreenText.js` | Fixed-width columns |
| `csv` | `text/csv` | `ScreenCsv.js` | A header line and a line per row for each form-list |
| `xml` | `text/xml` | `ScreenXml.js` | An element per screen, an entry per form-list row |
| `xlsx` | the spreadsheet type | `ScreenXlsx.js` | A workbook with a sheet per form-list. A binary mode (`screen-output`): the unit fills the workbook with `sri.xlsx*` calls and writes no text |

Other files there:

| File | |
|---|---|
| `ScreenQvtStatic.js` | The `qvt` module of a server-static screen: a Vue template with no request data, whose expressions the browser evaluates ([Qvt-Client, Server-static screens](../ui/Qvt-Client.md#server-static-screens)). Its internal mode name is `qvt-static`; conf does not list it |
| `ScreenSupport.js` | The helpers every mode module shares (`S`) |
| `../TemplateSupport.js` | The macro model (`T`), shared with XML Actions |
| `../XmlActions.js` | The macros of XML Actions ([JS-XML-Actions](JS-XML-Actions.md)) |
| `../report/PrintReport.js` | Not a macro module: the browser script of a scheduled report's print page (`crates/moqui-report/src/schedule.rs`) |

Attributes of a `screen-text-output`:

| Attribute | |
|---|---|
| `type` | The mode's name: the `renderMode` parameter, or the extension of the request path (`.qvt`, `.csv`) |
| `mime-type` | The response's content type |
| `macro-template-location` | The mode's module |
| `always-standalone` | A path that ends in the mode's extension renders its last screen alone, without the screens above it |
| `output-kind` | How a failed section is written: `text`, `html`, `vue` or `json`. Without it the mime type decides: `application/json` is json, `text/html` is html, anything else text |

A screen limits its modes with `render-modes`. Each screen must work in `qvt` and `tui`
([Practices-App-Dev](Practices-App-Dev.md#languages-and-render-modes)).

**Which module a render uses.** The last `<macro-template type="..." location="..."/>` of a screen on the render's
path, else the conf's `macro-template-location`. The modules in `framework/template/screen/` are compiled into the
binary (`include_str!` in `generate.rs`): a location that is the built-in path uses the embedded copy, so **a change
to a built-in module needs `./moq build`**. A location that is another path and names a file that exists is read
from that file, which is how a theme or component overrides a mode. `TemplateSupport.js` and `ScreenSupport.js` are
always the embedded ones.

## JavaScript module patterns

The framework's JavaScript files are not ES modules: there is no `import` or `export`. A file is a script, and its
value is the value of its last expression.

| Pattern | Used by |
|---|---|
| The file is one expression, a function called at once: `(function () { ...; return {...}; })()` | `TemplateSupport.js`: its value is the support object |
| The file is one expression, a function of the modules before it: `(function (T) {...})`, `(function (T, S) {...})` | `ScreenSupport.js`, each mode module, `XmlActions.js`. The walker calls it with the support objects, so nothing is left in the global scope |
| Declarations, then a last expression statement: `;({execute: run})`, `;({plain, code, link})` | Service script modules and libraries. The leading `;` keeps the object literal from joining the line before it |
| `ec.resource.script(location)` | Loads a script file and gives its value: `const lib = ec.resource.script("component://my-app/script/Lib.js")`, or in a template `<#assign md = ec.resource.script("...")>` |

A module whose top level only declares is made once for the outermost script run, so module state never reaches the
next request; the rule and the conventions for service modules are in
[Practices-App-Dev, section 4](Practices-App-Dev.md#script-modules) and [JS-Service-Scripts](JS-Service-Scripts.md).
The macro modules are different in one way: they are loaded on a generator runtime that has no `ec` and no host, so a
macro module cannot call `ec.resource.script`. Shared macro code goes in `ScreenSupport.js`.

## The macro model

`TemplateSupport.js` defines the model, for XML Actions and screens alike.

- **One macro per XML element**, called as `macro(node, children)`.
- **`node`** is `{name, attrs, text, children, parent, source}`. `attrs` holds the attributes as written, `children`
  the child nodes, `parent` the element above, and `source` the code the node's own macro returned (set after it
  runs, so a parent reads `kid.source`). A screen element that is stamped also has `src`, its file and line.
- **`children`** is the code of the child elements, already generated and joined. The walk is post-order: children
  first.
- **A macro returns a string of JavaScript.** It prepares values in local constants, then returns one template.
- **`this` is `undefined`** in a macro.

A macro is wrapped by its kind, which checks the result and names the element and its attributes in any error:

| Kind | Result |
|---|---|
| `T.stmt(fn)` | One or more whole statements. A newline is added at the end when it is missing. Every screen macro is a `stmt` |
| `T.expr(fn)` | A fragment used inside a larger line. No trailing newline |
| `T.chain(fn)` | A method call appended to a call chain, such as `.condition(...)`. Empty, or starts with a dot |
| `T.macro(fn)` | A statement or an expression depending on its children. Only the error context is added |

Reading the element and writing code, from `T`:

| Helper | |
|---|---|
| `attr(node, name)` | The attribute as text, `""` when absent |
| `has(node, name)` | Whether the attribute is written |
| `attrOrNull(node, name)` | The attribute as written, or the text `null` |
| `child(node, name)`, `childrenNamed(node, name)` | The first child element of a name, or all of them |
| `jsString(text)` | A JavaScript string literal of the text |
| `` js`...` `` | A template tag for multi-line code: it is dedented, a value alone on its line is indented to that line, and an empty one removes its line. A value must be a string, number or boolean, so an `undefined` is an error, not the text `undefined` |
| `raw(text)` | Marks user code (a condition, a script body) so that `js` does not re-indent its lines |
| `sourcesOf(list)`, `bodyOf(list)` | The code of a list of nodes joined; `bodyOf` ends each as a statement |
| `joinPresent(pieces)` | Joins the pieces that are not empty |
| `noop`, `noops(names)` | A macro that returns `""`, and a table of them by element name |
| `options` | `options.mode` is the mode being generated, `options.debug` whether to put a comment naming the element before each statement |

### A mode module

A mode module is a function of `(T, S)` that returns:

| Member | |
|---|---|
| `macros` | The table of macros by element name. Required |
| `opaque` | Element names whose subtree the element's own macro reads: the walker builds their descendants' nodes but calls no macro for them. Required; most modes use `S.opaqueElements` |
| `prelude` | Code every unit starts with: the mode's run-time helpers, or the lookup of them |
| `epilogue` | Code every unit ends with. With a `try {` in the prelude it is the `finally`, so a unit gives back the render state it took |
| `runtime` | Code run once for each script runtime before the first unit that needs it. `html` and `qvt` install the functions that write each widget here (`globalThis.$mqHtml`, `globalThis.$mqQvt`), so a unit is short calls and a form-list of many rows runs plain functions |

**How a macro is found.** By the element's name in `macros`. Each module starts its table with
`noops(S.allElements)`, the list in `ScreenSupport.js` of every widget and form element, then `S.anyMacros(MODE)`,
then its own. So every known element has an entry, and an element a mode does not render is a decision in that
mode's file. An element with no entry takes the `@element` macro: the walker logs
`No screen macro in <mode>, element for <name>; it renders as an unknown element`, the element's children are not
rendered, and `html` and `qvt` write a placeholder, `xml` a comment, the other modes nothing.

`S.anyMacros(mode)` holds what every mode shares:

| Element | Code |
|---|---|
| `section`, `section-iterate` | `sri.renderSection(name)` |
| `subscreens-active`, `subscreens-panel` | `sri.renderSubscreen()` |
| `include-screen` | `sri.renderIncludeScreen(location, shareScope)` |
| `render-mode`, `text` | The text for this mode ([below](#per-mode-text-and-files)) |
| `render-dynamic-form` | `sri.renderDynamicForm(name)` |
| `form-list-user-columns` | The form with the user's columns, else the form as defined |
| `subscreens-menu` | Nothing |

Helpers a macro uses from `S`:

| Helper | |
|---|---|
| `nodeLit(node, withChildren, withParent)` | The element as an object literal for an `sri` call: name, attributes, text, children when asked, and the chain of parents. The host reads it to find a widget's field, a link's parameters or a drop-down's options |
| `bareLit(node)` | A literal with no parent chain, for an element the host reads alone (`section-include`, `set`) |
| `fieldLit(widget)` | The literal of a widget's field and form, for `sri.getFieldValueString` and the other field methods |
| `write(expression)` | `sri.write(expression);` |
| `expandExpr(text)`, `expandNoL10nExpr(text)` | `ec.resource.expand(...)` and `expandNoL10n(...)` of an attribute's text, evaluated when the unit runs |
| `fieldChain(field, emit)` | The code of a field: the first `conditional-field` whose condition holds, else the `default-field` |
| `hideGuard(field, code)` | The code under the field's `hide` condition |
| `setCode(sub)`, `widgetsSource(sub)`, `widgetless(sub)` | The `set` elements of a field sub-node, its widgets' code, and whether it shows nothing |
| `titleText(sub)`, `localizedTitle(sub)` | A field's title: its `title`, else the field name in words |
| `formListColumns(node)`, `headerSub(field)` | The columns of a form-list and a field's header sub-node |
| `displayValueExpr(widget)`, `linkTextExpr(node)`, `labelTextExpr(node)`, `dateTimeFormat(node)` | The text of a `display`, a `link`, a `label`, and a `date-time`'s pattern |
| `renderTextCode(node)` | The statements of a `text` element |
| `heldCode(expr, what)` | A condition that logs and gives false when it throws, so one bad condition leaves its part out |

Literals are hoisted: each distinct `nodeLit` is made once when the unit is compiled, as `$lits[i]`, and frozen.

**What the generated code may name.** It runs in the screen's context, so a name it declares starts with `$`
(`$index`, `$info`, `$csv`) and cannot hide a screen field. An expression from the screen (a `condition`, a `from`)
is put in as written and reads bare names from the context.

## From screen to output

1. **The tree.** A section's `widgets` (or `fail-widgets`) is the tree; the root section's is wrapped in a `screen`
   element. Before the walk, each `form-single` and `form-list` is replaced by its assembled form (`screen/form.rs`).
   A dynamic form, or one with database extensions, becomes a `render-dynamic-form` call, assembled when it renders.
   A form-list whose columns or saved finds the user may choose is wrapped in `form-list-user-columns`.
2. **The walk.** The tree goes to the generator as JSON. The generator is a script runtime of its own on each thread,
   with no `ec` and no host, a 64 MiB heap and the thread's script deadline; the modules are loaded on it once and
   kept. The walker calls the macros in post-order. A stamped element's code is wrapped in `sri.stamp(src)` calls.
3. **The unit.** The code becomes `function ($depth) { "use strict"; <prelude> <body> <epilogue> }`, inside a
   function that makes `$lits` with `sri.holdNodeLits([...])` when it has literals. The source is parsed once as a
   check.
4. **Kept.** The unit is kept with the screen definition by mode, module location, section, and widgets or
   fail-widgets, until the screen file (or a file it depends on, such as an extended form) changes. Units of dynamic
   forms are kept by the text of the assembled form, up to 128 for a definition.
5. **Run.** A render asks for the unit of each section it reaches and runs it on the thread's script runtime
   (`script/oxide/units.rs`). The mode's `runtime` runs once per runtime, the unit's function is compiled once per
   unit, and it is called with `$depth`, how many units it runs inside. A `sri.render*` call inside a unit runs the
   next unit in the same run. A unit that throws leaves the context stack as it found it.
6. **Written.** `sri.write` goes to the response, which streams or is kept whole by mode
   ([Screen-Output](../ui/Screen-Output.md)).

While a server-static screen is checked, a widget expression runs with `ec` replaced by a wrapper that records what
the browser's `ec` does not have (`script/oxide/widget.rs`).

For reading what a screen runs:

| Variable | |
|---|---|
| `MOQUI_ACTIONS_DEBUG` | Set (any value): a comment naming the element comes before each statement of generated code |
| `MOQUI_UNIT_DUMP` | A directory: each unit's source and its mode runtime are written there, named for the screen and the unit |

## The `sri` global

`sri` is the screen render in progress. It is there only while a screen renders: in a unit, in a trusted template
the screen renders, and in a script a widget calls. Outside a render a method throws `no screen render is running`.
An element argument is a `nodeLit`. A method that renders writes to the output and returns empty text unless the
table says it returns the text.

Writing and the render:

| Method | |
|---|---|
| `write(text)` | Writes to the output: a string as it is, anything else as its plain text |
| `encodeHtml(value)` | The value HTML-encoded; `null` and `undefined` are empty text |
| `getRenderMode()` | The mode's name, `html-print` for `xsl-fo` |
| `doBoundaryComments()` | Whether the screen facade's `boundary-comments` is on |
| `once(key)` | True the first time this render sees the key, for output a page needs once |
| `stamp(src)` | Sets the source stamp of the next element written, in a render that writes stamps; `null` clears it |
| `sendRedirectAndStopRender(url)` | Sends a redirect and stops the render |
| `baseLinkUrl(url)`, `getBaseLinkUrl()` | Sets (`null` clears) and reads the base URL of relative links in rendered content, such as markdown |
| `setFocusField(id)` | The field to focus, in the response envelope: the first id asked for wins |
| `appendToAfterScreenWriter(text)`, `getAfterScreenWriterText()` | Text held for the end of the page, and its read |
| `appendToScriptWriter(text)`, `getScriptWriterText()` | Script text held for the end of the page, and its read |
| `uniqueId(id)` | A `tui` node id that no earlier node of the document has |
| `htmlText(html)` | The JSON text of the blocks and numbered links of an `html` node, for `tui` |

Screens, sections and text:

| Method | |
|---|---|
| `renderSubscreen()` | Renders the next screen of the path |
| `renderSection(name)` | Runs a section: its condition and actions, then its widgets or fail-widgets |
| `renderSectionValue(name)` | A section as its JSON array, returned, for a `tui` list cell |
| `renderSectionInclude(element)` | Renders the section a `section-include` names |
| `getSectionIncludedNode(element)` | The `section` element a `section-include` stands for: its `name`, else the name after `#` in its `location` |
| `renderIncludeScreen(location, shareScope)` | Renders another screen in place; `"true"` shares the context |
| `renderText(location, template)` | Renders a text file with the renderer of its extension (else of `template` as an extension) and writes it; `template` `"false"` returns the file's text |
| `renderTextValue(location, template)` | As `renderText`, but returns the text |
| `renderInlineText(body)` | Renders template text as a trusted fancy template and returns it |
| `writeInlineText(body)` | The same, written as it renders, so a screen it renders keeps its place |
| `takeTemplateWrite()` | Asked by a trusted template as it starts: whether it writes its output as it goes. Not for macros |
| `renderDynamicForm(name)` | Assembles a `dynamic="true"` form from the context, generates its code (once for each different result) and runs it |
| `renderUserColumnsForm(name)` | Renders a form-list with the columns the user chose. Returns false, rendering nothing, when the user has none |
| `getActiveScreenDef()` | The screen being rendered: `location`, `screenName`, `getLocation()`, `getScreenName()`, `getDefaultMenuName()` |
| `getScreenUrlInfo()` | The request's target: `targetScreen` and `parentScreen` (as `getActiveScreenDef` gives), the path name lists (`fullPathNameList`, `minimalPathNameList`, `preTransitionPathNameList`, `extraPathNameList`) and `density` |
| `holdNodeLits(literals)` | Converts and freezes a unit's element literals once. Generated code calls it; a macro does not |

The context:

| Method | |
|---|---|
| `pushContext()`, `popContext()` | A frame of the context stack |
| `setInContext(element)` | Runs a `<set>` under a field |
| `pushSingleFormMapContext(map)` | A frame holding the fields of a form-single's map (the map, or its expression) and `_formMap` |
| `getSingleFormMap(map)` | That map |

Fields:

| Method | |
|---|---|
| `getFieldValue(field, defaultValue)` | The field's value |
| `getFieldValueString(widgetOrField, defaultValue, format, type)` | The value as text, formatted |
| `getFieldValuePlainString(field, defaultValue)` | The value as plain text, not formatted for display |
| `getFieldValueClass(field)` | The Java class name of the field's value: `String`, `BigDecimal`, `Timestamp`, ... |
| `getFieldValueTexts(widget, withString)` | `[value string, plain string, value class]` of a display widget's field in one call |
| `getNamedValuePlain(fieldName, form)` | A value by name as plain text: from a form-single's map, else from the context |
| `getFieldEntityValue(widget)` | The text of the entity value a `display-entity` field's value points at |
| `getFieldOptions(widget)` | A `Map` of key to text, in order: the options of a drop-down, check or radio |
| `getFieldTransitionValue(transition, element, term, labelField, alwaysGet)` | The label a transition gives for a value |
| `getFieldValidation(sub)` | `{classes, regexp?, message?}` for a field sub-node |
| `getFieldValidationJsRules(sub)` | `[{expr, message}]`: the checks a browser runs |
| `getFieldValidationRules(sub)` | The same checks as data, `[{kind, ..., message}]`, for a client that runs no JavaScript |

Forms and lists:

| Method | |
|---|---|
| `getFormNode(name)` | The assembled form as an element |
| `getFormInstance(name)` | `{formNode, name, list}` |
| `getFormHiddenParameters(form)` | The form's hidden parameters as a map |
| `openFormList(name, streamed)` | The list's handle `{id, count}`. With `streamed` true the rows are read as they are written and `count` is null |
| `startFormListRow(info, index)` | Pushes a frame holding the row and `_formMap`. False, with no frame, past the last row, which ends a streamed loop |
| `endFormListRow()` | Pops the row's context |
| `formListSubRowCount(info, index)` | The sub-rows of a row of an aggregated list |
| `startFormListSubRow(info, index, subIndex)`, `endFormListSubRow()` | A sub-row's context |
| `safeCloseList(info)` | Closes the list and its cursor |
| `formListColumnCharWidths(columns, lineWidth)` | The width of each column in characters, from the `print-width` and `print-width-type` of each field of each column, for the text mode |
| `getSavedFindFullLocation(formName)` | The location a saved find is kept under: the screen and form, plus `#entity` or `#service` for a dynamic form |
| `getUserFormListFinds(formName)` | The saved finds of the user and the user's groups for the form |
| `getUserDefaultFormListFindId()` | The user's default saved find of the screen, or null |
| `getFormListFindScreenScheduled(formListFindId)` | The user's schedule of a saved find, or null |

URLs (`host/urls.rs`):

| Method | |
|---|---|
| `buildUrl(path)` | The URL of a path from the screen being rendered |
| `buildUrlFromTarget(path)` | The URL of a path from the request's target screen |
| `makeUrlByType(url, urlType, element, expandTransitionUrl)` | A URL by its type: `transition` or `screen` is a screen path, any other type is the expanded text as a plain URL, and `content` is an error (not supported). The parameters of `element` (a link's `parameter-map` and `parameter` children) are added. With the string `"true"` last, a transition that only names another screen becomes that screen's URL |
| `getScreenUrlInstance()`, `getCurrentScreenUrl()` | The URL of the request |
| `isAnchorLink(element, url)` | Whether a link is a plain anchor (a GET), not a form post. A transition with `read-only="true"` is an anchor even when it has actions |
| `activeInCurrentMenu` | A property, not a method: whether a subscreen of the screen being rendered that is in its menu is on the request's path |

A URL object has `path`, `url`, `pathWithParams` and `readOnly`, and the methods of Java's `UrlInstance`: `getPath()`,
`getUrl()`, `getPathWithParams()`, `getUrlWithParams(extension)`, `getScreenOnlyPath()`, `getScreenPathUrl()`,
`getMinimalPathUrl()`, `isReadOnly()`, `isScreenUrl()`, `isTransition()`, `getTransitionMethod()`,
`getDisableLink()`, `cloneUrlInstance()`, `getParameterMap()`, `getPassThroughParameterMap()`, and
`addParameter(name, value)`, `addParameters(map)` and `removeParameter(name)`, which change the URL and return it.

Theme, menus and the shell:

| Method | |
|---|---|
| `getCurrentThemeId()` | The theme of the render: the user's, else one of the user's groups', else the default of the screen theme type |
| `getThemeValues(resourceTypeEnumId, themeId)` | The theme's `ScreenThemeResource` values of a type, in sequence (`STRT_STYLESHEET`, `STRT_SCRIPT`, ...) |
| `getThemeIconClass(text)` | The icon class of the first `ScreenThemeIcon` pattern the text matches, or null |
| `getMenuItems(path)` | The subscreen menu items of a screen: `name`, `title`, `url`, `disabled`, `inPath`, `image`, `imageType`, `hasParameters`, `location`, `menuIndex`, `component` |
| `getMenuData(pathNames)` | The data of the `menuData` transition, or null after an error was sent |
| `getExtItems(pathNames)` | The declared actions and panels of the screen at the path, or null after an error was sent |
| `getShellData()` | The data of the `shellData` transition |
| `getShellProviders(kind)` | The `shell-providers` of the apps the user may open, only `kind` when given |
| `getShellSearch(text, groups, limit)` | The palette groups of the `shellSearch` transition |

Server-static screens and xlsx:

| Method | |
|---|---|
| `staticUrl(text, urlType)` | A static link's target (`path`, `exists`, `isTransition`, `isScreen`, `readOnly`, `method`) and its parameter definitions |
| `staticDisableKey(element)` | The key in the request package of a static control's availability |
| `staticCheck(json)` | Records the expressions a static macro wrote, `[{expr, element, attr}]`, for a compatibility report |
| `staticNote(kind, element, detail)` | Records server-only work a static macro could not write into the template |
| `xlsxSheet(name)` | Starts a sheet |
| `xlsxHeader(title)` | Adds a header cell |
| `xlsxCell(values, formats, currency, alignRight)` | One cell from the values of a field's widgets |
| `xlsxCellCount()`, `xlsxEndRow()` | The cells of the row so far, and the end of the row |

The typed declaration is `MoquiScreenRenderer` in `crates/moqui-devkit/src/typegen/moqui-host.d.ts`, which lists the
methods screens and templates call, not every method here.

## Per-mode text and files

A screen gives a mode its own output with a `text` element, usually inside `render-mode`:

```xml
<render-mode>
    <text type="qvt"><![CDATA[<m-chart :data="..."/>]]></text>
    <text type="tui"><![CDATA[{"t":"text","style":"h3","text":${JSON.stringify("Total " + total)}}]]></text>
    <text type="html" location="component://webroot/screen/includes/Messages.html.fancy"/>
</render-mode>
```

- **`type`** is a mode or a comma list of modes. In a `render-mode`, the last text whose type names the mode wins,
  else the last with no type or `any`. An `html` render of a screen whose widgets are only `render-mode` elements
  falls back to its `html-print` HTML text.
- **`location`** names a file, rendered by its extension's renderer; inline text is a trusted template
  ([JS-Fancy-Templates](JS-Fancy-Templates.md#where-templates-are-used)). `template="false"` writes either as it is.
- **`no-boundary-comment="true"`** leaves out the BEGIN and END comments that `boundary-comments` adds.

| File | |
|---|---|
| `<Name>.qvue` | A Vue single-file component for the browser: `<text type="qvue" location="....qvue" template="false"/>`, in a screen whose `render-modes` names `qvue`. The `qvue` and `qjs` modes write only such text ([Qvt-Client, Loading a screen](../ui/Qvt-Client.md#loading-a-screen), [Practices-App-Dev, section 6](Practices-App-Dev.md#6-vue-single-file-screens-qvue)) |
| `<Name>.tui.json` | The terminal's nodes for the same screen: `<text type="tui" location="....tui.json" template="false"/>`, or `template="fancy"` for a file with `${}`. The text is checked as JSON before it is written ([TUI, For screen authors](../ui/TUI.md#for-screen-authors)) |
| `*.html.fancy`, `*.qvt.fancy` | A template for one mode, named by a `text` of that type ([JS-Fancy-Templates](JS-Fancy-Templates.md)) |

How a `text` streams, and the order of a template's text and the screens it renders, is in
[Screen-Output, Templates](../ui/Screen-Output.md#templates).

## Writing a macro

The csv module (`ScreenCsv.js`) is small enough to read whole. Its prelude is the run-time code each unit starts
with, and a helper turns an expression into a statement that writes it:

```js
const prelude = `let $hasPrev = false;
const $csv = (value) => { ... };
const $sep = () => { if ($hasPrev) sri.write(","); else $hasPrev = true; };
`;

const csv = (expression) => `sri.write($csv(${expression}));\n`;
```

A widget macro returns the statement for its element. The text is an expression the unit evaluates for each row:

```js
const textWidget = stmt((node) => csv(`sri.getFieldValueString(${nodeLit(node, false)})`));
```

For `<text-line/>` in a field this generates `sri.write($csv(sri.getFieldValueString($lits[3])));`. A container macro
passes its children's code through, and one with markup of its own puts it around them with the `js` tag, as the
html module's `container`:

```js
const containerMacro = stmt((node, children) => {
    const type = attr(node, "type") || "div";
    const id = attr(node, "id");
    const style = attr(node, "style");
    return js`
        sri.write("<" + ${lit(type)} + $h.A({id: ${id ? "$h.nodeId(" + bareLit(node) + ")" : "null"}, class: ${style ? noL10n(style) : "null"}}) + ">");
        ${children}
        sri.write(${lit(`</${type}>\n`)});
    `;
});
```

The table names each element:

```js
const macros = Object.assign({}, noops(S.allElements), S.anyMacros(MODE), {
    "container": stmt((node, children) => children),
    "form-list": formListMacro,
    "text-line": textWidget,
    "display": stmt((node) => csv(S.displayValueExpr(node))),
});
return { macros, opaque: S.opaqueElements, prelude };
```

Rules:

- **Decide at generation, evaluate at run time.** What depends only on the XML (a tag name, whether a title is set,
  the columns) is decided in the macro. What depends on the request is code: `S.expandExpr(attr(node, "title"))`,
  never the attribute's text written into the output.
- **Encode what is written.** A value from the request goes through the mode's encoder in the generated code
  (`$h.e`, `$csv`, `JSON.stringify`).
- **Declare with `$`**, and wrap a statement's declarations in a block (`{ ... }`) so that two widgets do not
  collide.
- **A condition is user code**: put it in with `raw(condition)` inside `js`, and guard the body with it.
- **A new element** is added to `widgetElements` or `formElements` in `ScreenSupport.js`, so every mode has a no-op
  for it, then given a macro in each mode that shows it: at least `qvt` and `tui`. An element whose macro reads its
  own subtree goes in `opaqueElements`.
- **Heavy markup belongs in the mode's runtime** (`html`, `qvt`): the macro generates a call
  (`$h.label($lits[2])`) and the function that writes the element is in `runtimeFunction`.
- **Rebuild** after changing a built-in module.

## Adding a render mode

1. **Write the module**: a file that is `(function (T, S) { ... return { macros, opaque, prelude }; })`. Start the
   table with `noops(S.allElements)` and `S.anyMacros("<mode>")`, as `ScreenCsv.js` and `ScreenXml.js` do, and add
   the elements the mode shows.
2. **Register it** in the conf's `screen-facade`:
   `<screen-text-output type="<mode>" mime-type="..." macro-template-location="component://my-comp/template/ScreenMyMode.js"/>`,
   with `always-standalone` and `output-kind` as the mode needs. A location outside `framework/template/screen/` is
   read from its file, so a component's mode needs no rebuild. A framework mode is added to `MODE_MODULES` in
   `generate.rs`, and to the mode tables of its test helpers.
3. **Let screens use it**: a screen whose `render-modes` is not `all` must name the mode.
4. **A binary mode** (as `xlsx`) is a `screen-output` and needs Rust for what the unit fills
   (`crates/moqui-core/src/screen/xlsx.rs`).
5. **A mode for one screen or app** needs no conf: a `<macro-template type="html" location="..."/>` in a screen
   replaces the module of an existing mode for that screen and the screens under it.

## Testing

| Kind | |
|---|---|
| A module's run-time functions | A `database: false` spec reads the module's text and evaluates it with `runInContext(source, sandbox)`, as `runtime/base-component/framework/test/ScreenHtml.spec.js` and `ScreenQvt.spec.js` do ([Testing](../Build-Test.md#component-specs)) |
| Generated code | The Rust tests in `crates/moqui-core/src/screen/tests/` (`golden.rs`, `modules.rs`) and in `generate.rs` generate a unit from XML with `generate_xml(mode, xml, debug)` and check its text |
| A screen's output | A spec renders the screen with `screenTest(...)` in the mode and checks the text, once per mode ([Rendering screens in tests](../Build-Test.md#rendering-screens-in-tests)) |

The spec pattern loads a module without the walker. The module is a function, so the spec calls it with stand-ins
for `T` and `S`, runs its `runtime` text in a sandbox that holds a fake `sri` and `ec`, and calls a widget function:

```js
spec({ database: false });

function readModule(file) {
    return runInContext(ec.resource.getLocationText("component://framework/template/screen/" + file), {});
}

// the ScreenSupport helpers the runtime takes as source; the macro tables are not compiled here
function support() {
    return { anyMacros: () => ({}), allElements: [], opaqueElements: [], parseOrder: readModule("ScreenSupport.js")({}).parseOrder };
}

feature("a dialog button shows its label", { expect() {
    const module = readModule("ScreenHtml.js")({ stmt: fn => fn, noops: () => ({}) }, support());
    let html = "";
    const context = {
        sri: { write: text => { html += text; } },
        ec: { resource: { expand: text => text }, context: { get: () => null } }
    };
    runInContext(module.runtime, context);
    context.$mqHtml.containerDialog({ attrs: { id: "NewDialog", "button-text": "New" } });
    assert.match(html, /New/);
} });
```

Such a spec reads the module from the component's files, so it runs a changed module without a build; a screen
render uses the copy in the binary. Run the spec with `./moq test framework --spec ScreenHtml`, and
`./moq test-framework` before a commit ([Build-Test](../Build-Test.md#the-two-suites)).
