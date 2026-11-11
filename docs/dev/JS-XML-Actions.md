# XML actions as JavaScript

XML actions (`set`, `if`, `iterate`, `service-call`, `entity-find` and the rest) are not interpreted. Each block of
them is compiled once to a JavaScript script, and the script runs on the thread's script engine with the context
stack as its variables. This guide lists every action element with the JavaScript it becomes, the rules for the
expressions in their attributes, what differs from the Groovy of Moqui Java, and the names a rule's actions see.

What a service body sees and returns is in [JS-Service-Scripts](JS-Service-Scripts.md), a screen's actions in
[JS-Screen-Scripts](JS-Screen-Scripts.md), and the members of `ec` in [JS-Core-API](JS-Core-API.md).

## Where they run

| Place | |
|---|---|
| A service's `<actions>` | The body of an inline service |
| A screen's `<always-actions>`, `<pre-actions>`, `<actions>`, a section's `<actions>`, a form-list's `<row-actions>` | Data preparation for a render |
| A transition's `<service-call>` and `<actions>` | What a request to the transition does |
| A `<condition>` element (a section, a transition, a response, a rule) | The condition elements below, joined as an `<and>` |
| A `<seca>`, `<eeca>`, `<emeca>` or `<giteca>` rule's `<actions>` | [Rule actions](#rule-actions) |

The generator is `runtime/base-component/framework/template/XmlActions.js`: one function for each element, which
returns the JavaScript text for it. `crates/moqui-core/src/script/actions.rs` walks the XML, calls those functions,
and keeps the result. The module is built into the binary.

- The script is generated once for each XML text and kept (up to 4096 scripts), and a definition keeps its own
  prepared script after its first run.
- A block that does not generate, or whose script does not parse, fails when its service or screen loads.
- A block of only simple `set` elements runs without the engine (`script/actions_set.rs`), with the same results.
- `MOQUI_ACTIONS_DEBUG=1` puts a comment naming the element before each statement; `=dump` also prints each
  generated script with line numbers. An error in actions shows the generated line it came from.

The whole block is one function called at once, in sloppy mode:

```js
(function() {
    // the statements of the actions
    return;
})()
```

So `<return/>` and `<check-errors/>` leave the block, and a `const` or `let` in a `<script>` is a local of that
function.

## Expressions

| Attribute | |
|---|---|
| `from`, `condition`, `in-map`, `list`, `map`, `field` of `compare`, `ignore` of `econdition`, a `<script>` or `<expression>` body | JavaScript, copied into the generated script as written |
| `value`, `default-value`, a `field-map` `value`, a `compare` `value`, a service or entity name, an `order-by` or `select-field` name | Text. With a `${}` in it, `ec.resource.expandNoL10n(text)`; otherwise a string literal |
| `<message>` text, `message` of `return` and `log`, `default-parameters` | Text, localized and then expanded: `ec.resource.expand(text)` |
| `field` of `set`, `value-field`, `list` and `count-field` of a find, `entry`, `key` | A name (or a path such as `order.total`) that is assigned |

`${expression}` in text is a JavaScript template expression: the text becomes a template literal and is evaluated in
the same context. Text with no `$`, or with `$` only as its last character, is returned as it is. A `\${` stays
literal.

### Names

- **A bare name reads the context stack.** `orderId` in an expression is the value any frame of the stack holds for
  it, the top frame first. The JavaScript built-ins, `ec`, `context` and the helper globals are found before the
  stack.
- **An unset name is null.** It does not throw. `if (unset)` is false, and `typeof unset` is `"object"`.
- **A bare assignment writes the top frame.** `<set field="total" from="5"/>` is `total = 5;`.
- **`context` is the stack**, the same object as `ec.context`: `context.total`, `context.put(name, value)`,
  `context.containsKey(name)`.
- **A list or map read by name is tracked.** A change to it in place (`items.push(x)`, `map.key = value`) is written
  back to the stack.
- **Types.** A decimal is a host decimal, a timestamp, date or time is text of its epoch milliseconds, an entity
  value and an entity list are host objects ([JS-Service-Scripts](JS-Service-Scripts.md#parameters-validation-and-types)).

### Truth and comparison

**A condition is JavaScript truth.** `condition=""`, an `<expression>`, and a `<condition>` element are evaluated as
JavaScript and nothing is rewritten (`script/runtime.rs` `eval_condition`). Null, `false`, `0`, `NaN` and `""` are
false. An empty array, an empty object, and every decimal, zero included, are true. This differs from Groovy, where
an empty list or map and a zero `BigDecimal` are false.

Groovy's rules are applied in three places only:

| Where | Rule |
|---|---|
| `default-value` of `set` | The default is taken when the value is false by Groovy truth: null, `""`, `false`, zero (a zero decimal too), an empty list, map or `Set`, empty bytes (`__isGroovyFalse`) |
| `set-if-empty="false"` of `set` | The set is skipped when the value is empty as Java's `ObjectUtilities.isEmpty` has it: null, `""`, an empty list, map or `Set`. Zero and `false` are not empty (`__isEmpty`) |
| `<compare>`, and `filter-map-list` | Java's `ObjectUtilities.compare`: both sides are converted to the `type`, or to the field's own type, and compared, so two decimals order as numbers. `equals` on decimals also compares the scale: `10.0` does not equal `"10.00"` ([JS-Globals](JS-Globals.md#converting-and-comparing)) |

Everywhere else, plain `<script>` text included, `==`, `<` and `!` are JavaScript's:

| Expression | In JavaScript |
|---|---|
| `amount == 0`, `amount > 5`, `amount + 1` on a decimal | Throws: a decimal's `valueOf` is not supported. Use `amount.isZero()`, `amount.compare(5) > 0`, `amount.add(1)` |
| `a == b` on two decimals | False unless they are the same object. Use `a.eq(b)` |
| `if (amount)`, `amount \|\| fallback` on a decimal | Always true, always `amount` |
| `if (list)` | True for an empty list. Use `list.length`, or `list.isEmpty()` on an entity list |
| `a == b` on two entity values | Identity. Use `a.equals(b)` |

The spec files of `moqui test` are a different case: their `then` and `expect` conditions are rewritten to Groovy
truth and value comparison ([JS-Testkit](JS-Testkit.md)).

## Action elements

Each row shows the JavaScript an element generates, shortened, with attribute names standing for their values.

### Values and control

| Element | Becomes |
|---|---|
| `<set field from/>` | `field = from;` |
| `<set field value/>` | `field = "text";` or `field = ec.resource.expandNoL10n("text ${x}");` |
| `<set ... default-value/>` | `var _temp_default = from; field = (__isGroovyFalse(_temp_default) ? "default" : _temp_default);` |
| `<set ... type/>` | `field = basicConvert(value, "type");` with the type names of a service parameter. `type="NewList"` is `[]` and `type="NewMap"` is `{}` |
| `<set ... set-if-empty="false"/>` | `var _temp_internal = value; if (!__isEmpty(_temp_internal)) field = _temp_internal;` |
| `<if condition>` with `<then>`, `<else-if condition>`, `<else>` | `if (condition) { ... } else if (condition) { ... } else { ... }`. A `condition` attribute and a `<condition>` child are joined with `&&` |
| `<while condition>` | `while (condition) { ... }` |
| `<iterate list entry>` | A `for` loop over the list that sets `entry`, `entry_index` and `entry_has_next` in the context on each pass. A null list runs nothing |
| `<iterate list entry key>` | The same over the entries of a map (or of a `Map`), setting `key` and `entry` |
| `<break/>`, `<continue/>` | `break;`, `continue;` |
| `<return/>` | `return;`. With `message`, the message is added first, as an error with `error="true"` |
| `<check-errors/>` | `if (ec.message.hasError()) return;` |
| `<script>` | The text as written, between two comments. With `location`, `ec.resource.script("location");` runs that file first |
| `<message>` | `ec.message.addMessage(text, type)`; with `public="true"` `addPublic(text, type)`; with `error="true"` `addError(text)`. `type` is `info` when not given |
| `<log level message/>` | `ec.logger.log("level", text, null);` with `info` when no level is given |
| `<assert title>` | For each condition child: `if (!(condition)) ec.message.addError("[title] Assert failed: condition");` |

An `iterate` list may be an array, an entity list, or an entity iterator (from `use-iterator`), which is read one row
ahead until it gives null. The entry is the row itself, so a field set on it is kept in the list.

### Conditions

These are the children of `<condition>`, `<if>`, `<while>`, `<assert>`, and of each other.

| Element | Becomes |
|---|---|
| `<expression>` | The text as an expression |
| `<compare field operator value to-field format type/>` | `compare(field, "operator", value, toField, format, "type")`. Operators: `equals` (the default), `not-equals`, `less`, `greater`, `less-equals`, `greater-equals`, `contains`, `not-contains`, `empty`, `not-empty`, `matches`, `not-matches` |
| `<and>`, `<or>`, `<not>` | `(a && b)`, `(a \|\| b)`, `!(a)` |

A `<compare>` or an `<expression>` with child actions is also a statement: `if (...) { children }`, and `<compare>`
takes an `<else>`.

### Service calls

```xml
<service-call name="mantle.party.PartyServices.find#Party" in-map="context" out-map="found"/>
```

```js
if (true) {
    var call_service_result = ec.service.callXml("mantle.party.PartyServices.find#Party", 0, null, ec.context, null);
    if (call_service_result != null) found = Object.assign({}, found || {}, call_service_result);
    if (ec.message.hasError()) return;
}
```

| Attribute or child | |
|---|---|
| `name` | The service; a `${}` in it is expanded |
| `in-map` | An expression for the parameters. `true` or `context` passes the whole context, of which the service takes its declared in-parameters |
| `<field-map field-name from value/>` | One parameter: `from` is an expression, `value` is text, and with neither the value is the name `field-name` |
| `out-map` | The name the results are assigned to, merged over what it holds. With `out-map-add-to-existing="false"` it is replaced. `context` puts each result in the context by name |
| `transaction` | `ignore`, `force-new`, `cache`, `force-cache`; `transaction-timeout` in seconds |
| `disable-authz="true"` | The call runs with authorization off |
| `multi="true"` | A multi call: once for each row of `name_0`, `name_1` parameters. `multi="parameter"` does that when the request's `_isMulti` is `true` |
| `ignore-error="true"` | An error the call leaves is logged and cleared. Otherwise the actions return |
| `async="true"` | `ec.service.async().name(...).parameters(...).call()`. `async="distribute"` is refused when the actions load |
| `web-send-json-response` | `true` sends the results as the JSON response, a name sends that out-parameter; sent before the error check |

### Entity finds

```xml
<entity-find entity-name="moqui.basic.Enumeration" list="enumList" cache="true">
    <econdition field-name="enumTypeId" value="UserPreferenceKey"/>
    <order-by field-name="sequenceNum"/>
</entity-find>
```

```js
enumList_xafind = ec.entity.find("moqui.basic.Enumeration").useCache(true).orderBy("sequenceNum").condition("enumTypeId", "UserPreferenceKey");
enumList = enumList_xafind.list();
```

The find is kept in the context as `<list>_xafind`.

| Element | Becomes |
|---|---|
| `<entity-find-one entity-name value-field>` | `valueField = ec.entity.findOneXml(name, flags, autoMap, fields, selectFields);`, or the chain `ec.entity.find(name)...one()` when `cache`, `for-update` or `use-clone` is an expression |
| `<entity-find entity-name list>` | `ec.entity.find(name)` with the options, then `.list()` |
| `<entity-find-count entity-name count-field>` | The same find, then `.count()` |
| `<entity-find-related value-field relationship-name list>` | `list = value.findRelated(relationship, map, orderByList, cache, forUpdate);` |
| `<entity-find-related-one value-field relationship-name to-value-field>` | `toValue = value.findRelatedOne(relationship, cache, forUpdate);` |

Attributes of a find: `cache`, `for-update`, `for-no-key-update` (not with `for-update`), `distinct`, `use-clone`,
`disable-authz`, `offset`, `limit`. An entity name with `${}` is expanded.

`entity-find-one` takes its condition from its `<field-map>` children. With none, or with `auto-field-map="true"`,
it uses the fields of the context; `auto-field-map` may also be an expression for a map.

| Child of a find | Becomes |
|---|---|
| `<econdition field-name from value/>` | `.condition("field", value)` for a plain equals. With `operator`, `ignore-case`, `ignore-if-empty`, `or-null`, `to-field-name` or `ignore`: `.condition(ec.entity.conditionFactory.makeActionConditionDirect(...))` |
| `<econditions combine>` | `.condition(ec.entity.conditionFactory.makeCondition([...], "and"))`, nested ones inside it |
| `<econdition-object field/>` | `.condition(field)`, a condition object held in the context |
| `<date-filter from-field-name thru-field-name valid-date ignore-if-empty ignore/>` | `.condition(ec.entity.conditionFactory.makeConditionDate(...))`. On a cached find the result is filtered: `list = list.filterByDate(...)` |
| `<having-econditions>` | `.havingCondition(...)` for each child |
| `<select-field field-name/>`, `<order-by field-name/>` | `.selectField("name")`, `.orderBy("name")` |
| `<limit-range start size/>` | `.offset(start).limit(size)` |
| `<limit-view view-index view-size/>` | `.offset((index - 1) * size).limit(size)` |
| `<use-iterator/>` | `.iterator()` in place of `.list()`, unless the find is cached |
| `<search-form-inputs input-fields-map default-order-by skip-fields paginate require-parameters>` | `.searchFormMap(inputMap, defaults, skipFields, defaultOrderBy, paginate)`, the input map being the context when not given. `<default-parameters>` gives the defaults |

A find with `search-form-inputs` that paginates also sets `<list>Count`, `<list>PageIndex`, `<list>PageSize`,
`<list>PageMaxIndex`, `<list>PageRangeLow` and `<list>PageRangeHigh`, which a form-list's pagination reads.

### Entity values

| Element | Becomes |
|---|---|
| `<entity-make-value entity-name value-field map/>` | `value = ec.entity.makeValue(name);` and with `map`, `value.setFields(map, true, null, null);` |
| `<entity-set value-field map prefix include set-if-empty/>` | `value.setFields(map, setIfEmpty, prefix, pkFlag);` with the context as the map when not given; `include` is `pk`, `nonpk` or all |
| `<entity-create value-field/>` | `value.create();`, or `value.createOrUpdate();` with `or-update="true"` |
| `<entity-update value-field/>`, `<entity-delete value-field/>` | `value.update();`, `value.delete();` |
| `<entity-sequenced-id-primary value-field/>`, `<entity-sequenced-id-secondary value-field/>` | `value.setSequencedIdPrimary();`, `value.setSequencedIdSecondary();` |
| `<entity-delete-related value-field relationship-name/>` | `value.deleteRelated("relationship");` |
| `<entity-delete-by-condition entity-name>` | The find of its conditions, then `.deleteAll();` |

`disable-authz="true"` on `entity-create`, `entity-update`, `entity-delete` and the sequenced id elements wraps the
call:

```js
var _xa_was_disabled = ec.artifactExecution.disableAuthz();
try { value.update(); } finally { if (!_xa_was_disabled) ec.artifactExecution.enableAuthz(); }
```

`entity-data` is not implemented: actions that use it do not load.

### Lists

| Element | Becomes |
|---|---|
| `<order-map-list list>` with `<order-by field-name/>` | `orderMapList(list, ["field", "-other"]);`. A plain array is sorted in place; an entity list is copied to an array, sorted, and the name set to the copy |
| `<filter-map-list list to-list>` with `<field-map>` and `<date-filter>` | A copy of the list as an array, `filterMapList(copy, {field: value});` and `filterMapListByDate(copy, ...)`, then `to-list` (or `list`) set to the copy |

## Groovy that does not carry over

Expressions written for Moqui Java are Groovy. The load of a service or screen fails on Groovy that is not also
JavaScript, with "expression ... is Groovy (`?:`)", "(`->`)" or "(closure)" when it can tell.

| Groovy | Here |
|---|---|
| `a ?: b` | Not JavaScript. `a \|\| b` takes `b` for null, `""`, `0` and `false`, but not for an empty list or a zero decimal. `a ?? b` takes `b` only for null |
| `a?.b` | The same in JavaScript |
| `list.size()`, `map.keySet()`, `list.contains(x)`, `map.containsKey(k)`, `list.isEmpty()` | `list.length`, `Object.keys(map)`, `list.includes(x)`, `k in map`. An entity list keeps `size()` and `isEmpty()`, and `context` keeps the Map methods |
| `"total ${x}"` in a script | A template literal: `` `total ${x}` ``. In a `value` attribute `${x}` works as before |
| `list.each { ... }`, `collect`, `findAll`, `{ a -> ... }` | `forEach`, `map`, `filter`, `a => ...` |
| `[a: 1]`, `[1, 2]`, `1..5`, `x as String`, `def x` | `{a: 1}`, `[1, 2]`, a loop or `slice`, `String(x)`, `let x` |
| `x in [a, b]` | `[a, b].includes(x)`. `in` on an array tests an index |
| `1.5` (a `BigDecimal` in Groovy) | A number. `toDecimal("1.5")` makes a decimal |
| `amount > 0`, `a + b` on `BigDecimal` | The decimal methods |
| `text.replaceAll('[^a-z]+', '')` | The pattern is text in JavaScript: use a regular expression, `/[^a-z]+/g` |
| `catch (Exception e)` | `catch (e)` |
| `new java.util.Date()`, a static call on a Java class | A JavaScript or host equivalent ([JS-Globals](JS-Globals.md)) |

### `moqui lint`

| Command | |
|---|---|
| `moqui lint [service-file]` | Each service's expressions, scripts and generated actions must parse. With no file, the service files the conf names |
| `moqui lint --screens [report-file]` | The same for every screen and form file, with the idioms counted ([JS-Screen-Scripts](JS-Screen-Scripts.md#moqui-lint---screens)) |
| `moqui lint --groovy [--all] <dir> ...` | The Groovy idioms and run-time hazards left under the directories, by file, and the `.groovy` files found |
| `moqui convert --groovy [--write] <dir> ...` | Rewrites map literals, `context + [...]`, Elvis and EDI grammar files |

The report classes each idiom (`crates/moqui-devkit/src/screen_lint.rs` `Idiom`, `lint_hazards.rs`):

| Class | Idioms |
|---|---|
| Groovy, to rewrite | `elvis`, `range`, `closure`, `gstring`, `gstring-dollar-name`, `regex-operator`, `decimal-literal`, `bigdecimal`, `new-type`, `map-literal`, `list-star`, `as-cast`, `in-operator`, `def`, `static-class`, `quoted-property`, `collection-method`, `catch`, `context-plus`, `string-as-regex` |
| Hazard: parses, and probably misbehaves | `empty-collection-truthiness`, `decimal-operator`, `find-list-mutation`, `in-on-list`, `duplicate-declaration`, `entity-value-identity` |
| Valid JavaScript, counted and not flagged without `--all` | `safe-navigation`, `decimal-point-literal`, `set-scale`, `new-js-type`, `add-call`, `catch-bare`, `host-static` |

The hazards are found by name within one service: a name declared `BigDecimal` or set from a decimal call under an
operator or a truth test (`decimal-operator`); `push`, `add` or `each` on the result of an `entity-find`, which is a
read-only host list (`find-list-mutation`); `x in list` on an array; a `const` or `let` that a second `<script>` of
the same service declares again; `==` on a value copied with `cloneValue()`. The scan has no types, so it knows only
what the service text says. An `empty-collection-truthiness` warning is dropped when the parameter's declared type
is not a collection.

## Rule actions

The actions of an ECA rule are XML actions in a context frame the rule fills. The frame is pushed on the stack the
event happened in, so the names under it stay readable, and it is popped when the rule ends. A rule's `<condition>`
is its first child element only.

| Rule | Names in the context |
|---|---|
| Service ECA (`*.secas.xml`) | Each in-parameter by name, and `parameters`, the map of them. For `post-service`, `post-commit`, `tx-commit` and `tx-rollback`: each result by name, and `results` |
| Entity ECA (`*.eecas.xml`) | Each field of the entity by name (null when the write does not set it), `entityValue`, `originalValue` and `eecaOperation` (`create`, `update` or `delete`) |
| Email ECA (`*.emecas.xml`) | `fields` (`toList`, `ccList`, `bccList`, `from`, `subject`, `sentDate`, `receivedDate`), `headers`, `bodyPartList`, `flags`, `message` (`messageId`, `subject`, `size`, `raw`) and `emailServerId`; for a message the mail client stored, `emailAccountId` and `emailMessageId` |
| Git ECA (`*.gitecas.xml`) | `repositoryId`, `source`, `transport`, `userId`, `updateId`, `pushOptions`, `refUpdates`, and the event's own values ([Git-Hosting](../interfaces/Git-Hosting.md#giteca-rules)) |

**Service ECA** (`service/seca.rs`). `when` is `pre-validate`, `pre-auth`, `pre-service`, `post-service`,
`post-commit`, `tx-commit` or `tx-rollback`.

- `parameters` and `results` are the call's own maps: `<set field="parameters.statusId" value="Active"/>` in a
  `pre-*` rule changes the service's input, and a change to `results` in a `post-*` rule changes what the call
  returns. Setting the bare name `statusId` changes only the rule's frame.
- A rule is skipped while `ec.message` has an error unless it has `run-on-error="true"`.
- A failure is reported as "service ECA `<id>` on `<when>` of `<service>`: ...".
- `tx-commit` and `tx-rollback` rules run on a service worker when the transaction ends, in a context and
  transaction of their own.
- **A refusal needs `force-new`.** A `pre-service` rule that calls a service which adds an error stops the guarded
  service. The refusing service joins the guarded service's transaction and marks it rollback-only, so the user sees
  a second error, "Transaction marked for rollback ...", beside the refusal. Give the refusing service
  `transaction="force-new"`.

**Entity ECA** (`service/eca.rs` `run_matched_eca`).

- `entityValue` is the value being written. A field set on it in a `run-before="true"` rule is written.
- `originalValue` is the database row before an update or delete, with `get-original-value="true"`; else null.
- `get-entire-entity="true"` fills the fields the write does not set from the database row, for an update or delete.
- `set-results="true"` copies the non-primary-key field names the actions set in the rule's frame onto the value.
- A failure is reported as "entity ECA `<id>` on `<operation>` of `<entity>`: ...".

**Email ECA** (`service/email_eca.rs`, [Mail-Inbound](../interfaces/Mail-Inbound.md)). Each rule runs when its
condition passes. A rule that fails is logged and the others still run.

**Git ECA** (`service/git_eca.rs`). Rules run with authorization off. An error a `pre-receive` or `pre-ref-update`
rule leaves in `ec.message` refuses the push or the ref. Each rule of a post event runs in a transaction of its own.

## Pitfalls

- `<if condition="list">` is true for an empty list. Write `list.length`.
- `<set field="x" from="y" default-value="0"/>` takes the default for a zero or an empty list, as Groovy's `?:` did;
  `from="y || 0"` in the same place does not take it for an empty list.
- `value="5"` is the text `"5"`. Use `from="5"` for a number, or add `type`.
- `value="${amount}"` on a decimal gives its text. Use `from="amount"` to keep the decimal.
- A `<script>` that declares `const total` makes `total` a local for the rest of the block; `<set field="total">`
  after it no longer reaches the context.
- `iterate` leaves `entry`, `entry_index` and `entry_has_next` in the context after the loop.
- A timestamp read in an expression is text of its epoch milliseconds: compare two with `Number(a) < Number(b)`,
  and format one with `ec.l10n.format`.
