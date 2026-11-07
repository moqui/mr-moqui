// Business datasets (crates/moqui-report/src/business.rs; docs/tools/Monitoring.md "Business datasets"; BOSS impl-insights P3,
// INS-03): a dataset of signal business is read only once admitted, and admission needs its document in a registered DataFeed,
// its index built and a registered scope policy bound; index#BusinessDataset rebuilds one feed's document for the dataset's
// steward; the row filter, called with the dataset's rowFilterParameters, narrows rows, value counts and an export alike; a field
// a definition over the dataset uses cannot be removed, and a stale version is refused. The fixtures are enumerations of a type
// of the spec's own, each scoped to a user group (enumCode) by the framework's ReportUserGroups policy, on feeds that index only
// when asked (manual pull). Run: ./moq test framework --spec BusinessDatasets
const R = "org.moqui.report.ReportServices."
const READER = "BDS_READER_USER"
const READER_NAME = "bds.reader"
const types = {fed: "BdsFixture", unfed: "BdsUnfed", unbuilt: "BdsUnbuilt"}
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
// a call as the steward (john.doe, ADMIN: the datasets' update artifact), refused or not
function asSteward(name, parameters) {
    try {
        return {out: call(name, parameters)}
    } catch (error) {
        ec.message.clearErrors()
        return {error: String(error.message)}
    }
}
// the text of a base64 export (ASCII here)
function fromBase64(encoded) {
    const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    let bits = 0
    let count = 0
    let out = ""
    for (const character of String(encoded).replace(/[^A-Za-z0-9+/]/g, "")) {
        bits = (bits << 6) | alphabet.indexOf(character)
        count += 6
        if (count >= 8) {
            count -= 8
            out += String.fromCharCode((bits >> count) & 255)
        }
    }
    return out
}
const dataset = (id, document, extra) =>
    call(R + "save#Definition", {
        definition: {
            id,
            type: "dataset",
            title: id,
            userGroupId: "BDS_READER",
            body: Object.assign({documentType: document, timeField: "_timestamp", signal: "business"}, extra)
        }
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

feature("three fixture documents, one on a fed feed and one on a feed never built, and a reader of one scope group", {
    when() {
        sys(() => {
            const from = new Date(Date.now() - 60000)
            make("moqui.basic.EnumerationType", {enumTypeId: "BdsFixture", description: "Business datasets spec"})
            for (const [enumId, enumCode, description] of [
                ["BDS_E1", "BDS_A", "Alpha one"],
                ["BDS_E2", "BDS_A", "Alpha two"],
                ["BDS_E3", "BDS_B", "Beta one"]
            ])
                make("moqui.basic.Enumeration", {enumId, enumTypeId: "BdsFixture", enumCode, description})
            for (const [documentId, indexName] of [
                [types.fed, "bds_fixture"],
                [types.unfed, "bds_unfed"],
                [types.unbuilt, "bds_unbuilt"]
            ]) {
                make("moqui.entity.document.DataDocument", {
                    dataDocumentId: documentId,
                    indexName,
                    documentName: documentId,
                    primaryEntityName: "moqui.basic.Enumeration"
                })
                for (const [fieldSeqId, fieldPath] of [
                    ["01", "enumId"],
                    ["02", "enumTypeId"],
                    ["03", "enumCode"],
                    ["04", "description"]
                ])
                    make("moqui.entity.document.DataDocumentField", {
                        dataDocumentId: documentId,
                        fieldSeqId,
                        fieldPath,
                        sortable: fieldPath === "enumCode" ? "Y" : null
                    })
                make("moqui.entity.document.DataDocumentCondition", {
                    dataDocumentId: documentId,
                    conditionSeqId: "01",
                    fieldNameAlias: "enumTypeId",
                    fieldValue: "BdsFixture"
                })
            }
            for (const [dataFeedId, documentId] of [
                ["BdsFeed", types.fed],
                ["BdsUnbuiltFeed", types.unbuilt]
            ]) {
                make("moqui.entity.feed.DataFeed", {dataFeedId, dataFeedTypeEnumId: "DTFDTP_MAN_PULL", feedName: dataFeedId})
                make("moqui.entity.feed.DataFeedDocument", {dataFeedId, dataDocumentId: documentId})
            }
            // the reader: in the scope group BDS_A, and in BDS_READER, which may read bds-sales and its document and call the
            // reading operations and index#BusinessDataset (refused by the dataset's own check, which it lacks)
            make("moqui.security.UserAccount", {
                userId: READER,
                username: READER_NAME,
                userFullName: "Datasets Reader",
                disabled: "N"
            })
            for (const userGroupId of ["BDS_A", "BDS_B", "BDS_READER"])
                make("moqui.security.UserGroup", {userGroupId, description: "Business datasets spec " + userGroupId})
            for (const userGroupId of ["BDS_A", "BDS_READER"])
                make("moqui.security.UserGroupMember", {userGroupId, userId: READER, fromDate: from})
            make("moqui.security.ArtifactGroup", {artifactGroupId: "BDS_READER", description: "Business datasets spec"})
            for (const verbNoun of [
                "query#Documents",
                "run#Query",
                "find#BusinessDatasets",
                "preview#Report",
                "index#BusinessDataset"
            ])
                make("moqui.security.ArtifactGroupMember", {
                    artifactGroupId: "BDS_READER",
                    artifactName: R + verbNoun,
                    artifactTypeEnumId: "AT_SERVICE"
                })
            make("moqui.security.ArtifactAuthz", {
                artifactAuthzId: "BDS_READER",
                artifactGroupId: "BDS_READER",
                userGroupId: "BDS_READER",
                authzTypeEnumId: "AUTHZT_ALLOW",
                authzActionEnumId: "AUTHZA_ALL"
            })
            make("moqui.security.ArtifactGroup", {artifactGroupId: "BDS_READER_DATA", description: "Business datasets spec"})
            make("moqui.security.ArtifactGroupMember", {
                artifactGroupId: "BDS_READER_DATA",
                artifactName: "org.moqui.report.dataset.bds-sales",
                artifactTypeEnumId: "AT_SERVICE"
            })
            make("moqui.security.ArtifactAuthz", {
                artifactAuthzId: "BDS_READER_DATA",
                artifactGroupId: "BDS_READER_DATA",
                userGroupId: "BDS_READER",
                authzTypeEnumId: "AUTHZT_ALLOW",
                authzActionEnumId: "AUTHZA_VIEW"
            })
            make("moqui.entity.document.DataDocumentUserGroup", {dataDocumentId: types.fed, userGroupId: "BDS_READER"})
        })
        dataset("bds-sales", types.fed, policy)
        dataset("bds-unfed", types.unfed, policy)
        dataset("bds-unbuilt", types.unbuilt, policy)
        dataset("bds-unbound", types.fed, {})
        return {searchAvailable: ec.search.isAvailable()}
    },
    then({searchAvailable}) {
        searchAvailable
    }
})

feature("index#BusinessDataset builds one dataset's document of its feed, for its steward only", {
    when() {
        const refused = asReader(R + "index#BusinessDataset", {datasetId: "bds-sales"})
        const built = asSteward(R + "index#BusinessDataset", {datasetId: "bds-sales"})
        const states = Object.fromEntries(
            Array.from(call(R + "find#BusinessDatasets", {}).datasets)
                .filter(row => row.datasetId.startsWith("bds-"))
                .map(row => [row.datasetId, row])
        )
        const check = (id, name) => Array.from(states[id].checks).find(row => row.check === name)
        return {
            refused: refused.error || "",
            built: built.out || {},
            salesIndex: check("bds-sales", "index"),
            unbuiltIndex: check("bds-unbuilt", "index"),
            indexedCount: states["bds-sales"].lastIndexedCount
        }
    },
    then({refused, built, salesIndex, unbuiltIndex, indexedCount}) {
        refused.includes("bds-sales")
        built.documentsIndexed == 3
        built.dataFeedId == "BdsFeed"
        built.dataDocumentId == "BdsFixture"
        salesIndex.passed
        !unbuiltIndex.passed
        unbuiltIndex.message.includes("not built")
        indexedCount == 3
    }
})

feature("admission is refused for an unregistered feed, an unbuilt index and an unbound row filter, and opens the dataset", {
    when() {
        const admit = datasetId => asSteward(R + "store#BusinessDataset", {datasetId, admit: true})
        const before = asReader(R + "query#Documents", Object.assign({datasetId: "bds-sales"}, range))
        const unfed = admit("bds-unfed")
        const unbuilt = admit("bds-unbuilt")
        const unbound = admit("bds-unbound")
        const sales = admit("bds-sales")
        return {
            before: before.error || "",
            unfed: unfed.error || "",
            unbuilt: unbuilt.error || "",
            unbound: unbound.error || "",
            sales: sales.out?.dataset?.status
        }
    },
    then({before, unfed, unbuilt, unbound, sales}) {
        before.includes("not admitted")
        unfed.includes("in no registered data feed")
        unbuilt.includes("is not built")
        unbound.includes("no row filter is bound")
        sales == "admitted"
    }
})

feature("the row filter narrows the reader's rows, value counts and export to their scope group", {
    when() {
        const rows = asReader(R + "query#Documents", Object.assign({datasetId: "bds-sales", pageSize: 50}, range)).out
        const forged = asReader(
            R + "query#Documents",
            Object.assign({datasetId: "bds-sales", queryString: "enumCode:BDS_B"}, range)
        ).out
        const counts = asReader(R + "run#Query", {
            query: Object.assign(
                {
                    datasetId: "bds-sales",
                    aggregations: [{id: "scope", type: "terms", role: "split-rows", params: {field: "enumCode", size: 10}}]
                },
                range
            )
        }).out
        // a table panel's CSV, the export a dashboard offers, of a dashboard shared with the reader
        call(R + "save#Definition", {
            definition: {
                id: "bds-board",
                type: "dashboard",
                title: "Business datasets spec",
                userGroupId: "BDS_READER",
                body: {
                    panels: {
                        rows: {
                            title: "Rows",
                            visualization: "table",
                            queries: [{datasetId: "bds-sales", mode: "documents", pageSize: 50}]
                        }
                    }
                }
            }
        })
        const csv = asReader(
            R + "preview#Report",
            Object.assign({definitionId: "bds-board", panelId: "rows", renderMode: "csv"}, range)
        )
        const text = csv.out?.contentBase64 ? fromBase64(csv.out.contentBase64) : csv.error || ""
        // the scope groups among the frames' values, whatever the terms field is named
        const codes = Array.from(
            new Set(
                Array.from(counts?.frames || []).flatMap(frame =>
                    Array.from(frame.fields || []).flatMap(field =>
                        Array.from(field.values || []).filter(value => /^BDS_[AB]$/.test(String(value)))
                    )
                )
            )
        )
        return {
            total: rows?.total,
            ids: Array.from(rows?.hits || [])
                .map(hit => hit.enumId || hit.id)
                .sort()
                .join(","),
            forged: forged?.total,
            codes: codes.join(","),
            csv: String(text)
        }
    },
    then({total, ids, forged, codes, csv}) {
        total == 2
        ids == "BDS_E1,BDS_E2"
        forged == 0
        codes == "BDS_A"
        csv.includes("Alpha one")
        !csv.includes("Beta one")
    }
})

feature("a field a definition over the dataset uses cannot be removed, nor a field change made on a stale version", {
    when() {
        call(R + "save#Definition", {
            definition: {
                id: "bds-panel",
                type: "panel",
                title: "By description",
                body: {
                    queries: [
                        {
                            datasetId: "bds-sales",
                            aggregations: [{id: "d", type: "terms", role: "split-rows", params: {field: "description"}}]
                        }
                    ]
                }
            }
        })
        const fields = Array.from(
            sys(() => ec.entity.find("moqui.entity.document.DataDocumentField").condition("dataDocumentId", types.fed).list())
        )
        const description = fields.find(row => row.fieldPath === "description").fieldSeqId
        const version = call(R + "find#BusinessDatasets", {datasetId: "bds-sales"}).datasets[0].version
        const used = asSteward(R + "store#BusinessDatasetFields", {
            datasetId: "bds-sales",
            fields: [{fieldSeqId: description, remove: true}]
        })
        const stale = asSteward(R + "store#BusinessDatasetFields", {
            datasetId: "bds-sales",
            expectedVersion: version - 1,
            fields: [{fieldPath: "enumCode", fieldNameAlias: "scopeCode"}]
        })
        const expression = asSteward(R + "store#BusinessDatasetFields", {
            datasetId: "bds-sales",
            fields: [{fieldPath: "(enumCode + 'x')", fieldNameAlias: "computed"}]
        })
        const added = asSteward(R + "store#BusinessDatasetFields", {
            datasetId: "bds-sales",
            expectedVersion: version,
            fields: [{fieldPath: "enumCode", fieldNameAlias: "scopeCode"}]
        })
        const index = Array.from(added.out?.dataset?.checks || []).find(row => row.check === "index")
        return {used: used.error || "", stale: stale.error || "", expression: expression.error || "", index}
    },
    then({used, stale, expression, index}) {
        used.includes("bds-panel (description)")
        stale.includes("changed since it was read")
        expression.includes("not a plain field path")
        !index.passed
        index.message.includes("rebuild")
    }
})
