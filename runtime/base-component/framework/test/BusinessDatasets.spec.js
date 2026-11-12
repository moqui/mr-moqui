// Business datasets (crates/moqui-report/src/business.rs; docs/tools/Monitoring.md "Business datasets"; BOSS impl-insights P3,
// INS-03): a dataset of signal business is read only once admitted, and admission needs its document in a registered DataFeed,
// its index built and a registered scope policy bound; index#BusinessDataset rebuilds one feed's document for the dataset's
// steward; the row filter, called with the dataset's rowFilterParameters, narrows rows, value counts and an export alike; a field
// a definition over the dataset uses cannot be removed, and a stale version is refused; an alias of a telemetry index or of a
// business dataset's document needs the source's own grant, saved or read (alias.rs); what the dataset screens read beside
// the engine (ReportReads.js: get#BusinessDataset, get#BusinessDatasetOptions) is answered by what the caller reads, stewards
// and may create, and a dataset is created only under an id the creator's own grant names. The fixtures are enumerations of a type
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
    return asUser(READER_NAME, name, parameters)
}
function asUser(username, name, parameters) {
    ec.user.internalLoginUser(username)
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
            // bds-sales, and the ways around it a grant could open by mistake: its document by id, its index, a copy
            // without the business signal (each refused by the engine, F-P3-6)
            for (const id of ["bds-sales", types.fed, "index:bds_fixture", "bds-raw"])
                make("moqui.security.ArtifactGroupMember", {
                    artifactGroupId: "BDS_READER_DATA",
                    artifactName: "org.moqui.report.dataset." + id,
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
        // the field the dataset's own row filter reads (groupField enumCode)
        const scoped = asSteward(R + "store#BusinessDatasetFields", {
            datasetId: "bds-sales",
            fields: [{fieldSeqId: fields.find(row => row.fieldPath === "enumCode").fieldSeqId, remove: true}]
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
        return {
            used: used.error || "",
            scoped: scoped.error || "",
            stale: stale.error || "",
            expression: expression.error || "",
            index
        }
    },
    then({used, scoped, stale, expression, index}) {
        used.includes("bds-panel (description)")
        scoped.includes("bds-sales (enumCode)")
        stale.includes("changed since it was read")
        expression.includes("not a plain field path")
        !index.passed
        index.message.includes("rebuild")
    }
})

feature("the document by id, its index and a copy without the signal reach no reader; a changed definition returns to review", {
    when() {
        const query = id => asReader(R + "query#Documents", Object.assign({datasetId: id}, range))
        dataset("bds-raw", types.fed, {signal: null})
        const raw = [types.fed, "index:bds_fixture", "bds-raw"].map(id => query(id).error || "")
        // the steward drops the row filter through save#Definition instead of store#BusinessDataset
        dataset("bds-sales", types.fed, {})
        const unbound = query("bds-sales")
        const status = call(R + "find#BusinessDatasets", {datasetId: "bds-sales"}).datasets[0].status
        // an encrypted field (AgentRemoteA2a.authJson) through a document of the steward's
        sys(() => {
            make("moqui.entity.document.DataDocument", {
                dataDocumentId: "BdsRemote",
                indexName: "bds_remote",
                documentName: "BdsRemote",
                primaryEntityName: "moqui.agent.AgentRemoteA2a"
            })
            make("moqui.entity.document.DataDocumentField", {
                dataDocumentId: "BdsRemote",
                fieldSeqId: "01",
                fieldPath: "remoteId"
            })
        })
        dataset("bds-remote", "BdsRemote", policy)
        const encrypted = asSteward(R + "store#BusinessDatasetFields", {
            datasetId: "bds-remote",
            fields: [{fieldPath: "authJson", fieldNameAlias: "remoteAuth"}]
        })
        const kept = sys(() =>
            ec.entity.find("moqui.entity.document.DataDocumentField").condition("dataDocumentId", "BdsRemote").count()
        )
        return {raw, unbound: unbound.error || "", status, encrypted: encrypted.error || "", kept}
    },
    then({raw, unbound, status, encrypted, kept}) {
        raw.every(error => error.includes("a business dataset's document"))
        unbound.includes("not admitted")
        status == "review"
        encrypted.includes("encrypted field")
        kept == 1
    }
})

feature("an alias of a log index or of a business document needs the source's own grant, when saved and when read", {
    when() {
        // a steward as an application grants one: saving dataset definitions, and every artifact of the datasets they name
        // (a pattern), but neither the telemetry indexes nor the fixture document by its own id (BOSS impl-admin F277)
        const STEWARD = "bds.steward"
        sys(() => {
            make("moqui.security.UserAccount", {
                userId: "BDS_STEWARD_USER",
                username: STEWARD,
                userFullName: "Datasets Steward",
                disabled: "N"
            })
            make("moqui.security.UserGroup", {userGroupId: "BDS_STEWARD", description: "Business datasets spec steward"})
            make("moqui.security.UserGroupMember", {
                userGroupId: "BDS_STEWARD",
                userId: "BDS_STEWARD_USER",
                fromDate: new Date(Date.now() - 60000)
            })
            make("moqui.security.ArtifactGroup", {artifactGroupId: "BDS_STEWARD", description: "Business datasets spec"})
            for (const [artifactName, nameIsPattern] of [
                [R + "save#Definition", "N"],
                [R + "query#Documents", "N"],
                [R + "create#BusinessDataset", "N"],
                ["org.moqui.report.dataset", "N"],
                ["org\\.moqui\\.report\\.dataset\\.bds-own-.*", "Y"]
            ])
                make("moqui.security.ArtifactGroupMember", {
                    artifactGroupId: "BDS_STEWARD",
                    artifactName,
                    nameIsPattern,
                    artifactTypeEnumId: "AT_SERVICE"
                })
            make("moqui.security.ArtifactAuthz", {
                artifactAuthzId: "BDS_STEWARD",
                artifactGroupId: "BDS_STEWARD",
                userGroupId: "BDS_STEWARD",
                authzTypeEnumId: "AUTHZT_ALLOW",
                authzActionEnumId: "AUTHZA_ALL"
            })
            make("moqui.entity.document.DataDocumentUserGroup", {dataDocumentId: types.fed, userGroupId: "BDS_STEWARD"})
        })
        const everyLine = {rowFilter: {match_all: {}}}
        const save = (id, body) =>
            asUser(STEWARD, R + "save#Definition", {definition: {id, type: "dataset", title: id, body}}).error || ""
        const query = id => asUser(STEWARD, R + "query#Documents", Object.assign({datasetId: id}, range))
        const saved = {
            logs: save("bds-own-logs", Object.assign({indexName: "otel_logs", timeField: "timestamp"}, everyLine)),
            signal: save("bds-own-signal", Object.assign({documentType: "otel_logs", signal: "business"}, everyLine)),
            raw: save("bds-own-raw", {documentType: types.fed, timeField: "_timestamp"}),
            // the signal with no registered scope policy bound is no way past the document's row filter either
            unbound: save("bds-own-unbound", {documentType: types.fed, timeField: "_timestamp", signal: "business"}),
            index: save("bds-own-index", {indexName: "bds_fixture", timeField: "_timestamp"})
        }
        // the same written past the save (an entity write, an older row), owned by the steward; and a log alias shared
        // with the steward's group by a holder of the index whose account has since been disabled for good
        sys(() => {
            make("moqui.security.UserAccount", {
                userId: "BDS_CLOSED_USER",
                username: "bds.closed",
                userFullName: "Datasets Closed Holder",
                disabled: "Y"
            })
            make("moqui.security.UserGroupMember", {
                userGroupId: "ADMIN",
                userId: "BDS_CLOSED_USER",
                fromDate: new Date(Date.now() - 60000)
            })
            for (const [definitionId, ownerId, body] of [
                [
                    "bds-own-stored-logs",
                    "BDS_STEWARD_USER",
                    Object.assign({indexName: "otel_logs", timeField: "timestamp"}, everyLine)
                ],
                ["bds-own-stored-raw", "BDS_STEWARD_USER", {documentType: types.fed, timeField: "_timestamp"}],
                [
                    "bds-own-stored-unbound",
                    "BDS_STEWARD_USER",
                    {documentType: types.fed, timeField: "_timestamp", signal: "business"}
                ],
                ["bds-own-closed", "BDS_CLOSED_USER", Object.assign({indexName: "otel_logs", timeField: "timestamp"}, everyLine)]
            ])
                make("moqui.report.Definition", {
                    definitionId,
                    definitionType: "dataset",
                    title: definitionId,
                    userGroupId: "BDS_STEWARD",
                    bodyJson: JSON.stringify(body),
                    ownerId,
                    readOnly: "N",
                    versionNumber: 1
                })
        })
        const stored = {
            logs: query("bds-own-stored-logs").error || "",
            raw: query("bds-own-stored-raw").error || "",
            unbound: query("bds-own-stored-unbound").error || "",
            closed: query("bds-own-closed").error || ""
        }
        // the system administrator holds the log index, so their alias of it is saved, and its group reads it by the alias's
        // own grant under its filter: the owner is asked for the index, not the reader
        const shared = asSteward(R + "save#Definition", {
            definition: {
                id: "bds-own-shared",
                type: "dataset",
                title: "bds-own-shared",
                userGroupId: "BDS_STEWARD",
                body: {
                    indexName: "otel_logs",
                    timeField: "timestamp",
                    rowFilter: {field: {name: "scope_name", op: "eq", value: "bds.spec.none"}}
                }
            }
        })
        const read = query("bds-own-shared")
        // and the steward's own business dataset over the document, under a registered policy, is theirs as before
        const created = asUser(STEWARD, R + "create#BusinessDataset", {
            datasetId: "bds-own-sales",
            dataDocumentId: types.fed,
            scopePolicyId: "ReportUserGroups",
            rowFilterParameters: policy.rowFilterParameters
        })
        const own = query("bds-own-sales")
        return {
            saved,
            stored,
            shared: shared.error || "",
            read: read.error || "",
            hits: Array.from(read.out?.hits || []).length,
            created: created.error || created.out?.dataset?.status,
            own: own.error || "",
            ownHits: Array.from(own.out?.hits || []).length
        }
    },
    then({saved, stored, shared, read, hits, created, own, ownHits}) {
        saved.logs.includes("needs its own grant (org.moqui.report.dataset.otel_logs)")
        saved.signal.includes("needs its own grant")
        saved.raw.includes("a business dataset's document")
        saved.unbound.includes("a business dataset's document")
        saved.index.includes("a business dataset's document")
        stored.logs.includes("neither its reader nor its owner may read")
        stored.raw.includes("a business dataset's document")
        stored.unbound.includes("a business dataset's document")
        stored.closed.includes("neither its reader nor its owner may read")
        shared == ""
        read == ""
        hits == 0
        // in review, read by its steward under the policy: the steward is in no scope group, so no row
        created == "review"
        own == ""
        ownHits == 0
    }
})

feature(
    "the dataset reads answer a reader the dataset, its steward its document, a creator the sources; an id or scope named gives no right",
    {
        when() {
            const STEWARD = "bds.steward"
            // both may call the two reads: what each answers is by the caller's own dataset grants
            sys(() => {
                for (const [artifactGroupId, verbNouns] of [
                    ["BDS_READER", ["get#BusinessDataset", "get#BusinessDatasetOptions"]],
                    ["BDS_STEWARD", ["get#BusinessDataset", "get#BusinessDatasetOptions", "find#BusinessDatasets"]]
                ])
                    for (const verbNoun of verbNouns)
                        make("moqui.security.ArtifactGroupMember", {
                            artifactGroupId,
                            artifactName: R + verbNoun,
                            artifactTypeEnumId: "AT_SERVICE"
                        })
            })
            // bds-sales bound, built and admitted again (it was left unbound, in review), so that the reader reads it
            dataset("bds-sales", types.fed, policy)
            call(R + "index#BusinessDataset", {datasetId: "bds-sales"})
            call(R + "store#BusinessDataset", {datasetId: "bds-sales", admit: true})
            const read = "get#BusinessDataset"
            const steward = asSteward(R + read, {datasetId: "bds-sales"})
            const reader = asReader(R + read, {datasetId: "bds-sales"})
            // a dataset that exists and the reader holds no artifact of, and one that does not exist: told the same
            const hidden = asReader(R + read, {datasetId: "bds-unfed"})
            const missing = asReader(R + read, {datasetId: "bds-none"})
            // the spec's steward stewards bds-own-* alone: bds-sales is not theirs either
            const others = asUser(STEWARD, R + read, {datasetId: "bds-sales"})
            const creator = asUser(STEWARD, R + "get#BusinessDatasetOptions", {})
            const plain = asReader(R + "get#BusinessDatasetOptions", {})
            // a dataset under an id the steward's grant does not name, and a scope parameter that is no field name
            const create = extra =>
                asUser(
                    STEWARD,
                    R + "create#BusinessDataset",
                    Object.assign({dataDocumentId: types.fed, scopePolicyId: "ReportUserGroups"}, extra)
                ).error || ""
            const foreign = create({datasetId: "bds-other-sales", rowFilterParameters: policy.rowFilterParameters})
            const expression = create({datasetId: "bds-own-expression", rowFilterParameters: {groupField: "enumCode OR 1=1"}})
            const kept = sys(() =>
                ec.entity
                    .find("moqui.report.Definition")
                    .condition("definitionId", "in", ["bds-other-sales", "bds-own-expression"])
                    .count()
            )
            const names = (out, list, field) =>
                Array.from(out?.[list] || [])
                    .map(row => row[field])
                    .sort()
                    .join(",")
            return {
                steward: steward.out?.dataset || {},
                stewardFields: names(steward.out, "fields", "fieldName"),
                stewardCondition: Array.from(steward.out?.conditions || [])[0] || {},
                stewardUses: names(steward.out, "uses", "id"),
                usesAvailable: steward.out?.usesAvailable,
                reader: reader.out?.dataset || {},
                readerParts: ["fields", "conditions", "uses"].map(list => Array.from(reader.out?.[list] || []).length).join(","),
                hidden: hidden.error || "",
                missing: missing.error || "",
                others: others.error || "",
                creator: creator.out || {},
                creatorDocuments: names(creator.out, "documents", "dataDocumentId"),
                policy: Array.from(creator.out?.policies || []).find(row => row.scopePolicyId === "ReportUserGroups") || {},
                plain: plain.out || {},
                plainDocuments: names(plain.out, "documents", "dataDocumentId"),
                foreign,
                expression,
                kept
            }
        },
        then({
            steward,
            stewardFields,
            stewardCondition,
            stewardUses,
            usesAvailable,
            reader,
            readerParts,
            hidden,
            missing,
            others,
            creator,
            creatorDocuments,
            policy,
            plain,
            plainDocuments,
            foreign,
            expression,
            kept
        }) {
            steward.status == "admitted"
            steward.stewarded == true
            // the policy's scope parameter, from its registration
            steward.scopeParameter == "groupField"
            stewardFields == "description,enumCode,enumId,enumTypeId,scopeCode"
            stewardCondition.fieldNameAlias == "enumTypeId"
            stewardCondition.operator == "equals"
            stewardUses.includes("bds-board") && stewardUses.includes("bds-panel")
            usesAvailable == true
            // the reader reads it and stewards nothing: the dataset alone
            reader.readable == true
            reader.stewarded == false
            readerParts == "0,0,0"
            hidden.includes("Dataset bds-unfed is not available")
            missing.includes("Dataset bds-none is not available")
            others.includes("Dataset bds-sales is not available")
            // the sources are a creator's; the policies, operators and functions any caller's
            creator.canCreate == true
            creatorDocuments.includes(types.fed) && creatorDocuments.includes(types.unbuilt)
            !creatorDocuments.includes(types.unfed)
            policy.scopeParameter == "groupField"
            Array.from(creator.operators).includes("not-between") && Array.from(creator.functions).includes("count-distinct")
            plain.canCreate == false
            plainDocuments == ""
            Array.from(plain.policies).length >= 1
            foreign.includes("bds-other-sales would not be yours to steward")
            expression.includes("rowFilterParameters groupField must be a name")
            kept == 0
        }
    }
)
