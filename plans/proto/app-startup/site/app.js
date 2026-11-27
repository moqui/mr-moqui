const rows = d => d.items.map(i => `<li elementtiming="row">${i}</li>`).join('')
const render = d => { document.getElementById('list').innerHTML = rows(d) }
const mark = n => { if (!performance.getEntriesByName(n).length) performance.mark(n) }
async function main() {
  const v = window.VARIANT
  if (window.SHOWN || document.querySelector('#list li[data-last]')) mark('content')
  const d = await (await fetch('data')).json()
  const first = !window.SHOWN
  render(d); mark('content'); mark('fresh')
  if (first && window.probe) requestAnimationFrame(() => setTimeout(() => probe.shown(d.items.slice(0, 15).join('\n'))))
  if (v === 'f') localStorage.setItem('last:u1', JSON.stringify(d))
  if (v === 'b') { localStorage.setItem('last:u1', JSON.stringify(d)); navigator.serviceWorker.register('sw.js') }
  if (v === 'd' || v === 'e') {
    navigator.serviceWorker.register('sw.js')
    const page = await (await fetch(location.pathname, { cache: 'no-store', headers: { 'X-Shell': '1' } })).text()
    const html = page.replace("<!--LAST--><li>Loading…</li>", d.items.slice(0, v === 'e' ? 15 : 40).map(i => `<li data-last elementtiming="row">${i}</li>`).join(""))
    const c = await caches.open('shell-v3'); await c.put(location.pathname, new Response(html, { headers: { 'Content-Type': 'text/html' } }))
  }
}
main()
