/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The page bridge of the desktop app (src/bridge.rs, plans/Mobile-Desktop.md section 4, configuration 1): the host
// adapter window.moquiDesktop that the webroot client calls when it is there (MoquiLib.js notifyNotification and
// saveBlob, WebrootVue.qvt.js openLink and the shell's counts and presence, WebrootCall.js the ring). An
// initialization script of the main window, so it runs on every page the window shows; it defines the adapter only on
// the node's loopback origin, and the host's capability lets only that exact origin call the commands. A client of a
// server (src/client/) has it on its shell, with the accounts and sign-in, and on its app windows with the sockets, the
// outbox and the drafts only.
//
// No saveFile: a download link (moqui.saveBlob without it, an export link, an attachment) goes to Chromium's download,
// which the host takes with a save dialog and a notification when done (bridge.rs on_download).
(function () {
    "use strict";
    // the node's loopback origin (configuration 1, and 2 with its ports open), its app scheme in process (configuration 2,
    // src/node/scheme.rs), or the client's shell and app windows (configuration 3, src/client/)
    const inProcess = location.origin === "http://moqui-node.localhost";
    const node = inProcess || (location.protocol === "http:" && location.hostname === "127.0.0.1");
    const shell = location.origin === "http://moqui-shell.localhost";
    const appWindow = location.origin === "http://moqui-app.localhost";
    if (window.moquiDesktop || !(node || shell || appWindow)) return;
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
    // a link of the page as a full URL, so the host can tell it is on the node's origin; the shell's links are its
    // server's, which the app windows show
    const linkBase = shell ? "http://moqui-app.localhost/" : location.href;
    const absolute = (link) => {
        if (!link) return null;
        try {
            return new URL(String(link), linkBase).href;
        } catch (err) {
            return null;
        }
    };

    // The server's socket of a client's page (src/client/sockets.rs): the host opens the URL's path on the page's
    // account's server with its key, and hands each frame back to __moquiSocketEvent. As a WebSocket to the page
    // (moqui.openSocket): readyState, send, close, and onopen, onmessage, onclose, onerror.
    const sockets = {};
    // events of a socket the page has not heard the id of yet: the host's open can come before socket_open's answer;
    // kept a few seconds, a few per socket, and none for a socket that ended
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
        } else if (event.kind === "binary") {
            // the terminal's bytes (the node in process), in base64 from the host; as binaryType arraybuffer
            const raw = atob(String(event.data || ""));
            const bytes = new Uint8Array(raw.length);
            for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
            if (this.onmessage) this.onmessage({ type: "message", data: bytes.buffer });
        } else this.ended(event.kind === "revoked" ? 1008 : 1006, event.reason);
    };
    // the reason of a close the server gave (the terminal's: "opened elsewhere"), as a browser socket's close event has it
    HostSocket.prototype.ended = function (code, reason) {
        if (this.readyState === 3) return;
        this.readyState = 3;
        if (this.id !== null) { delete sockets[this.id]; ended.add(this.id); invoke("socket_close", { id: this.id }); }
        if (this.onclose) this.onclose({ type: "close", code: reason ? 1000 : code, reason: reason || "", wasClean: code === 1000 || !!reason });
    };
    HostSocket.prototype.send = function (data) {
        if (this.readyState !== 1 || this.id === null) throw new Error("The socket is not open");
        if (data instanceof ArrayBuffer || ArrayBuffer.isView(data)) {
            const bytes = data instanceof ArrayBuffer ? new Uint8Array(data) : new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
            let raw = "";
            for (let i = 0; i < bytes.length; i += 0x8000) raw += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
            invoke("socket_send", { id: this.id, text: btoa(raw), binary: true });
            return;
        }
        invoke("socket_send", { id: this.id, text: String(data) });
    };
    HostSocket.prototype.close = function () {
        if (this.readyState >= 2) return;
        this.readyState = 2;
        // as a browser socket: the close is heard after the call returns
        setTimeout(() => this.ended(1000), 0);
    };

    const pageCommands = {
        notify: (options) => {
            const given = options || {};
            return invoke("notify", { title: String(given.title || ""), body: text(given.body), link: absolute(given.link), tag: text(given.tag) });
        },
        // the shellCounts of the shell, each a number
        setCounts: (counts) => {
            const numbers = {};
            Object.keys(counts || {}).forEach((key) => {
                const count = Number(counts[key]);
                if (isFinite(count)) numbers[key] = count;
            });
            return invoke("set_counts", { counts: numbers });
        },
        // my own presence: online, away, dnd or offline
        setPresence: (status) => invoke("set_presence", { status: String(status || "") }),
        openExternal: (url) => invoke("open_external", { url: String(url || "") }),
        // whether the user sees this window: focused and shown, as the host tells it (bridge.rs tell_focus) and, before
        // it has, as the page finds it; a call rings with a native notification when not (WebrootCall.js)
        windowSeen: () => !document.hidden && (typeof window.__moquiWindowFocused === "boolean" ? window.__moquiWindowFocused : document.hasFocus())
    };
    if (node) {
        // the node's Comms app, which the host's mailto:, tel: and sip: links open (WebrootVue.qvt.js, on load)
        // in process the node's sockets are the host's too, which has the window's cookies (moqui.openSocket)
        const own = { setCommsRoot: (root) => invoke("set_comms_root", { root: String(root || "") }) };
        if (inProcess) own.socket = (url) => new HostSocket(url);
        window.moquiDesktop = Object.freeze(Object.assign({}, pageCommands, own));
        return;
    }
    const socket = (url) => new HostSocket(url);
    // The browser storage of accounts whose data the host deleted (a shared device's account signed out, an account
    // removed): the keys and IndexedDB databases under their storage scopes (MoquiLib scopedKey, `<account>:`), on this
    // origin. Other accounts' are under other scopes and stay.
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
    // The outbox and drafts the host keeps for the window's account (src/client/outbox.rs, flush.rs): an operation kept
    // is sent by the host, also while no window shows it; on() hears each change ({kind: "changed"}, or "sent" with the
    // server's answer). Used by WebrootChat.js and MoquiLib.js (moqui.drafts) where present; the web keeps localStorage.
    const outboxListeners = [];
    window.__moquiOutboxEvent = (event) => {
        outboxListeners.slice().forEach((fn) => {
            try { fn(event || {}); } catch (err) { console.error("moquiDesktop outbox listener failed", err); }
        });
    };
    const outbox = Object.freeze({
        // kind chat-send, chat-read, mail-send or mail-state, and with a server at clientApi 4 chat-edit, chat-redact,
        // chat-react, poll-vote and internal-send (kinds() says which); files: the store's names (drafts.saveFile) it sends
        add: (kind, payload, files) => ask("outbox_add", { kind: String(kind || ""), payload: payload || {}, files: Array.isArray(files) ? files.map(String) : null }),
        // each with its state (a conflict's current, a rejection's status and error)
        list: () => ask("outbox_list"),
        // {clientApi, kinds, internalSend}: what the account's server takes queued
        kinds: () => ask("outbox_kinds"),
        retry: (id) => ask("outbox_retry", { id: String(id || "") }),
        discard: (id) => ask("outbox_discard", { id: String(id || "") }),
        on: (fn) => {
            if (typeof fn !== "function") return () => {};
            outboxListeners.push(fn);
            return () => { const at = outboxListeners.indexOf(fn); if (at >= 0) outboxListeners.splice(at, 1); };
        }
    });
    // a Blob or File as base64, for the host to keep
    const base64Of = (blob) => new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => { const text = String(reader.result || ""); resolve(text.slice(text.indexOf(",") + 1)); };
        reader.onerror = () => reject(reader.error || new Error("The file was not read"));
        reader.readAsDataURL(blob);
    });
    const drafts = Object.freeze({
        get: (key) => ask("draft_get", { key: String(key || "") }),
        list: (prefix) => ask("draft_list", { prefix: String(prefix || "") }),
        // value: text (JSON); files: the store's names of the files attached to it
        put: (key, value, files) => ask("draft_put", { key: String(key || ""), value: String(value ?? ""), files: Array.isArray(files) ? files.map(String) : null }),
        remove: (key) => ask("draft_remove", { key: String(key || "") }),
        // a File or Blob kept by the host for a draft or a send: resolves to its name in the store
        saveFile: (file, name) => base64Of(file).then((data) => ask("file_save", { name: String(name || (file && file.name) || "file"), data }))
    });
    if (appWindow) {
        // the scopes the host gave this window to clear, once (its later pages carry the same list)
        const scopes = window.__moquiClearScopes;
        if (Array.isArray(scopes) && scopes.length) {
            let first = true;
            try {
                first = !window.sessionStorage.getItem("moqui.scopesCleared");
                window.sessionStorage.setItem("moqui.scopesCleared", "1");
            } catch (err) {
                /* no session storage: cleared on each page, which holds nothing of theirs */
            }
            if (first) clearScopes(scopes);
        }
        // a server's page: its sockets, the storage scope of its account (MoquiLib scopedKey), and a comms screen of the
        // main window shown there (an encrypted room: the shell owns the chat crypto device, window.__moquiCryptoInShell)
        window.moquiDesktop = Object.freeze({ socket, outbox, drafts, storageScope: String(window.__moquiStorageScope || ""),
            openInShell: (path) => ask("open_in_shell", { path: String(path || "") }) });
        return;
    }
    window.moquiDesktop = Object.freeze(Object.assign({}, pageCommands, {
        socket,
        outbox: Object.freeze(Object.assign({ unsent: () => ask("outbox_unsent") }, outbox)),
        drafts,
        // the shell (shell/js/shell.js): the accounts and sign-in, which the host keeps; never the key
        state: () => ask("client_state"),
        // the name of the shell's chat crypto device (WebrootCrypto deviceName): "MR Moqui desktop on <host>"
        deviceName: String(window.__moquiDeviceName || ""),
        // the storage of the accounts state's clearScopes names, as the shell loads
        clearScopes,
        // deviceMode: personal or shared, when the build asks (state's deviceMode is choose)
        signIn: (given) => ask("client_sign_in", { server: String(given.server || ""), username: String(given.username || ""),
            password: String(given.password || ""), code: given.code ? String(given.code) : null,
            deviceMode: given.deviceMode ? String(given.deviceMode) : null }),
        sendCode: (given) => ask("client_send_code", { server: String(given.server || ""), username: String(given.username || ""),
            password: String(given.password || ""), factor: String(given.factor || "") }),
        signOut: () => ask("client_sign_out"),
        switchAccount: (id) => ask("client_switch_account", { id: String(id) }),
        addAccount: () => ask("client_add_account"),
        removeAccount: (id) => ask("client_remove_account", { id: String(id) }),
        openRemote: (path) => ask("client_open_remote", { path: String(path || "") }),
        // the comms screen the host keeps for the shell to show (an OS mailto:, a notification), once
        takeOpen: () => ask("client_take_open"),
        // SSO in the system browser: the server's providers, and a sign-in through one (the server hands it back)
        ssoProviders: (server) => ask("client_sso_providers", { server: String(server || "") }),
        ssoStart: (server, authFlowId, deviceMode) => ask("client_sso_start", { server: String(server || ""), authFlowId: String(authFlowId || ""),
            deviceMode: deviceMode ? String(deviceMode) : null }),
        ssoCancel: () => ask("client_sso_cancel")
    }));
})();
