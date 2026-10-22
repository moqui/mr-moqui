---
name: openui-lang
title: OpenUI Lang canvases
description: Use when no screen shows what the user asked for and a generated view fits: a chart, a dashboard, a batch confirm, several lists.
kind: guide
unlocks: [write_ui]
aliases: [canvas, chart, dashboard, graph, generated view, openui]
surfaces: [drawer]
locked: true
version: 1
sections:
  syntax: Statements, arguments, $variables, operators
  components: Layout, content, tables and charts, with their signatures
  forms: Forms, controls, filters and the filter wiring rule
  functions: The @ functions, and what does not exist
  query: Query and Mutation, the read proxy and writes
  links: Links to screens
  edit: Patches, Adjust and feedback
  examples: Whole programs on framework screens
  verify: The checklist before you send
---
A canvas is a generated view the drawer shows over the page. Take the user to a real screen first (screen, search);
write a canvas only when no screen shows what they asked for: a chart of a list, a dashboard of several lists, a batch
confirm. Call write_ui with kind openui, a short title and program, the whole OpenUI Lang program. The server parses it
before anything is shown; an error names the section of this skill to read and quotes its rule: fix that line and send
it again. To change a canvas, send patch with only the changed statements.

The rules broken most often:

1. One statement per line, name = expression; root = Stack([...]) is the first line.
2. Arguments are positional; there are no named arguments (no direction: "row").
3. A $name holds a plain value ($q = ""); a Query or a computed value has a plain name.
4. Never put an expression in quotes, never use single quotes.
5. Lists of children are one array argument: Card([Table([...])]), never Card(Table(...)).
6. Only the @ functions of section functions; there is no @Map, no @If, no Number() or .toFixed().
7. Query("request", {path, query: {...}}, {rows: []}): close the options with }} before the defaults.
8. A Query path is an /apps screen with a list; its rows are keyed by the field names data shows as row keys.
9. A column is query.rows.field or @Each(rows, "r", ...); r is only inside the @Each.
10. A control's value is its second argument, $ and its name: Input("q", $q).
11. Every visible filter $variable is in the arguments of a Query.
12. A Mutation runs only from a Button's Action([@Run(name)]): the user's click is the approval.

Read data first (data on the screen list) to see the real rows and their keys, then write the program. Load a section
with skill name "openui-lang#query" when you need it; verify is the checklist to walk before you send.

## Syntax

Each statement is one line, name = expression, and root = Stack([...]) is the first line.

- Expressions: strings in double quotes with backslash escapes, numbers, true, false, null, arrays [a, b], objects
  {key: value}, component calls Name(arg1, arg2), references to other statements by name, and $variables.
- Every statement other than root is reached from root through the names it uses; one that is not is not shown.
- Hoisting: a name may be used before the line that defines it. Write root first, then the $variables, the Queries,
  the components and the leaf values, so the canvas streams in from the top.
- Arguments are positional: Stack([a, b], "row"), never Stack([a, b], direction: "row"). A colon is only for object literals and the ternary.
- Optional arguments may be left off the end. null fills a slot; it does not skip to the next one. Do not pad with null.
- Brackets, braces and parentheses balance in every statement, and strings use double quotes.
- Operators: + - * / % (a + with a string joins text: "Total " + n), == != > < >= <=, && || !, cond ? a : b,
  parentheses, member access rows.field (on a list it takes the field of every element), index access rows[0].
- A $name holds a plain value, such as $q = "". A Query, a Mutation or a computed value has a plain name: orders = Query(...), placed = @Count(orders.rows).
- A $name not declared starts as null. Declare it to give a default: $days = "14".
- Never put an expression in quotes and never use single quotes: Stat("Total", @Sum(orders.rows.total)), not "@Sum(orders.rows.total)".

WRONG: `$orders = Query("request", {path: "/apps/x/List"}, {rows: []})` (a $ is stored as written: the canvas shows
nothing). RIGHT: `orders = Query("request", {path: "/apps/x/List"}, {rows: []})`.

WRONG: `header = Stack([title, chart], direction: "row", gap: "l")`. RIGHT: `header = Stack([title, chart], "row", "l")`.

WRONG: `total = Stat("Hits", "@Sum(hits.rows.hitCount)")`. RIGHT: `total = Stat("Hits", @Sum(hits.rows.hitCount))`.

## Components

Use only the components listed here, with their arguments in the order shown; ? marks an optional one.

Stack, Card, Accordion, Steps, ListBlock, Buttons, Tabs and Table take their children as one array argument:
`Card([Table([Col("Name", rows.name)])])`. A bare `Card(Table(...))` is not shown.

Layout:

- Stack(children, direction?, gap?, wrap?): vertical or horizontal layout; root is a Stack. direction: column (default) or row; gap: s, m or l; wrap: true wraps a row of cards or charts.
- Card(children): a bordered section.
- Separator(inset?): a horizontal rule.
- Accordion(children): Accordion([AccordionItem(value, title, content), ...]).
- AccordionItem(value, title, content): one expandable panel.
- Steps(children): a stepper, Steps([StepsItem(value, title, content), ...]).
- StepsItem(value, title, content): one step.
- Tabs(children): Tabs([TabItem(value, trigger, content), ...]).
- TabItem(value, trigger, content): one tab; trigger is its label.
- Modal(name, title, children): a dialog shown while $name is true; open it with @Set($name, true); its close button sets it false.

Content:

- CardHeader(title, subtitle?): a section title.
- TextContent(text, size?): plain text; size body (default), small or large. No HTML.
- Callout(text, type?): a banner; type info, warning, positive or negative.
- Stat(label, value?, caption?): a number card.
- Tag(label, color?, size?): a chip; color primary, positive, negative, warning, info or grey; size sm or md.
- ListBlock(children): ListBlock([ListItem(title, caption?, side?), ...]).
- ListItem(title, caption?, side?): a list row; side is often a Link or a Tag.
- Image(src, alt?): an image of this site; src is a path from the root, never another host.
- MarkDownRenderer(textMarkdown, variant?): Markdown (tables, code, mermaid fences), sanitized; variant clear or card. No HTML.
- Mermaid(text, height?): a diagram (status flows, processes).
- CodeBlock(code, language?): code.

Tables:

- Table(columns): Table([Col(label, values), ...]); every Col has one value a row.
- Col(label, values?): a column: a plucked field of a Query, rows.field, or @Each of components.

WRONG: `Col("Customer", r.name)` next to an @Each: r exists only inside its @Each. RIGHT: `Col("Customer", orders.rows.name)`.

Charts:

- Series(category, values): one series of a chart, Series("Hits", hits.rows.hitCount).
- BarChart(labels, series, variant?, xLabel?, yLabel?): vertical bars; variant grouped or stacked.
- LineChart(labels, series, variant?, xLabel?, yLabel?): lines; variant linear, natural or step.
- AreaChart(labels, series, variant?, xLabel?, yLabel?): filled lines.
- PieChart(labels, values, variant?): pie or donut (variant); the values are numbers, such as @Count or @Sum of a filter.

Labels and values come from a Query: `BarChart(hits.rows.artifactName, [Series("Hits", hits.rows.hitCount)])`. Never
write the numbers by hand when a list has them.

## Forms

The value of a control is its second argument, $ and its name: Input("q", $q), Select("status", $status, [SelectItem("A", "Active")]).

- Form(name, submit?, children): a form; Form(name, submitButton, [controls]). A Button with no Action submits the
  form's values to you; one with Action([@ToAssistant("Continue")]) sends that message with them.
- FormControl(label, control): a control with its label.
- Input(name, value?, placeholder?, label?): one line of text.
- TextArea(name, value?, placeholder?, label?): several lines.
- Select(name, value?, items, label?): a drop-down of fixed choices. The value is always the second argument and the items the third.
- SelectItem(value, label): a choice of a Select.
- CheckBox(name, value?, label?), Switch(name, value?, label?): true or false.
- RadioGroup(name, value?, items, label?): one of a few choices, items of RadioItem(value, label).
- RadioItem(value, label): a choice of a RadioGroup.
- Slider(name, value?, min?, max?, label?): a number; min 0 and max 100 unless given.
- DateTime(name, value?, type?, label?): a date (type date), a time (time) or both (date-time, the default). The value is YYYY-MM-DD HH:mm, YYYY-MM-DD, HH:mm or now; never epoch milliseconds.
- DatePeriod(name, label?, type?): a find period; it sets $name_period, $name_poffset, $name_pdate, $name_from and $name_thru, the find parameters of a date field: pass them in a Query's query.
- Lookup(name, value?, optionsUrl, valueField?, labelField?, dependsOn?, label?): a drop-down that searches as the user types; optionsUrl is an /apps transition that answers options (a path a screen's drop-down uses), never guessed. dependsOn names another control whose value it sends.
- Display(text?, valueUrl?, label?): a read-only value.
- Button(label, action?, color?): Button(label, Action([@Run(mutation), @Run(query), @Reset($a)]), color?); color primary (default), secondary or negative.
- Buttons(children): a row of buttons.

There is no validation object and no input type argument: never `Input("email", $email, {required: true})`.

WRONG: `Select("status", [SelectItem("A", "Active")], $status)`. RIGHT: `Select("status", $status, [SelectItem("A", "Active")])`.

Interactive filters: declare the $variable with its default, bind it as the control's second argument, and pass it in
the arguments of every Query it filters. When the user changes the control, the $variable changes and the Query reads
again.

```
$type = "AT_SERVICE"
typeSelect = FormControl("Type", Select("type", $type, [SelectItem("AT_SERVICE", "Services"), SelectItem("AT_ENTITY", "Entities")]))
hits = Query("request", {path: "/apps/system/ArtifactHitSummary", query: {artifactType: $type}}, {rows: []})
```

FILTER WIRING RULE: every $variable a visible filter sets appears in the arguments of every Query it filters. Never
show a filter while the Query's arguments are written by hand.

## Functions

Use only the @ functions listed here. There is no @Map and no @If: write rows.field for a column and cond ? a : b for a choice.

- @Count(list): its length. @First(list), @Last(list): one element.
- @Sum(numbers), @Avg(numbers), @Min(numbers), @Max(numbers): of a plucked column, @Sum(hits.rows.hitCount).
- @Sort(list, field, direction?): "asc" (default) or "desc".
- @Filter(list, field, operator, value): operator is "==", "!=", ">", "<", ">=", "<=" or "contains".
- @Round(number, decimals?), @Abs(number), @Floor(number), @Ceil(number).
- @Each(list, "r", template): the template for each element; r is the element, only inside the template. Write the
  template inline, never as a statement of its own.
- In an Action only: @Run(queryOrMutation), @ToAssistant("message"), @Set($name, value), @Reset($name, ...).

There are no JavaScript calls: no Number(), .toFixed() or .toString(); round with @Round(x, 2).

They compose: `@Count(@Filter(orders.rows, "statusId", "==", "Open"))`, `@Round(@Avg(hits.rows.averageTime), 1)`,
`@Each(rows, "r", Tag(r.statusId))`. Compute from a Query, never write a computed value by hand.

WRONG: `names = @Map(hits.rows, "r", r.artifactName)`. RIGHT: `names = hits.rows.artifactName`.

WRONG: `label = @If(n > 0, "Some", "None")`. RIGHT: `label = n > 0 ? "Some" : "None"`.

## Query

Query is Query("request", {path, query: {...}}, {rows: []}): close the options object with }} before the default data.

A Query reads a list of a screen: path is an /apps screen path with a list (one data reads), and list names it when the screen has several.

- The first argument is the tool, "request", the only one. The second is the options: path, list (the list's name when
  the screen has several), query (the list's find parameters: field, field_op, field_from, field_thru, pageSize,
  orderByField; they may be $variables, and a change reads again). The third is the data shown until it answers,
  {rows: []}. A fourth, optional, is a refresh interval in seconds.
- The answer is {rows, totalCount}: each row keyed by the names of the fields the list shows (data lists them as row
  keys), with _id (the row's id) and _link (its screen). Values are the text the list shows.
- Every read goes through the server's read proxy as the user: what the user cannot open, the canvas cannot read.
- Refresh: `Button("Refresh", Action([@Run(hits)]), "secondary")`.

WRONG: `data = Query("request", {path: "/apps/system/Visit/VisitList", query: {initialRequest: $q}, {rows: []})`.
RIGHT: `data = Query("request", {path: "/apps/system/Visit/VisitList", query: {initialRequest: $q}}, {rows: []})`.

WRONG: `Query("request", {path: "/qapps/system/Visit/VisitList"}, {rows: []})` (a /qapps link is a page). RIGHT: the
/apps path.

A Mutation is Mutation("request", {method: "POST", path, body: {...}}) on a transition of an /apps screen, run by a Button's Action([@Run(name)]).

- The path is the transition a screen's form posts to (`/apps/system/Security/UserGroup/UserGroupDetail/updateUserGroup`);
  body holds its parameters. It never runs when the canvas renders; it runs when the user clicks its Button. The click
  is the user's approval: the server runs the program's own Mutation with the canvas's values, as the user.
- A Mutation's body uses $variables and literals only; to send a row's value, @Set it into a $variable in the same Action first.
- name.status is "idle", "loading", "success" or "error", name.error the message: `saved.status == "error" ? Callout(saved.error, "negative") : null`.
- To run a Mutation without the user's click, call write_ui with run and its name: that asks the user's approval.

```
$description = ""
save = Mutation("request", {method: "POST", path: "/apps/system/Security/UserGroup/UserGroupDetail/updateUserGroup", body: {userGroupId: "ADMIN", description: $description}})
saveButton = Button("Save", Action([@Run(save), @Run(groups)]))
```

## Links

Link(label, path, params) opens a screen the user can view: path is an /apps or /qapps screen path, never a transition, a file or another site.

- Link(label, path, params?, hash?): params is an object of the screen's parameters; the screen opens in this window
  over the canvas, and the drawer stays.
- In a table: `Col("Open", @Each(visits.rows, "r", Link("View", "/apps/system/Visit/VisitDetail", {visitId: r.visitId})))`.
- A Query reads a list into the canvas; a Link takes the user to the screen itself. Prefer the Link when the user wants
  the screen.

## Edit

Reuse the existing statement names exactly; a patch with a new name for an existing part renames it, and the old one is dropped.

- write_ui with patch sends only the statements that changed or are new; the server merges them by name onto the
  canvas: the same name replaces, a new name is added, name = null removes one, and what root no longer reaches is
  dropped. The canvas is a new version.
- Changing a label is 1 statement; adding a component 2 or 3 (it and its parent); removing one, its parent again;
  wiring a new filter 3 to 5. More than 10 is a new program: send program instead.
- When the existing program already does what is asked, send only root.
- Adjust: when the user writes about the canvas ("make it a line chart"), the message names the canvas and its
  version. Their feedback is now your only task: change what they asked for with a patch, nothing else.
- Problems the page reports (a Query that failed, a name that does not resolve) come back in the next message's
  context with their sections: fix them with a patch.

## Examples

Service hits on System › Artifact Hit Summary:

```
root = Stack([CardHeader("Service hits"), stats, chart, table])
$type = "AT_SERVICE"
hits = Query("request", {path: "/apps/system/ArtifactHitSummary", query: {artifactType: $type, pageSize: "50", orderByField: "-hitCount"}}, {rows: []})
stats = Stack([Stat("Services", @Count(hits.rows)), Stat("Hits", @Sum(hits.rows.hitCount)), Stat("Slow", @Sum(hits.rows.slowHitCount))], "row")
chart = BarChart(hits.rows.artifactName, [Series("Hits", hits.rows.hitCount)], "grouped", "Service", "Hits")
table = Table([Col("Service", hits.rows.artifactName), Col("Hits", hits.rows.hitCount), Col("Average ms", hits.rows.averageTime)])
```

Visits with a filter and links:

```
root = Stack([CardHeader("Visits"), FormControl("Request starts with", Input("q", $q)), list])
$q = ""
visits = Query("request", {path: "/apps/system/Visit/VisitList", query: {initialRequest: $q, initialRequest_op: "begins"}}, {rows: []})
list = Table([Col("Visit", visits.rows.visitId), Col("From", visits.rows.fromDate), Col("Open", @Each(visits.rows, "r", Link("View", "/apps/system/Visit/VisitDetail", {visitId: r.visitId})))])
```

A patch of the first that makes the chart a line chart: `chart = LineChart(hits.rows.artifactName, [Series("Hits", hits.rows.hitCount)])`.

## Verify

Walk the program before you send it:

1. root = Stack([...]) is the first line, one statement per line.
2. Every referenced name is defined.
3. Every defined name other than root is reachable from root, and every Query is shown by a component.
4. Every visible filter $variable appears in the arguments of a Query.
5. No Query, Mutation or @ function result is declared with $; no expression is in quotes; no @Map, @If or JavaScript.
6. Every Query is Query("request", {path, query: {...}}, {rows: []}) on an /apps screen with a list; every Query's
   options close with }} before {rows: []}.
7. Lists of children are arrays: Card([...]), Table([Col(...)]).
8. Columns are rows.field or @Each with its variable inside it.
9. Every control's value is its second argument, $ and its name.
10. A Mutation's body uses $variables and literals, and only a Button's Action runs it.
11. Links are /apps or /qapps screen paths.
12. Brackets, braces and parentheses balance; strings use double quotes.
