// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// Dated DbForms and their dependencies (docs/ui/Db-Forms.md): the bodies of the DbForm services of
// org.moqui.impl.ScreenServices. A form with DbForm.isDated changes by effective-dated rows: a draft (no fromDate) is
// published from a date and ends the row it replaces, an answered field or option is ended, never deleted, and a field
// whose type or meaning changes is a successor field from a date. A DbFormFieldDependency shows or requires its target
// field while its source field meets the operator; the form renders the rows in effect as visible-when and require-when
// (crates/moqui-core/src/screen/db_form.rs), and evaluate() checks a submit against the same rules on the server. The
// key fields are audit-logged by the entity definitions (ScreenEntities.xml).

const FORM = 'moqui.screen.form.DbForm'
const FIELD = 'moqui.screen.form.DbFormField'
const OPTION = 'moqui.screen.form.DbFormFieldOption'
const ATTRIBUTE = 'moqui.screen.form.DbFormFieldAttribute'
const DEPENDENCY = 'moqui.screen.form.DbFormFieldDependency'
const ANSWER = 'moqui.screen.form.FormResponseAnswer'

/** The operators and effects of a dependency, as db_form.rs OPERATORS and EFFECTS. */
const OPERATORS = ['equals', 'not-equals', 'empty', 'not-empty']
const EFFECTS = ['show', 'require']

const ms = (value) => {
  if (value == null || value === '') return null
  const number = Number(value)
  return Number.isNaN(number) ? new Date(String(value)).getTime() : number
}
const nowMs = () => ms(ec.user.nowTimestamp)
const dateOf = (value, fallback) => new Date(ms(value) ?? fallback)

function refuse(text) {
  ec.message.addError(ec.l10n.localize(text))
  return {}
}

function formOf(formId) {
  return ec.entity.find(FORM).condition('formId', formId).one()
}

function fieldsOf(formId) {
  return Array.from(ec.entity.find(FIELD).condition('formId', formId).orderBy('layoutSequenceNum').list())
}

function dependenciesOf(formId) {
  return Array.from(ec.entity.find(DEPENDENCY).condition('formId', formId).orderBy('dependencySeqId').list())
}

/** Whether a row is in effect at a time: started (or a draft, when drafts count) and not ended. */
function inEffectAt(row, at, drafts) {
  const thru = ms(row.thruDate)
  if (thru != null && thru <= at) return false
  const from = ms(row.fromDate)
  return from == null ? drafts : from <= at
}

const isAnswered = (formId, fieldName, keyValue) => {
  const fields = keyValue != null ? { formId, fieldName, valueText: keyValue } : { formId, fieldName }
  return ec.entity.find(ANSWER).condition(fields).count() > 0
}

/** Whether a dependency holds for a field's value, as db_form.rs condition_holds. */
function holds(operator, expected, actual) {
  const parts = (Array.isArray(actual) ? actual : actual == null ? [] : String(actual).split(','))
    .map((part) => String(part).trim()).filter((part) => part !== '')
  if (operator === 'empty') return parts.length === 0
  if (operator === 'not-empty') return parts.length > 0
  const wanted = String(expected ?? '').split(',').map((part) => part.trim())
  const any = parts.some((part) => wanted.includes(part))
  return operator === 'not-equals' ? !any : any
}

/**
 * The refusal of the form's dependency rows, or null: at now and at each date a row starts or ends, earlier ones too
 * since a response reads the form as of its date (drafts count as started now), no field depends on itself through a
 * chain (a cycle), and no field has two rules of one effect.
 */
function dependencyProblem(formId) {
  const rows = dependenciesOf(formId)
  const dates = new Set([nowMs()])
  for (const row of rows) {
    for (const value of [row.fromDate, row.thruDate]) {
      const at = ms(value)
      if (at != null) dates.add(at)
    }
  }
  // the fields a draft replaces: with the drafts counted, their rows are ended by the publish
  const replaced = new Set(fieldsOf(formId).filter((field) => field.fromDate == null && field.replacesFieldName)
    .map((field) => field.replacesFieldName))
  const namesReplaced = (row) => replaced.has(row.sourceFieldName) || replaced.has(row.targetFieldName)
  const scenarios = []
  for (const at of dates) {
    scenarios.push(rows.filter((row) => inEffectAt(row, at, false)))
    scenarios.push(rows.filter((row) => inEffectAt(row, at, true) && (row.fromDate != null ? !namesReplaced(row) : true)))
  }
  for (const live of scenarios) {
    const seen = new Map()
    for (const row of live) {
      const key = row.targetFieldName + ' ' + row.effect
      if (seen.has(key)) {
        return `Field ${row.targetFieldName} already has a ${row.effect} dependency (${seen.get(key)}); end it first`
      }
      seen.set(key, row.dependencySeqId)
    }
    const edges = new Map()
    for (const row of live) {
      if (!edges.has(row.sourceFieldName)) edges.set(row.sourceFieldName, [])
      edges.get(row.sourceFieldName).push(row.targetFieldName)
    }
    // depth first: a field met again while it is on the path closes a cycle
    const state = new Map()
    const visit = (name, path) => {
      if (state.get(name) === 'done') return null
      if (state.get(name) === 'open') return path.slice(path.indexOf(name)).concat(name)
      state.set(name, 'open')
      for (const next of edges.get(name) || []) {
        const cycle = visit(next, path.concat(name))
        if (cycle) return cycle
      }
      state.set(name, 'done')
      return null
    }
    for (const name of edges.keys()) {
      const cycle = visit(name, [])
      if (cycle) return 'The dependencies make a cycle: ' + cycle.join(' -> ')
    }
  }
  return null
}

/** Refuse (with the transaction rolled back by the error) when the form's dependencies have a problem. */
function checkDependencies(formId) {
  const problem = dependencyProblem(formId)
  if (problem) refuse(problem)
  return !problem
}

/** set#DbFormDated: the form becomes dated, its rows with no fromDate starting at fromDate (now by default). */
function setDated({ formId, fromDate }) {
  const form = formOf(formId)
  if (!form) return refuse(`DbForm ${formId} not found`)
  if (form.isDated === 'Y') return {}
  const from = dateOf(fromDate, nowMs())
  for (const entity of [FIELD, OPTION, DEPENDENCY]) {
    for (const row of ec.entity.find(entity).condition('formId', formId).list()) {
      if (row.fromDate == null) {
        row.set('fromDate', from)
        row.update()
      }
    }
  }
  form.set('isDated', 'Y')
  form.update()
  return {}
}

/**
 * publish#DbForm: the form's drafts start at fromDate (now by default): fields (ending the field each replaces, and
 * that field's dependencies), options (ending the option of the same key) and dependencies. fieldNames limits it to
 * those fields' drafts and options.
 */
function publish({ formId, fromDate, fieldNames }) {
  const form = formOf(formId)
  if (!form) return refuse(`DbForm ${formId} not found`)
  if (form.isDated !== 'Y') return refuse(`DbForm ${formId} is not dated: set#DbFormDated first`)
  const from = dateOf(fromDate, nowMs())
  const only = fieldNames && fieldNames.length ? new Set(Array.from(fieldNames).map(String)) : null
  const wanted = (name) => !only || only.has(name)
  const endAt = (row) => {
    const thru = ms(row.thruDate)
    if (thru == null || thru > from.getTime()) {
      row.set('thruDate', from)
      row.update()
    }
  }
  const published = []
  for (const field of fieldsOf(formId)) {
    if (field.fromDate != null || !wanted(field.fieldName)) continue
    field.set('fromDate', from)
    field.update()
    published.push(field.fieldName)
    if (field.replacesFieldName) {
      const old = ec.entity.find(FIELD).condition({ formId, fieldName: field.replacesFieldName }).one()
      if (old) endAt(old)
      for (const row of dependenciesOf(formId)) {
        if (row.sourceFieldName === field.replacesFieldName || row.targetFieldName === field.replacesFieldName) endAt(row)
      }
    }
  }
  for (const option of Array.from(ec.entity.find(OPTION).condition('formId', formId).list())) {
    if (option.fromDate != null || !wanted(option.fieldName)) continue
    for (const other of ec.entity.find(OPTION).condition({ formId, fieldName: option.fieldName, keyValue: option.keyValue }).list()) {
      if (other.sequenceNum !== option.sequenceNum && other.fromDate != null) endAt(other)
    }
    option.set('fromDate', from)
    option.update()
  }
  for (const row of dependenciesOf(formId)) {
    if (row.fromDate != null || (only && !only.has(row.targetFieldName) && !only.has(row.sourceFieldName))) continue
    row.set('fromDate', from)
    row.update()
  }
  if (!checkDependencies(formId)) return {}
  return { publishedFieldNames: published }
}

/** end#DbFormField: the field ends at thruDate (now by default), with the dependencies that name it. */
function endField({ formId, fieldName, thruDate }) {
  const field = ec.entity.find(FIELD).condition({ formId, fieldName }).one()
  if (!field) return refuse(`Field ${fieldName} of DbForm ${formId} not found`)
  const thru = dateOf(thruDate, nowMs())
  field.set('thruDate', thru)
  field.update()
  for (const row of dependenciesOf(formId)) {
    if (row.sourceFieldName !== fieldName && row.targetFieldName !== fieldName) continue
    const end = ms(row.thruDate)
    if (end == null || end > thru.getTime()) {
      row.set('thruDate', thru)
      row.update()
    }
  }
  return {}
}

/** delete#DbFormFieldOption: refused on a dated form for an option with answers, which is ended instead. */
function deleteOption({ formId, fieldName, sequenceNum }) {
  const option = ec.entity.find(OPTION).condition({ formId, fieldName, sequenceNum }).one()
  if (!option) return refuse(`Option ${sequenceNum} of field ${fieldName} of DbForm ${formId} not found`)
  const form = formOf(formId)
  if (form && form.isDated === 'Y' && option.keyValue != null && isAnswered(formId, fieldName, option.keyValue)) {
    return refuse(`Option ${option.keyValue} of field ${fieldName} has answers: end it (end#DbFormFieldOption) rather than delete it`)
  }
  option.delete()
  return {}
}

/** end#DbFormFieldOption: the option ends at thruDate (now by default). */
function endOption({ formId, fieldName, sequenceNum, thruDate }) {
  const option = ec.entity.find(OPTION).condition({ formId, fieldName, sequenceNum }).one()
  if (!option) return refuse(`Option ${sequenceNum} of field ${fieldName} of DbForm ${formId} not found`)
  option.set('thruDate', dateOf(thruDate, nowMs()))
  option.update()
  return {}
}

/**
 * create#DbFormFieldSuccessor: a new field that succeeds fieldName, with its options, attributes and dependencies
 * copied: a draft that replaces it when published, or, with fromDate, started then with the old field ended then.
 * The old field's answers stay under its name, so a response reads as it was submitted.
 */
function createSuccessor(p) {
  const { formId, fieldName } = p
  const form = formOf(formId)
  if (!form) return refuse(`DbForm ${formId} not found`)
  if (form.isDated !== 'Y') return refuse(`DbForm ${formId} is not dated: set#DbFormDated first`)
  const old = ec.entity.find(FIELD).condition({ formId, fieldName }).one()
  if (!old) return refuse(`Field ${fieldName} of DbForm ${formId} not found`)
  const taken = new Set(fieldsOf(formId).map((field) => field.fieldName))
  let newFieldName = p.newFieldName
  if (newFieldName && taken.has(newFieldName)) return refuse(`DbForm ${formId} already has a field ${newFieldName}`)
  if (!newFieldName) {
    const base = String(fieldName).replace(/_\d+$/, '')
    for (let n = 2; !newFieldName; n++) if (!taken.has(base + '_' + n)) newFieldName = base + '_' + n
  }
  const from = p.fromDate != null && p.fromDate !== '' ? dateOf(p.fromDate) : null
  const field = ec.entity.makeValue(FIELD)
  const copy = {}
  for (const name of ['condition', 'entryName', 'title', 'tooltip', 'fieldTypeEnumId', 'layoutSequenceNum', 'isRequired',
    'printPageNumber', 'printTop', 'printLeft', 'printBottom', 'printRight', 'printWidth', 'printHeight', 'printTextAlign',
    'printVerticalAlign', 'printFontSize', 'printFontFamily']) {
    copy[name] = p[name] !== undefined && p[name] !== null ? p[name] : old[name]
  }
  field.setFields(copy, true, null, null)
  field.setFields({ formId, fieldName: newFieldName, replacesFieldName: fieldName, fromDate: from }, true, null, null)
  field.create()
  if (p.copyOptions !== false && p.copyOptions !== 'false') {
    for (const option of ec.entity.find(OPTION).condition({ formId, fieldName }).list()) {
      if (!inEffectAt(option, from ? from.getTime() : nowMs(), false)) continue
      const row = ec.entity.makeValue(OPTION)
      row.setFields({ formId, fieldName: newFieldName, sequenceNum: option.sequenceNum, keyValue: option.keyValue,
        text: option.text, fromDate: from }, true, null, null)
      row.create()
    }
  }
  for (const attribute of ec.entity.find(ATTRIBUTE).condition({ formId, fieldName }).list()) {
    const row = ec.entity.makeValue(ATTRIBUTE)
    row.setFields({ formId, fieldName: newFieldName, attributeName: attribute.attributeName, value: attribute.value }, true, null, null)
    row.create()
  }
  const at = from ? from.getTime() : nowMs()
  for (const dep of dependenciesOf(formId)) {
    if (!inEffectAt(dep, at, false) || (dep.sourceFieldName !== fieldName && dep.targetFieldName !== fieldName)) continue
    const row = ec.entity.makeValue(DEPENDENCY)
    row.setFields({ formId, sourceFieldName: dep.sourceFieldName === fieldName ? newFieldName : dep.sourceFieldName,
      operator: dep.operator, value: dep.value, effect: dep.effect, fromDate: from,
      targetFieldName: dep.targetFieldName === fieldName ? newFieldName : dep.targetFieldName }, true, null, null)
    row.set('dependencySeqId', nextDependencySeqId(formId))
    row.create()
  }
  if (from) {
    const thru = ms(old.thruDate)
    if (thru == null || thru > from.getTime()) {
      old.set('thruDate', from)
      old.update()
    }
    for (const dep of dependenciesOf(formId)) {
      if (dep.sourceFieldName !== fieldName && dep.targetFieldName !== fieldName) continue
      const end = ms(dep.thruDate)
      if (end == null || end > from.getTime()) {
        dep.set('thruDate', from)
        dep.update()
      }
    }
  }
  if (!checkDependencies(formId)) return {}
  return { newFieldName }
}

function nextDependencySeqId(formId) {
  let max = 0
  for (const row of dependenciesOf(formId)) {
    const number = Number(row.dependencySeqId)
    if (!Number.isNaN(number) && number > max) max = number
  }
  return String(max + 1).padStart(5, '0')
}

/**
 * store#DbFormFieldDependency: create, or with dependencySeqId update, a dependency. The operator and effect are from
 * the allowlist, the value is data (no ${}), both fields are the form's and differ, and the form's dependencies have
 * no cycle and no field two rules of one effect. On a dated form a new row with no fromDate is a draft.
 */
function storeDependency(p) {
  const { formId } = p
  const form = formOf(formId)
  if (!form) return refuse(`DbForm ${formId} not found`)
  const operator = p.operator || 'equals'
  if (!OPERATORS.includes(operator)) return refuse(`Operator ${operator} is not one of ${OPERATORS.join(', ')}`)
  if (!EFFECTS.includes(p.effect)) return refuse(`Effect ${p.effect} is not one of ${EFFECTS.join(', ')}`)
  if (String(p.value ?? '').includes('${')) return refuse('A dependency value is a value, not an expression')
  if ((operator === 'equals' || operator === 'not-equals') && String(p.value ?? '').trim() === '') {
    return refuse(`Operator ${operator} needs a value: use empty or not-empty for a field with no value`)
  }
  if (!p.sourceFieldName || !p.targetFieldName) return refuse('A dependency names a source field and a target field')
  if (p.sourceFieldName === p.targetFieldName) return refuse(`Field ${p.targetFieldName} cannot depend on itself`)
  const names = new Set(fieldsOf(formId).map((field) => field.fieldName))
  for (const name of [p.sourceFieldName, p.targetFieldName]) {
    if (!names.has(name)) return refuse(`DbForm ${formId} has no field ${name}`)
  }
  let row = null
  if (p.dependencySeqId) {
    row = ec.entity.find(DEPENDENCY).condition({ formId, dependencySeqId: p.dependencySeqId }).one()
    if (!row) return refuse(`Dependency ${p.dependencySeqId} of DbForm ${formId} not found`)
  } else {
    row = ec.entity.makeValue(DEPENDENCY)
    row.setFields({ formId, dependencySeqId: nextDependencySeqId(formId) }, true, null, null)
  }
  row.setFields({ sourceFieldName: p.sourceFieldName, operator, value: p.value ?? null, targetFieldName: p.targetFieldName,
    effect: p.effect }, true, null, null)
  if (p.fromDate !== undefined) row.set('fromDate', p.fromDate === '' || p.fromDate == null ? null : dateOf(p.fromDate))
  if (p.thruDate !== undefined) row.set('thruDate', p.thruDate === '' || p.thruDate == null ? null : dateOf(p.thruDate))
  if (p.dependencySeqId) row.update()
  else row.create()
  if (!checkDependencies(formId)) return {}
  return { dependencySeqId: row.dependencySeqId }
}

/** end#DbFormFieldDependency: the dependency ends at thruDate (now by default). */
function endDependency({ formId, dependencySeqId, thruDate }) {
  const row = ec.entity.find(DEPENDENCY).condition({ formId, dependencySeqId }).one()
  if (!row) return refuse(`Dependency ${dependencySeqId} of DbForm ${formId} not found`)
  row.set('thruDate', dateOf(thruDate, nowMs()))
  row.update()
  return {}
}

/**
 * evaluate#DbFormDependencies: the form as of asOfDate (its drafts too when includeDrafts) against submitted values:
 * which fields show, which are required (isRequired or a require dependency, while shown) and which of those are
 * empty. A hidden field counts as empty for the fields that depend on it, as in the browser.
 */
function evaluate({ formId, values, asOfDate, includeDrafts }) {
  const node = ec.screen.getFormNode('DbForm#' + formId, asOfDate ?? null, includeDrafts === true || includeDrafts === 'true')
  const submitted = values || {}
  const fields = node.children('field').map((field) => {
    const sub = field.first('default-field') || field.first('conditional-field')
    return {
      name: field.attribute('name'),
      required: !!sub && sub.attribute('required') === 'true',
      show: sub ? sub.first('visible-when') : null,
      require: sub ? sub.first('require-when') : null,
    }
  })
  const visible = new Map(fields.map((field) => [field.name, true]))
  const valueOf = (name) => (visible.get(name) === false ? [] : submitted[name])
  const ruleHolds = (rule) => holds(rule.attribute('operator') || 'equals', rule.attribute('value'), valueOf(rule.attribute('field')))
  // a field hidden can hide the fields after it: repeat until nothing changes (the rules have no cycle)
  for (let pass = 0; pass <= fields.length; pass++) {
    let changed = false
    for (const field of fields) {
      const shown = !field.show || ruleHolds(field.show)
      if (visible.get(field.name) !== shown) {
        visible.set(field.name, shown)
        changed = true
      }
    }
    if (!changed) break
  }
  const visibleFieldNames = []
  const hiddenFieldNames = []
  const requiredFieldNames = []
  const missingFieldNames = []
  for (const field of fields) {
    if (!visible.get(field.name)) {
      hiddenFieldNames.push(field.name)
      continue
    }
    visibleFieldNames.push(field.name)
    if (field.required || (field.require && ruleHolds(field.require))) {
      requiredFieldNames.push(field.name)
      if (!holds('not-empty', '', submitted[field.name])) missingFieldNames.push(field.name)
    }
  }
  return { visibleFieldNames, hiddenFieldNames, requiredFieldNames, missingFieldNames }
}

/**
 * The checks of update#DbFormField and delete#DbFormField on a dated form: a published field keeps its type (a
 * successor field takes a new one), its dates and the field it replaces change only through publish#DbForm,
 * end#DbFormField and create#DbFormFieldSuccessor, and an answered field is ended, not deleted. Returns whether to go on.
 */
function checkFieldChange({ formId, fieldName, fieldTypeEnumId, deleting, fromDate, thruDate, replacesFieldName }) {
  const form = formOf(formId)
  if (!form || form.isDated !== 'Y') return { dated: false }
  const field = ec.entity.find(FIELD).condition({ formId, fieldName }).one()
  if (!field) {
    refuse(`Field ${fieldName} of DbForm ${formId} not found`)
    return { dated: true }
  }
  if (deleting) {
    if (isAnswered(formId, fieldName)) refuse(`Field ${fieldName} has answers: end it (end#DbFormField) rather than delete it`)
  } else if (fieldTypeEnumId && fieldTypeEnumId !== field.fieldTypeEnumId && field.fromDate != null) {
    refuse(`Field ${fieldName} is published: a new type is a successor field (create#DbFormFieldSuccessor)`)
  } else if ((ms(fromDate) != null && ms(fromDate) !== ms(field.fromDate)) || (ms(thruDate) != null && ms(thruDate) !== ms(field.thruDate))
    || (replacesFieldName && replacesFieldName !== field.replacesFieldName)) {
    refuse(`Field ${fieldName} of a dated form changes its dates through publish#DbForm, end#DbFormField and create#DbFormFieldSuccessor`)
  }
  return { dated: true }
}

const operations = { setDated, publish, endField, endOption, deleteOption, createSuccessor, storeDependency, endDependency,
  evaluate, checkFieldChange }

function execute(op, parameters) {
  if (!operations[op]) throw new Error('Unknown DbForm operation ' + op)
  return operations[op](parameters || {}) || {}
}

;({ execute })
