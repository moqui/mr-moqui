# Fancy templates

Fancy is MR Moqui's text template dialect, in the place FreeMarker has in Moqui Java: text with `${expr}` and four
tags (`if`, `list`, `assign` and comments), where every expression is JavaScript. A template is compiled to one
JavaScript function and kept. A **trusted** template runs on the script runtime with the live context and `ec`; a
**locked** template runs in a sandbox that holds only the data it was given. The code is
`crates/moqui-core/src/script/template.rs` (the compiler, the renderers and the registry) and
`crates/moqui-core/src/script/oxide/templates.rs` (how each form runs); the tests are `script/template/tests.rs` and
the golden files in `script/testdata/template/`.

The `ec` members a trusted template calls are in [JS-Core-API](JS-Core-API.md), and the other globals in
[JS-Globals](JS-Globals.md). How a screen's widgets become output is [JS-Templates-Macros](JS-Templates-Macros.md).

## Where templates are used

| Place | |
|---|---|
| A file with a renderer's extension | `ec.resource.template(location, data)` renders it and returns the text. `data` is optional. A string second argument is a default extension, for a location whose own extension has no renderer |
| A screen `<text location="...">` | Rendered by the renderer of the file's extension, else of the `template` attribute's value as an extension (`template="fancy"` for a `.tui.json`), else written as it is. `template="false"` writes the file as it is. A `.ftl` location is an error |
| A screen `<text template="true">` body | Inline text as a trusted template with no escaping. `template="true"` is the default for inline text |
| A file under a screen with `include-child-content` | A file that has a renderer is rendered inside the screen; a `.fancy` or `.fancy-locked` file is never sent as a static file |
| Email and print documents | An `.html.fancy` file a service renders with `ec.resource.template(location, data)` (`mr-mantle/template/account/InvoiceEmail.html.fancy`, `Invoice.print.html.fancy`) |
| Autodoc | The `.fancy` templates in `framework/template/autodoc/` ([Autodoc](Autodoc.md#templates)) |
| Themes | `.html.fancy-locked` sections and blocks, composed by `ec.resource.theme` ([Themes](../ui/Themes.md)) |
| A check without a render | `moqui lint --screens` compiles the inline text of each screen `text` and reports one that does not compile, and each `.ftl` location |

## Syntax

```
<#-- a comment: no output -->
<#assign total = price * quantity>
<#if status == "open">
Open: ${count}
<#elseif status == "closed">
Closed
<#else>
Other
</#if>
<#list rows as row>
${row_index}: ${row.name}<#if row_has_next>,</#if>
</#list>
Escaped: \${literal} and \<#if not a tag>
```

| Form | |
|---|---|
| `${expr}` | The value of a JavaScript expression. `null` and `undefined` write nothing; any other value is its `String(...)`. Whether it is HTML-escaped depends on the renderer ([Escaping](#escaping)) |
| `\${` and `\<#` | The literal text `${` and `<#` |
| `<#if expr>` ... `<#elseif expr>` ... `<#else>` ... `</#if>` | JavaScript truthiness. `<#elseif>` after `<#else>`, or a second `<#else>`, is an error |
| `<#list expr as name>` ... `</#list>` | Repeats its body with `name` set to each item. `name_index` is the item's index from 0 and `name_has_next` is true before the last. The split is at the last ` as ` |
| `<#assign name = expr>` | Sets a variable of the template for the text after it. Before the assign the name reads the context's value: `${n}<#assign n = n + 1>${n}` with `n` 5 writes `56` |
| `<#-- ... -->` | A comment, which may span lines |

What `<#list>` iterates:

| Value | Items |
|---|---|
| `null`, `undefined` | None |
| An array | Its elements |
| A string | The string, once |
| Any other iterable (a `Set`, a `Map`, a generator) | What it yields |
| An object with a numeric `length` | Its indexed members |
| Any other object | Its values (`Object.values`) |

Rules of the text:

- **A line of only tags and white space writes nothing**, so tags on their own lines leave no blank lines.
- **A variable name** is ASCII letters, digits and `_`, not starting with a digit or `$`.
- **Tags must nest and close.** An open `<#if>` or `<#list>` at the end is an error.
- **Any other tag is an error**, not text: `<#visit>`, `<#macro>`, `<#include>` and the rest fail with
  `unknown tag <#name>; the dialect has if, elseif, else, list and assign`. A FreeMarker macro call (`<@name>`,
  `</@name>`) fails with `FreeMarker macro calls are not part of the fancy dialect`. Write `\<#` for the literal text.

### Expressions

Every expression is JavaScript, parsed when the template compiles. The template reads it as JavaScript to find where
it ends, so a `}` in a string, a template literal, a comment or a regular expression does not end a `${}`
([Screen-Output, Templates](../ui/Screen-Output.md#templates) has the full rules). What an author must know:

- **A tag ends at the first `>` outside brackets**, so a comparison goes in parentheses: `<#if (a > b)>`. An arrow
  (`=>`) does not end a tag.
- **No `//` comment in a tag**; use `/* */`. A `//` comment in a `${}` runs to the end of its line.
- **An object literal in `${}`** works as written: `${ {a: 1}.a }`.
- **A nested template literal** works: `` ${`Total ${count}`} ``.

## Escaping

The renderer decides whether `${}` output is HTML-escaped. The escape replaces `&`, `<`, `>`, `"` and `'`, so it
serves XML and attribute values too.

| Renderer | `${}` output |
|---|---|
| `.html.fancy`, `.html.fancy-locked`, `.md.fancy`, `.markdown.fancy` | HTML-escaped |
| `.fancy`, `.fancy-locked` | As it is |
| Inline `<text template="true">` | As it is, in every render mode |

Every template, trusted or locked, has these helpers:

| Helper | |
|---|---|
| `raw(value)` | The value as it is, in a renderer that escapes: `${raw(sri.renderInlineText(item))}`. Use it only for markup the server made |
| `html(value)` | The value HTML-escaped once, in any renderer; an escaping renderer does not escape it again |
| `cssString(value)` | The value as a quoted CSS string that cannot end a `<style>` element, for a `content:` value such as an `@page` margin box. `cssString(null)` is `""` |

An inline `<text>` in an `html` or `qvt` screen is not escaped, so a value from the request goes through `html(...)`
there, and in a `tui` text through `JSON.stringify(...)` ([TUI, For screen authors](../ui/TUI.md#for-screen-authors)).
A screen `text` with a `location` and `encode="true"` HTML-encodes the whole rendered file.

## Trusted templates

A trusted template (`fancy`, `fancy-html`, `markdown-fancy`, inline text) runs on the thread's script runtime, as a
script does.

- **Bare names are the context stack.** `${orderId}` reads the context; a name that is not set is `null` and writes
  nothing. An assignment in an expression (`${count = 5}`) writes the context, as in a script; an `<#assign>` is the
  template's own variable.
- **`ec` and the script globals are there**: `ec.l10n.localize(...)`, `ec.resource.expand(...)`, `ec.user`,
  `ec.web`, `System.getProperty(...)` ([JS-Core-API](JS-Core-API.md), [JS-Globals](JS-Globals.md)).
- **`sri` is there during a screen render** ([the `sri` table](JS-Templates-Macros.md#the-sri-global)):
  `sri.buildUrl(path).url`, `sri.getThemeValues(type)`, `sri.renderSubscreen()`.
- **`data` is a frame of its own.** The entries of the map passed to `ec.resource.template(location, data)` are put
  in a context frame pushed for the render and popped after it, over what the caller's context holds.
- **A screen `text` writes as it renders.** The template's text and a screen it renders keep their order:
  `<div>${sri.renderSubscreen()}</div>` writes the `div`, the subscreen, then `</div>`
  ([Screen-Output, Templates](../ui/Screen-Output.md#templates)). A template rendered for its value
  (`ec.resource.template`, `sri.renderTextValue`, `sri.renderInlineText`) returns its text.

## Locked templates

A locked template (`fancy-locked`, `fancy-locked-html`) is for text its author is not trusted to run as code against
the server: a theme a merchant uploads. It runs in a sandbox runtime of its own, apart from the one scripts run on.

| | |
|---|---|
| Names | Only the keys of the map passed in. A name the map does not have is `null`. There is no `ec`, no `sri`, no context stack and no host object |
| Functions | `format(value, pattern)`, `localize(text)` and `t(key)`, with `raw`, `html` and `cssString`. These names hide a data key of the same name |
| Built-ins | The JavaScript built-ins, all frozen: a template cannot change what the next one runs. The template function is strict and frozen, and keeps nothing from one render to the next. Code made at run time (`Function(...)`) sees no name of the data |
| Deadline | 5 seconds; a longer render fails with `interrupted` |
| Heap | 32 MiB; past it the render fails with `out of memory` |

The data is a plain snapshot of the map, made before the render:

| Value | In the template |
|---|---|
| A string, boolean, integer or float | Itself |
| A decimal | A string (`"10.50"`), so compare and add through `format` or `Number(...)` with care |
| A date | `"yyyy-MM-dd"` text |
| A time | `"HH:mm:ss"` text |
| A list, a map | An array, an object |
| An entity value | An object of its fields |
| An entity list | An array of those |
| Any other host object (`ec`, a find, a function) | `null`, with a warning in the log |

- **`format(value, pattern)`** formats as `ec.l10n.format` does. Decimals and dates arrive as text, so the pattern
  says which it is: a date pattern reads a date or time stamp, a number pattern a number. With no pattern, text of
  epoch milliseconds is a time stamp. A pattern that is passed must be a string.
- **`localize(text)`** is the framework's localization.
- **`t(key)`** is a theme's own text for the view's locale; a key with no text shows as the key. Outside a theme
  render it always gives the key.

A theme may lower the deadline and the heap for a page, never raise them. The theme adapter, its snippets
(`<#include "name">`, which the adapter puts in before the render; the dialect itself has no such tag) and its
budgets are in [Themes](../ui/Themes.md).

## Renderers and runners

`MoquiDefaultConf.xml` maps a file extension to a renderer in `resource-facade`. The longest extension a file name
ends with wins, so `Page.html.fancy` is `html.fancy`, not `fancy`. A `#version` suffix on a location is not part of
the name.

| Extension | `class` | |
|---|---|---|
| `fancy` | `fancy` | Trusted, no escaping |
| `html.fancy` | `fancy-html` | Trusted, HTML-escaped |
| `fancy-locked` | `fancy-locked` | Locked, no escaping |
| `html.fancy-locked` | `fancy-locked-html` | Locked, HTML-escaped |
| `md`, `markdown` | `markdown` | CommonMark with tables to HTML, raw HTML passed through. Not a template: it has no context. Relative links follow the rendering screen's `sri.baseLinkUrl` |
| `md.fancy`, `markdown.fancy` | `markdown-fancy` | The markdown is converted to HTML, then the result is a trusted template with HTML-escaped `${}` |
| `html` | `org.moqui.impl.context.renderer.NoTemplateRenderer` | Written as it is. The entry makes `html` a default extension |

A factory with no conf has all of these but `html`. A `class` that is none of these names is looked up among the
template renderers a native component registers ([Native-Components](Native-Components.md)).
`ec.resource.getTemplateRendererExtensionSet()` lists the registered extensions in conf order.

In a `.md.fancy` file the tags, comments and `${}` are set aside before the markdown pass, so markdown does not
escape or reflow them. A tag alone on a line stays out of a paragraph, and a tag line inside a table does not become
a row, so a `<#list>` around a table row repeats the row. A link destination that starts with a `${}` is left as
written. Name a template `.md.fancy` only when HTML is wanted: a template that writes Markdown text is a `.fancy`
file, as the Autodoc templates are.

Scripts have runners, not renderers:

| Extension | |
|---|---|
| `.xml` | An XML Actions file, run by `XmlActionsScriptRunner` ([JS-XML-Actions](JS-XML-Actions.md)) |
| `.js` | A JavaScript file (`engine="javascript"`), loaded with `ec.resource.script(location)` ([JS-Service-Scripts](JS-Service-Scripts.md)) |

## Including a template and calling a library

The dialect has no include tag (only a theme package's snippets, which its adapter puts in). A template includes
another by rendering it for its value:

```
<div class="panel">${raw(ec.resource.template("component://my-app/template/Panel.html.fancy", {title: title, rows: rows}))}</div>
```

- **`raw(...)` is needed in an escaping renderer**, or the included markup is escaped.
- **The second argument is the included template's own names**, in a frame over the caller's context, so the
  included template still reads the context below it.
- **In a screen**, `sri.renderTextValue(location)` gives a text file's render and `sri.renderInlineText(body)` renders
  template text held in a value (the `navbar.html.fancy` theme items); `sri.renderSection(name)`,
  `sri.renderSubscreen()` and `sri.renderIncludeScreen(location)` write screen parts where they are called.

A template has no macros. Shared logic is a JavaScript module the template loads once and calls, as every Autodoc
template does with `framework/template/autodoc/AutodocLib.js`:

```
<#assign md = ec.resource.script("component://framework/template/autodoc/AutodocLib.js")>
# ${md.plain(title)}

- **Defined in:** ${md.defined(component, location, line)}
<#if entity.pkFields>
- **Primary key:** ${md.code(entity.pkFields)}
</#if>
```

The module is a script file whose last expression is its value, here
`;({text, plain, code, link, codeLink, cell, table, fence, mermaid, yes, join, nav, defined, usage})`
([script modules](JS-Templates-Macros.md#javascript-module-patterns)). Its functions return text; one that returns
markup is called through `raw(...)` in an escaping renderer. A small function local to one template is an
`<#assign>` of an arrow: `<#assign money = (v) => ec.l10n.format(v, "#,##0.00")>`.

## Errors

A template is compiled, and every expression parsed, before any of it runs, so a syntax error writes no partial
output.

| Error | Message |
|---|---|
| A tag or `${` that is not closed, an unknown tag, a macro call, a tag with the wrong form | `template line N: ...`, with the line of the tag: `template line 2: unknown tag <#visit>; the dialect has if, elseif, else, list and assign` |
| Tags that do not nest (`</#if>` with no `<#if>`, an `<#if>` not closed) | `template: ...`, with no line |
| An expression that is not JavaScript | `template: ...` with the parser's message |
| The compiled function does not compile | `template does not compile: ...` |
| An expression throws | The JavaScript error |
| A locked render past its budget | `interrupted`, or `out of memory` |

The message does not name the file: `ec.resource.template` and a screen `text` pass the renderer's error on as it
is. The theme adapter reports each finding with its package file and line
([Themes](../ui/Themes.md)). `moqui lint --screens` reports inline template text that does not compile, with its
screen file and line.

## Caching

- **The compiled function** is kept by the template's source text, for each of the four combinations of escaping and
  trusted or locked, up to 4096 sources each. The same text compiles once however it is reached: a file, inline
  text, or a string from the database.
- **A file's text** is kept by location and last-modified time, up to 4096 files, so a changed file is read again on
  its next render. A resource with no modified time (a database resource, a version named with `#version`, storage
  that keeps versions) is kept by a hash of its text.
- **A markdown file's HTML** is kept the same way, so the markdown is converted once. While a screen with a
  `baseLinkUrl` renders, a markdown file is converted from its source so that its relative links follow the base.
- **Each thread's runtime** keeps the functions it has compiled. The locked sandbox keeps up to 4096, and compiles
  text that changes on every render as transient code so that it cannot grow the sandbox.
- **Nothing a render sets is kept.** A trusted render's frame is popped at its end; a locked render's function and
  globals are frozen.

## Porting from FreeMarker

A `.ftl` file is not rendered: a screen `text` that names one fails with
`FreeMarker template ... is not supported; port it to the fancy dialect`. The tags that remain look the same; the
expressions change from FreeMarker's language to JavaScript.

| FreeMarker | Fancy |
|---|---|
| `<#if>`, `<#elseif>`, `<#else>`, `<#list x as y>`, `<#assign n = v>`, `<#-- -->` | The same tags |
| `y_index`, `y_has_next` | The same names. `y?index`, `y?has_next`, `<#sep>` and `<#items>` are not there |
| `<#list>` with `<#else>` | `<#if list.length>` around the list |
| `<#assign x>...</#assign>` (captured text) | Not there: `<#assign>` needs `name = expression` |
| `<#local>`, `<#global>` | `<#assign>` |
| `<#macro>`, `<#function>`, `<@name/>` | A function in a JavaScript module loaded with `ec.resource.script(location)`, or a second template rendered with `ec.resource.template(location, data)` |
| `<#include "file">` | `${raw(ec.resource.template(location))}` |
| `<#import "lib.ftl" as lib>` | `<#assign lib = ec.resource.script("component://.../Lib.js")>` |
| `<#switch>`, `<#break>`, `<#return>`, `<#visit>`, `<#t>`, `<#compress>` and every other directive | Not there: an unknown tag is an error. Use `<#if>` and `<#elseif>` |
| `${x?html}` | Automatic in `.html.fancy`; `${html(x)}` elsewhere |
| `${x?no_esc}`, `<#noescape>` | `${raw(x)}` |
| `${x!}`, `${x!"default"}`, `x??`, `x?has_content` | A missing name is `null` and writes nothing; `${x \|\| "default"}`, `${x ?? "default"}`, `<#if x>`, `<#if x != null>` |
| `x?size`, `x?length` | `x.length` |
| `map?keys`, `map?values` | `Object.keys(map)`, `Object.values(map)` |
| `x?string("0.00")`, `x?date` and the other built-ins | `ec.l10n.format(x, "0.00")` and the methods of the JavaScript value |
| `a gt b`, `a lt b` | `(a > b)`, `(a < b)` in parentheses inside a tag |
| `ec.l10n.localize("Text")`, `ec.resource.expand(...)`, `sri.buildUrl(...)` | The same calls, in JavaScript |

Also:

- **Values are JavaScript values.** A decimal from an entity is not a JavaScript number
  ([Practices-App-Dev](Practices-App-Dev.md#languages-and-render-modes)); a list from the context may need
  `Array.from(list || [])` before array methods.
- **An inline `<text>` is not escaped** in any mode, where the `.html.fancy` file a `.html.ftl` becomes is.
- **`.md.ftl` becomes `.md.fancy`**, with `${}` output HTML-escaped, which FreeMarker did not do.
- **The screen macros are not templates.** `DefaultScreenMacros.*.ftl` became JavaScript macro modules
  ([JS-Templates-Macros](JS-Templates-Macros.md)); a theme overrides a mode with a module, not a macro file.
