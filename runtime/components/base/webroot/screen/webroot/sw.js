/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
/*
 * The service worker of the /qapps PWA (plans/archive/Comms-Framework.md §7.2 P0). Served uncached at the site root
 * (screen/static_files.rs), registered by js/WebrootPush.js.
 *
 * Caching: only versioned shell assets (a `v` parameter, or under /libs/), which never change at a URL. Nothing
 * else: no screen render, transition, /rest, socket or any response that depends on who is signed in, so a shared
 * device keeps no private page and an account switch shows nothing of the last user's.
 *
 * Push: a push shows a notification (a browser may revoke push from a worker that shows none). The payload has the
 * title, link, topic and tag of a notification, never its body. A chat push (kind message) has its room, sender, count
 * and, when the server put one in, a snippet: the title and body are built from them, and it is acknowledged
 * (/qapps/ackClientPush). A clear (kind clear: the room was read on another device) closes the room's notifications
 * (its tag) and shows nothing; it is sent at normal urgency and only after a push of the room was shown. A click
 * focuses an open window and takes it to the link, or opens one. Push is a hint: the notification list is the
 * server's, and the page reads it again when it comes back.
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

// A chat push's title and body from its fields (plans/Comms-Messaging.md §7.3, Chat push): the room and sender, the
// snippet when the server put one in (a plain room, previews on), and how many messages it stands for.
function chatTitle(data) {
    const sender = data.senderName || ""
    const place = data.roomName || sender
    if (data.call === "missed") return "Missed call" + (place ? " · " + place : "")
    if (data.call === "voicemail") return "Voicemail" + (place ? " · " + place : "")
    if (Number(data.count) > 1) return data.count + " new messages" + (place ? " · " + place : "")
    return data.roomName && !data.direct ? sender + " · " + data.roomName : sender || data.title || "New message"
}

// The server counts what the device showed (System, Push Health). The worker has no session token: the transition takes
// a push id alone, counts only the signed-in user's own push, and a failure changes nothing for the person.
function ack(ackId) {
    if (!ackId) return Promise.resolve()
    return fetch("/qapps/ackClientPush", {
        method: "POST",
        credentials: "same-origin",
        headers: {"Content-Type": "application/x-www-form-urlencoded"},
        body: "ackId=" + encodeURIComponent(ackId)
    }).catch(() => null)
}

self.addEventListener("push", event => {
    let data = {}
    try {
        data = event.data ? event.data.json() : {}
    } catch (err) {
        data = {}
    }
    // a room read on another device: its notifications close, and nothing is shown
    if (data.kind === "clear") {
        event.waitUntil(
            self.registration
                .getNotifications(data.tag ? {tag: data.tag} : undefined)
                .then(shown => shown.forEach(notification => notification.close()))
        )
        return
    }
    const chat = data.kind === "message"
    const title = chat ? chatTitle(data) : data.title || "New notification"
    const options = {
        body: chat ? data.snippet || (data.encrypted ? "New message" : "") : data.body || "",
        tag: data.tag || undefined,
        icon: "/images/pwa-icon-192.png",
        badge: "/images/pwa-icon-192.png",
        data: {link: data.link || "/qapps", topic: data.topic || ""}
    }
    event.waitUntil(
        self.registration
            .showNotification(title, options)
            .then(() => (data.ackId ? ack(data.ackId) : null))
            .then(() =>
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
