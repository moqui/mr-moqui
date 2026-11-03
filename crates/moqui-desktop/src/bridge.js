/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The page bridge of the desktop app (src/bridge.rs, plans/Mobile-Desktop.md section 4, configuration 1): the host
// adapter window.moquiDesktop that the webroot client calls when it is there (MoquiLib.js notifyNotification and
// saveBlob, WebrootVue.qvt.js openLink and the shell's counts and presence, WebrootCall.js the ring). An
// initialization script of the main window, so it runs on every page the window shows; it defines the adapter only on
// the node's loopback origin, and the host's capability lets only that exact origin call the commands. A client of a
// server (src/client/) has it on its shell, with the accounts and sign-in, and on its app windows with the sockets only.
//
// No saveFile: a download link (moqui.saveBlob without it, an export link, an attachment) goes to Chromium's download,
// which the host takes with a save dialog and a notification when done (bridge.rs on_download).
(function () {
    "use strict";
    // the node's loopback origin (configuration 1 and 2), or the client's shell and app windows (configuration 3, src/client/)
    const node = location.protocol === "http:" && location.hostname === "127.0.0.1";
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
        openExternal: (url) => invoke("open_external", { url: String(url || "") })
    };
    if (node) {
        window.moquiDesktop = Object.freeze(pageCommands);
        return;
    }
    const socket = (url) => new HostSocket(url);
    if (appWindow) {
        // a server's page: its sockets, and the storage scope of its account (MoquiLib scopedKey); no other command
        window.moquiDesktop = Object.freeze({ socket, storageScope: String(window.__moquiStorageScope || "") });
        return;
    }
    window.moquiDesktop = Object.freeze(Object.assign({}, pageCommands, {
        socket,
        // the shell (shell/js/shell.js): the accounts and sign-in, which the host keeps; never the key
        state: () => ask("client_state"),
        signIn: (given) => ask("client_sign_in", { server: String(given.server || ""), username: String(given.username || ""),
            password: String(given.password || ""), code: given.code ? String(given.code) : null }),
        sendCode: (given) => ask("client_send_code", { server: String(given.server || ""), username: String(given.username || ""),
            password: String(given.password || ""), factor: String(given.factor || "") }),
        signOut: () => ask("client_sign_out"),
        switchAccount: (id) => ask("client_switch_account", { id: String(id) }),
        addAccount: () => ask("client_add_account"),
        removeAccount: (id) => ask("client_remove_account", { id: String(id) }),
        openRemote: (path) => ask("client_open_remote", { path: String(path || "") })
    }));
})();
