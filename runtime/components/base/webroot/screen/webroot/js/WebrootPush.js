/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The PWA and Web Push in the /qapps shell (plans/archive/Comms-Framework.md §7.2 P0): moqui.push, and the account menu's
 * "Notifications on this device" toggle (m-push-item).
 *
 * - start(): registers the service worker (/sw.js, root scope) and reconciles this browser with the server: a
 *   subscription the server does not hold for the signed-in user is ended in the browser (it was another account's, or
 *   the user turned notifications off elsewhere), so a push never crosses an account switch; one the user wanted here
 *   that the push service dropped is made again.
 * - enable(): from the user's click only. Asks the browser's permission, subscribes with the server's VAPID key
 *   (/WebPush/status) and stores the subscription (/WebPush/store).
 * - disable() and beforeLogout(): end the browser's subscription and forget it on the server (/WebPush/delete).
 *
 * Without push (a browser tab on iOS, which pushes only to a web app added to the Home Screen; an old browser; a server
 * without the web-push feature) the toggle still turns on desktop notifications while a page is open, and says why push
 * is not there. In-app notifications and their history work in every case.
 *
 * Loaded after MoquiLib.js and before WebrootVue.qvt.js (qapps.xml): its component is in moqui.qvtComponents, which
 * WebrootVue.qvt.js registers, and the shell root calls moqui.push.start() once mounted.
 */
;(function (root) {
    "use strict"

    const moqui = root.moqui
    const reactive = obj => (root.Vue && root.Vue.reactive ? root.Vue.reactive(obj) : obj)
    const nav = root.navigator || {}

    const state = reactive({
        // the browser can register a worker and subscribe; the server can push (built and keyed)
        browserPush: !!(nav.serviceWorker && root.PushManager && root.Notification),
        serverPush: false,
        // the browser's permission: default, granted or denied ("" without the Notification API)
        permission: root.Notification ? root.Notification.permission : "",
        // this browser is subscribed for the signed-in user
        subscribed: false,
        busy: false,
        // an iPhone or iPad not running the installed web app: push needs Add to Home Screen
        needsInstall: false
    })

    function wantedKey() {
        return root.moqui.scopedKey("moqui.push.wanted.") + ((root.moqui.webrootVue && root.moqui.webrootVue.userId) || "")
    }
    function setWanted(on) {
        try {
            if (on) root.localStorage.setItem(wantedKey(), "1")
            else root.localStorage.removeItem(wantedKey())
        } catch (err) {
            /* storage off: the toggle still works for this page */
        }
    }
    function wanted() {
        try {
            return root.localStorage.getItem(wantedKey()) === "1"
        } catch (err) {
            return false
        }
    }

    function keyBytes(base64url) {
        const padded = (base64url + "===".slice((base64url.length + 3) % 4)).replace(/-/g, "+").replace(/_/g, "/")
        const raw = root.atob(padded)
        const out = new Uint8Array(raw.length)
        for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i)
        return out
    }
    function b64url(buffer) {
        const bytes = new Uint8Array(buffer)
        let text = ""
        for (let i = 0; i < bytes.length; i++) text += String.fromCharCode(bytes[i])
        return root.btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")
    }

    function status(endpoint) {
        return moqui
            .request("/WebPush/status", {params: endpoint ? {endpoint} : {}})
            .then(result => result.json || {})
            .catch(() => ({}))
    }
    function store(subscription) {
        return moqui.request("/WebPush/store", {
            method: "POST",
            params: {
                endpoint: subscription.endpoint,
                p256dhKey: b64url(subscription.getKey("p256dh")),
                authSecret: b64url(subscription.getKey("auth"))
            }
        })
    }
    function forget(endpoint) {
        return moqui.request("/WebPush/delete", {method: "POST", params: {endpoint}}).catch(() => null)
    }

    // the shell's labels (qapps.xml, in the user's language), with English when a page has none
    const DEFAULT_LABELS = {
        pushNotifications: "Notifications on this device",
        pushBlocked: "Blocked in the browser's site settings",
        pushBlockedHelp: "Notifications are blocked for this site; allow them in the browser's site settings",
        pushInstall: "Add to Home Screen to get notifications when closed",
        pushPageOpen: "While a page is open",
        pushServerOff: "While a page is open; this server does not send push",
        pushClosed: "Also when the app is closed",
        pushFailed: "Could not turn on notifications on this device"
    }
    function label(key) {
        const labels = (moqui.webrootVue && moqui.webrootVue.labels) || {}
        return labels[key] || DEFAULT_LABELS[key]
    }

    let registration = null
    function worker() {
        if (registration) return Promise.resolve(registration)
        if (!nav.serviceWorker) return Promise.reject(new Error("no service worker"))
        return nav.serviceWorker.register("/sw.js", {scope: "/"}).then(found => (registration = found))
    }
    function current() {
        return worker()
            .then(found => found.pushManager.getSubscription())
            .catch(() => null)
    }

    function subscribe(publicKey) {
        return worker()
            .then(found => found.pushManager.subscribe({userVisibleOnly: true, applicationServerKey: keyBytes(publicKey)}))
            .then(subscription => store(subscription).then(() => subscription))
    }

    const push = {
        state,

        start() {
            const ios = /iPad|iPhone|iPod/.test(nav.userAgent || "") || (nav.platform === "MacIntel" && nav.maxTouchPoints > 1)
            const standalone =
                nav.standalone === true || (root.matchMedia && root.matchMedia("(display-mode: standalone)").matches)
            state.needsInstall = ios && !standalone && !state.browserPush
            if (!nav.serviceWorker) return Promise.resolve()
            nav.serviceWorker.addEventListener("message", event => {
                const data = event.data || {}
                // a push arrived while the page is open: the counts come from the server, not the push
                if (data.type === "moqui-push" && moqui.webrootVue && moqui.webrootVue.pollShellCounts)
                    moqui.webrootVue.pollShellCounts()
                if (data.type === "moqui-push-resubscribe") push.reconcile()
            })
            return push.reconcile()
        },

        // Match this browser's subscription to the server's for the signed-in user.
        reconcile() {
            if (!state.browserPush) {
                return status().then(found => {
                    state.serverPush = !!(found.supported && found.configured)
                })
            }
            return current().then(subscription =>
                status(subscription && subscription.endpoint).then(found => {
                    state.serverPush = !!(found.supported && found.configured)
                    state.permission = root.Notification.permission
                    if (!found.signedIn) return null
                    if (subscription && found.subscribed) {
                        state.subscribed = true
                        return null
                    }
                    if (subscription && !wanted()) {
                        // another account's, or turned off: this browser gets no more of its pushes
                        state.subscribed = false
                        return subscription.unsubscribe().catch(() => null)
                    }
                    if (state.serverPush && state.permission === "granted" && wanted()) {
                        // the user wants push here and the push service dropped it, or it is held for another user
                        const make = subscription ? store(subscription).then(() => subscription) : subscribe(found.publicKey)
                        return make
                            .then(() => {
                                state.subscribed = true
                            })
                            .catch(err => console.info("Web Push subscription not renewed", err))
                    }
                    state.subscribed = false
                    return null
                })
            )
        },

        // From the user's click: permission first, then the subscription.
        enable() {
            const Notification = root.Notification
            if (!Notification) return Promise.resolve(false)
            state.busy = true
            const asked = Notification.permission === "granted" ? Promise.resolve("granted") : Notification.requestPermission()
            return asked
                .then(permission => {
                    state.permission = permission
                    if (permission !== "granted") {
                        moqui.notifyGrowl({type: "warning", title: label("pushBlockedHelp")})
                        return false
                    }
                    if (!state.browserPush || !state.serverPush) return true
                    return status().then(found =>
                        subscribe(found.publicKey).then(() => {
                            setWanted(true)
                            state.subscribed = true
                            return true
                        })
                    )
                })
                .catch(err => {
                    console.warn("Web Push subscribe failed", err)
                    moqui.notifyGrowl({type: "negative", title: label("pushFailed")})
                    return false
                })
                .finally(() => {
                    state.busy = false
                })
        },

        disable() {
            state.busy = true
            setWanted(false)
            return current()
                .then(subscription => {
                    if (!subscription) return null
                    return forget(subscription.endpoint).then(() => subscription.unsubscribe().catch(() => null))
                })
                .then(() => {
                    state.subscribed = false
                })
                .finally(() => {
                    state.busy = false
                })
        },

        // Before the logout link is followed: end this browser's subscription. Bounded, so a slow network does not hold
        // the logout; the server also deletes the visit's subscriptions when it logs out.
        beforeLogout() {
            setWanted(false)
            const work = current().then(subscription => {
                if (!subscription) return null
                return forget(subscription.endpoint).then(() => subscription.unsubscribe().catch(() => null))
            })
            return Promise.race([work, new Promise(resolve => setTimeout(resolve, 1500))]).catch(() => null)
        }
    }
    moqui.push = push

    // ---- the account menu's toggle (registered by WebrootVue.qvt.js) ----
    const components = (moqui.qvtComponents = moqui.qvtComponents || {})
    components["m-push-item"] = {
        name: "mPushItem",
        data() {
            return {push: state}
        },
        computed: {
            on() {
                return (
                    this.push.permission === "granted" &&
                    (this.push.subscribed || !this.push.browserPush || !this.push.serverPush)
                )
            },
            caption() {
                if (this.push.permission === "denied") return label("pushBlocked")
                if (this.push.needsInstall) return label("pushInstall")
                if (!this.push.browserPush) return label("pushPageOpen")
                if (!this.push.serverPush) return label("pushServerOff")
                return this.on ? label("pushClosed") : ""
            },
            title() {
                return label("pushNotifications")
            }
        },
        methods: {
            toggle(value) {
                if (value) moqui.push.enable()
                else moqui.push.disable()
            }
        },
        template:
            '<q-item v-if="push.permission !== \'\'" id="m-account-push" tag="label" clickable>' +
            '<q-item-section avatar><q-icon name="notifications_active"></q-icon></q-item-section>' +
            "<q-item-section><q-item-label>{{title}}</q-item-label>" +
            '<q-item-label v-if="caption" caption>{{caption}}</q-item-label></q-item-section>' +
            '<q-item-section side><q-toggle dense :model-value="on" :disable="push.busy || push.permission === \'denied\'"' +
            ' @update:model-value="toggle"></q-toggle></q-item-section>' +
            "</q-item>"
    }
})(globalThis)
