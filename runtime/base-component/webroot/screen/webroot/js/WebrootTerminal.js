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
    const DEFAULT_SETTINGS = { fontSize: 14, fontFamily: "", scrollback: 5000, copyOnSelect: false, pasteProtection: true, opensFirst: "", bell: "visual",
        notifyBlocked: true, notifyDone: false, notifyBell: false, notifyLongCommandSeconds: 0 };

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
        loaded: false,
        // the sessions shown in a floating window, by id, and the one floated last (the quick terminal's)
        floating: {},
        lastFloat: "",
        // a session to show in a pane once the Terminal screen is there
        pendingShow: ""
    });
    // Per session key, not reactive: { key, kind, term, fit, el, socket, received, drawn, acked, opened, shown }.
    const sessions = new Map();
    let libs = null;
    let dark = null;

    function view(key) {
        if (!state.views[key]) state.views[key] = { closed: false, reason: "", failed: "", taken: false, reauth: false, reconnecting: false, searching: false };
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
        const ticket = currentTicket();
        return scheme + host + (shell.appRootPath || "") + "/termws?session=" + encodeURIComponent(key) + (ticket ? "&ticket=" + ticket : "");
    }

    // ---- a recent sign-in in the Desktop and Mobile apps (§12) ----
    // Their host signs each request in by the device's key, and keeps no session to hold a sign-in in: the user confirms
    // who they are in one request (confirm#TerminalUser), which answers a ticket the opening and the socket show.
    const TICKET_MS = 4 * 60 * 1000;
    let ticket = null;
    function inApp() {
        return !!(root.moquiDesktop && typeof root.moquiDesktop.socket === "function");
    }
    function currentTicket() {
        return ticket && Date.now() < ticket.until ? ticket.value : "";
    }
    function dialogOf() {
        const vm = shellVm();
        if (vm.$q && vm.$q.dialog) return vm.$q.dialog.bind(vm.$q);
        return root.Quasar && root.Quasar.Dialog ? root.Quasar.Dialog.create.bind(root.Quasar.Dialog) : null;
    }
    function ask(title, message, type) {
        const dialog = dialogOf();
        if (!dialog) return Promise.reject({ reasonCode: "cancelled" });
        return new Promise((resolve, reject) => {
            dialog({ title, message, prompt: { model: "", type: type || "password", isValid: (value) => !!value }, cancel: true, persistent: true })
                .onOk(resolve)
                .onCancel(() => reject({ reasonCode: "cancelled" }));
        });
    }
    function confirmTicket() {
        const title = t("Confirm it is you");
        return ask(title, t("Your password, to open a terminal on this server from here")).then((password) =>
            request("confirm", { password }).then((out) => {
                if (out.ticket) return out;
                if (!out.codeNeeded) throw { reasonCode: "notConfirmed", errors: t("Not confirmed") };
                return ask(title, out.codeText || t("The code of your second factor"), "text").then((code) => request("confirm", { password, code }));
            })
        ).then((out) => {
            if (!out.ticket) throw { reasonCode: "notConfirmed", errors: t("Not confirmed") };
            ticket = { value: out.ticket, until: Date.now() + TICKET_MS };
            return ticket.value;
        });
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

    // With the recent sign-in check where a refusal asks for it: moqui.reauth in a browser, a ticket in an app (§12).
    function withReauth(fn) {
        if (inApp()) {
            return Promise.resolve().then(fn).catch((err) => {
                if (!err || err.reasonCode !== "reauthRequired") throw err;
                return confirmTicket().then(() => fn());
            });
        }
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
            session.visibleSent = null;
            reportView(session);
        } else if (message.type === "state") {
            if (info) {
                info.title = message.title;
                info.cwd = message.cwd;
                info.status = message.status;
                info.statusText = message.statusText;
                info.progress = message.progress;
                if (message.unseen !== undefined) info.unseen = message.unseen;
            }
        } else if (message.type === "ended") {
            if (info) Object.assign(info, { state: "ended", exitStatus: message.status, reason: message.reason });
        }
    }

    // Whether the user sees the session (§11.1): shown in a view, in a page they look at, not in a hidden or minimized
    // window. The server tells nobody of what the user sees.
    function seen(session) {
        const doc = root.document;
        if (!doc || doc.visibilityState === "hidden" || !session.el.isConnected) return false;
        return !(session.el.closest && session.el.closest(".m-float--hidden"));
    }

    function reportView(session) {
        if (!session.attached) return;
        const visible = seen(session);
        if (session.visibleSent === visible) return;
        session.visibleSent = visible;
        control(session, { type: "view", visible });
        const info = state.sessions[session.key];
        if (visible && info) info.unseen = false;
    }

    function reportViews() {
        for (const session of sessions.values()) reportView(session);
    }

    let viewWatch = null;
    function watchViews() {
        if (viewWatch || !root.document) return;
        root.document.addEventListener("visibilitychange", reportViews);
        // a window minimized or hidden changes no event of the page's own: looked at each 2 s
        viewWatch = root.setInterval(reportViews, 2000);
    }

    // A prompt's place in the scrollback (OSC 133;A, §11.2), for jumping between prompts: xterm.js passes over the
    // sequence, so the page marks the line.
    function onMark(session, data) {
        if (String(data).charAt(0) !== "A") return false;
        const marker = session.term.registerMarker(0);
        if (marker) {
            session.prompts.push(marker);
            marker.onDispose(() => {
                session.prompts = session.prompts.filter((other) => other !== marker);
            });
        }
        return false;
    }

    // The bell (§9.6): a flash of the terminal, a sound, or nothing.
    let bellAudio = null;
    function onBell(session) {
        const bell = state.settings.bell || "visual";
        if (bell === "visual") {
            session.el.classList.add("m-terminal-bell");
            root.setTimeout(() => session.el.classList.remove("m-terminal-bell"), 150);
        } else if (bell === "sound" && root.AudioContext) {
            try {
                bellAudio = bellAudio || new root.AudioContext();
                const tone = bellAudio.createOscillator();
                const gain = bellAudio.createGain();
                gain.gain.value = 0.05;
                tone.frequency.value = 880;
                tone.connect(gain).connect(bellAudio.destination);
                tone.start();
                tone.stop(bellAudio.currentTime + 0.08);
            } catch (e) {
                /* no sound */
            }
        }
    }

    // A phone's key bar (§12): Ctrl makes the next letter its control character, Alt sends ESC before it; each once.
    function applyModifiers(session, data) {
        const mods = session.mods || {};
        if (!mods.ctrl && !mods.alt) return data;
        let text = data;
        if (mods.ctrl && text.length === 1) {
            const code = text.toUpperCase().charCodeAt(0);
            if (code >= 64 && code <= 95) text = String.fromCharCode(code - 64);
            else if (text === " ") text = "\x00";
        }
        if (mods.alt) text = "\x1b" + text;
        session.mods = { ctrl: false, alt: false };
        state.modsChanged = (state.modsChanged || 0) + 1;
        return text;
    }

    // Touch (§12): two fingers pinch the font size; a long press selects the line under it and copies it.
    function touchGestures(session) {
        const el = session.el;
        let pinch = null;
        let press = null;
        const distance = (touches) => Math.hypot(touches[0].clientX - touches[1].clientX, touches[0].clientY - touches[1].clientY);
        el.addEventListener("touchstart", (event) => {
            if (event.touches.length === 2) {
                clearTimeout(press);
                pinch = { from: distance(event.touches), size: session.term.options.fontSize };
            } else if (event.touches.length === 1) {
                const touch = event.touches[0];
                press = root.setTimeout(() => {
                    const rect = el.getBoundingClientRect();
                    const row = Math.floor(((touch.clientY - rect.top) / rect.height) * session.term.rows);
                    const line = session.term.buffer.active.viewportY + Math.max(0, Math.min(session.term.rows - 1, row));
                    session.term.selectLines(line, line);
                    if (session.term.hasSelection()) copyText(session.term.getSelection().trim());
                }, 550);
            }
        }, { passive: true });
        el.addEventListener("touchmove", (event) => {
            clearTimeout(press);
            if (pinch && event.touches.length === 2) {
                const size = Math.round(Math.max(8, Math.min(32, (pinch.size * distance(event.touches)) / pinch.from)));
                if (size !== session.term.options.fontSize) {
                    session.term.options.fontSize = size;
                    fit(session);
                }
            }
        }, { passive: true });
        el.addEventListener("touchend", () => {
            clearTimeout(press);
            pinch = null;
        }, { passive: true });
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
        // the quick terminal's key (Ctrl+`) shows and hides it from inside it too
        if (event.ctrlKey && !event.altKey && !event.metaKey && event.key === "`") return true;
        if (!event.ctrlKey || !event.shiftKey || event.altKey || event.metaKey) return false;
        const key = String(event.key || "").toLowerCase();
        return key !== "c" && key !== "v" && key !== "shift" && key !== "control";
    }

    // ---- floating terminals (§10.4) ----
    const FLOAT = "terminal";
    // this browser has had a terminal: a load restores floating terminals only then, so a user who never opens one costs no
    // request (§9.1)
    const USED_KEY = "moqui.terminal.used";
    // a session's status as a Quasar color, for a window's dot
    const DOT_COLORS = { working: "info", blocked: "warning", done: "positive", error: "negative", ended: "grey-6" };
    const scoped = (key) => (moqui.scopedKey ? moqui.scopedKey(key) : key);

    function usedHere() {
        try {
            return !!root.localStorage.getItem(scoped(USED_KEY)) || !!root.localStorage.getItem(scoped("moqui.terminal.layout"));
        } catch (e) {
            return false;
        }
    }

    function markUsed() {
        try {
            root.localStorage.setItem(scoped(USED_KEY), "1");
        } catch (e) {
            /* storage refused: a load does not restore floating terminals */
        }
    }

    function statusOf(info) {
        if (!info) return "idle";
        if (info.state === "ended") return "ended";
        return info.status || "idle";
    }

    function registerFloat() {
        const floats = moqui.floats;
        if (!floats || floats.kinds[FLOAT]) return;
        floats.register(FLOAT, {
            title: (key) => (state.sessions[key] && state.sessions[key].title) || t("Terminal"),
            icon: "sym_o_terminal",
            dot: (key) => DOT_COLORS[statusOf(state.sessions[key])] || "",
            defaultSize: { w: 760, h: 440 },
            minSize: { w: 320, h: 180 },
            place: "right",
            // never in Assist's reach (D18)
            inReach: false,
            gatherLabel: t("Gather terminals"),
            actions: (key) => [
                { id: "popout", icon: "sym_o_open_in_new", label: t("Pop out"), run: () => terminal.popOut(key) },
                { id: "end", icon: "sym_o_power_settings_new", label: t("End session"), run: () => terminal.endFloating(key) }
            ],
            // shown: the terminal is started there if it was not (a chip of a session attached elsewhere takes it here)
            focus: (key, body) => {
                terminal.showFloating(key, body);
                return true;
            },
            dock: (shown, key) => {
                delete state.floating[key];
                if (state.lastFloat === key) state.lastFloat = "";
                terminal.detach(key);
                if (shown) terminal.showInScreen(key);
            }
        });
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
                // the addons that only add (search, plain URLs as links, wide characters) do not stop a terminal that lacks them
                const optional = (src, check) => moqui.loadScript(src, null, check).catch(() => null);
                libs = Promise.all([
                    moqui.loadScript("/libs/xterm/xterm.js", null, () => !!root.Terminal),
                    moqui.loadScript("/libs/xterm/addon-fit.js", null, () => !!root.FitAddon),
                    optional("/libs/xterm/addon-search.js", () => !!root.SearchAddon),
                    optional("/libs/xterm/addon-web-links.js", () => !!root.WebLinksAddon),
                    optional("/libs/xterm/addon-unicode11.js", () => !!root.Unicode11Addon)
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
            if (opts.resume) body.resume = "true";
            return withReauth(() => request("open", Object.assign({}, body, currentTicket() ? { ticket: currentTicket() } : {}))).then((out) => {
                markUsed();
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
                        linkHandler: { activate: openLink },
                        // the unicode11 addon's widths are a proposed API
                        allowProposedApi: true
                    },
                    termOptions(kind)
                )
            );
            const fitAddon = new root.FitAddon.FitAddon();
            term.loadAddon(fitAddon);
            const session = { key, kind, term, fit: fitAddon, el, socket: null, received: 0, drawn: 0, acked: 0, opened: false, shown: Date.now(), retries: 0, prompts: [] };
            sessions.set(key, session);
            term.parser.registerOscHandler(133, (data) => onMark(session, data));
            term.onBell(() => onBell(session));
            watchViews();
            // wide characters and emoji at their width, plain URLs as links, and search (§9.4)
            if (root.Unicode11Addon) {
                term.loadAddon(new root.Unicode11Addon.Unicode11Addon());
                term.unicode.activeVersion = "11";
            }
            if (root.WebLinksAddon) term.loadAddon(new root.WebLinksAddon.WebLinksAddon(openLink));
            if (root.SearchAddon) {
                session.search = new root.SearchAddon.SearchAddon();
                term.loadAddon(session.search);
            }
            term.parser.registerOscHandler(52, onCopy);
            term.attachCustomKeyEventHandler((event) => {
                // copy and paste (§9.5): Ctrl+Shift+C copies a selection, Ctrl+Shift+V pastes; the program gets neither
                if (event.type === "keydown" && event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey) {
                    const letter = String(event.key || "").toLowerCase();
                    if (letter === "c") {
                        if (term.hasSelection()) copyText(term.getSelection());
                        return false;
                    }
                    if (letter === "v") {
                        terminal.pasteFromClipboard(key);
                        return false;
                    }
                }
                return !isOwnKey(event);
            });
            // copy on select, where the clipboard takes it without asking (§9.5)
            term.onSelectionChange(() => {
                if (state.settings.copyOnSelect && term.hasSelection() && root.navigator && root.navigator.clipboard && root.isSecureContext)
                    root.navigator.clipboard.writeText(term.getSelection()).catch(() => {});
            });
            // a paste goes through paste protection first
            el.addEventListener("paste", (event) => {
                const text = event.clipboardData && event.clipboardData.getData("text/plain");
                event.preventDefault();
                event.stopPropagation();
                if (text) terminal.paste(key, text);
            }, true);
            const encoder = new TextEncoder();
            const out = opts.send || ((bytes) => send(session, bytes));
            session.out = (data) => out(encoder.encode(data));
            // the key bar's Ctrl and Alt (§12) apply to the next key typed
            term.onData((data) => session.out(applyModifiers(session, data)));
            touchGestures(session);
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
            reportView(session);
        },

        // Take the session's element out of `container`, if it is there; nothing is disposed.
        detach(key, container) {
            const session = sessions.get(key);
            if (session && session.el.parentNode && (!container || session.el.parentNode === container)) {
                session.shown = Date.now();
                session.el.parentNode.removeChild(session.el);
                reportView(session);
            }
        },

        // Scroll to the prompt before (or after) the top of the view (§11.2).
        jumpPrompt(key, back) {
            const session = sessions.get(key);
            if (!session || !session.prompts.length) return false;
            const top = session.term.buffer.active.viewportY;
            const lines = session.prompts.map((marker) => marker.line).filter((line) => line >= 0).sort((a, b) => a - b);
            const target = back ? lines.filter((line) => line < top).pop() : lines.find((line) => line > top);
            if (target === undefined) {
                if (!back) session.term.scrollToBottom();
                return false;
            }
            session.term.scrollToLine(target);
            return true;
        },

        // A session's own notify setting, cycled: default (the user's settings), all, off.
        cycleNotify(id) {
            const info = state.sessions[id];
            const next = { default: "all", all: "off", off: "default" }[(info && info.notify) || "default"];
            return request("rename", { sessionId: id, notify: next }).then((out) => {
                if (out.session && state.sessions[id]) Object.assign(state.sessions[id], out.session);
            });
        },

        focus(key) {
            const session = sessions.get(key);
            if (session) session.term.focus();
        },

        // Paste `text` into the session. Several lines into a host session whose program has not asked for bracketed
        // paste (a shell would run each line) ask first, unless the user turned paste protection off (§9.5).
        paste(key, text) {
            const session = sessions.get(key);
            if (!session) return;
            const lines = String(text).split(/\r\n|\r|\n/);
            const risky = session.kind === "host" && state.settings.pasteProtection && lines.length > 1 && !session.term.modes.bracketedPasteMode;
            if (!risky) {
                session.term.paste(text);
                return;
            }
            const vm = shellVm();
            const dialog = vm.$q && vm.$q.dialog ? vm.$q.dialog.bind(vm.$q) : root.Quasar && root.Quasar.Dialog ? root.Quasar.Dialog.create.bind(root.Quasar.Dialog) : null;
            if (!dialog) {
                session.term.paste(text);
                return;
            }
            const shown = lines.slice(0, 6).join("\n") + (lines.length > 6 ? "\n…" : "");
            dialog({ title: t("Paste several lines?"), message: t("The program may run each line as a command:") + "\n\n" + shown,
                style: "white-space: pre-wrap", cancel: true, ok: { label: t("Paste"), color: "primary", unelevated: true } })
                .onOk(() => session.term.paste(text))
                .onDismiss(() => session.term.focus());
        },

        pasteFromClipboard(key) {
            if (!root.navigator || !root.navigator.clipboard || !root.navigator.clipboard.readText) return;
            root.navigator.clipboard.readText().then((text) => { if (text) terminal.paste(key, text); }, () => {});
        },

        // The find bar of the session's views.
        openSearch(key) {
            view(key).searching = true;
        },

        copySelection(key) {
            const session = sessions.get(key);
            if (session && session.term.hasSelection()) copyText(session.term.getSelection());
        },

        selectAll(key) {
            const session = sessions.get(key);
            if (session) session.term.selectAll();
        },

        // Clear what scrolled off the screen.
        clearScrollback(key) {
            const session = sessions.get(key);
            if (session) session.term.clear();
        },

        // Find `text` in the session's scrollback, after (or before, `back`) the last match.
        find(key, text, back) {
            const session = sessions.get(key);
            if (!session || !session.search || !text) return false;
            const options = { caseSensitive: false, decorations: { matchOverviewRuler: "#888888", activeMatchColorOverviewRuler: "#ffaa00", activeMatchBackground: "#ffaa0080" } };
            return back ? session.search.findPrevious(text, options) : session.search.findNext(text, options);
        },

        endFind(key) {
            const session = sessions.get(key);
            if (session && session.search) session.search.clearDecorations();
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
        copyText,
        applyModifiers,

        // A key of the phone's key bar (§12): its bytes to the session, or Ctrl and Alt for the next key.
        key(key, name) {
            const session = sessions.get(key);
            if (!session) return;
            if (name === "ctrl" || name === "alt") {
                session.mods = Object.assign({ ctrl: false, alt: false }, session.mods);
                session.mods[name] = !session.mods[name];
                state.modsChanged = (state.modsChanged || 0) + 1;
                session.term.focus();
                return;
            }
            // the arrows as the program asked for them: application cursor keys (ESC O) or normal (ESC [)
            const app = session.term.modes.applicationCursorKeysMode;
            const arrows = { up: "A", down: "B", right: "C", left: "D" };
            const bytes = arrows[name] ? "\x1b" + (app ? "O" : "[") + arrows[name] : { esc: "\x1b", tab: "\t" }[name] || name;
            session.out(applyModifiers(session, bytes));
            session.term.focus();
        },
        mods(key) {
            const session = sessions.get(key);
            return (session && session.mods) || { ctrl: false, alt: false };
        },

        // ---- floating terminals ----

        // The shell's start: the float kind, the quick terminal's key, and the floating terminals of the last visit back.
        startShell(vm) {
            registerFloat();
            // a session that wants its user (a notification of the TerminalAttention topic): the list and the header count
            if (vm && vm.notificationClient && !terminal.attentionOff)
                terminal.attentionOff = vm.notificationClient.registerListener("TerminalAttention", () => terminal.refresh().catch(() => {}));
            if (moqui.keymap && moqui.keymap.registerScope && !terminal.keysOff)
                terminal.keysOff = moqui.keymap.registerScope("global", [
                    { keys: "Ctrl+`", action: "terminalToggle", label: t("Show or hide the quick terminal"), run: () => terminal.toggleQuick() }
                ]);
            if (usedHere()) terminal.restoreFloating();
        },

        // The floating terminals of the last visit, as they were: one attached in another window comes back as a chip, and
        // takes the session here only when it is shown (§10.9).
        restoreFloating() {
            const floats = moqui.floats;
            return terminal.refresh().then(() => {
                if (!floats) return;
                floats.load();
                const keys = Object.keys(floats.restoring || {})
                    .filter((pk) => pk.startsWith(FLOAT + "|"))
                    .map((pk) => pk.slice(FLOAT.length + 1));
                for (const key of keys) {
                    const info = state.sessions[key];
                    if (!info || info.state === "ended") continue;
                    if (info.attached) {
                        const id = floats.open(FLOAT, key, { restore: true });
                        if (id) {
                            state.floating[key] = true;
                            floats.minimize(id);
                        }
                    } else {
                        terminal.float(key, { restore: true });
                    }
                }
            }, () => {});
        },

        // Show the session `key` in a floating window, its element moved there; answers the window's id, or false where
        // nothing floats.
        float(key, options) {
            registerFloat();
            const floats = moqui.floats;
            if (!floats) return false;
            const id = floats.open(FLOAT, key, options || {});
            if (!id) return false;
            state.floating[key] = true;
            state.lastFloat = key;
            floats.whenReady(FLOAT, key).then((body) => {
                if (body && floats.shown(floats.find(FLOAT, key))) terminal.showFloating(key, body);
            });
            return id;
        },

        showFloating(key, body) {
            const target = body || (moqui.floats && moqui.floats.bodyOf(FLOAT, key));
            if (!target) return;
            terminal.start(key).then(() => {
                if (!moqui.floats.find(FLOAT, key)) return;
                terminal.attach(key, target);
                terminal.focus(key);
            }, () => {});
        },

        // Out of its window, docked closed: the session goes on, in the Terminal's list.
        unfloat(key) {
            const f = moqui.floats && moqui.floats.find(FLOAT, key);
            if (f) moqui.floats.close(f.id);
        },

        endFloating(key) {
            return terminal.close(key).then(() => {
                terminal.unfloat(key);
                terminal.forget(key);
            });
        },

        // A pane of the Terminal screen: there now, or once it opens.
        showInScreen(key) {
            if (key && terminal.workspace) {
                terminal.workspace.showHere(key);
                return;
            }
            state.pendingShow = key || "";
            const vm = shellVm();
            if (vm.setUrl) vm.setUrl((vm.appRootPath || "") + "/qapps/Terminal");
        },

        // A browser window of its own for the session, the page with no chrome; it takes the session, and this window offers
        // to take it back (§10.4).
        popOut(key) {
            const url = (shellVm().appRootPath || "") + "/qapps/Terminal?session=" + encodeURIComponent(key) + "&solo=true";
            root.open(url, "_blank", "popup,width=960,height=600");
        },

        // The quick terminal (§10.4): the last floating terminal shown or hidden, else a session floated, else a new one.
        toggleQuick() {
            registerFloat();
            const floats = moqui.floats;
            if (!floats || !floats.canFloat()) {
                terminal.showInScreen(state.lastFloat || state.order[0] || "");
                return;
            }
            if (state.lastFloat && floats.find(FLOAT, state.lastFloat)) {
                floats.toggle(FLOAT, state.lastFloat);
                return;
            }
            const floating = Object.keys(state.floating).find((key) => floats.find(FLOAT, key));
            if (floating) {
                floats.toggle(FLOAT, floating);
                return;
            }
            const pick = () => state.order.find((id) => state.sessions[id] && state.sessions[id].state !== "ended"
                && !state.sessions[id].attached && state.sessions[id].kind !== "apps");
            terminal.refresh().then(() => {
                const existing = pick();
                if (existing) return terminal.float(existing);
                const kinds = (state.options && state.options.kinds) || ["apps"];
                const kind = kinds.includes("command") ? "command" : "apps";
                return terminal.open(kind, { cols: 100, rows: 28 }).then((id) => terminal.float(id));
            }).catch((err) => {
                const vm = shellVm();
                if (vm.$q && vm.$q.notify) vm.$q.notify({ type: "negative", message: (err && err.errors) || String(err) });
            });
        }
    };

    // ---- the Terminal screen's layout: tabs, each a tree of splits whose leaves are panes showing a session ----
    // A node is {pane, id, session} or {split, id, dir: "row" | "column", ratio, a, b}; pure functions, kept by the page.
    let lastId = 0;
    const nodeId = () => "n" + Date.now().toString(36) + (++lastId).toString(36);
    const tree = {
        // a pane showing `session`; `about` keeps what opens it again (kind, profile, cwd) for Start again (§7.5)
        pane(session, about) {
            return Object.assign({ pane: true, id: nodeId(), session }, about || {});
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
        split(node, id, dir, session, about) {
            if (!node) return null;
            if (node.pane) {
                if (node.id !== id) return node;
                return { split: true, id: nodeId(), dir, ratio: 0.5, a: node, b: tree.pane(session, about) };
            }
            return Object.assign({}, node, { a: tree.split(node.a, id, dir, session, about), b: tree.split(node.b, id, dir, session, about) });
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
            return { views: state.views, query: "", found: true,
                labels: { reconnect: t("Reconnect"), takeBack: t("Take it back"), confirm: t("Confirm it is you"), reconnecting: t("Connecting again…"),
                    find: t("Find"), previous: t("Previous match"), next: t("Next match"), close: t("Close"), copy: t("Copy"),
                    paste: t("Paste"), selectAll: t("Select all"), clear: t("Clear scrollback") } };
        },
        computed: {
            shown() {
                return this.views[this.sessionKey] || { closed: false, reason: "", failed: "" };
            },
            // a phone or a tablet: the key bar (§12)
            touch() {
                return !!(this.$q.platform && this.$q.platform.is && (this.$q.platform.is.mobile || (this.$q.platform.has && this.$q.platform.has.touch && this.$q.screen.lt.md)));
            },
            dark() {
                return this.$q.dark.isActive;
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
            find(back) {
                this.found = terminal.find(this.sessionKey, this.query, back) !== false;
            },
            closeSearch() {
                if (this.views[this.sessionKey]) this.views[this.sessionKey].searching = false;
                terminal.endFind(this.sessionKey);
            },
            menu(action) {
                const key = this.sessionKey;
                if (action === "copy") terminal.copySelection(key);
                else if (action === "paste") terminal.pasteFromClipboard(key);
                else if (action === "selectAll") terminal.selectAll(key);
                else if (action === "find") terminal.openSearch(key);
                else if (action === "clear") terminal.clearScrollback(key);
            },
            reconnect() {
                // a remote attach to a command line or a shell wants a recent sign-in first (§8.2): a ticket in an app
                if (this.shown.reauth && inApp()) confirmTicket().then(() => terminal.connect(this.sessionKey), () => {});
                else if (this.shown.reauth && moqui.reauth) moqui.reauth.confirm().then(() => terminal.connect(this.sessionKey), () => {});
                else terminal.connect(this.sessionKey);
            }
        },
        watch: {
            dark(value) {
                terminal.setDark(value);
            },
            sessionKey(key, before) {
                terminal.detach(before, this.$refs.screen);
                this.show();
            },
            "shown.searching"(on) {
                if (on) this.$nextTick(() => this.$refs.search && this.$refs.search.focus());
            }
        },
        template:
            '<div class="m-terminal-view column no-wrap fit" @focusin="$emit(\'focused\')">' +
            '<div v-if="shown.searching" class="m-terminal-search row items-center no-wrap q-px-xs q-gutter-xs">' +
            '<q-input ref="search" dense borderless v-model="query" :placeholder="labels.find" class="col" :error="!found" hide-bottom-space ' +
            '@update:model-value="find(false)" @keydown.enter.prevent="find($event.shiftKey)" @keydown.esc.prevent.stop="closeSearch"></q-input>' +
            '<q-btn flat dense round size="sm" icon="sym_o_keyboard_arrow_up" :aria-label="labels.previous" @click="find(true)"></q-btn>' +
            '<q-btn flat dense round size="sm" icon="sym_o_keyboard_arrow_down" :aria-label="labels.next" @click="find(false)"></q-btn>' +
            '<q-btn flat dense round size="sm" icon="sym_o_close" :aria-label="labels.close" @click="closeSearch"></q-btn></div>' +
            '<div ref="screen" class="m-terminal-host col" style="min-height: 0; overflow: hidden" @click="focus"></div>' +
            '<q-menu context-menu touch-position auto-close><q-list dense style="min-width: 160px">' +
            '<q-item clickable @click="menu(\'copy\')"><q-item-section>{{labels.copy}}</q-item-section><q-item-section side class="text-caption">Ctrl+Shift+C</q-item-section></q-item>' +
            '<q-item clickable @click="menu(\'paste\')"><q-item-section>{{labels.paste}}</q-item-section><q-item-section side class="text-caption">Ctrl+Shift+V</q-item-section></q-item>' +
            '<q-item clickable @click="menu(\'selectAll\')"><q-item-section>{{labels.selectAll}}</q-item-section></q-item>' +
            '<q-item clickable @click="menu(\'find\')"><q-item-section>{{labels.find}}</q-item-section><q-item-section side class="text-caption">Ctrl+Shift+F</q-item-section></q-item>' +
            '<q-item clickable @click="menu(\'clear\')"><q-item-section>{{labels.clear}}</q-item-section></q-item>' +
            "</q-list></q-menu>" +
            '<div v-if="shown.reconnecting" class="m-terminal-closed row items-center no-wrap q-pa-xs q-gutter-sm text-caption">' +
            '<q-spinner size="1em"></q-spinner><span>{{labels.reconnecting}}</span></div>' +
            '<div v-else-if="shown.closed" class="m-terminal-closed row items-center no-wrap q-pa-sm q-gutter-sm" id="TerminalClosed">' +
            '<q-icon name="sym_o_link_off"></q-icon><span class="col">{{shown.reason}}</span>' +
            '<q-btn dense unelevated no-caps color="primary" :label="shown.taken ? labels.takeBack : shown.reauth ? labels.confirm : labels.reconnect" @click="reconnect"></q-btn></div>' +
            '<div v-else-if="shown.failed" class="m-terminal-closed q-pa-sm text-negative">{{shown.failed}}</div>' +
            '<m-terminal-keys v-if="touch" :session-key="sessionKey"></m-terminal-keys>' +
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
                unzoom: t("Restore"), close: t("Close pane"), end: t("End session"), float: t("Float"),
                gone: t("This session ended with the server."), startAgain: t("Start again"),
                floating: t("This session is in a floating window."), showHere: t("Show here") } };
        },
        computed: {
            info() {
                if (this.node.gone) return { title: this.node.title || this.node.profile, cwd: this.node.cwd, state: "ended" };
                return (this.node.pane && this.sessions[this.node.session]) || {};
            },
            focused() {
                return this.workspace.focusedPane === this.node.id;
            },
            zoomed() {
                return this.workspace.zoomedPane === this.node.id;
            },
            floating() {
                return !!(this.node.pane && state.floating[this.node.session]);
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
            '<q-btn v-if="!workspace.narrow" flat dense round size="sm" icon="sym_o_vertical_split" :aria-label="labels.splitRight" @click.stop="workspace.splitPane(node.id, \'row\')"><q-tooltip>{{labels.splitRight}}</q-tooltip></q-btn>' +
            '<q-btn v-if="!workspace.narrow" flat dense round size="sm" icon="sym_o_horizontal_split" :aria-label="labels.splitDown" @click.stop="workspace.splitPane(node.id, \'column\')"><q-tooltip>{{labels.splitDown}}</q-tooltip></q-btn>' +
            '<q-btn flat dense round size="sm" :icon="zoomed ? \'sym_o_close_fullscreen\' : \'sym_o_open_in_full\'" :aria-label="zoomed ? labels.unzoom : labels.zoom" @click.stop="workspace.zoomPane(node.id)"><q-tooltip>{{zoomed ? labels.unzoom : labels.zoom}}</q-tooltip></q-btn>' +
            '<q-btn v-if="workspace.canFloat" flat dense round size="sm" icon="sym_o_picture_in_picture" :aria-label="labels.float" @click.stop="workspace.floatPane(node.id)"><q-tooltip>{{labels.float}}</q-tooltip></q-btn>' +
            '<q-btn flat dense round size="sm" icon="sym_o_close" :aria-label="labels.close" @click.stop="workspace.closePane(node.id)"><q-tooltip>{{labels.close}}</q-tooltip></q-btn>' +
            '<q-btn flat dense round size="sm" icon="sym_o_power_settings_new" :aria-label="labels.end" @click.stop="workspace.endPane(node.id)"><q-tooltip>{{labels.end}}</q-tooltip></q-btn>' +
            "</div>" +
            '<div v-if="node.gone" class="col column flex-center q-pa-md text-center m-terminal-muted">' +
            '<div>{{labels.gone}}</div>' +
            '<q-btn unelevated no-caps color="primary" class="q-mt-sm" :label="labels.startAgain" @click="workspace.startAgain(node.id)"></q-btn></div>' +
            '<div v-else-if="floating" class="col column flex-center q-pa-md text-center m-terminal-muted">' +
            '<div>{{labels.floating}}</div>' +
            '<q-btn unelevated no-caps color="primary" class="q-mt-sm" :label="labels.showHere" @click="workspace.showHere(node.session)"></q-btn></div>' +
            '<m-terminal-view v-else class="col" style="min-height: 0" :session-key="node.session" @focused="workspace.focusPane(node.id)"></m-terminal-view>' +
            "</div>" +
            '<div v-else class="m-terminal-split no-wrap" :class="node.dir === \'row\' ? \'row\' : \'column\'">' +
            '<m-terminal-node :node="node.a" :style="{ flex: node.ratio + \' 1 0\' }" style="min-width: 0; min-height: 0"></m-terminal-node>' +
            '<div class="m-terminal-divider" :class="node.dir === \'row\' ? \'m-terminal-divider-row\' : \'m-terminal-divider-column\'" @pointerdown="startDrag"></div>' +
            '<m-terminal-node :node="node.b" :style="{ flex: (1 - node.ratio) + \' 1 0\' }" style="min-width: 0; min-height: 0"></m-terminal-node>' +
            "</div>"
    };

    // The header's terminal button, while the user has sessions (§10.4): the count of those that want attention, and a menu
    // of them, each shown floating (or in the Terminal screen where nothing floats).
    components["m-terminal-button"] = {
        name: "mTerminalButton",
        data() {
            return { state, labels: { terminal: t("Terminal sessions"), open: t("Open the Terminal") } };
        },
        computed: {
            running() {
                return this.state.order.filter((id) => this.state.sessions[id] && this.state.sessions[id].state !== "ended");
            },
            attention() {
                return this.running.filter((id) => {
                    const info = this.state.sessions[id];
                    return info.unseen && (info.status === "blocked" || info.status === "done");
                }).length;
            }
        },
        methods: {
            dot(id) {
                return "m-terminal-dot-" + statusOf(this.state.sessions[id]);
            },
            show(id) {
                if (moqui.floats && moqui.floats.canFloat()) terminal.float(id);
                else terminal.showInScreen(id);
            },
            openScreen() {
                const vm = shellVm();
                if (vm.setUrl) vm.setUrl((vm.appRootPath || "") + "/qapps/Terminal");
            },
            refresh() {
                terminal.refresh().catch(() => {});
            }
        },
        template:
            '<q-btn v-if="running.length" flat round dense icon="sym_o_terminal" id="m-terminal-button" :aria-label="labels.terminal">' +
            '<q-badge v-if="attention" color="warning" floating rounded>{{attention}}</q-badge>' +
            "<q-tooltip>{{labels.terminal}}</q-tooltip>" +
            '<q-menu @before-show="refresh"><q-list dense style="min-width: 260px">' +
            '<q-item v-for="id in running" :key="id" clickable v-close-popup @click="show(id)" :data-terminal-session="id">' +
            '<q-item-section avatar style="min-width: 0"><span class="m-terminal-dot" :class="dot(id)"></span></q-item-section>' +
            '<q-item-section><q-item-label class="ellipsis">{{state.sessions[id].title || id}}</q-item-label>' +
            '<q-item-label v-if="state.sessions[id].cwd" caption class="ellipsis">{{state.sessions[id].cwd}}</q-item-label></q-item-section></q-item>' +
            '<q-separator></q-separator><q-item clickable v-close-popup @click="openScreen"><q-item-section>{{labels.open}}</q-item-section></q-item>' +
            "</q-list></q-menu></q-btn>"
    };

    // The phone's key bar (§12), above the keyboard: what a phone's keyboard has not got.
    const BAR = [
        { name: "esc", label: "Esc" }, { name: "tab", label: "Tab" }, { name: "ctrl", label: "Ctrl" }, { name: "alt", label: "Alt" },
        { name: "left", icon: "sym_o_arrow_back" }, { name: "up", icon: "sym_o_arrow_upward" }, { name: "down", icon: "sym_o_arrow_downward" },
        { name: "right", icon: "sym_o_arrow_forward" }, { name: "|", label: "|" }, { name: "~", label: "~" }, { name: "/", label: "/" },
        { name: "-", label: "-" }, { name: "`", label: "`" }
    ];
    components["m-terminal-keys"] = {
        name: "mTerminalKeys",
        props: { sessionKey: { type: String, required: true } },
        data() {
            return { state, keys: BAR };
        },
        computed: {
            mods() {
                // read again when a modifier changes
                return this.state.modsChanged >= 0 ? terminal.mods(this.sessionKey) : {};
            }
        },
        methods: {
            press(name) {
                terminal.key(this.sessionKey, name);
            }
        },
        // mousedown.prevent: the terminal keeps the focus, and the phone its keyboard
        template:
            '<div class="m-terminal-keys row no-wrap items-center" role="toolbar" aria-label="Keys">' +
            '<q-btn v-for="key in keys" :key="key.name" dense flat no-caps size="sm" class="m-terminal-key" :data-key="key.name"' +
            ' :icon="key.icon" :label="key.label" :aria-label="key.label || key.name" :class="{ \'m-terminal-key-on\': mods[key.name] }"' +
            ' @mousedown.prevent @click="press(key.name)"></q-btn></div>'
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
