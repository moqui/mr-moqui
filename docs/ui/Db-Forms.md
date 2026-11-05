# Database forms

A `DbForm` is a form kept in the database: `DbFormField` rows with their options (`DbFormFieldOption`), attributes and
entity options, read into a `form-single` or `form-list` by `extends="DbForm#formId"` or by a `DbForm` that modifies a
screen form for a user group (`modifyXmlScreenForm`, `DbFormUserGroup`). Answers are a `FormResponse` with its
`FormResponseAnswer` rows, one per field name. The entities are in `framework/entity/ScreenEntities.xml`, the form is
built in `crates/moqui-core/src/screen/db_form.rs` and the services are `org.moqui.impl.ScreenServices`, their bodies in
`framework/service/org/moqui/impl/DbForms.js`.

## Dated forms

A form with `DbForm.isDated = Y` changes by effective-dated rows rather than by edits that lose what a response
answered:

- `DbFormField`, `DbFormFieldOption` and `DbFormFieldDependency` have `fromDate` and `thruDate` (not key fields). A row
  with no `fromDate` is a **draft**.
- The form is read **as of a date**, now by default, with drafts left out. A render reads the date from the form's
  `db-form-as-of` attribute and includes drafts when its `db-form-include-drafts` is true (a preview), each an
  expression evaluated in the screen's context, on the `form-single` or `form-list` that extends the `DbForm` (or the
  outermost form of an `extends` chain that has one; for a `DbForm` that modifies a screen form, that form). They are
  attributes and not context fields so that a request parameter cannot show the drafts or move the date. A draft field
  that replaces another, or a draft option with the key of another, puts that one out of the preview. `ec.screen.getFormNode("DbForm#id",
  asOfDate, includeDrafts)` reads it the same way, and `get#FormResponse` reads the form as of the response's
  `responseDate`, so a response reads as it was submitted.
- The key fields are audit-logged (`EntityAuditLog`): a field's `title`, `isRequired`, `fieldTypeEnumId`, dates and
  `replacesFieldName`; an option's `keyValue`, `text` and dates; every field of a dependency; the form's `isDated`.
- A form that is not dated reads every row with no `fromDate` as in effect, and `update#DbFormField` deletes and creates
  the field again named from its title, as it always has.

The services:

| Service | What it does |
| --- | --- |
| `set#DbFormDated` | The form becomes dated; its rows with no `fromDate` start at `fromDate` (now) |
| `publish#DbForm` | The drafts start at `fromDate` (now, or later: an earlier date is refused), each ending the row it replaces; `fieldNames` limits it. Refused when the dependencies would have a cycle |
| `create#DbFormFieldSuccessor` | A new field for a new type or meaning, copying the old one's options, attributes and dependencies: a draft, or started at `fromDate` (now or later) with the old field ended then. The answers keep the old name |
| `end#DbFormField`, `end#DbFormFieldOption` | End a field (with the dependencies that name it) or an option at `thruDate` (now, or later: an earlier date is refused, as for `end#DbFormFieldDependency`) |
| `update#DbFormField` | On a dated form, an in-place edit; a published field keeps its type, and the dates and `replacesFieldName` change only through the services above |
| `delete#DbFormField`, `delete#DbFormFieldOption` | On a dated form, refused for a field or option with answers: end it |

## Dependencies

A `DbFormFieldDependency` shows or requires one field by the value of another: `sourceFieldName`, `operator`, `value`,
`targetFieldName` and `effect` (`show` or `require`). The operators are a fixed list: `equals` (the value, or one of a
comma-separated list, each trimmed; with several values, as a multiple select has, any one of them), `not-equals` (none
of them), `empty` and `not-empty`. A value is data, never an expression (`${` is refused), and `equals` and
`not-equals` need one.

- `store#DbFormFieldDependency` creates or updates one and `end#DbFormFieldDependency` ends one. On a dated form a
  published dependency is never edited: storing it ends it now and creates its replacement from now, and a `fromDate`
  before now is refused. Both check the
  operator and effect, that the two fields are the form's and differ, and, at now and each date a row starts or ends,
  that the rules make no cycle and give no field two rules of one effect.
- The form maps the rows in effect to `visible-when` (show) and `require-when` (require) on the target field, and a
  field's `isRequired` to `required="true"` on its sub-field. These render in every mode: `qvt` (a style, and a bound
  `required` on text-line, password and date-time), `qvt-static` (`$mq.shown`, `$mq.holds`), `html` and `tui`. A field
  hidden by its rule is not required, and counts as empty for the rules that name it.
- In the `html` mode the field's box carries `data-visible-when` and `data-require-when` (JSON: `form`, `field`,
  `operator`, `values`), and `/js/moqui-form-when.js` applies them: a self-hosted script with no inline code, so a page
  under CSP `script-src 'self'` (a PASS page) loads it with a `<script src>`.
- The browser's check is a convenience. `evaluate#DbFormDependencies` checks submitted `values` on the server, as of
  `asOfDate`: it answers `visibleFieldNames`, `hiddenFieldNames`, `requiredFieldNames` and `missingFieldNames` (required,
  shown and empty), for the caller's own validation of a submit.
