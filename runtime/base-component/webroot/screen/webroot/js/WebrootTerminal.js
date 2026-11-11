/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */
/*
 * The browser terminal's store (moqui.terminal, plans/Terminal-Fancy.md §9) and its views.
 *
 * Sessions are the server's: the Terminal screen opens one by a transition (open#TerminalSession), which answers its id,
 * and the socket attaches to it by that id (/termws?session=, crates/moqui-host/src/termws.rs). A session outlives the
 * page: a reload, another device or a dropped connection attaches again, saying how many of the session's bytes it
 * already has, and goes on from there or is given the session's last output again (§5.4). The page acknowledges what
 * xterm.js has drawn, so a program that writes faster than the page draws waits for it.
 *
 * A session's xterm.js instance, its socket and its element live here, not in a view (D11). A view (a pane of the
 * Terminal screen, later a floating window) puts the session's element into itself when it is mounted and takes it out
 * when it is unmounted, without disposing anything, so moving a terminal between places moves its element and nothing
 * reconnects (§9.2). Past MAX_LIVE terminals the one shown longest ago is let go here; its session goes on, and it
 * replays when it is shown again.
 *
 * A link a session writes (OSC 8) opens in a new tab when it is a web link or a path of this site. Text it copies (a
 * mouse drag, OSC 52) goes to the clipboard, or to a dialog to copy by hand where the browser refuses. xterm.js and its
 * addons load on the first session. The keys of the terminal's own actions (Ctrl+Shift chords, moqui.keymap) are
 * passed over by xterm.js, so the shell's keymap gets them.
 *
 * Loaded after MoquiLib.js and before WebrootVue.qvt.js (qapps.xml) for a signed-in user when the server serves the
 * socket; its components go in moqui.qvtComponents.
 */
(function (root) {
    "use strict";
    const moqui = (root.moqui = root.moqui || {});
    const reactive = (obj) => (root.Vue && root.Vue.reactive ? root.Vue.reactive(obj) : obj);
    const t = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);

    // The terminal's colors: the apps ask for the background (OSC 11), and xterm.js answers with these.
    const THEMES = {
        light: { background: "#ffffff", foreground: "#1f2328", cursor: "#1f2328", selectionBackground: "#b6d6fd" },
        dark: { background: "#16181d", foreground: "#e6e6e6", cursor: "#e6e6e6", selectionBackground: "#3a4a63" }
    };
    const FONT = 'ui-monospace, "Cascadia Mono", "DejaVu Sans Mono", Menlo, Consolas, monospace';
    // The page acknowledges what it drew each this many bytes (§5.4).
    const ACK_EVERY = 100000;
    // Live xterm.js instances kept; past it the one shown longest ago is let go (§9.2).
    const MAX_LIVE = 12;
    // A dropped connection is tried again after these waits, then each last one.
    const RETRY = [500, 1000, 2000, 5000, 10000];
    // A refusal of an attach to a command line or a shell, as the socket's close reason (crates/moqui-host/src/termws.rs).
    const REFUSALS = { reauthRequired: "Confirm it is you to open this terminal from here.", notAvailable: "This terminal is not available here." };
    const SETTINGS_PREFERENCE = "TERMINAL_CONFIG";
    const DEFAULT_SETTINGS = { fontSize: 14, fontFamily: "", scrollback: 5000, copyOnSelect: false, pasteProtection: true, opensFirst: "" };

    function readSettings() {
        const prefs = (moqui.config && moqui.config.preferences) || {};
        let saved = {};
        try {
            saved = prefs[SETTINGS_PREFERENCE] ? JSON.parse(prefs[SETTINGS_PREFERENCE]) : {};
        } catch (e) {
            saved = {};
        }
        return Object.assign({}, DEFAULT_SETTINGS, saved && typeof saved === "object" ? saved : {});
    }

    const state = reactive({
        // per session key, what its views show: closed with a reason, taken by another window, reconnecting, failed
        views: {},
        // the copy the browser refused
        copy: { open: false, text: "" },
        // the server's sessions of this user, by id, as find#TerminalSessions lists them
        sessions: {},
        // their ids, oldest first
        order: [],
        // what the user may open from here (get#TerminalOptions)
        options: null,
        settings: readSettings(),
        loaded: false
    });
    // Per session key, not reactive: { key, kind, term, fit, el, socket, received, drawn, acked, opened, shown }.
    const sessions = new Map();
    let libs = null;
    let dark = null;

    function view(key) {
        if (!state.views[key]) state.views[key] = { closed: false, reason: "", failed: "", taken: false, reauth: false, reconnecting: false };
        return state.views[key];
    }

    function isDark() {
        if (dark !== null) return dark;
        const quasar = root.Quasar;
        return !!(quasar && quasar.Dark && quasar.Dark.isActive);
    }

    // Only web links and paths of this site open, in a new tab.
    function openLink(event, uri) {
        if (/^(https?:\/\/|\/(?!\/))/i.test(uri)) root.open(uri, "_blank", "noopener");
    }

    // A copy from the session (a drag selects text, OSC 52 "c;<base64>"): to the clipboard, which a browser allows shortly
    // after the mouse release that ended the drag, on a secure page or localhost; else the dialog to copy by hand.
    function onCopy(data) {
        const at = data.indexOf(";");
        if (at < 0) return true;
        let text;
        try {
            const bytes = Uint8Array.from(atob(data.slice(at + 1)), (ch) => ch.charCodeAt(0));
            text = new TextDecoder().decode(bytes);
        } catch (e) {
            return true;
        }
        copyText(text);
        return true;
    }

    function copyText(text) {
        const byHand = () => {
            state.copy.text = text;
            state.copy.open = true;
        };
        if (root.navigator && root.navigator.clipboard && root.isSecureContext) root.navigator.clipboard.writeText(text).catch(byHand);
        else byHand();
    }

    function shellVm() {
        return moqui.webrootVue || {};
    }

    function socketUrl(key) {
        const shell = shellVm();
        const scheme = root.location.protocol === "https:" ? "wss://" : "ws://";
        const host = shell.appHost || root.location.host;
        return scheme + host + (shell.appRootPath || "") + "/termws?session=" + encodeURIComponent(key);
    }

    // A transition of the Terminal screen, as JSON: {errors, reasonCode} rejects.
    function request(name, body) {
        const headers = { Accept: "application/json" };
        const init = { method: body ? "POST" : "GET", headers, credentials: "same-origin" };
        if (body) {
            headers["Content-Type"] = "application/json";
            if (moqui.sessionToken) headers["X-CSRF-Token"] = moqui.sessionToken;
            init.body = JSON.stringify(Object.assign({ moquiSessionToken: moqui.sessionToken || "" }, body));
        }
        return root.fetch((shellVm().appRootPath || "") + "/apps/Terminal/" + name, init).then((response) => {
            const token = response.headers.get("X-CSRF-Token");
            if (token && moqui.onSessionToken) moqui.onSessionToken(token);
            return response
                .json()
                .catch(() => ({}))
                .then((json) => {
                    if (!response.ok) throw { reasonCode: response.status === 401 ? "notSignedIn" : "failed", errors: (json && json.errors) || String(response.status) };
                    if (json && json.errors) throw { reasonCode: json.reasonCode || "failed", errors: json.errors };
                    return json || {};
                });
        });
    }

    // With the recent sign-in check (moqui.reauth) where a refusal asks for it.
    function withReauth(fn) {
        return moqui.reauth && moqui.reauth.retry ? moqui.reauth.retry(fn) : fn();
    }

    function send(session, bytes) {
        if (session.socket && session.socket.readyState === 1 && session.attached) session.socket.send(bytes);
    }

    function control(session, message) {
        if (session.socket && session.socket.readyState === 1) session.socket.send(JSON.stringify(message));
    }

    // The attach goes out once the terminal has been fitted in a view, so the session takes the size it is shown at.
    function attach(session) {
        if (!session.opened || session.attachSent || !session.socket || session.socket.readyState !== 1) return;
        session.attachSent = true;
        control(session, { type: "attach", cols: session.term.cols, rows: session.term.rows, have: session.everAttached ? session.received : null });
    }

    function fit(session) {
        if (!session.fit || !session.el.isConnected) return;
        try {
            session.fit.fit();
        } catch (e) {
            /* not laid out yet */
        }
    }

    // What the page drew is acknowledged each ACK_EVERY bytes, and when it has drawn everything it was sent.
    function drew(session, length) {
        session.drawn += length;
        if (session.drawn - session.acked >= ACK_EVERY || session.drawn === session.received) {
            if (session.drawn - session.acked > 0) {
                session.acked = session.drawn;
                control(session, { type: "ack", count: session.drawn });
            }
        }
    }

    function onControl(session, message) {
        const info = state.sessions[session.key];
        if (message.type === "attached") {
            session.attached = true;
            session.everAttached = true;
            if (message.reset) {
                session.term.reset();
                session.received = session.drawn = session.acked = Number(message.from) || 0;
            }
            if (info) Object.assign(info, { title: message.title, cwd: message.cwd, attached: true });
        } else if (message.type === "state") {
            if (info) {
                info.title = message.title;
                info.cwd = message.cwd;
                info.status = message.status;
                info.statusText = message.statusText;
                info.progress = message.progress;
            }
        } else if (message.type === "ended") {
            if (info) Object.assign(info, { state: "ended", exitStatus: message.status, reason: message.reason });
        }
    }

    // The settings a terminal takes from the user's preferences.
    function termOptions(kind) {
        const settings = state.settings;
        return {
            fontFamily: settings.fontFamily || FONT,
            fontSize: Number(settings.fontSize) || 14,
            // the apps draw their own screen and keep no scrollback; a shell keeps what scrolled off (§9.4)
            scrollback: kind === "apps" ? 0 : Math.max(0, Number(settings.scrollback) || 0)
        };
    }

    // The keys of the terminal's own actions: xterm.js passes them over, and the shell's keymap acts on them. Copy and
    // paste (Ctrl+Shift+C, V) stay the terminal's.
    function isOwnKey(event) {
        if (!event.ctrlKey || !event.shiftKey || event.altKey || event.metaKey) return false;
        const key = String(event.key || "").toLowerCase();
        return key !== "c" && key !== "v" && key !== "shift" && key !== "control";
    }

    const terminal = {
        state,
        THEMES,
        ACK_EVERY,
        MAX_LIVE,
        request,

        // xterm.js and its addons, once.
        loadLibs() {
            if (!libs) {
                moqui.loadStylesheet("/libs/xterm/xterm.css");
                libs = Promise.all([
                    moqui.loadScript("/libs/xterm/xterm.js", null, () => !!root.Terminal),
                    moqui.loadScript("/libs/xterm/addon-fit.js", null, () => !!root.FitAddon)
                ]).catch((err) => {
                    libs = null;
                    throw err;
                });
            }
            return libs;
        },

        // ---- the server's sessions ----

        // The user's sessions and what they may open, from the server.
        refresh() {
            return Promise.all([request("sessions"), state.options ? null : request("options")]).then(([listed, options]) => {
                if (options && options.options) state.options = options.options;
                const next = {};
                const order = [];
                for (const session of (listed && listed.sessions) || []) {
                    next[session.id] = Object.assign(state.sessions[session.id] || {}, session);
                    order.push(session.id);
                }
                state.sessions = next;
                state.order = order;
                state.loaded = true;
                return order;
            });
        },

        // Open a session: kind apps, command or host, with a host profile; answers its id.
        open(kind, options) {
            const opts = options || {};
            const body = { kind: kind || "apps", cols: opts.cols || 100, rows: opts.rows || 30 };
            for (const name of ["profileName", "directory", "title"]) if (opts[name]) body[name] = opts[name];
            return withReauth(() => request("open", body)).then((out) => {
                const session = out.session || { id: out.sessionId, kind: body.kind };
                state.sessions[session.id] = session;
                if (!state.order.includes(session.id)) state.order.push(session.id);
                return session.id;
            });
        },

        rename(id, title) {
            return request("rename", { sessionId: id, title: title || "" }).then((out) => {
                if (out.session && state.sessions[id]) Object.assign(state.sessions[id], out.session);
            });
        },

        // End the session on the server, and its terminal here.
        close(id) {
            return request("close", { sessionId: id }).then(() => {
                const info = state.sessions[id];
                if (info) info.state = "ended";
            });
        },

        // Let go of an ended session's listing here; the server drops it in its own time.
        forget(id) {
            terminal.end(id);
            delete state.sessions[id];
            state.order = state.order.filter((other) => other !== id);
        },

        saveSettings(changes) {
            Object.assign(state.settings, changes || {});
            for (const session of sessions.values()) {
                const options = termOptions(session.kind);
                session.term.options.fontFamily = options.fontFamily;
                session.term.options.fontSize = options.fontSize;
                if (session.kind !== "apps") session.term.options.scrollback = options.scrollback;
                fit(session);
            }
            const vm = shellVm();
            if (vm.setPreference) vm.setPreference(SETTINGS_PREFERENCE, JSON.stringify(state.settings));
        },

        // ---- terminals on this page ----

        session(key) {
            return sessions.get(key) || null;
        },

        // The session `key`: its terminal made and its socket opened the first time. A view calls this when mounted.
        start(key, options) {
            const known = sessions.get(key);
            if (known) return Promise.resolve(known);
            view(key);
            return terminal.loadLibs().then(
                () => sessions.get(key) || terminal.create(key, options),
                (err) => {
                    view(key).failed = t("The terminal could not load.");
                    throw err;
                }
            );
        },

        // A new terminal and element for the session `key`, and its socket unless `options.connect` is false (a test,
        // which may also take what it sends with `options.send`).
        create(key, options) {
            const opts = options || {};
            const info = state.sessions[key];
            const kind = opts.kind || (info && info.kind) || "apps";
            const el = root.document.createElement("div");
            el.className = "m-terminal-screen";
            // never in Assist's reach (plans/Terminal-Fancy.md D18): an agent does not type into a shell through the page
            el.setAttribute("data-assist-self", "terminal");
            el.style.cssText = "width: 100%; height: 100%; overflow: hidden";
            const term = new root.Terminal(
                Object.assign(
                    {
                        cursorBlink: false,
                        macOptionIsMeta: true,
                        // Muted text, frame titles and application theme overrides still need to be readable on this background.
                        minimumContrastRatio: 4.5,
                        theme: isDark() ? THEMES.dark : THEMES.light,
                        linkHandler: { activate: openLink }
                    },
                    termOptions(kind)
                )
            );
            const fitAddon = new root.FitAddon.FitAddon();
            term.loadAddon(fitAddon);
            const session = { key, kind, term, fit: fitAddon, el, socket: null, received: 0, drawn: 0, acked: 0, opened: false, shown: Date.now(), retries: 0 };
            sessions.set(key, session);
            term.parser.registerOscHandler(52, onCopy);
            term.attachCustomKeyEventHandler((event) => !isOwnKey(event));
            const encoder = new TextEncoder();
            const out = opts.send || ((bytes) => send(session, bytes));
            term.onData((data) => out(encoder.encode(data)));
            // the bytes of a mouse report in the X10 encoding, one per character
            term.onBinary((data) => out(Uint8Array.from(data, (ch) => ch.charCodeAt(0) & 0xff)));
            term.onResize(({ cols, rows }) => {
                if (session.attached) control(session, { type: "resize", cols, rows });
            });
            session.observer = new ResizeObserver(() => fit(session));
            session.observer.observe(el);
            if (opts.connect !== false) terminal.connect(key);
            terminal.letGoOldest();
            return session;
        },

        // Past MAX_LIVE terminals, the one shown longest ago that no view holds is let go; its session goes on.
        letGoOldest() {
            if (sessions.size <= MAX_LIVE) return;
            let oldest = null;
            for (const session of sessions.values()) {
                if (session.el.isConnected) continue;
                if (!oldest || session.shown < oldest.shown) oldest = session;
            }
            if (oldest) terminal.end(oldest.key);
        },

        // Put the session's element into `container` and open its terminal there the first time (xterm.js measures the
        // font in the page); its scroll and screen are kept.
        attach(key, container) {
            const session = sessions.get(key);
            if (!session || !container) return;
            if (session.el.parentNode !== container) container.appendChild(session.el);
            session.shown = Date.now();
            if (!session.opened) {
                session.term.open(session.el);
                session.opened = true;
            }
            fit(session);
            attach(session);
        },

        // Take the session's element out of `container`, if it is there; nothing is disposed.
        detach(key, container) {
            const session = sessions.get(key);
            if (session && session.el.parentNode && (!container || session.el.parentNode === container)) {
                session.shown = Date.now();
                session.el.parentNode.removeChild(session.el);
            }
        },

        focus(key) {
            const session = sessions.get(key);
            if (session) session.term.focus();
        },

        fit(key) {
            const session = sessions.get(key);
            if (session) fit(session);
        },

        // Attach to the session: a new socket that says how far this page got.
        connect(key) {
            const session = sessions.get(key);
            if (!session) return;
            const shown = view(key);
            Object.assign(shown, { closed: false, reason: "", taken: false, reauth: false, reconnecting: false });
            clearTimeout(session.retryTimer);
            if (session.socket) {
                session.socket.onclose = null;
                session.socket.close();
            }
            session.attached = false;
            session.attachSent = false;
            // the host's socket in a desktop client (moqui.openSocket); readyState 1 is open in either
            const socket = moqui.openSocket(socketUrl(key));
            socket.binaryType = "arraybuffer";
            socket.onopen = () => {
                session.retries = 0;
                fit(session);
                attach(session);
            };
            socket.onmessage = (event) => {
                if (event.data instanceof ArrayBuffer) {
                    const bytes = new Uint8Array(event.data);
                    session.received += bytes.length;
                    session.term.write(bytes, () => drew(session, bytes.length));
                } else if (typeof event.data === "string") {
                    let message = null;
                    try {
                        message = JSON.parse(event.data);
                    } catch (e) {
                        message = null;
                    }
                    if (message) onControl(session, message);
                }
            };
            socket.onclose = (event) => {
                if (session.socket !== socket) return;
                session.attached = false;
                const reason = event.reason || "";
                const info = state.sessions[key];
                const ended = info && info.state === "ended";
                // a connection that dropped, with no reason from the server, attaches again and goes on (§5.4)
                if (!reason && !ended && !session.disposed) {
                    shown.reconnecting = true;
                    const wait = RETRY[Math.min(session.retries, RETRY.length - 1)];
                    session.retries += 1;
                    session.retryTimer = setTimeout(() => {
                        if (!session.disposed && session.socket === socket) terminal.connect(key);
                    }, wait);
                    return;
                }
                shown.closed = true;
                shown.taken = reason === "opened elsewhere";
                shown.reauth = reason === "reauthRequired";
                shown.reason = shown.taken ? t("This terminal was opened in another window.")
                    : REFUSALS[reason] ? t(REFUSALS[reason]) : reason || t("The terminal session ended.");
                if (reason === "no such terminal session" && info) info.state = "ended";
            };
            session.socket = socket;
        },

        // Let go of the session's terminal here: its socket closed, its terminal disposed. The session goes on.
        end(key) {
            const session = sessions.get(key);
            if (!session) return;
            sessions.delete(key);
            session.disposed = true;
            clearTimeout(session.retryTimer);
            if (session.socket) {
                session.socket.onclose = null;
                session.socket.close();
            }
            if (session.observer) session.observer.disconnect();
            terminal.detach(key);
            session.term.dispose();
            delete state.views[key];
        },

        // The app's light or dark mode, to every terminal. An apps session is asked again for its background: xterm's
        // OSC 11 reply goes to the session over onData, and it changes its palette without losing its place.
        setDark(value) {
            if (dark === !!value) return;
            dark = !!value;
            for (const session of sessions.values()) {
                session.term.options.theme = dark ? THEMES.dark : THEMES.light;
                if (session.kind === "apps") session.term.write("\x1b]11;?\x1b\\");
            }
        },

        isOwnKey,
        copyText
    };

    // ---- the Terminal screen's layout: tabs, each a tree of splits whose leaves are panes showing a session ----
    // A node is {pane, id, session} or {split, id, dir: "row" | "column", ratio, a, b}; pure functions, kept by the page.
    let lastId = 0;
    const nodeId = () => "n" + Date.now().toString(36) + (++lastId).toString(36);
    const tree = {
        pane(session) {
            return { pane: true, id: nodeId(), session };
        },
        // the panes of `node`, in order
        panes(node) {
            if (!node) return [];
            return node.pane ? [node] : tree.panes(node.a).concat(tree.panes(node.b));
        },
        find(node, id) {
            return tree.panes(node).find((pane) => pane.id === id) || null;
        },
        // `node` with the pane `id` split in `dir`: the pane and a new one showing `session`, beside or below it
        split(node, id, dir, session) {
            if (!node) return null;
            if (node.pane) {
                if (node.id !== id) return node;
                return { split: true, id: nodeId(), dir, ratio: 0.5, a: node, b: tree.pane(session) };
            }
            return Object.assign({}, node, { a: tree.split(node.a, id, dir, session), b: tree.split(node.b, id, dir, session) });
        },
        // `node` without the pane `id`; its sibling takes the split's place. Null when it was the last pane.
        remove(node, id) {
            if (!node) return null;
            if (node.pane) return node.id === id ? null : node;
            const a = tree.remove(node.a, id);
            const b = tree.remove(node.b, id);
            if (!a) return b;
            if (!b) return a;
            return Object.assign({}, node, { a, b });
        },
        // the sessions `node` shows
        sessions(node) {
            return tree.panes(node).map((pane) => pane.session);
        },
        // a pane next to `id` in the direction of an arrow: the next or previous pane in order
        neighbor(node, id, forward) {
            const panes = tree.panes(node);
            const at = panes.findIndex((pane) => pane.id === id);
            if (at < 0 || panes.length < 2) return null;
            return panes[(at + (forward ? 1 : panes.length - 1)) % panes.length];
        }
    };
    terminal.tree = tree;
    moqui.terminal = terminal;

    // ---- components (registered by WebrootVue.qvt.js) ----
    const components = (moqui.qvtComponents = moqui.qvtComponents || {});

    // One session's terminal, where this view is: its element moves in when mounted and out when unmounted.
    components["m-terminal-view"] = {
        name: "mTerminalView",
        props: { sessionKey: { type: String, required: true }, kind: { type: String, default: "" } },
        emits: ["focused"],
        data() {
            return { views: state.views, labels: { reconnect: t("Reconnect"), takeBack: t("Take it back"), confirm: t("Confirm it is you"), reconnecting: t("Connecting again…") } };
        },
        computed: {
            shown() {
                return this.views[this.sessionKey] || { closed: false, reason: "", failed: "" };
            },
            dark() {
                return this.$q.dark.isActive;
            }
        },
        watch: {
            dark(value) {
                terminal.setDark(value);
            },
            sessionKey(key, before) {
                terminal.detach(before, this.$refs.screen);
                this.show();
            }
        },
        mounted() {
            terminal.setDark(this.dark);
            this.show();
        },
        beforeUnmount() {
            this.unmounted = true;
            terminal.detach(this.sessionKey, this.$refs.screen);
        },
        methods: {
            show() {
                const key = this.sessionKey;
                terminal.start(key, this.kind ? { kind: this.kind } : {}).then(
                    () => {
                        if (!this.unmounted && this.sessionKey === key) terminal.attach(key, this.$refs.screen);
                    },
                    () => {}
                );
            },
            focus() {
                terminal.focus(this.sessionKey);
                this.$emit("focused");
            },
            reconnect() {
                // a remote attach to a command line or a shell wants a recent sign-in first (§8.2)
                if (this.shown.reauth && moqui.reauth) moqui.reauth.confirm().then(() => terminal.connect(this.sessionKey), () => {});
                else terminal.connect(this.sessionKey);
            }
        },
        template:
            '<div class="m-terminal-view column no-wrap fit" @focusin="$emit(\'focused\')">' +
            '<div ref="screen" class="m-terminal-host col" style="min-height: 0; overflow: hidden" @click="focus"></div>' +
            '<div v-if="shown.reconnecting" class="m-terminal-closed row items-center no-wrap q-pa-xs q-gutter-sm text-caption">' +
            '<q-spinner size="1em"></q-spinner><span>{{labels.reconnecting}}</span></div>' +
            '<div v-else-if="shown.closed" class="m-terminal-closed row items-center no-wrap q-pa-sm q-gutter-sm" id="TerminalClosed">' +
            '<q-icon name="sym_o_link_off"></q-icon><span class="col">{{shown.reason}}</span>' +
            '<q-btn dense unelevated no-caps color="primary" :label="shown.taken ? labels.takeBack : shown.reauth ? labels.confirm : labels.reconnect" @click="reconnect"></q-btn></div>' +
            '<div v-else-if="shown.failed" class="m-terminal-closed q-pa-sm text-negative">{{shown.failed}}</div>' +
            "</div>"
    };

    // A node of a tab's layout: a pane with its header, or a split of two nodes with a divider to drag. The Terminal
    // screen provides `workspace`, which acts on the layout; a zoomed tab draws its one pane's node alone.
    components["m-terminal-node"] = {
        name: "mTerminalNode",
        props: { node: { type: Object, required: true } },
        inject: ["workspace"],
        data() {
            return { sessions: state.sessions, labels: { splitRight: t("Split right"), splitDown: t("Split down"), zoom: t("Zoom"),
                unzoom: t("Restore"), close: t("Close pane"), end: t("End session"), float: t("Float") } };
        },
        computed: {
            info() {
                return (this.node.pane && this.sessions[this.node.session]) || {};
            },
            focused() {
                return this.workspace.focusedPane === this.node.id;
            },
            zoomed() {
                return this.workspace.zoomedPane === this.node.id;
            },
            dot() {
                return this.workspace.dotOf(this.info);
            }
        },
        methods: {
            startDrag(event) {
                const box = this.$el.getBoundingClientRect();
                const row = this.node.dir === "row";
                const move = (e) => {
                    const ratio = row ? (e.clientX - box.left) / box.width : (e.clientY - box.top) / box.height;
                    this.workspace.setRatio(this.node.id, Math.min(0.9, Math.max(0.1, ratio)));
                };
                const up = () => {
                    root.removeEventListener("pointermove", move);
                    root.removeEventListener("pointerup", up);
                    this.workspace.save();
                };
                root.addEventListener("pointermove", move);
                root.addEventListener("pointerup", up);
                event.preventDefault();
            }
        },
        template:
            '<div v-if="node.pane" class="m-terminal-pane column no-wrap" :class="{ \'m-terminal-pane-focused\': focused }" :data-pane="node.id">' +
            '<div class="m-terminal-pane-header row items-center no-wrap q-px-xs" @click="workspace.focusPane(node.id)">' +
            '<span class="m-terminal-dot" :class="\'m-terminal-dot-\' + dot" :title="dot"></span>' +
            '<span class="ellipsis q-ml-xs">{{info.title || node.session}}</span>' +
            '<span v-if="info.cwd" class="ellipsis q-ml-sm text-caption m-terminal-muted">{{info.cwd}}</span>' +
            "<q-space></q-space>" +
            '<q-btn flat dense round size="sm" icon="sym_o_vertical_split" :aria-label="labels.splitRight" @click.stop="workspace.splitPane(node.id, \'row\')"><q-tooltip>{{labels.splitRight}}</q-tooltip></q-btn>' +
            '<q-btn flat dense round size="sm" icon="sym_o_horizontal_split" :aria-label="labels.splitDown" @click.stop="workspace.splitPane(node.id, \'column\')"><q-tooltip>{{labels.splitDown}}</q-tooltip></q-btn>' +
            '<q-btn flat dense round size="sm" :icon="zoomed ? \'sym_o_close_fullscreen\' : \'sym_o_open_in_full\'" :aria-label="zoomed ? labels.unzoom : labels.zoom" @click.stop="workspace.zoomPane(node.id)"><q-tooltip>{{zoomed ? labels.unzoom : labels.zoom}}</q-tooltip></q-btn>' +
            '<q-btn v-if="workspace.canFloat" flat dense round size="sm" icon="sym_o_picture_in_picture" :aria-label="labels.float" @click.stop="workspace.floatPane(node.id)"><q-tooltip>{{labels.float}}</q-tooltip></q-btn>' +
            '<q-btn flat dense round size="sm" icon="sym_o_close" :aria-label="labels.close" @click.stop="workspace.closePane(node.id)"><q-tooltip>{{labels.close}}</q-tooltip></q-btn>' +
            '<q-btn flat dense round size="sm" icon="sym_o_power_settings_new" :aria-label="labels.end" @click.stop="workspace.endPane(node.id)"><q-tooltip>{{labels.end}}</q-tooltip></q-btn>' +
            "</div>" +
            '<m-terminal-view class="col" style="min-height: 0" :session-key="node.session" @focused="workspace.focusPane(node.id)"></m-terminal-view>' +
            "</div>" +
            '<div v-else class="m-terminal-split no-wrap" :class="node.dir === \'row\' ? \'row\' : \'column\'">' +
            '<m-terminal-node :node="node.a" :style="{ flex: node.ratio + \' 1 0\' }" style="min-width: 0; min-height: 0"></m-terminal-node>' +
            '<div class="m-terminal-divider" :class="node.dir === \'row\' ? \'m-terminal-divider-row\' : \'m-terminal-divider-column\'" @pointerdown="startDrag"></div>' +
            '<m-terminal-node :node="node.b" :style="{ flex: (1 - node.ratio) + \' 1 0\' }" style="min-width: 0; min-height: 0"></m-terminal-node>' +
            "</div>"
    };

    // The copy the browser refused, to select and copy by hand.
    components["m-terminal-copy"] = {
        name: "mTerminalCopy",
        data() {
            return { copy: state.copy, labels: { copyByHand: t("The browser did not allow the copy. Select the text and copy it:"), close: t("Close") } };
        },
        methods: {
            shown() {
                if (this.$refs.text) this.$refs.text.select();
            }
        },
        template:
            '<q-dialog v-model="copy.open" @show="shown">' +
            '<q-card style="min-width: 360px; max-width: 90vw">' +
            '<q-card-section class="text-subtitle1">{{labels.copyByHand}}</q-card-section>' +
            '<q-card-section class="q-pt-none"><textarea ref="text" readonly :value="copy.text" rows="8" style="width: 100%; font-family: monospace"></textarea></q-card-section>' +
            '<q-card-actions align="right"><q-btn flat no-caps :label="labels.close" v-close-popup></q-btn></q-card-actions>' +
            "</q-card></q-dialog>"
    };
})(typeof window !== "undefined" ? window : globalThis);
