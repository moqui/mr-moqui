// Expand a wiki board reference where markdown accepts wiki links. Look up only previously indexed
// targets visible to this reader. Other links keep the markdown renderer's normal behavior.
function expandBoards(text, pageId) {
    if (!pageId || !text.includes('[[')) return text
    const call = (verb, params) => ec.service.sync().name('org.moqui.resource.WikiServices.' + verb).parameters(params).call()
    const links = call('get#WikiPageLinks', {wikiPageId:pageId}).linkList || []
    const targets = new Map(links.filter(link => link.found && link.linkTarget.startsWith('wiki:'))
        .map(link => [link.linkTarget, link.toWikiPageId]))
    const boards = new Map()
    let fence = ''
    return text.split('\n').map(line => {
        const delimiter = line.match(/^ {0,3}(`{3,}|~{3,})/)
        if (delimiter) {
            const token = delimiter[1]
            if (!fence) fence = token
            else if (token[0] === fence[0] && token.length >= fence.length) fence = ''
            return line
        }
        if (fence) return line
        return line.split(/(`[^`]*`)/).map((part,index) => index % 2 ? part :
            part.replace(/\[\[([^\]|\n]+)(?:\|([^\]\n]+))?\]\]/g, (original,path,label) => {
                const id = targets.get('wiki:' + path.trim())
                if (!id) return original
                if (!boards.has(id)) {
                    const info = call('get#WikiPageInfo', {wikiPageId:id})
                    boards.set(id,info.wikiType === 'excalidraw' ? info : null)
                }
                const board = boards.get(id)
                if (!board) return original
                const url = (ec.factory.getConfValue('comms@app-root') || '/qapps/BasicComms').replace(/\/+$/, '') + '/Boards/Board'
                const query = '?wikiPageId=' + encodeURIComponent(id)
                const title = WebUtilities.encodeHtml(label || board.pageName)
                const preview = (board.attachmentList || []).some(a => a.filename === '_board.svg')
                    ? '<img alt="" src="' + url + '/image' + query + '&amp;filename=_board.svg"> ' : ''
                return '<a href="' + url + query + '">' + preview + title + '</a>'
            })).join('')
    }).join('\n')
}
// After a version is saved, the board's files its scene no longer shows go, so a board's attachments do not grow
// with every image ever placed on it (board@max-total-image-bytes caps them). Kept: the previews (_board*.svg), a
// meeting snapshot's capture.png, and a file written in the last graceMillis (ten minutes), whose element may not have reached the
// saved scene yet. An older version that showed a removed image shows it no more. Out: the names removed; a file that
// will not delete is logged and the others go on.
function pruneFiles(wikiPageId, versionName, graceMillis = 600000) {
    const info = ec.service.sync().name('org.moqui.resource.WikiServices.get#WikiPageInfo').parameters({wikiPageId}).disableAuthz().call() || {}
    if (info.wikiType !== 'excalidraw' || !info.pageReference) return []
    let scene
    try { scene = JSON.parse(info.pageReference.getText(versionName || null)) }
    catch (e) { ec.logger.warn('Board ' + wikiPageId + ' version ' + versionName + ' is not a scene; its files are kept: ' + e); return [] }
    const files = scene?.files || {}
    const shown = new Set((Array.isArray(scene?.elements) ? scene.elements : [])
        .filter(e => e && !e.isDeleted && e.type === 'image' && files[e.fileId]?.attachmentName)
        .map(e => files[e.fileId].attachmentName))
    const since = Number(ec.user.nowTimestamp) - graceMillis
    // when the file was written: its time, else (a database file) its current version's
    const written = file => Number(file.lastModified || 0) ||
        Number(file.resourceReference.supportsVersion() ? file.resourceReference.getCurrentVersion()?.getVersionDate() || 0 : 0) ||
        Number(ec.user.nowTimestamp)
    const removed = []
    for (const file of Array.from(info.attachmentList || [])) {
        const name = String(file.filename)
        if (shown.has(name) || /^_board.*\.svg$/.test(name) || name === 'capture.png' || (graceMillis > 0 && written(file) > since)) continue
        try { file.resourceReference.delete(); removed.push(name) }
        catch (e) { ec.logger.error('Could not remove board file ' + name + ' of page ' + wikiPageId + ': ' + e) }
    }
    return removed
}
;({expandBoards, pruneFiles})
