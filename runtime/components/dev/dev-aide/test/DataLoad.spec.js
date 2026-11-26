// Load the changed records, three ways (plans/design-dev-aide.md 12, src/data_load.rs): on a throw-away plain root, the
// first load writes every record and keeps the file's text as the baseline; a changed file is planned against that
// baseline and the database, a record the database changed too is a conflict a load refuses without a choice and
// writes nothing for, and the choices settle it (Keep database, Take file deleting a removed record); a load whose
// baseline was lost finds what it wrote already applied; a plan made on another text is refused; an encrypted field
// never shows in a conflict. The pure rules are the crate's (data_load_tests.rs).
// Run: ./moq test dev-aide --spec DataLoad
const fixture = () => ec.resource.script("component://dev-aide/test/GuidedFixture.js")
const ENTITY = "org.moqui.devaide.EntityServices."
const ROOT = "dev-aide-data-load-spec"
const PATH = "data/SpecLoad.xml"
// a suffix keeps each run's records apart on a database kept between runs
const RUN = Date.now().toString(36).slice(-5).toUpperCase()
const TYPE = "SpecLoad" + RUN
const id = letter => "SpecLoad" + letter + RUN
const file = (records, type) =>
    '<entity-facade-xml type="' +
    (type || "seed") +
    '">\n    <moqui.basic.EnumerationType enumTypeId="' +
    TYPE +
    '" description="Spec load"/>\n' +
    records
        .map(
            ([letter, description]) =>
                '    <moqui.basic.Enumeration enumId="' +
                id(letter) +
                '" enumTypeId="' +
                TYPE +
                '" description="' +
                description +
                '"/>\n'
        )
        .join("") +
    "</entity-facade-xml>\n"
const V1 = file([
    ["A", "One"],
    ["B", "Two"],
    ["C", "Three"]
])
const V2 = file([
    ["A", "Uno"],
    ["B", "Dos"],
    ["D", "Four"]
])

const call = (name, parameters) => {
    try {
        const out =
            ec.service
                .sync()
                .name(ENTITY + name)
                .parameters(Object.assign({rootId: ROOT, path: PATH}, parameters || {}))
                .call() || {}
        const errors = ec.message.getErrorsString()
        ec.message.clearErrors()
        return {out, errors}
    } catch (error) {
        const errors = ec.message.getErrorsString() || String(error.message)
        ec.message.clearErrors()
        return {out: {}, errors}
    }
}
const asSystem = body => {
    ec.artifactExecution.disableAuthz()
    try {
        return body()
    } finally {
        ec.artifactExecution.enableAuthz()
    }
}
const description = letter =>
    asSystem(() => {
        const row = ec.entity.find("moqui.basic.Enumeration").condition("enumId", id(letter)).one()
        return row ? row.description : null
    })
const byKey = (plan, letter) =>
    (plan.records || []).find(record => record.key === "moqui.basic.Enumeration[enumId=" + id(letter) + "]") || {}

spec({
    stepwise: true,
    setupSpec() {
        login("john.doe")
        fixture().setup(ROOT, {[PATH]: V1})
    },
    cleanupSpec() {
        asSystem(() => {
            ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", TYPE).deleteAll()
            ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", TYPE).deleteAll()
            ec.entity.find("moqui.dev.DevAideTaskSource").condition("taskSourceId", id("TS")).deleteAll()
            ec.entity.find("moqui.dev.DevDataLoadBaseline").condition("rootId", ROOT).deleteAll()
        })
        fixture().cleanup(ROOT, [PATH])
    }
})

feature("the first load writes every record and keeps the file's text as the baseline", {
    when() {
        const plan = call("plan#DataLoad").out
        const loaded = call("load#ChangedRecords", {baseHash: plan.baseHash, fileHash: plan.fileHash})
        const again = call("plan#DataLoad").out
        return {plan, loaded, again, a: description("A")}
    },
    then({plan, loaded, again, a}) {
        plan.baseFrom == "none"
        plan.counts.write == 4
        loaded.errors == ""
        loaded.out.written == 4
        a == "One"
        // the next plan is against what was loaded
        again.baseFrom == "baseline"
        again.counts.unchanged == 4
        again.counts.write == 0
    }
})

feature("a changed file is planned three ways, a conflict needs a choice, and the choices settle it", {
    when() {
        fixture().write(ROOT, PATH, V2)
        // the database changed B since the load
        asSystem(() => {
            const row = ec.entity.find("moqui.basic.Enumeration").condition("enumId", id("B")).one()
            row.description = "By hand"
            row.update()
        })
        const plan = call("plan#DataLoad").out
        const hashes = {baseHash: plan.baseHash, fileHash: plan.fileHash}
        const unchosen = call("load#ChangedRecords", hashes)
        const untouched = description("A")
        const choices = {}
        choices[byKey(plan, "B").key] = "database"
        choices[byKey(plan, "C").key] = "file"
        const loaded = call("load#ChangedRecords", Object.assign({choices: JSON.stringify(choices)}, hashes))
        const after = call("plan#DataLoad").out
        return {plan, unchosen, untouched, loaded, after, rows: ["A", "B", "C", "D"].map(description)}
    },
    then({plan, unchosen, untouched, loaded, after, rows}) {
        plan.counts.write == 2
        plan.counts.conflict == 1
        plan.counts.remove == 1
        byKey(plan, "A").action == "write"
        byKey(plan, "D").action == "write"
        byKey(plan, "C").action == "remove"
        byKey(plan, "B").action == "conflict"
        byKey(plan, "B").change == "changed"
        byKey(plan, "B")
            .fields.map(field => [field.name, field.base, field.file, field.db].join("|"))
            .join(";") == "description|Two|Dos|By hand"
        // without a choice for B the load is refused, and nothing of it is written
        unchosen.errors.includes("conflict")
        untouched == "One"
        loaded.errors == ""
        loaded.out.written == 2
        loaded.out.deleted == 1
        loaded.out.kept == 1
        JSON.stringify(rows) == JSON.stringify(["Uno", "By hand", null, "Four"])
        // B kept as the database has it: the file is the baseline now, so it is unchanged in the next plan
        after.counts.write == 0
        after.counts.conflict == 0
        after.counts.unchanged == 4
    }
})

feature("a load whose baseline was lost finds what it wrote already applied", {
    when() {
        asSystem(() => ec.entity.find("moqui.dev.DevDataLoadBaseline").condition("rootId", ROOT).deleteAll())
        return {plan: call("plan#DataLoad").out}
    },
    then({plan}) {
        plan.baseFrom == "none"
        plan.counts.applied == 3
        plan.counts.write == 0
        // the database's B is not the file's, so with no base it is a conflict, never written by default
        byKey(plan, "B").action == "conflict"
        byKey(plan, "B").change == "added"
    }
})

feature("a plan made on another text of the file is refused", {
    when() {
        return {refused: call("load#ChangedRecords", {fileHash: "not-the-plan"}), b: description("B")}
    },
    then({refused, b}) {
        refused.errors.includes("changed since the plan")
        b == "By hand"
    }
})

feature("an encrypted field is never shown in a conflict", {
    when() {
        asSystem(() => {
            const row = ec.entity.makeValue("moqui.dev.DevAideTaskSource")
            row.setFields({taskSourceId: id("TS"), kind: "moqui-rest", label: "Spec", token: "db-token-" + RUN}, true, null, null)
            row.createOrUpdate()
        })
        const text = V2.replace(
            "</entity-facade-xml>",
            '    <moqui.dev.DevAideTaskSource taskSourceId="' +
                id("TS") +
                '" kind="moqui-rest" label="Spec" token="file-token-' +
                RUN +
                '"/>\n</entity-facade-xml>'
        )
        const plan = call("plan#DataLoad", {text}).out
        const source = (plan.records || []).find(record => record.entity == "moqui.dev.DevAideTaskSource") || {}
        return {source, shown: JSON.stringify(plan.records || [])}
    },
    then({source, shown}) {
        source.action == "conflict"
        source.fields.map(field => [field.name, field.base == null, field.file, field.db].join("|")).join(";") ==
            "token|true|(hidden)|(hidden)"
        !shown.includes("token-" + RUN)
    }
})
