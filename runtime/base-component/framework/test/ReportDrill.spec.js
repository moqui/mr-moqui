// Drill and the semantic guards of business datasets (crates/moqui-report/src/drill.rs, semantics.rs; docs/tools/Monitoring.md
// "Drill" and "Semantic guards"; BOSS impl-insights P6, INS-03, INS-04). get#Drill names a saved panel and the buckets chosen,
// never a query: the engine runs the panel as the viewer, takes each bucket's predicate from its own result and answers the saved
// query narrowed by it, so the dataset's row filter is applied again, an "Other" bucket keeps its exact predicate, and a viewer
// who reads a dataset's aggregates and not its records gets no record. The guards refuse an amount summed across units, a ratio
// averaged, a balance summed across a dimension it is not added over, and a total of a bounded page of records. The fixtures are
// enumerations of a type of the spec's own, scoped to a user group (enumCode) by the framework's ReportUserGroups policy, with a
// region (relatedEnumId), a currency (parentEnumId) and an amount (sequenceNum). Run: ./moq test framework --spec ReportDrill
const R = "org.moqui.report.ReportServices."
const READER = "RDR_READER_USER"
const READER_NAME = "rdr.reader"
const policy = {rowFilterService: R + "get#UserGroupRowFilter", rowFilterParameters: {groupField: "enumCode"}}
const range = {from: "now-1d", thru: "now+1d"}

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
function make(entityName, fields) {
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// a call as the reader, with authorization on: its result, or its error
function asReader(name, parameters) {
    ec.user.internalLoginUser(READER_NAME)
    ec.artifactExecution.enableAuthz()
    try {
        return {out: call(name, parameters)}
    } catch (error) {
        ec.message.clearErrors()
        return {error: String(error.message)}
    } finally {
        login("john.doe")
    }
}
// a call as the steward (john.doe, ADMIN), refused or not
function asSteward(name, parameters) {
    try {
        return {out: call(name, parameters)}
    } catch (error) {
        ec.message.clearErrors()
        return {error: String(error.message)}
    }
}
const dataset = (id, extra) =>
    call(R + "save#Definition", {
        definition: {
            id,
            type: "dataset",
            title: id,
            userGroupId: "RDR_READER",
            body: Object.assign({documentType: "RdrFixture", timeField: "_timestamp", signal: "business"}, policy, extra)
        }
    })
// a library panel counting the dataset's records by region, the largest region and the rest as Other
const panel = (id, datasetId) =>
    call(R + "save#Definition", {
        definition: {
            id,
            type: "panel",
            title: id,
            userGroupId: "RDR_READER",
            body: {
                title: id,
                visualization: "bar",
                queries: [
                    {
                        datasetId,
                        aggregations: [
                            {
                                id: "region",
                                type: "terms",
                                role: "x-axis",
                                params: {field: "relatedEnumId", size: 1, other_bucket: true, other_bucket_key: "Other"}
                            }
                        ]
                    }
                ]
            }
        }
    })
const ids = rows =>
    Array.from(rows || [])
        .map(row => row.id)
        .sort()
        .join(",")
const sum = (datasetId, extra) => ({
    query: Object.assign({datasetId, aggregations: [{id: "total", type: "sum", params: {field: "sequenceNum"}}]}, range, extra)
})

spec({
    stepwise: true,
    setupSpec() {
        login("john.doe")
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.enableAuthz()
    }
})

feature("five fixture records of two scope groups, three datasets over them and a reader of one group", {
    when() {
        sys(() => {
            const from = new Date(Date.now() - 60000)
            make("moqui.basic.EnumerationType", {enumTypeId: "RdrFixture", description: "Report drill spec"})
            for (const [enumId, enumCode, relatedEnumId, parentEnumId, sequenceNum] of [
                ["RDR_E1", "RDR_A", "north", "USD", 10],
                ["RDR_E2", "RDR_A", "north", "USD", 20],
                ["RDR_E3", "RDR_A", "south", "EUR", 5],
                ["RDR_E4", "RDR_A", "east", "USD", 7],
                ["RDR_E5", "RDR_B", "north", "USD", 100]
            ])
                make("moqui.basic.Enumeration", {
                    enumId,
                    enumTypeId: "RdrFixture",
                    enumCode,
                    relatedEnumId,
                    parentEnumId,
                    sequenceNum,
                    description: enumId
                })
            make("moqui.entity.document.DataDocument", {
                dataDocumentId: "RdrFixture",
                indexName: "rdr_fixture",
                documentName: "RdrFixture",
                primaryEntityName: "moqui.basic.Enumeration"
            })
            for (const [fieldSeqId, fieldPath] of [
                ["01", "enumId"],
                ["02", "enumTypeId"],
                ["03", "enumCode"],
                ["04", "relatedEnumId"],
                ["05", "parentEnumId"],
                ["06", "sequenceNum"],
                ["07", "description"]
            ])
                make("moqui.entity.document.DataDocumentField", {
                    dataDocumentId: "RdrFixture",
                    fieldSeqId,
                    fieldPath,
                    sortable: fieldPath === "enumCode" ? "Y" : null
                })
            make("moqui.entity.document.DataDocumentCondition", {
                dataDocumentId: "RdrFixture",
                conditionSeqId: "01",
                fieldNameAlias: "enumTypeId",
                fieldValue: "RdrFixture"
            })
            make("moqui.entity.feed.DataFeed", {
                dataFeedId: "RdrFeed",
                dataFeedTypeEnumId: "DTFDTP_MAN_PULL",
                feedName: "RdrFeed"
            })
            make("moqui.entity.feed.DataFeedDocument", {dataFeedId: "RdrFeed", dataDocumentId: "RdrFixture"})
            make("moqui.security.UserAccount", {
                userId: READER,
                username: READER_NAME,
                userFullName: "Drill Reader",
                disabled: "N"
            })
            for (const userGroupId of ["RDR_A", "RDR_B", "RDR_READER"])
                make("moqui.security.UserGroup", {userGroupId, description: "Report drill spec " + userGroupId})
            for (const userGroupId of ["RDR_A", "RDR_READER"])
                make("moqui.security.UserGroupMember", {userGroupId, userId: READER, fromDate: from})
            // the reader: the reading operations and the three datasets (view), with their document, but no dataset's records
            // artifact (org.moqui.report.rows.<id>) and no annotation grant
            const grant = (artifactGroupId, names, authzActionEnumId) => {
                make("moqui.security.ArtifactGroup", {artifactGroupId, description: "Report drill spec"})
                for (const artifactName of names)
                    make("moqui.security.ArtifactGroupMember", {artifactGroupId, artifactName, artifactTypeEnumId: "AT_SERVICE"})
                make("moqui.security.ArtifactAuthz", {
                    artifactAuthzId: artifactGroupId,
                    artifactGroupId,
                    userGroupId: "RDR_READER",
                    authzTypeEnumId: "AUTHZT_ALLOW",
                    authzActionEnumId
                })
            }
            grant(
                "RDR_READER",
                ["query#Documents", "run#Query", "get#Drill", "render#Dashboard", "create#Annotation"].map(name => R + name),
                "AUTHZA_ALL"
            )
            // view only: the update action is a dataset's steward's
            grant(
                "RDR_READER_DATA",
                ["rdr-sales", "rdr-totals", "rdr-stock", "rdr-events"].map(id => "org.moqui.report.dataset." + id),
                "AUTHZA_VIEW"
            )
            for (const userGroupId of ["RDR_READER", "ADMIN"])
                make("moqui.entity.document.DataDocumentUserGroup", {dataDocumentId: "RdrFixture", userGroupId})
        })
        // an amount in each record's currency; a dataset whose records are restricted, its amount a ratio; a balance not added
        // across regions, left waiting for review
        dataset("rdr-sales", {fieldDefaults: {sequenceNum: {unitField: "parentEnumId"}}})
        dataset("rdr-totals", {
            rows: "restricted",
            fieldDefaults: {sequenceNum: {ratio: {numerator: "won", denominator: "offered"}}}
        })
        dataset("rdr-stock", {fieldDefaults: {sequenceNum: {nonAdditive: ["relatedEnumId"]}}})
        const built = asSteward(R + "index#BusinessDataset", {datasetId: "rdr-sales"})
        const admitted = ["rdr-sales", "rdr-totals"].map(
            datasetId => asSteward(R + "store#BusinessDataset", {datasetId, admit: true}).out?.dataset?.status
        )
        panel("rdr-panel", "rdr-sales")
        panel("rdr-totals-panel", "rdr-totals")
        panel("rdr-stock-panel", "rdr-stock")
        return {indexed: built.out?.documentsIndexed, admitted: admitted.join(",")}
    },
    then({indexed, admitted}) {
        indexed == 5
        admitted == "admitted,admitted"
    }
})

feature("a drill answers the records of the chosen bucket in the viewer's scope, and an Other bucket keeps its predicate", {
    when() {
        const drill = extra => asReader(R + "get#Drill", Object.assign({definitionId: "rdr-panel", range}, extra))
        const north = drill({selectedBuckets: [{field: "region", value: "north"}]})
        // the rest: everything but the bucket shown, with a breakdown by currency
        const other = drill({selectedBuckets: [{field: "region", value: "Other"}], breakdownDimension: "parentEnumId"})
        const currencies = Array.from(other.out?.breakdown?.frames || [])
            .flatMap(frame => Array.from(frame.fields || []))
            .filter(field => field.name === "parentEnumId")
            .flatMap(field => Array.from(field.values || []))
        const whole = drill({pageSize: 1})
        const absent = drill({selectedBuckets: [{field: "region", value: "west"}]})
        const notDimension = drill({breakdownDimension: "noSuchField"})
        return {
            north: north.out || {},
            northError: north.error || "",
            northIds: ids(north.out?.rows),
            otherIds: ids(other.out?.rows),
            otherTotal: other.out?.total,
            currencies: currencies.sort().join(","),
            whole: whole.out || {},
            absent: absent.error || "",
            notDimension: notDimension.error || ""
        }
    },
    then({north, northError, northIds, otherIds, otherTotal, currencies, whole, absent, notDimension}) {
        northError == ""
        north.available
        north.datasetId == "rdr-sales"
        // the reader's two north records, not the other scope group's (RDR_E5), though the request named no scope
        northIds == "RDR_E1,RDR_E2"
        north.total == 2
        north.rowsAvailable
        !north.bounded
        north.selected[0].label == "north"
        Array.from(north.dimensions).includes("parentEnumId")
        // Other is everything but north in scope: south and east
        otherIds == "RDR_E3,RDR_E4"
        otherTotal == 2
        currencies == "EUR,USD"
        // a page of the whole panel is a sample of its four records, and says so
        whole.total == 4
        whole.rows.length == 1
        whole.bounded
        absent.includes("not in this panel's result")
        notDimension.includes("is not a dimension")
    }
})

feature("a viewer who reads a dataset's aggregates and not its records drills to no record, and an unread panel says why", {
    when() {
        const totals = asReader(R + "get#Drill", {
            definitionId: "rdr-totals-panel",
            range,
            selectedBuckets: [{field: "region", value: "north"}],
            breakdownDimension: "parentEnumId"
        })
        const counted = asReader(R + "run#Query", {
            query: Object.assign(
                {
                    datasetId: "rdr-totals",
                    aggregations: [{id: "region", type: "terms", role: "x-axis", params: {field: "relatedEnumId"}}]
                },
                range
            )
        })
        const direct = asReader(R + "query#Documents", Object.assign({datasetId: "rdr-totals"}, range))
        // a holder of the records artifact (ADMIN): no bucket chosen, since the row filter gives john.doe no fixture record
        const steward = asSteward(R + "get#Drill", {definitionId: "rdr-totals-panel", range})
        // a panel over a dataset waiting for review
        const waiting = asReader(R + "get#Drill", {definitionId: "rdr-stock-panel", range})
        return {
            totals: totals.out || {},
            totalsError: totals.error || "",
            breakdownFrames: Array.from(totals.out?.breakdown?.frames || []).length,
            counted: Array.from(counted.out?.frames || []).length,
            direct: direct.error || "",
            stewardReads: steward.out?.rowsAvailable === true || steward.error,
            waiting: waiting.out || {}
        }
    },
    then({totals, totalsError, breakdownFrames, counted, direct, stewardReads, waiting}) {
        totalsError == ""
        totals.available
        !totals.rowsAvailable
        totals.rows.length == 0
        totals.rowsReason.includes("ROWS_RESTRICTED")
        // the aggregate and its breakdown are still the reader's
        breakdownFrames == 1
        counted == 1
        direct.includes("ROWS_RESTRICTED")
        stewardReads === true
        !waiting.available
        waiting.reason.includes("not admitted")
        waiting.rows.length == 0
    }
})

feature("the semantic guards refuse an amount across units, a ratio averaged, a balance summed and a total of a bounded page", {
    when() {
        const mixed = asReader(R + "run#Query", sum("rdr-sales"))
        const grouped = asReader(
            R + "run#Query",
            sum("rdr-sales", {
                aggregations: [
                    {id: "currency", type: "terms", role: "x-axis", params: {field: "parentEnumId"}},
                    {id: "total", type: "sum", params: {field: "sequenceNum"}}
                ]
            })
        )
        const oneUnit = asReader(
            R + "run#Query",
            sum("rdr-sales", {filters: [{query: {field: {name: "parentEnumId", op: "eq", value: "USD"}}}]})
        )
        const ratio = asReader(R + "run#Query", {
            query: Object.assign(
                {datasetId: "rdr-totals", aggregations: [{id: "rate", type: "avg", params: {field: "sequenceNum"}}]},
                range
            )
        })
        // the balance, read by its steward while it waits for review
        const balance = asSteward(R + "run#Query", sum("rdr-stock"))
        const byRegion = asSteward(
            R + "run#Query",
            sum("rdr-stock", {
                aggregations: [
                    {id: "region", type: "terms", role: "x-axis", params: {field: "relatedEnumId"}},
                    {id: "total", type: "sum", params: {field: "sequenceNum"}}
                ]
            })
        )
        const page = {datasetId: "rdr-sales", mode: "documents", pageSize: 2}
        const sample = asReader(R + "run#Query", {query: Object.assign({}, page, range)})
        const totalled = asReader(R + "run#Query", {
            query: Object.assign({transformations: [{type: "reduce", calculations: ["count"]}]}, page, range)
        })
        const value = out =>
            Array.from(out?.frames || [])
                .flatMap(frame => Array.from(frame.fields || []))
                .filter(field => field.name === "total")
                .flatMap(field => Array.from(field.values || []))
                .map(Number)
        return {
            mixed: mixed.error || "",
            grouped: value(grouped.out)
                .sort((a, b) => a - b)
                .join(","),
            groupedError: grouped.error || "",
            oneUnit: value(oneUnit.out).join(","),
            ratio: ratio.error || "",
            balance: balance.error || "",
            byRegion: byRegion.error || "",
            sampleNotice: Array.from(sample.out?.notices || []).find(notice => notice.code === "bounded_result") || {},
            sampleCount: sample.out?.sourceCount,
            totalled: totalled.error || ""
        }
    },
    then({mixed, grouped, groupedError, oneUnit, ratio, balance, byRegion, sampleNotice, sampleCount, totalled}) {
        mixed.includes("UNIT_MIXED")
        mixed.includes("parentEnumId")
        groupedError == ""
        // 5 in EUR and 37 in USD, each apart
        grouped == "5,37"
        oneUnit == "37"
        ratio.includes("RATIO_AGGREGATE")
        ratio.includes("won")
        balance.includes("NONADDITIVE")
        byRegion == ""
        sampleNotice.returned == 2
        sampleNotice.total == 4
        sampleCount == 4
        totalled.includes("BOUNDED_RESULT")
    }
})
