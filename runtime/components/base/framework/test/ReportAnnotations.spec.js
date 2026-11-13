// Business events on the time series (crates/moqui-report/src/annotations.rs; docs/tools/Monitoring.md "Business events"; BOSS
// impl-insights P6, F13, F31): create#Annotation's source business is its owner's to write (the artifact
// org.moqui.report.annotation.business), not MONITOR_INGEST's; it carries an organization, an event and a record or a link on this
// server; one event key keeps one marker; and a dashboard over business data shows the events of each registered source
// (a ReportAnnotationSource dataset over the annotations) its viewer may read, through that dataset's own row filter. The
// reader, the datasets and the panel are ReportDrill's. Run: ./moq test framework --spec ReportAnnotations
const R = "org.moqui.report.ReportServices."
const READER_NAME = "rdr.reader"
const range = {from: "now-1d", thru: "now+1d"}

function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
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
function asAdmin(name, parameters) {
    try {
        return {out: call(name, parameters)}
    } catch (error) {
        ec.message.clearErrors()
        return {error: String(error.message)}
    }
}
// an event's time is its record's, so writing it again names the same instant (an instant for the engine, never shown)
const at = new Date(Date.now() - 3600000).toISOString()
const event = extra =>
    Object.assign(
        {
            source: "business",
            text: "Price list published: Spring",
            organizationPartyId: "RDR_A",
            event: "PriceListPublished",
            time: at
        },
        extra
    )

spec({
    stepwise: true,
    dependsOn: ["ReportDrill"],
    setupSpec() {
        login("john.doe")
    },
    cleanupSpec() {
        ec.message.clearErrors()
        ec.artifactExecution.enableAuthz()
    }
})

feature("the business source is refused without its grant, whatever MONITOR_INGEST allows, and checks what it carries", {
    when() {
        // the reader may call create#Annotation and holds MONITOR_INGEST, the technical sources' permission
        sys(() => {
            const permission = ec.entity.makeValue("moqui.security.UserGroupPermission")
            permission.setFields(
                {userGroupId: "RDR_READER", userPermissionId: "MONITOR_INGEST", fromDate: new Date(Date.now() - 60000)},
                true,
                null,
                null
            )
            permission.create()
        })
        const technical = asReader(R + "create#Annotation", {source: "user", text: "A deploy note"})
        const refused = asReader(R + "create#Annotation", event({eventKey: "PriceListPublished:RAN_1"}))
        const written = asAdmin(
            R + "create#Annotation",
            event({eventKey: "PriceListPublished:RAN_1", recordType: "RanList", recordId: "RAN_1"})
        )
        const again = asAdmin(
            R + "create#Annotation",
            event({eventKey: "PriceListPublished:RAN_1", recordType: "RanList", recordId: "RAN_1"})
        )
        const otherScope = asAdmin(
            R + "create#Annotation",
            event({
                organizationPartyId: "RDR_B",
                text: "Period closed: March",
                event: "PeriodClosed",
                eventKey: "PeriodClosed:RAN_2"
            })
        )
        return {
            technical: technical.out?.annotation?.source,
            refused: refused.error || "",
            written: written.out || {},
            sameMarker: written.out?.annotationId === again.out?.annotationId,
            otherScope: otherScope.out?.annotation?.organizationPartyId,
            noOrganization: asAdmin(R + "create#Annotation", {source: "business", text: "No scope", event: "X"}).error || "",
            noEvent:
                asAdmin(R + "create#Annotation", {source: "business", text: "No event", organizationPartyId: "RDR_A"}).error ||
                "",
            otherHost: asAdmin(R + "create#Annotation", event({link: "//elsewhere.example/x"})).error || "",
            path: asAdmin(R + "create#Annotation", event({link: "/apps/lists/RAN_1", eventKey: "PriceListPublished:RAN_3"})).out
                ?.annotation?.link
        }
    },
    then({technical, refused, written, sameMarker, otherScope, noOrganization, noEvent, otherHost, path}) {
        technical == "user"
        refused.includes("org.moqui.report.annotation.business")
        written.annotationId.startsWith("business-")
        written.annotation.source == "business"
        written.annotation.organizationPartyId == "RDR_A"
        written.annotation.event == "PriceListPublished"
        written.annotation.recordId == "RAN_1"
        // a business event has no technical scope
        written.annotation.scope == null
        sameMarker
        otherScope == "RDR_B"
        noOrganization.includes("organizationPartyId")
        noEvent.includes("event")
        otherHost.includes("path on this server")
        path == "/apps/lists/RAN_1"
    }
})

feature("a dashboard over business data shows the events of a registered source in the viewer's scope only", {
    when() {
        // the source: a dataset over the annotations, scoped to the viewer's user groups on the event's organization
        sys(() => {
            const source = ec.entity.makeValue("moqui.basic.Enumeration")
            source.setFields(
                {
                    enumId: "RanEvents",
                    enumTypeId: "ReportAnnotationSource",
                    enumCode: "rdr-events",
                    description: "Report annotations spec"
                },
                true,
                null,
                null
            )
            source.create()
        })
        call(R + "save#Definition", {
            definition: {
                id: "rdr-events",
                type: "dataset",
                title: "Report annotations spec events",
                userGroupId: "RDR_READER",
                body: {
                    indexName: "monitor_annotations",
                    timeField: "time",
                    rowFilterService: R + "get#UserGroupRowFilter",
                    rowFilterParameters: {groupField: "organizationPartyId"}
                }
            }
        })
        const rendered = asReader(R + "render#Dashboard", Object.assign({definitionId: "rdr-panel"}, range))
        const markers = Array.from(rendered.out?.dashboard?.annotations || [])
        const panels = Object.values(rendered.out?.dashboard?.panels || {})
        // the source dataset holds the technical markers too (the note above has no organization, so the row filter drops it,
        // and the engine takes only business markers from a source whatever its dataset says)
        return {
            error: rendered.error || "",
            texts: markers
                .map(marker => marker.text)
                .sort()
                .join("|"),
            sources: Array.from(new Set(markers.map(marker => marker.source))).join(","),
            record: markers.find(marker => marker.recordId === "RAN_1") || {},
            onPanel: Array.from(panels[0]?.annotations || []).length
        }
    },
    then({error, texts, sources, record, onPanel}) {
        error == ""
        // the reader's organization's two events (RAN_1 written twice is one marker), never RDR_B's period close
        texts == "Price list published: Spring|Price list published: Spring"
        sources == "business"
        record.recordType == "RanList"
        record.event == "PriceListPublished"
        onPanel == 2
    }
})
