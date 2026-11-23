// CC0 plus patent grant. A board version may be written after its last browser has left.
function parseScene(text) {
    if (typeof text !== 'string' || encodeURIComponent(text).replace(/%[0-9a-f]{2}/gi, 'x').length > Number(ec.factory.getConfValue('board@max-document-bytes')))
        throw Error(ec.l10n.localize('Board exceeds the document size limit'))
    const scene = JSON.parse(text)
    if (scene?.type !== 'excalidraw' || !Array.isArray(scene.elements) || scene.elements.some(e => !e || typeof e.id !== 'string' || !e.id))
        throw Error(ec.l10n.localize('Expected an Excalidraw scene with named elements'))
    const files = Object.values(scene.files || {})
    if (files.length > Number(ec.factory.getConfValue('board@max-images')) || files.some(file =>
            !file || file.dataURL != null || typeof file.attachmentName !== 'string' || !file.attachmentName ||
            /[\/\\#?]/.test(file.attachmentName) || ['.', '..'].includes(file.attachmentName) ||
            !['image/png','image/jpeg','image/webp','image/gif','image/svg+xml'].includes(file.mimeType)))
        throw Error(ec.l10n.localize('Board images must be page attachments within the board image limit'))
    return scene
}

// Closed color and geometry vocabularies keep board data out of SVG markup and external URLs.
function preview(scene, attachments) {
    const num = value => Number.isFinite(value) ? value : 0
    const color = (value, fallback) => /^#[0-9a-f]{3,8}$/i.test(value) ? value : fallback
    const shapes = []
    let left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity
    let imageBudget = Number(ec.factory.getConfValue('board@max-image-bytes')) * 0.7
    for (const el of scene.elements) {
        if (el.isDeleted || !el.type) continue
        const x = num(el.x), y = num(el.y), w = Math.max(1, num(el.width)), h = Math.max(1, num(el.height))
        const angle = num(el.angle), cos = Math.abs(Math.cos(angle)), sin = Math.abs(Math.sin(angle))
        const cx = x + w/2, cy = y + h/2, halfW = (w*cos + h*sin)/2, halfH = (w*sin + h*cos)/2
        left = Math.min(left, cx-halfW)
        top = Math.min(top, cy-halfH)
        right = Math.max(right, cx+halfW)
        bottom = Math.max(bottom, cy+halfH)
        const stroke = color(el.strokeColor, '#242424'), fill = color(el.backgroundColor, 'none')
        let body = `<rect width="${w}" height="${h}"/>`
        if (el.type === 'ellipse') {
            body = `<ellipse cx="${w/2}" cy="${h/2}" rx="${w/2}" ry="${h/2}"/>`
        } else if (el.type === 'diamond') {
            body = `<polygon points="${w/2},0 ${w},${h/2} ${w/2},${h} 0,${h/2}"/>`
        } else if (el.type === 'text') {
            const size = Math.max(1, num(el.fontSize) || 20), lines = String(el.text || '').split('\n')
            body = `<text stroke="none" fill="${stroke}" font-family="sans-serif" font-size="${size}">` +
                lines.map((line, i) => `<tspan x="0" y="${size*(1+i*(num(el.lineHeight)||1.2))}">${WebUtilities.encodeHtml(line)}</tspan>`).join('') + '</text>'
        } else if (['line','arrow','freedraw'].includes(el.type)) {
            const points = Array.isArray(el.points) ? el.points.filter(Array.isArray).map(p => [num(p[0]), num(p[1])]) : []
            body = `<polyline fill="none" points="${points.map(p => p.join(',')).join(' ')}"/>`
            if (el.type === 'arrow' && points.length > 1) {
                const [x1, y1] = points[points.length-2], [x2, y2] = points[points.length-1]
                body += `<path d="M -9 -4 L 0 0 L -9 4" fill="none" transform="translate(${x2} ${y2}) rotate(${Math.atan2(y2-y1, x2-x1)*180/Math.PI})"/>`
            }
        } else if (el.type === 'image') {
            const file = scene.files?.[el.fileId]
            const name = file?.attachmentName
            if (typeof name === 'string' && /^[\w.-]+\.(?:png|jpe?g|webp|gif)$/i.test(name) &&
                    ['image/png','image/jpeg','image/webp','image/gif'].includes(file.mimeType)) {
                const ref = attachments.getChild(name)
                const size = ref.exists ? ref.getSize() : 0
                if (size > 0 && size <= imageBudget) {
                    imageBudget -= size
                    const stream = ref.openStream()
                    try {
                        body = `<image width="${w}" height="${h}" href="data:${file.mimeType};base64,${StringUtilities.base64Encode(stream.readAllBytes())}"/>`
                    } finally { stream.close() }
                }
            }
        }
        shapes.push(`<g transform="translate(${x} ${y}) rotate(${angle*180/Math.PI} ${w/2} ${h/2})" stroke="${stroke}" stroke-width="${num(el.strokeWidth)||1}" fill="${fill}">${body}</g>`)
    }
    const box = shapes.length ? [left-8, top-8, right-left+16, bottom-top+16] : [0,0,320,180]
    const background = color(scene.appState?.viewBackgroundColor, '#ffffff')
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${box.join(' ')}"><rect x="${box[0]}" y="${box[1]}" width="${box[2]}" height="${box[3]}" fill="${background}"/>${shapes.join('')}</svg>`
}
;({parseScene, preview})
