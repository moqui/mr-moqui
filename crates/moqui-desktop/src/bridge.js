/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The page bridge of the desktop app (src/bridge.rs, plans/Mobile-Desktop.md section 4, configuration 1): the host
// adapter window.moquiDesktop that the webroot client calls when it is there (MoquiLib.js notifyNotification and
// saveBlob, WebrootVue.qvt.js openLink and the shell's counts and presence, WebrootCall.js the ring). An
// initialization script of the main window, so it runs on every page the window shows; it defines the adapter only on
// the node's loopback origin, and the host's capability lets only that exact origin call the commands.
//
// No saveFile: a download link (moqui.saveBlob without it, an export link, an attachment) goes to Chromium's download,
// which the host takes with a save dialog and a notification when done (bridge.rs on_download).
(function () {
    "use strict";
    if (window.moquiDesktop || location.protocol !== "http:" || location.hostname !== "127.0.0.1") return;
    const invoke = (cmd, args) => {
        const ipc = window.__TAURI_INTERNALS__;
        if (!ipc || !ipc.invoke) return Promise.resolve(null);
        return ipc.invoke(cmd, args).catch((err) => {
            console.info("moquiDesktop " + cmd + " failed", err);
            return null;
        });
    };
    const text = (value) => (value === undefined || value === null ? null : String(value));
    // a link of the page as a full URL, so the host can tell it is on the node's origin
    const absolute = (link) => {
        if (!link) return null;
        try {
            return new URL(String(link), location.href).href;
        } catch (err) {
            return null;
        }
    };
    window.moquiDesktop = Object.freeze({
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
    });
})();
