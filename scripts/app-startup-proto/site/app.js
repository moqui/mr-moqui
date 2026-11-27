const db = () => new Promise((ok, no) => { const r = indexedDB.open('last', 1); r.onupgradeneeded = () => r.result.createObjectStore('s'); r.onsuccess = () => ok(r.result); r.onerror = no })
const iget = async k => { const d = await db(); return new Promise(ok => { const r = d.transaction('s').objectStore('s').get(k); r.onsuccess = () => ok(r.result); r.onerror = () => ok(null) }) }
const iput = async (k, v) => { const d = await db(); d.transaction('s', 'readwrite').objectStore('s').put(v, k) }
const rows = d => d.items.map(i => `<li elementtiming="row">${i}</li>`).join('')
const render = d => { document.getElementById('list').innerHTML = rows(d) }
const mark = n => { if (!performance.getEntriesByName(n).length) performance.mark(n) }
async function main() {
  const v = window.VARIANT
    if (window.SHOWN || document.querySelector('#list li[data-last]')) mark('content')
  const d = await (await fetch('data')).json()
  render(d); mark('content'); mark('fresh')
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
