/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
/*
 * The service worker of the /qapps PWA (plans/Comms-Framework.md §7.2 P0). Served uncached at the site root
 * (screen/static_files.rs), registered by js/WebrootPush.js.
 *
 * Caching: only versioned shell assets (a `v` parameter, or under /libs/), which never change at a URL. Nothing
 * else: no screen render, transition, /rest, socket or any response that depends on who is signed in, so a shared
 * device keeps no private page and an account switch shows nothing of the last user's.
 *
 * Push: every push shows a notification (a browser may revoke push from a worker that shows none). The payload has the
 * title, link, topic and tag of a notification, never its body. A click focuses an open window and takes it to the
 * link, or opens one. Push is a hint: the notification list is the server's, and the page reads it again when it
 * comes back.
 */
const CACHE = "moqui-shell-v1"

self.addEventListener("install", () => self.skipWaiting())

self.addEventListener("activate", event => {
    event.waitUntil(
        caches
            .keys()
            .then(names => Promise.all(names.filter(name => name !== CACHE).map(name => caches.delete(name))))
            .then(() => self.clients.claim())
    )
})

function cacheable(request) {
    if (request.method !== "GET") return false
    const url = new URL(request.url)
    if (url.origin !== self.location.origin) return false
    return url.pathname.startsWith("/libs/") || url.searchParams.has("v")
}

self.addEventListener("fetch", event => {
    const request = event.request
    if (!cacheable(request)) return
    event.respondWith(
        caches.open(CACHE).then(cache =>
            cache.match(request).then(
                found =>
                    found ||
                    fetch(request).then(response => {
                        // only a whole, public, same-origin answer is kept
                        const control = response.headers.get("Cache-Control") || ""
                        if (response.ok && response.type === "basic" && !/private|no-store/i.test(control)) {
                            cache.put(request, response.clone())
                        }
                        return response
                    })
            )
        )
    )
})

self.addEventListener("push", event => {
    let data = {}
    try {
        data = event.data ? event.data.json() : {}
    } catch (err) {
        data = {}
    }
    const title = data.title || "New notification"
    const options = {
        body: data.body || "",
        tag: data.tag || undefined,
        icon: "/images/pwa-icon-192.png",
        badge: "/images/pwa-icon-192.png",
        data: {link: data.link || "/qapps", topic: data.topic || ""}
    }
    event.waitUntil(
        self.registration.showNotification(title, options).then(() =>
            // an open page refreshes its counts; the server's list is what it shows
            self.clients
                .matchAll({type: "window", includeUncontrolled: true})
                .then(windows => windows.forEach(client => client.postMessage({type: "moqui-push", topic: options.data.topic})))
        )
    )
})

function sameOriginLink(link) {
    try {
        const url = new URL(link || "/qapps", self.location.origin)
        return url.origin === self.location.origin ? url.href : new URL("/qapps", self.location.origin).href
    } catch (err) {
        return new URL("/qapps", self.location.origin).href
    }
}

self.addEventListener("notificationclick", event => {
    event.notification.close()
    const target = sameOriginLink(event.notification.data && event.notification.data.link)
    event.waitUntil(
        self.clients.matchAll({type: "window", includeUncontrolled: true}).then(windows => {
            const open = windows.find(client => new URL(client.url).pathname.startsWith("/qapps")) || windows[0]
            if (open) return open.focus().then(client => (client && client.navigate ? client.navigate(target) : null))
            return self.clients.openWindow(target)
        })
    )
})

// The push service replaced or ended the subscription. A worker holds no session token to store a new one, so an open
// page subscribes again now, and any page does when it next loads (js/WebrootPush.js checks on each load).
self.addEventListener("pushsubscriptionchange", event => {
    event.waitUntil(
        self.clients
            .matchAll({type: "window", includeUncontrolled: true})
            .then(windows => windows.forEach(client => client.postMessage({type: "moqui-push-resubscribe"})))
    )
})
