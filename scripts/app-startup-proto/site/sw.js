// Navigations: the kept page, cache-first (the page itself writes a new one after fresh data). Versioned files:
// cache-first. Everything else (data, the X-Shell fetch): the network, never kept.
const CACHE = 'shell-v3'
self.addEventListener('install', () => self.skipWaiting())
self.addEventListener('activate', e => e.waitUntil(self.clients.claim()))
self.addEventListener('fetch', e => {
  const url = new URL(e.request.url), nav = e.request.mode === 'navigate'
  if (!nav && !url.pathname.startsWith('/libs/') && !url.pathname.endsWith('/app.js')) return
  e.respondWith(caches.open(CACHE).then(async c => {
    const key = nav ? url.pathname : e.request
    const hit = await c.match(key)
    if (hit) return hit
    const r = await fetch(e.request)
    if (r.ok) c.put(key, r.clone())
    return r
  }))
})
