/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The desktop client's shell (plans/Mobile-Desktop.md §4, configuration 3): bundled with the app, so it opens with no
// network. The host (src/client/, window.moquiDesktop) keeps the accounts and their device keys; this page asks it which
// account is current and either signs in (Shell/SignIn) or shows the account's shell (Shell/Root). Requests go through
// the host's app scheme to the account's server with its key, so the page holds no credential, sends no cookie and needs
// no CSRF token; the host answers the shell's own requests from its cache when the server cannot be reached.
//
// On the runtime build of Vue, under a CSP without unsafe-eval: the components are the precompiled ones of
// WebrootVue.qvt.render.js (with the call components of WebrootCall.js) and the bundled screens of bundled-screens.js (the
// shell's own, and BasicComms' chat panel, Chat, Inbox, Compose, Call and Dialer, which Shell/Root shows); a screen that is
// not bundled opens in a window of its own, the server's page through the host (moquiDesktop.openRemote).
(function () {
    "use strict";
    const host = window.moquiDesktop;
    const qvt = moqui.qvt;

    // the config until the server's: the runtime build, so loadComponent asks for nothing it cannot make
    moqui.config = { qvt: { build: "runtime", handles: ["static-template"] } };
    // the page's CSP keeps images to its own origin (src/client/assets.rs SHELL_CSP): a mail's images from other sites
    // cannot show here, so the Inbox offers no Show images
    moqui.remoteImages = false;

    // The comms app of the server (its clientConfig's calls.appRoot, under /qapps), and the bundled BasicComms screens named
    // under it, as the server's paths, so their requests go to its transitions (an app such as BOSS that mounts them under
    // its own root, with its transitions added by screen-extend, gets those): ChatPanel, which opens a room with Chat's room
    // view; Chat; the Inbox (Mail), whose compose docks load Compose; Compose; a call's screen (Calls/Call, m-call), which
    // the page's call store (moqui.call) opens; and the Dialer.
    const commsApp = (config) => String((config.calls && config.calls.appRoot) || "/qapps/BasicComms").replace(/^\/qapps/, "/apps");
    const BUNDLED_COMMS = ["Panel/Chat", "Chat", "Inbox/Mail", "Inbox/Compose", "Calls/Call", "Calls/Dialer"];
    const nameBundled = (config) => {
        BUNDLED_COMMS.forEach((name) => {
            const screen = moqui.qvtBundled["BasicComms/" + name];
            if (screen) moqui.qvtBundled[commsApp(config) + "/" + name] = screen;
        });
    };

    // A comms screen the host keeps for the shell (an OS mailto: link, a notification's link: src/client/windows.rs
    // open_page), taken by Shell/Root as it mounts and when the host pokes it; until then (a sign-in, a load) the host keeps it.
    let rootVm = null;
    const takeOpen = () => {
        if (!rootVm || !host || typeof host.takeOpen !== "function") return;
        host.takeOpen().then((path) => { if (path && rootVm) rootVm.openPathFromHost(String(path)); }, (err) => console.info("No link taken from the app", err));
    };
    window.moquiShell = Object.freeze({ poke: takeOpen });

    // A Vue app of a bundled screen, with Quasar and the client's components, mounted on #shell (the one mounted before
    // is unmounted first).
    let mounted = null;
    const mount = (name, props, config) => {
        const root = moqui.bundledComponent(name);
        if (!root) throw new Error("The shell screen " + name + " is not bundled");
        if (mounted) { mounted.unmount(); mounted = null; rootVm = null; }
        const app = window.Vue.createApp(root, props);
        const quasarOptions = {
            config: Object.assign({ brand: { info: "#1e7b8e" }, notify: { progress: true, closeBtn: "X", position: "top-right" } },
                (config && config.quasar) || {}),
            plugins: qvt.quasarPlugins(window.Quasar)
        };
        if (window.Quasar.IconSet && window.Quasar.IconSet.materialSymbolsOutlined) quasarOptions.iconSet = window.Quasar.IconSet.materialSymbolsOutlined;
        app.use(window.Quasar, quasarOptions);
        Object.keys(qvt.components).forEach((component) => app.component(component, qvt.components[component]));
        app.config.globalProperties.moqui = moqui;
        app.config.globalProperties.$mq = moqui;
        app.config.globalProperties.window = window;
        app.config.errorHandler = (err, vm, info) => console.error("Vue error (" + info + ")", err);
        mounted = app;
        return app.mount("#shell");
    };

    // The server's answer to path through the host: its JSON (null when it failed), its status, its text, and whether it
    // came from the host's cache (the server not reached).
    const fetchJson = (path) => moqui.request(path).then((result) => ({
        json: result.json, status: result.status, text: "",
        offline: !!(result.headers && result.headers.get && result.headers.get("X-Moqui-Offline"))
    }), (err) => ({ json: null, status: (err && err.status) || 0, text: (err && moqui.isString(err.text) ? err.text : ""), offline: false }));

    // Why the shell could not open the account: what the server or the host answered.
    const why = (answer) => {
        if (answer.status === 401) return "The server no longer takes this computer's key; sign in again.";
        if (answer.status === 0 || answer.status === 502) return answer.text || "The server could not be reached, and nothing is kept for this account yet.";
        return "The server did not answer with its settings (HTTP " + answer.status + ").";
    };

    const signIn = (state, failed, asked, retryServer) =>
        mount("Shell/SignIn", { state, failed: failed || "", asked: !!asked, retryServer: retryServer || "" }, null);

    const start = () => {
        if (!host || !host.state) {
            document.getElementById("shell").textContent = "This page is the desktop client's shell; it runs in the MR Moqui app.";
            return;
        }
        qvt.applyRenders();
        const params = new URL(location.href).searchParams;
        let asked = null;
        let failedBefore = "";
        let retryServer = "";
        let state = { accounts: [] };
        // the browser storage of accounts the host deleted goes first (a shared device's account signed out)
        host.state().then((given) => (host.clearScopes ? host.clearScopes(given && given.clearScopes) : Promise.resolve()).then(() => given)).then((given) => {
            state = given || state;
            // the page's query only from the host's own load of it (its token, answered once by its state): the sign-in it
            // asks for, why the last one failed (an SSO sign-in that did not finish), and its server
            const token = params.get("load");
            if (token && token === state.loadToken) {
                asked = params.get("signIn");
                failedBefore = params.get("failed") || "";
                retryServer = params.get("server") || "";
            }
            // what this page keeps in the browser's storage is the account's (MoquiLib scopedKey)
            moqui.storageScope = state.current || "";
            if (!state.current || !state.signedIn || asked) {
                signIn(state, failedBefore, asked, retryServer);
                return null;
            }
            return Promise.all([fetchJson("/qapps/clientConfig"), fetchJson("/shellData")]).then(([configAnswer, shellAnswer]) => {
                const config = configAnswer.json;
                if (!moqui.isPlainObject(config)) {
                    signIn(state, why(configAnswer));
                    return null;
                }
                config.qvt = { build: "runtime", handles: ["static-template"] };
                moqui.config = config;
                qvt.config = config;
                nameBundled(config);
                moqui.setNumberSeparators(config.locale);
                // no apps (a server that did not answer them) is an empty list, not a shell that does not open
                const shellData = moqui.isPlainObject(shellAnswer.json) ? shellAnswer.json : { apps: [], counts: null };
                if (moqui.l10n) {
                    moqui.l10n.setLocale(config.locale);
                    moqui.l10n.setTimeZone(config.timeZone);
                    if (config.currencies) moqui.l10n.setCurrencies(config.currencies);
                }
                const ready = moqui.l10n
                    ? Promise.race([
                        moqui.l10n.loadCatalog(config.locale, () => moqui.request("/apps/l10nCatalog?locale=" + encodeURIComponent((config.locale || "en").replace("-", "_")))
                            .then((result) => result.json)).catch((err) => console.warn("no locale catalog", err)),
                        new Promise((resolve) => setTimeout(resolve, 3000))
                    ])
                    : Promise.resolve();
                return ready.then(() => {
                    rootVm = mount("Shell/Root", {
                        state,
                        config,
                        shellData,
                        offline: configAnswer.offline || shellAnswer.offline,
                        chatPath: commsApp(config) + "/Panel/Chat",
                        commsPath: commsApp(config)
                    }, config);
                    // a link the host kept for the shell (opened while it signed in or loaded)
                    takeOpen();
                });
            });
        }).catch((err) => {
            console.error("The shell did not start", err);
            signIn(state, String((err && err.message) || "The shell did not start"));
        });
    };

    if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start, { once: true });
    else start();
})();
