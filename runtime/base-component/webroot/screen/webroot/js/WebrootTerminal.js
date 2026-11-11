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
 * The browser terminal's store (moqui.terminal, plans/Terminal-Fancy.md §9) and m-terminal-view.
 *
 * A session's xterm.js instance, its socket and its element live here, not in a view (D11). A view (a pane of the
 * Terminal screen, later a floating window) puts the session's element into itself when it is mounted and takes it out
 * when it is unmounted, without disposing anything, so moving a terminal between places moves its element and nothing
 * reconnects (§9.2).
 *
 * The socket is /termws (crates/moqui-host/src/termws.rs): the terminal's bytes as binary messages both ways, and JSON
 * control as text. A link the session writes (OSC 8) opens in a new tab when it is a web link or a path of this site.
 * Text it copies (a mouse drag, OSC 52) goes to the clipboard, or to a dialog to copy by hand where the browser
 * refuses. xterm.js and its addons load on the first session.
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

    // What views show: per session key its closed state and the reason, and the copy-by-hand dialog.
    const state = reactive({ views: {}, copy: { open: false, text: "" } });
    // Per session key, not reactive: { key, kind, term, fit, el, socket, drawn }.
    const sessions = new Map();
    let libs = null;
    let dark = null;

    function view(key) {
        if (!state.views[key]) state.views[key] = { closed: false, reason: "", failed: "" };
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
        const byHand = () => {
            state.copy.text = text;
            state.copy.open = true;
        };
        if (root.navigator && root.navigator.clipboard && root.isSecureContext) root.navigator.clipboard.writeText(text).catch(byHand);
        else byHand();
        return true;
    }

    function socketUrl() {
        const shell = moqui.webrootVue || {};
        const scheme = root.location.protocol === "https:" ? "wss://" : "ws://";
        const host = shell.appHost || root.location.host;
        return scheme + host + (shell.appRootPath || "") + "/termws";
    }

    function send(session, bytes) {
        if (session.socket && session.socket.readyState === 1) session.socket.send(bytes);
    }

    // The size goes out once the terminal has been fitted in a view, so the session starts at the size it is shown at.
    function sendSize(session) {
        if (!session.opened || !session.socket || session.socket.readyState !== 1) return;
        session.sizeSent = true;
        session.socket.send(JSON.stringify({ type: "resize", cols: session.term.cols, rows: session.term.rows }));
    }

    function fit(session) {
        if (!session.fit || !session.el.isConnected) return;
        try {
            session.fit.fit();
        } catch (e) {
            /* not laid out yet */
        }
    }

    const terminal = {
        state,
        THEMES,

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

        // A new session's terminal and element, and its socket unless `options.connect` is false (a test).
        create(key, options) {
            const opts = options || {};
            const el = root.document.createElement("div");
            el.className = "m-terminal-screen";
            el.style.cssText = "width: 100%; height: 100%; overflow: hidden";
            const term = new root.Terminal({
                fontFamily: FONT,
                fontSize: 14,
                cursorBlink: false,
                scrollback: 0,
                macOptionIsMeta: true,
                // Muted text, frame titles and application theme overrides still need to be readable on this background.
                minimumContrastRatio: 4.5,
                theme: isDark() ? THEMES.dark : THEMES.light,
                linkHandler: { activate: openLink }
            });
            const fitAddon = new root.FitAddon.FitAddon();
            term.loadAddon(fitAddon);
            const session = { key, kind: opts.kind || "apps", term, fit: fitAddon, el, socket: null, drawn: 0, opened: false };
            sessions.set(key, session);
            term.parser.registerOscHandler(52, onCopy);
            const encoder = new TextEncoder();
            term.onData((data) => (opts.send || ((bytes) => send(session, bytes)))(encoder.encode(data)));
            // the bytes of a mouse report in the X10 encoding, one per character
            term.onBinary((data) => (opts.send || ((bytes) => send(session, bytes)))(Uint8Array.from(data, (ch) => ch.charCodeAt(0) & 0xff)));
            term.onResize(() => sendSize(session));
            session.observer = new ResizeObserver(() => fit(session));
            session.observer.observe(el);
            if (opts.connect !== false) terminal.connect(key);
            return session;
        },

        // Put the session's element into `container` and open its terminal there the first time (xterm.js measures the
        // font in the page); its scroll and screen are kept.
        attach(key, container) {
            const session = sessions.get(key);
            if (!session || !container) return;
            if (session.el.parentNode !== container) container.appendChild(session.el);
            if (!session.opened) {
                session.term.open(session.el);
                session.opened = true;
            }
            fit(session);
            if (!session.sizeSent) sendSize(session);
        },

        // Take the session's element out of `container`, if it is there; nothing is disposed.
        detach(key, container) {
            const session = sessions.get(key);
            if (session && session.el.parentNode && (!container || session.el.parentNode === container))
                session.el.parentNode.removeChild(session.el);
        },

        focus(key) {
            const session = sessions.get(key);
            if (session) session.term.focus();
        },

        fit(key) {
            const session = sessions.get(key);
            if (session) fit(session);
        },

        connect(key) {
            const session = sessions.get(key);
            if (!session) return;
            const shown = view(key);
            shown.closed = false;
            shown.reason = "";
            session.term.reset();
            session.sizeSent = false;
            if (session.socket) {
                session.socket.onclose = null;
                session.socket.close();
            }
            // the host's socket in a desktop client (moqui.openSocket); readyState 1 is open in either
            const socket = moqui.openSocket(socketUrl());
            socket.binaryType = "arraybuffer";
            socket.onopen = () => {
                fit(session);
                sendSize(session);
                session.term.focus();
            };
            socket.onmessage = (event) => {
                if (event.data instanceof ArrayBuffer) session.term.write(new Uint8Array(event.data));
            };
            socket.onclose = (event) => {
                if (session.socket !== socket) return;
                shown.closed = true;
                shown.reason = event.reason || t("The terminal session ended.");
            };
            session.socket = socket;
        },

        // End the session here: its socket closed, its terminal disposed.
        end(key) {
            const session = sessions.get(key);
            if (!session) return;
            sessions.delete(key);
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
        }
    };
    moqui.terminal = terminal;

    // ---- components (registered by WebrootVue.qvt.js) ----
    const components = (moqui.qvtComponents = moqui.qvtComponents || {});

    // One session's terminal, where this view is: its element moves in when mounted and out when unmounted.
    components["m-terminal-view"] = {
        name: "mTerminalView",
        props: { sessionKey: { type: String, required: true }, kind: { type: String, default: "apps" } },
        data() {
            return { views: state.views, labels: { reconnect: t("Reconnect") } };
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
            }
        },
        mounted() {
            terminal.setDark(this.dark);
            terminal.start(this.sessionKey, { kind: this.kind }).then(
                () => {
                    if (!this.unmounted) terminal.attach(this.sessionKey, this.$refs.screen);
                },
                () => {}
            );
        },
        beforeUnmount() {
            this.unmounted = true;
            terminal.detach(this.sessionKey, this.$refs.screen);
        },
        methods: {
            focus() {
                terminal.focus(this.sessionKey);
            },
            reconnect() {
                terminal.connect(this.sessionKey);
            }
        },
        template:
            '<div class="m-terminal-view column no-wrap fit">' +
            '<div ref="screen" class="m-terminal-host col" style="min-height: 0; overflow: hidden" @click="focus"></div>' +
            '<div v-if="shown.closed" class="m-terminal-closed row items-center no-wrap q-pa-sm q-gutter-sm" id="TerminalClosed">' +
            '<q-icon name="sym_o_link_off"></q-icon><span class="col">{{shown.reason}}</span>' +
            '<q-btn dense unelevated no-caps color="primary" :label="labels.reconnect" @click="reconnect"></q-btn></div>' +
            '<div v-else-if="shown.failed" class="m-terminal-closed q-pa-sm text-negative">{{shown.failed}}</div>' +
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
