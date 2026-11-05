// Dated DbForms and their dependencies (docs/ui/Db-Forms.md; org.moqui.impl.ScreenServices, DbForms.js): an undated
// form keeps update#DbFormField's delete and recreate; on a dated form a draft is left out until it is published, an
// edit is in place and audit-logged, a published field keeps its type and a successor field from a date takes a new
// one, an answered field or option is ended and never deleted, and a response reads the form as of its date. A
// dependency's operator is from the allowlist, an equals has a value, a cycle is refused, and evaluate#DbFormDependencies checks a submit on
// the server. The renders of the dependencies are crates/moqui-core screen::tests::db_forms.
// Run: ./moq test framework --spec DbForms
const S = 'org.moqui.impl.ScreenServices.'
const FORM = 'CSF_SPEC'
const LEGACY = 'CSF_SPEC_LEGACY'
const ENTITIES = ['moqui.screen.form.FormResponseAnswer', 'moqui.screen.form.FormResponse', 'moqui.screen.form.DbFormFieldDependency',
    'moqui.screen.form.DbFormFieldAttribute', 'moqui.screen.form.DbFormFieldOption', 'moqui.screen.form.DbFormField', 'moqui.screen.form.DbForm']

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// a service's out map, with the errors it added (cleared after)
function invoke(name, parameters) {
    let out = {}
    let thrown = ''
    try {
        out = sys(() => ec.service.sync().name(name.includes('#') && !name.includes('.') ? S + name : name)
            .parameters(parameters || {}).call()) || {}
    } catch (err) {
        thrown = String(err.message || err)
    }
    const errors = thrown + (ec.message.hasError() ? ec.message.errorsString : '')
    ec.message.clearErrors()
    return Object.assign({ errors }, out)
}
const fieldNames = (formId, asOf, drafts) => sys(() => ec.screen.getFormNode('DbForm#' + formId, asOf ?? null, !!drafts)
    .children('field').map((field) => field.attribute('name')))
const optionKeys = (formId, fieldName) => {
    const field = sys(() => ec.screen.getFormNode('DbForm#' + formId)).children('field').find((f) => f.attribute('name') === fieldName)
    return field.first('default-field').first('drop-down').children('option').map((option) => option.attribute('key'))
}
const audit = (fieldName, changedFieldName) => sys(() => Array.from(ec.entity.find('moqui.entity.EntityAuditLog')
    .condition({ changedEntityName: 'moqui.screen.form.DbFormField', pkPrimaryValue: FORM, pkSecondaryValue: fieldName, changedFieldName })
    .list()))
function clean() {
    sys(() => {
        for (const entity of ENTITIES) ec.entity.find(entity).condition('formId', 'in', [FORM, LEGACY]).deleteAll()
    })
}
function make(entity, fields) {
    sys(() => {
        const value = ec.entity.makeValue(entity)
        value.setFields(fields, true, null, null)
        value.create()
    })
}

spec({
    stepwise: true,
    setupSpec() {
        clean()
        for (const formId of [FORM, LEGACY]) make('moqui.screen.form.DbForm', { formId })
        const field = (formId, fieldName, title, type, seq) => make('moqui.screen.form.DbFormField',
            { formId, fieldName, title, fieldTypeEnumId: type, layoutSequenceNum: seq })
        field(FORM, 'kind', 'Kind', 'DBFFT_drop-down', 1)
        field(FORM, 'serial', 'Serial', 'DBFFT_text-line', 2)
        field(FORM, 'q1', 'Question one', 'DBFFT_text-line', 3)
        for (const [seq, key] of [[1, 'A'], [2, 'B'], [3, 'C']]) {
            make('moqui.screen.form.DbFormFieldOption', { formId: FORM, fieldName: 'kind', sequenceNum: seq, keyValue: key, text: key })
        }
        field(LEGACY, 'oldTitle', 'Old Title', 'DBFFT_text-line', 1)
    },
    cleanupSpec() {
        clean()
    },
})

feature('an undated form keeps update#DbFormField: the field is made again, named from its title', {
    when() {
        const updated = invoke('update#DbFormField', { formId: LEGACY, fieldName: 'oldTitle', title: 'New Title', fieldTypeEnumId: 'DBFFT_text-line' })
        return { updated, names: fieldNames(LEGACY) }
    },
    then({ updated, names }) {
        updated.errors == ''
        names.length == 1
        names[0] == 'newTitle'
    },
})

feature('a dated form leaves a draft out until it is published, and logs its key fields', {
    when() {
        const dated = invoke('set#DbFormDated', { formId: FORM, fromDate: new Date(Date.now() - 2 * 86400000) })
        make('moqui.screen.form.DbFormField', { formId: FORM, fieldName: 'extra', title: 'Extra', fieldTypeEnumId: 'DBFFT_text-line', layoutSequenceNum: 4 })
        const before = fieldNames(FORM)
        const preview = fieldNames(FORM, null, true)
        const published = invoke('publish#DbForm', { formId: FORM, fieldNames: ['extra'] })
        const after = fieldNames(FORM)
        const edited = invoke('update#DbFormField', { formId: FORM, fieldName: 'serial', title: 'Serial number', isRequired: 'Y' })
        return { dated, before, preview, published, after, edited, titleLog: audit('serial', 'title'), fromLog: audit('extra', 'fromDate'),
            names: fieldNames(FORM) }
    },
    then({ dated, before, preview, published, after, edited, titleLog, fromLog, names }) {
        dated.errors == ''
        !before.includes('extra')
        preview.includes('extra')
        published.errors == ''
        published.publishedFieldNames.includes('extra')
        after.includes('extra')
        edited.errors == ''
        names.includes('serial')
        titleLog.some((row) => row.oldValueText == 'Serial' && row.newValueText == 'Serial number')
        fromLog.length >= 1
    },
})

let oldResponseId = null

feature('a successor field from a date takes a new type, and an earlier response reads the form as it was', {
    when() {
        const hourAgo = new Date(Date.now() - 3600000)
        const response = invoke('create#FormResponse', { formId: FORM, responseDate: hourAgo, responseMap: { kind: 'A', q1: 'first answer' } })
        oldResponseId = response.formResponseId
        const retyped = invoke('update#DbFormField', { formId: FORM, fieldName: 'q1', fieldTypeEnumId: 'DBFFT_text-area' })
        const successor = invoke('create#DbFormFieldSuccessor', { formId: FORM, fieldName: 'q1', title: 'Question one, reworded',
            fieldTypeEnumId: 'DBFFT_text-area', fromDate: new Date(Date.now() - 60000) })
        const read = invoke('get#FormResponse', { formResponseId: oldResponseId })
        return { response, retyped, successor, now: fieldNames(FORM), then: fieldNames(FORM, hourAgo), read }
    },
    then({ response, retyped, successor, now, then, read }) {
        response.errors == ''
        retyped.errors.includes('successor')
        successor.errors == ''
        successor.newFieldName == 'q1_2'
        now.includes('q1_2') && !now.includes('q1')
        then.includes('q1') && !then.includes('q1_2')
        read.responseMap.q1 == 'first answer'
        read.dbFormFieldList.some((field) => field.fieldName == 'q1')
        !read.dbFormFieldList.some((field) => field.fieldName == 'q1_2')
    },
})

feature('an answered field or option is ended, never deleted', {
    when() {
        const deleteField = invoke('delete#DbFormField', { formId: FORM, fieldName: 'q1' })
        const deleteOption = invoke('delete#DbFormFieldOption', { formId: FORM, fieldName: 'kind', sequenceNum: 1 })
        const unanswered = invoke('delete#DbFormFieldOption', { formId: FORM, fieldName: 'kind', sequenceNum: 3 })
        const ended = invoke('end#DbFormFieldOption', { formId: FORM, fieldName: 'kind', sequenceNum: 1 })
        return { deleteField, deleteOption, unanswered, ended, keys: optionKeys(FORM, 'kind'),
            stillThere: sys(() => ec.entity.find('moqui.screen.form.DbFormField').condition({ formId: FORM, fieldName: 'q1' }).count()) }
    },
    then({ deleteField, deleteOption, unanswered, ended, keys, stillThere }) {
        deleteField.errors.includes('end it')
        stillThere == 1
        deleteOption.errors.includes('end it')
        unanswered.errors == ''
        ended.errors == ''
        keys.length == 1
        keys[0] == 'B'
    },
})

feature('a dependency takes an operator from the allowlist and no expression, and a cycle is refused', {
    when() {
        return {
            require: invoke('store#DbFormFieldDependency', { formId: FORM, sourceFieldName: 'kind', value: 'B', targetFieldName: 'serial', effect: 'require' }),
            show: invoke('store#DbFormFieldDependency', { formId: FORM, sourceFieldName: 'kind', operator: 'not-empty', targetFieldName: 'extra', effect: 'show' }),
            operator: invoke('store#DbFormFieldDependency', { formId: FORM, sourceFieldName: 'kind', operator: 'matches', value: '.*', targetFieldName: 'q1_2', effect: 'show' }),
            expression: invoke('store#DbFormFieldDependency', { formId: FORM, sourceFieldName: 'kind', value: '${ec.user.userId}', targetFieldName: 'q1_2', effect: 'show' }),
            blank: invoke('store#DbFormFieldDependency', { formId: FORM, sourceFieldName: 'kind', value: ' ', targetFieldName: 'q1_2', effect: 'show' }),
            twice: invoke('store#DbFormFieldDependency', { formId: FORM, sourceFieldName: 'q1_2', operator: 'not-empty', targetFieldName: 'serial', effect: 'require' }),
            cycle: invoke('store#DbFormFieldDependency', { formId: FORM, sourceFieldName: 'extra', operator: 'not-empty', targetFieldName: 'kind', effect: 'show' }),
            rows: sys(() => ec.entity.find('moqui.screen.form.DbFormFieldDependency').condition('formId', FORM).count()),
        }
    },
    then({ require, show, operator, expression, blank, twice, cycle, rows }) {
        require.errors == '' && require.dependencySeqId != null
        show.errors == ''
        operator.errors.includes('not one of')
        expression.errors.includes('not an expression')
        blank.errors.includes('needs a value')
        twice.errors.includes('already has a require')
        cycle.errors.includes('cycle')
        rows == 2
    },
})

feature('the server evaluates a submit against the dependencies as of a date', {
    when() {
        // the dependencies stored above are drafts of the dated form until published
        invoke('publish#DbForm', { formId: FORM })
        const check = (values) => invoke('evaluate#DbFormDependencies', { formId: FORM, values })
        return { b: check({ kind: 'B' }), bSerial: check({ kind: 'B', serial: 'SN-1' }), none: check({ kind: '' }) }
    },
    then({ b, bSerial, none }) {
        b.errors == ''
        b.requiredFieldNames.includes('serial')
        b.missingFieldNames.includes('serial')
        b.visibleFieldNames.includes('extra')
        bSerial.missingFieldNames.length == 0
        none.hiddenFieldNames.includes('extra')
        none.requiredFieldNames.includes('serial')
        none.missingFieldNames.includes('serial')
    },
})
