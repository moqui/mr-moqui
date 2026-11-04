/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The page bridge of the mobile app (src/host.rs, plans/Mobile-Desktop.md §4): window.moquiDesktop, the host adapter the
// webroot client and the shell call where it is there, as the desktop app's bridge.js has it for a client. One script
// for the one webview: on the shell's origin it has the shell's commands, on the server's pages' (moqui-app) the
// sockets, the outbox and the drafts and a way back to the shell. The host checks every command again by the page
// (the shell's need the shell's page); nothing here is the boundary.
//
// Android's WebView hands the app's scheme no request body, so a page's fetch with one goes to the host as a command
// (scheme_fetch), answered as the scheme would, and its redirect followed here.
(function () {
    "use strict";
    const shell = location.origin === "http://moqui-shell.localhost";
    const appPage = location.origin === "http://moqui-app.localhost";
    // the main frame only: a frame of the app's origins in a page gets no adapter (the host takes no IPC from frames,
    // and gives frames of other origins no scripts at all: patches/README.md)
    if (window.moquiDesktop || !(shell || appPage) || window.top !== window.self) return;
    const invoke = (cmd, args) => {
        const ipc = window.__TAURI_INTERNALS__;
        if (!ipc || !ipc.invoke) return Promise.resolve(null);
        return ipc.invoke(cmd, args).catch((err) => {
            console.info("moquiDesktop " + cmd + " failed", err);
            return null;
        });
    };
    // a command whose failure the caller hears (the shell's sign-in shows the server's reason)
    const ask = (cmd, args) => {
        const ipc = window.__TAURI_INTERNALS__;
        if (!ipc || !ipc.invoke) return Promise.reject(new Error("Not in the MR Moqui app"));
        return ipc.invoke(cmd, args).catch((err) => { throw new Error(String((err && err.message) || err)); });
    };
    const text = (value) => (value === undefined || value === null ? null : String(value));
    // The shell's commands carry the nonce of the shell's current load, which the host hands this document once it has
    // loaded (host.rs nonce_script); they wait for it. A document that came in the shell's place never gets it.
    let shellNonce = null;
    let nonceArrived = null;
    const nonceReady = new Promise((resolve) => { nonceArrived = resolve; });
    if (shell) {
        Object.defineProperty(window, "__moquiShellNonce", { configurable: false, writable: false,
            value: (nonce) => { if (shellNonce === null) { shellNonce = String(nonce); nonceArrived(); } } });
    }
    const shellAsk = (cmd, args) => nonceReady.then(() => ask(cmd, Object.assign({}, args || {}, { nonce: shellNonce })));

    // fetch with a body, through the host
    const pageFetch = window.fetch.bind(window);
    const toBase64 = (buffer) => {
        const bytes = new Uint8Array(buffer);
        let raw = "";
        for (let i = 0; i < bytes.length; i += 0x8000) raw += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
        return btoa(raw);
    };
    const fromBase64 = (data) => {
        const raw = atob(String(data || ""));
        const bytes = new Uint8Array(raw.length);
        for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
        return bytes;
    };
    const viaHost = (request, hops) => request.arrayBuffer().then((buffer) => {
        const headers = {};
        request.headers.forEach((value, name) => { headers[name] = value; });
        return ask("scheme_fetch", { method: request.method, url: request.url, headers,
            body: buffer.byteLength ? toBase64(buffer) : null });
    }).then((answer) => {
        const location = answer.headers && answer.headers.location;
        if (answer.status >= 300 && answer.status < 400 && location && request.redirect !== "manual" && hops < 10) {
            const next = new URL(location, request.url);
            // 307 and 308 keep the method and body; the rest are a GET, which the scheme takes
            if (answer.status === 307 || answer.status === 308) return viaHost(new Request(next.href, request), hops + 1);
            return pageFetch(next.href, { credentials: request.credentials, headers: request.headers });
        }
        const nullBody = answer.status === 204 || answer.status === 205 || answer.status === 304;
        return new Response(nullBody ? null : fromBase64(answer.body), { status: answer.status, headers: answer.headers });
    });
    window.fetch = function (input, init) {
        let request;
        try { request = new Request(input, init); } catch (err) { return pageFetch(input, init); }
        const sameOrigin = new URL(request.url, location.href).origin === location.origin;
        if (!sameOrigin || request.method === "GET" || request.method === "HEAD") return pageFetch(input, init);
        return viaHost(request.clone(), 0);
    };

    // The server's socket of a page: the host opens the URL's path on the account's server with its key, and hands
    // each frame back to __moquiSocketEvent. As a WebSocket to the page (moqui.openSocket).
    const sockets = {};
    const early = {};
    const ended = new Set();
    window.__moquiSocketEvent = (id, event) => {
        const socket = sockets[id];
        if (socket) { socket.deliver(event); return; }
        if (ended.has(id)) return;
        const now = Date.now();
        Object.keys(early).forEach((key) => { if (now - early[key].at > 10000) delete early[key]; });
        const waiting = early[id] || (early[id] = { at: now, events: [] });
        if (waiting.events.length < 20) waiting.events.push(event);
    };
    function HostSocket(url) {
        this.url = String(url);
        this.readyState = 0;
        this.id = null;
        let path = "/";
        try { const parsed = new URL(this.url, location.href); path = parsed.pathname + parsed.search; } catch (err) { path = "/"; }
        ask("socket_open", { path }).then((id) => {
            if (this.readyState >= 2) { ended.add(id); delete early[id]; invoke("socket_close", { id }); return; }
            this.id = id;
            sockets[id] = this;
            const waiting = early[id] ? early[id].events : [];
            delete early[id];
            waiting.forEach((event) => this.deliver(event));
        }, (err) => {
            console.info("The host's socket did not open", err);
            this.ended(1006);
        });
    }
    HostSocket.prototype.deliver = function (event) {
        if (!event) return;
        if (event.kind === "open") {
            this.readyState = 1;
            if (this.onopen) this.onopen({ type: "open" });
        } else if (event.kind === "text") {
            if (this.onmessage) this.onmessage({ type: "message", data: event.text });
        } else this.ended(event.kind === "revoked" ? 1008 : 1006);
    };
    HostSocket.prototype.ended = function (code) {
        if (this.readyState === 3) return;
        this.readyState = 3;
        if (this.id !== null) { delete sockets[this.id]; ended.add(this.id); invoke("socket_close", { id: this.id }); }
        if (this.onclose) this.onclose({ type: "close", code, wasClean: code === 1000 });
    };
    HostSocket.prototype.send = function (data) {
        if (this.readyState !== 1 || this.id === null) throw new Error("The socket is not open");
        invoke("socket_send", { id: this.id, text: String(data) });
    };
    HostSocket.prototype.close = function () {
        if (this.readyState >= 2) return;
        this.readyState = 2;
        setTimeout(() => this.ended(1000), 0);
    };
    const socket = (url) => new HostSocket(url);

    // The browser storage of accounts whose data the host deleted, under their storage scopes (MoquiLib scopedKey).
    const clearScopes = (scopes) => {
        const prefixes = (Array.isArray(scopes) ? scopes : []).filter((scope) => typeof scope === "string" && scope).map((scope) => scope + ":");
        if (!prefixes.length) return Promise.resolve();
        const owned = (name) => prefixes.some((prefix) => String(name).indexOf(prefix) === 0);
        [window.localStorage, window.sessionStorage].forEach((storage) => {
            try {
                for (let i = storage.length - 1; i >= 0; i--) {
                    const key = storage.key(i);
                    if (key && owned(key)) storage.removeItem(key);
                }
            } catch (err) {
                /* storage off: nothing kept */
            }
        });
        const idb = window.indexedDB;
        if (!idb || !idb.databases) return Promise.resolve();
        return idb.databases().then((databases) => Promise.all(databases.filter((db) => db.name && owned(db.name)).map((db) => new Promise((resolve) => {
            const request = idb.deleteDatabase(db.name);
            request.onsuccess = request.onerror = request.onblocked = () => resolve();
        })))).catch((err) => console.info("Deleted accounts' browser databases not all removed", err));
    };
    const outboxListeners = [];
    window.__moquiOutboxEvent = (event) => {
        outboxListeners.slice().forEach((fn) => {
            try { fn(event || {}); } catch (err) { console.error("moquiDesktop outbox listener failed", err); }
        });
    };
    const outbox = Object.freeze({
        add: (kind, payload, files) => ask("outbox_add", { kind: String(kind || ""), payload: payload || {}, files: Array.isArray(files) ? files.map(String) : null }),
        list: () => ask("outbox_list"),
        // {clientApi, kinds, internalSend}: what the account's server takes queued (MD5)
        kinds: () => ask("outbox_kinds"),
        retry: (id) => ask("outbox_retry", { id: String(id || "") }),
        discard: (id) => ask("outbox_discard", { id: String(id || "") }),
        on: (fn) => {
            if (typeof fn !== "function") return () => {};
            outboxListeners.push(fn);
            return () => { const at = outboxListeners.indexOf(fn); if (at >= 0) outboxListeners.splice(at, 1); };
        }
    });
    const base64Of = (blob) => new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => { const data = String(reader.result || ""); resolve(data.slice(data.indexOf(",") + 1)); };
        reader.onerror = () => reject(reader.error || new Error("The file was not read"));
        reader.readAsDataURL(blob);
    });
    const drafts = Object.freeze({
        get: (key) => ask("draft_get", { key: String(key || "") }),
        list: (prefix) => ask("draft_list", { prefix: String(prefix || "") }),
        put: (key, value, files) => ask("draft_put", { key: String(key || ""), value: String(value ?? ""), files: Array.isArray(files) ? files.map(String) : null }),
        remove: (key) => ask("draft_remove", { key: String(key || "") }),
        saveFile: (file, name) => base64Of(file).then((data) => ask("file_save", { name: String(name || (file && file.name) || "file"), data }))
    });
    const notify = (options) => {
        const given = options || {};
        return invoke("notify", { title: String(given.title || ""), body: text(given.body), link: null, tag: text(given.tag) });
    };
    const windowSeen = () => !document.hidden;

    if (appPage) {
        // a server's page: its account's storage scope and the scopes to clear, asked of the host as the page starts
        // (the one init script cannot carry them); the shell owns the chat crypto device
        let page = { storageScope: "", clearScopes: [] };
        try {
            const request = new XMLHttpRequest();
            request.open("GET", "/__moqui-app/page", false);
            request.send(null);
            if (request.status === 200) page = JSON.parse(request.responseText);
        } catch (err) {
            console.info("The host's page settings were not read", err);
        }
        window.__moquiCryptoInShell = true;
        if (Array.isArray(page.clearScopes) && page.clearScopes.length) clearScopes(page.clearScopes);
        const backToShell = () => ask("client_show_shell");
        window.moquiDesktop = Object.freeze({ socket, outbox, drafts, notify, windowSeen,
            storageScope: String(page.storageScope || ""),
            openInShell: (path) => ask("open_in_shell", { path: String(path || "") }),
            openExternal: (url) => invoke("open_external", { url: String(url || "") }),
            backToShell });
        // the way back to the shell: a small button over the server's page
        const addButton = () => {
            if (!document.body || document.getElementById("moqui-back-to-shell")) return;
            const button = document.createElement("button");
            button.id = "moqui-back-to-shell";
            button.type = "button";
            button.title = "Back to the app";
            button.setAttribute("aria-label", "Back to the app");
            button.textContent = "⌂";
            button.style.cssText = "position:fixed;left:12px;bottom:calc(12px + env(safe-area-inset-bottom));z-index:2147483647;" +
                "width:44px;height:44px;border-radius:22px;border:none;background:rgba(25,118,210,.9);color:#fff;" +
                "font-size:22px;line-height:44px;padding:0;box-shadow:0 2px 6px rgba(0,0,0,.3);";
            button.addEventListener("click", backToShell);
            document.body.appendChild(button);
        };
        if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", addButton);
        else addButton();
        return;
    }
    window.moquiDesktop = Object.freeze({
        socket, drafts, notify, windowSeen, clearScopes,
        outbox: Object.freeze(Object.assign({ unsent: () => ask("outbox_unsent") }, outbox)),
        setCounts: () => Promise.resolve(null),
        setPresence: () => Promise.resolve(null),
        openExternal: (url) => invoke("open_external", { url: String(url || "") }),
        state: () => shellAsk("client_state"),
        deviceName: String(window.__moquiDeviceName || ""),
        signIn: (given) => shellAsk("client_sign_in", { server: String(given.server || ""), username: String(given.username || ""),
            password: String(given.password || ""), code: given.code ? String(given.code) : null,
            deviceMode: given.deviceMode ? String(given.deviceMode) : null }),
        sendCode: (given) => shellAsk("client_send_code", { server: String(given.server || ""), username: String(given.username || ""),
            password: String(given.password || ""), factor: String(given.factor || "") }),
        signOut: () => shellAsk("client_sign_out"),
        switchAccount: (id) => shellAsk("client_switch_account", { id: String(id) }),
        addAccount: () => shellAsk("client_add_account"),
        removeAccount: (id) => shellAsk("client_remove_account", { id: String(id) }),
        openRemote: (path) => shellAsk("client_open_remote", { path: String(path || "") }),
        takeOpen: () => shellAsk("client_take_open"),
        ssoProviders: (server) => shellAsk("client_sso_providers", { server: String(server || "") }),
        ssoStart: (server, authFlowId, deviceMode) => shellAsk("client_sso_start", { server: String(server || ""), authFlowId: String(authFlowId || ""),
            deviceMode: deviceMode ? String(deviceMode) : null }),
        ssoCancel: () => shellAsk("client_sso_cancel"),
        // the shell's call joined or over (WebrootCall.js): the phone's native call (MT2) is kept while it lasts
        callState: (callSessionId, active) => shellAsk("call_state", { callSessionId: String(callSessionId || ""), active: !!active })
            .catch((err) => console.info("The call state was not told to the app", err))
    });
    console.info("moqui-mobile: the shell's bridge is ready");
})();
