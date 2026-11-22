// The visible caption on the Monitor screens. A detail link still addresses the full fingerprint.
function label(group) {
    const message = String(group.message || "").trim()
    const type = String(group["exception.type"] || "").trim()
    const fingerprint = String(group.fingerprint || "").slice(0, 8)
    return message || type || (fingerprint ? "Error group " + fingerprint : "Error group")
}

// The overview table receives a columnar report frame. Only its displayed message values change.
function labelFrames(frames) {
    for (const frame of frames) {
        const fields = Object.fromEntries(frame.fields.map(field => [field.name, field]))
        if (!fields.fingerprint) continue
        const values = fields.fingerprint.values.map((fingerprint, row) => label({
            message: fields.message?.values[row], "exception.type": fields["exception.type"]?.values[row], fingerprint
        }))
        if (fields.message) fields.message.values = values
        else frame.fields.unshift({name: "message", type: "string", values})
    }
}
;({label, labelFrames})
