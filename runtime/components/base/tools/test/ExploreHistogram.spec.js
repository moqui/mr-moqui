// A fixed range ends halfway through a minute. The only matching log sits in that last bucket.
spec({setupSpec() { login("john.doe") }})

feature("Explore includes a partial bucket when the matching document is in it", {
    expect() {
        const id = "explore-histogram-partial"
        ec.search.indexDocuments([{_type: "otel_logs", _id: id, timestamp: "2026-02-02T12:00:20Z", body: id}])
        try {
            const result = ec.screen.makeTest().rootScreen("component://tools/screen/Dashboards.xml")
                .renderMode("qvt").withStaticData().render("Explore", {lastStandalone: "true", datasetId: "otel_logs", queryString: 'body:"' + id + '"',
                    from: "2026-02-02T11:00:30Z", thru: "2026-02-02T12:00:30Z"}, null)
            assert.equal(result.status, 200)
            const panel = String(result.output).match(/<m-report-panel\b[^>]*v-bind="([^"]+)"/)
            assert.ok(panel, "the matching document must have a histogram")
            const props = JSON.parse(panel[1].replace(/&quot;/g, '"').replace(/&amp;/g, "&"))
            const count = props.frame.fields.find(field => field.name === "count")
            assert.equal(count.values.reduce((total, value) => total + Number(value), 0), 1)
        } finally {
            ec.search.deleteDocument("otel_logs", id)
        }
    }
})
