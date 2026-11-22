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
                const url = '/qapps/BasicComms/Boards/Board'
                const query = '?wikiPageId=' + encodeURIComponent(id)
                const title = WebUtilities.encodeHtml(label || board.pageName)
                const preview = (board.attachmentList || []).some(a => a.filename === '_board.svg')
                    ? '<img alt="" src="' + url + '/image' + query + '&amp;filename=_board.svg"> ' : ''
                return '<a href="' + url + query + '">' + preview + title + '</a>'
            })).join('')
    }).join('\n')
}
;({expandBoards})
