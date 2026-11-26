// Load the changed records, three ways (plans/archive/design-dev-aide.md 12, src/data_load.rs): on a throw-away plain root, the
// first load writes every record and keeps the file's text as the baseline; a changed file is planned against that
// baseline and the database, a record the database changed too is a conflict a load refuses without a choice and
// writes nothing for, and the choices settle it (Keep database, Take file deleting a removed record); a load whose
// baseline was lost finds what it wrote already applied; a load needs the plan's hashes and refuses another text; a
// password or encrypted field is neither compared nor shown, and for a reader of Dev AIDE's view tier a credential
// entity's record only differs (the plan) or is not checked (the data check); a removal left without a choice holds
// the baseline, and removed records are deleted the last first, so a type goes after its values; a load that passed
// over an element keeps the
// baseline; a date and a time stamp compare with the database across a change of the user's time zone; the terminal's
// Load the changed records loads when nothing needs a choice and names the conflicts otherwise. The pure rules
// are the crate's (data_load_tests.rs).
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

const DATES = "data/SpecDates.xml"
const READER = "spec.load.reader"

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
        asSystem(() => {
            const make = (entity, fields) => {
                const row = ec.entity.makeValue(entity)
                row.setFields(fields, true, null, null)
                row.createOrUpdate()
            }
            // a reader of Dev AIDE's view tier, as WorkspaceReader's
            const since = new Date(Date.now() - 60000)
            make("moqui.security.UserGroup", {userGroupId: "SPEC_LOAD_READERS", description: "Dev AIDE data load spec"})
            make("moqui.security.UserGroupPermission", {
                userGroupId: "SPEC_LOAD_READERS",
                userPermissionId: "DEV_AIDE_VIEW",
                fromDate: since
            })
            make("moqui.security.ArtifactAuthz", {
                artifactAuthzId: "SPEC_LOAD_READERS_APP",
                artifactGroupId: "DEV_AIDE_APP",
                userGroupId: "SPEC_LOAD_READERS",
                authzTypeEnumId: "AUTHZT_ALLOW",
                authzActionEnumId: "AUTHZA_VIEW"
            })
            make("moqui.security.UserAccount", {userId: "specLoadReader", username: READER, userFullName: "Spec Load Reader"})
            make("moqui.security.UserGroupMember", {userGroupId: "SPEC_LOAD_READERS", userId: "specLoadReader", fromDate: since})
        })
        login("john.doe")
        fixture().setup(ROOT, {[PATH]: V1})
    },
    cleanupSpec() {
        asSystem(() => {
            ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", TYPE).deleteAll()
            ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", TYPE).deleteAll()
            ec.entity.find("moqui.dev.DevAideTaskSource").condition("taskSourceId", id("TS")).deleteAll()
            ec.entity.find("moqui.dev.DevDataLoadBaseline").condition("rootId", ROOT).deleteAll()
            ec.entity.find("moqui.test.TestEntity").condition("testId", id("T")).deleteAll()
        })
        ec.user.internalLoginUser("john.doe")
        fixture().cleanup(ROOT, [PATH, DATES])
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

feature("a load needs the plan's hashes, and refuses a plan made on another text of the file", {
    when() {
        const plan = call("plan#DataLoad").out
        return {
            bare: call("load#ChangedRecords"),
            refused: call("load#ChangedRecords", {baseHash: plan.baseHash, fileHash: "not-the-plan"}),
            b: description("B")
        }
    },
    then({bare, refused, b}) {
        bare.errors.includes("for field Base Hash")
        refused.errors.includes("changed since the plan")
        b == "By hand"
    }
})

const SOURCE = (label, token) =>
    V2.replace(
        "</entity-facade-xml>",
        '    <moqui.dev.DevAideTaskSource taskSourceId="' +
            id("TS") +
            '" kind="moqui-rest" label="' +
            label +
            '" token="' +
            token +
            '"/>\n</entity-facade-xml>'
    )
const sourceOf = plan => (plan.records || []).find(record => record.entity == "moqui.dev.DevAideTaskSource") || {}

feature("an encrypted field is neither compared nor shown, so a plan confirms no guess of it", {
    when() {
        asSystem(() => {
            const row = ec.entity.makeValue("moqui.dev.DevAideTaskSource")
            row.setFields({taskSourceId: id("TS"), kind: "moqui-rest", label: "Spec", token: "db-token-" + RUN}, true, null, null)
            row.createOrUpdate()
        })
        // the token right or wrong: the same answer
        const wrong = call("plan#DataLoad", {text: SOURCE("Spec", "guess-token-" + RUN)}).out
        const right = call("plan#DataLoad", {text: SOURCE("Spec", "db-token-" + RUN)}).out
        const relabelled = call("plan#DataLoad", {text: SOURCE("Other", "guess-token-" + RUN)}).out
        return {
            wrong: sourceOf(wrong),
            right: sourceOf(right),
            relabelled: sourceOf(relabelled),
            shown: JSON.stringify([wrong, right, relabelled])
        }
    },
    then({wrong, right, relabelled, shown}) {
        wrong.action == "applied"
        right.action == "applied"
        relabelled.action == "conflict"
        relabelled.fields.map(field => field.name).join(",") == "label"
        !shown.includes("token-" + RUN)
    }
})

feature("a reader of the view tier sees only that a credential entity's record differs, in the plan and the data check", {
    when() {
        ec.user.internalLoginUser(READER)
        const plan = call("plan#DataLoad", {text: SOURCE("Other", "guess-token-" + RUN)}).out
        const checked = call("check#DataFile", {text: SOURCE("Other", "guess-token-" + RUN)})
        const rightGuess = call("check#DataFile", {text: SOURCE("Spec", "db-token-" + RUN)})
        // a password hash is never compared: a guess at it gives no message, right or wrong
        const account = call("check#DataFile", {
            text: '<entity-facade-xml>\n    <moqui.security.UserAccount userId="EX_JOHN_DOE" currentPassword="guess"/>\n</entity-facade-xml>\n'
        })
        ec.user.internalLoginUser("john.doe")
        return {source: sourceOf(plan), plan: JSON.stringify(plan), checked, rightGuess, account}
    },
    then({source, plan, checked, rightGuess, account}) {
        source.action == "differs"
        source.fields.length == 0
        !plan.includes("Other") && !plan.includes("token-" + RUN)
        checked.errors == ""
        // not checked at all: the same one line whatever the database has
        checked.out.messages.includes("Entity moqui.dev.DevAideTaskSource: not checked for you")
        JSON.stringify(checked.out.messages) == JSON.stringify(rightGuess.out.messages)
        checked.out.fieldsChecked == rightGuess.out.fieldsChecked
        !JSON.stringify(checked.out).includes("token-" + RUN)
        !JSON.stringify(checked.out).includes("[Spec]")
        account.errors == ""
        !JSON.stringify(account.out).includes("currentPassword")
    }
})

feature("removed records are deleted the last first, so a type goes after its values", {
    when() {
        asSystem(() => ec.entity.find("moqui.dev.DevAideTaskSource").condition("taskSourceId", id("TS")).deleteAll())
        // a baseline of V2 first: B as the database has it
        const settle = call("plan#DataLoad").out
        call("load#ChangedRecords", {
            baseHash: settle.baseHash,
            fileHash: settle.fileHash,
            choices: JSON.stringify({[byKey(settle, "B").key]: "database"})
        })
        fixture().write(ROOT, PATH, '<entity-facade-xml type="seed">\n</entity-facade-xml>\n')
        // the removals left without a choice: kept, and the baseline holds so they are offered again
        const first = call("plan#DataLoad").out
        const unchosen = call("load#ChangedRecords", {
            baseHash: first.baseHash,
            fileHash: first.fileHash,
            choices: JSON.stringify({[byKey(first, "B").key]: "database"})
        })
        const plan = call("plan#DataLoad").out
        const choices = {}
        for (const record of plan.records || []) choices[record.key] = "file"
        const loaded = call("load#ChangedRecords", {
            baseHash: plan.baseHash,
            fileHash: plan.fileHash,
            choices: JSON.stringify(choices)
        })
        const type = asSystem(() => ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", TYPE).count())
        return {first, unchosen, plan, loaded, rows: ["A", "B", "D"].map(description), type}
    },
    then({first, unchosen, plan, loaded, rows, type}) {
        unchosen.errors == ""
        unchosen.out.deleted == 0
        unchosen.out.baselineMoved == false
        unchosen.out.baselineNote.includes("had no choice")
        plan.baseHash == first.baseHash
        plan.counts.remove == 3
        plan.counts.conflict == 1
        loaded.errors == ""
        loaded.out.deleted == 4
        JSON.stringify(rows) == JSON.stringify([null, null, null])
        type == 0
    }
})

feature("a load that passed over an element writes the rest and keeps the baseline, so the element's change is not lost", {
    when() {
        const text = file([["E", "Five"]]).replace(
            "</entity-facade-xml>",
            '    <moqui.basic.NoSuchEntity spec="1"/>\n</entity-facade-xml>'
        )
        fixture().write(ROOT, PATH, text)
        const plan = call("plan#DataLoad").out
        const loaded = call("load#ChangedRecords", {baseHash: plan.baseHash, fileHash: plan.fileHash})
        const after = call("plan#DataLoad").out
        return {plan, loaded, after, e: description("E")}
    },
    then({plan, loaded, after, e}) {
        plan.skipped.length == 1
        loaded.errors == ""
        loaded.out.written == 2
        e == "Five"
        loaded.out.baselineMoved == false
        loaded.out.baselineNote.includes("passed over")
        // compared with the same base again: what was written is applied, the element still passed over
        after.baseHash == plan.baseHash
        after.counts.applied == 2
        after.skipped.length == 1
    }
})

feature("a date and a time stamp compare with the database across a change of the user's time zone", {
    when() {
        const zone = ec.user.timeZone
        const dated = medium =>
            '<entity-facade-xml type="seed">\n    <moqui.test.TestEntity testId="' +
            id("T") +
            '" testMedium="' +
            medium +
            '" testDate="2026-03-01" testDateTime="2026-03-01 23:15:30.123"/>\n</entity-facade-xml>\n'
        const dates = (name, parameters) => call(name, Object.assign({path: DATES}, parameters || {}))
        fixture().write(ROOT, DATES, dated("one"))
        try {
            ec.user.setTimeZone("America/New_York")
            const first = dates("plan#DataLoad").out
            const loaded = dates("load#ChangedRecords", {baseHash: first.baseHash, fileHash: first.fileHash})
            const same = dates("plan#DataLoad").out
            // another user's zone: the file's text is read in it, the base's too
            ec.user.setTimeZone("Asia/Tokyo")
            fixture().write(ROOT, DATES, dated("two"))
            const changed = dates("plan#DataLoad").out
            return {first, loaded, same, changed}
        } finally {
            ec.user.setTimeZone(zone)
        }
    },
    then({first, loaded, same, changed}) {
        first.counts.write == 1
        loaded.errors == ""
        loaded.out.written == 1
        same.counts.unchanged == 1
        // the database still has the base's values, millisecond and date included: a write, not a conflict
        changed.counts.write == 1
        changed.counts.conflict == 0
        changed.records[0].fields.map(field => field.name).join(",") == "testMedium"
    }
})

feature("the terminal's Load the changed records loads when nothing needs a choice, and names a conflict for the browser", {
    when() {
        const tui = parameters =>
            ec.screen
                .makeTest()
                .rootScreen("component://dev-aide/screen/DevAide.xml")
                .render("WorkspaceTui/loadChangedRecords", parameters, "post")
        // the file as the database has it, loaded so the baseline moves to it
        fixture().write(ROOT, PATH, file([["E", "Five"]]))
        const settle = call("plan#DataLoad").out
        call("load#ChangedRecords", {baseHash: settle.baseHash, fileHash: settle.fileHash})
        // E changed in the file, the database still the base's: loaded
        fixture().write(ROOT, PATH, file([["E", "Cinq"]]))
        const loaded = tui({rootId: ROOT, path: PATH})
        const e = description("E")
        // the database changes E too: a conflict, not settled here
        asSystem(() => {
            const row = ec.entity.find("moqui.basic.Enumeration").condition("enumId", id("E")).one()
            row.description = "By hand"
            row.update()
        })
        fixture().write(ROOT, PATH, file([["E", "Fünf"]]))
        const named = tui({rootId: ROOT, path: PATH})
        return {loaded, e, named, after: description("E")}
    },
    then({loaded, e, named, after}) {
        loaded.status == 302
        loaded.errorMessages.length == 0
        e == "Cinq"
        named.status == 302
        after == "By hand"
    }
})
