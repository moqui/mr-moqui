// The address chips of an email header, and the actual recipients of a message the account sends.
const splitAddresses = (text) => {
    const out = []
    if (!text) return out
    let current = '', quote = false, angle = 0
    for (const ch of String(text)) {
        if (ch === '"') quote = !quote
        else if (ch === '<' && !quote) angle++
        else if (ch === '>' && !quote && angle > 0) angle--
        else if (ch === ',' && !quote && angle === 0) { if (current.trim()) out.push(current.trim()); current = ''; continue }
        current += ch
    }
    if (current.trim()) out.push(current.trim())
    return out.map((entry) => {
        const match = /^(.*)<([^>]+)>\s*$/.exec(entry)
        if (!match) return {address: entry, name: null}
        const name = match[1].trim().replace(/^"(.*)"$/, '$1').trim()
        return {address: match[2].trim(), name: name || null}
    })
}

const recipientsOf = (message) => ['ES_DRAFT', 'ES_READY', 'ES_SCHEDULED', 'ES_SENT'].includes(message.statusId)
    ? [].concat(splitAddresses(message.toAddresses), splitAddresses(message.ccAddresses), splitAddresses(message.bccAddresses)).filter((entry) => entry.address) : null
;({splitAddresses, recipientsOf})
