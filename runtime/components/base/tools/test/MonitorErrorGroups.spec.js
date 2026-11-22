feature("Top error groups labels a missing message by type or short fingerprint", {
    expect() {
        const {labelFrames} = ec.resource.script("component://tools/template/MonitorErrorGroups.js")
        const messages = {name: "message", values: ["Example failure", " ", null]}
        const types = {name: "exception.type", values: ["Exception", "Timeout", "  "]}
        const fingerprints = {name: "fingerprint", values: ["aaaaaaaa0011", "bbbbbbbb0022", "cccccccc0033"]}
        const missing = {fields: [fingerprints]}
        labelFrames([{fields: [messages, types, fingerprints]}, missing])
        missing.fields[0].values[0] == "Error group aaaaaaaa"
        messages.values == ["Example failure", "Timeout", "Error group cccccccc"]
        fingerprints.values[2] == "cccccccc0033"
    }
})
