/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The public shell of the anonymous token routes (/s/token, /u/token, /call/token; PublicShell.xml): a Vue and Quasar
 * application with the m-* components of WebrootVue.qvt.js (loaded before this, for its components only) that shows one
 * screen, the route's, with no header, drawers or login. It takes its settings from
 * <script type="application/json" id="moqui-public-config">: route {name, basePath, title, renderModes}, context (what the
 * route's token check answered), the locale and the theme's Quasar configuration. The screen loads as basePath.qvt (or
 * .qvue, .qjs, by its render modes) and its transitions are under basePath. The root offers the parts of m-qapps-root a
 * screen and the m-* components use (setUrl, currentParameters, currentLinkPath, basePath, ...); a link outside the route
 * leaves the page.
 *
 * Island mode (config shell: "island") renders a route's screen without the q-layout at a named element of a page the
 * server rendered, possibly several per page, each its own Vue app with its own config: a
 * <script type="application/json" data-moqui-island="mountId"> holds each island's config (the same shape, with shell
 * "island"), and the island mounts at the element with that id. A link under an island's route reloads only that island
 * and leaves the page's URL as it is. The page mode is unchanged.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const qvt = moqui && moqui.qvt;

    const SCREEN_TEMPLATE = '<component v-if="activeComponent" :is="activeComponent"></component>' +
        '<div v-else-if="failed" class="text-body1 q-pa-lg text-center">{{labels.notFound}}</div>' +
        '<div v-else class="row justify-center q-pa-xl"><q-spinner size="2em"></q-spinner></div>';
    const ROOT_TEMPLATE = '<q-layout view="hHh lpR fFf" class="m-public-shell"><q-page-container><q-page class="q-pa-md">' +
        '<div class="m-public-page" style="max-width:1100px;margin:0 auto;">' + SCREEN_TEMPLATE +
        '</div></q-page></q-page-container></q-layout>';
    // An island: the screen alone, in the page around it.
    const ISLAND_TEMPLATE = '<div class="m-public-island">' + SCREEN_TEMPLATE + '</div>';
    const isIsland = (config) => config && config.shell === "island";

    const queryOf = (search) => {
        const out = {};
        new root.URLSearchParams(search || "").forEach((value, key) => { out[key] = value; });
        return out;
    };

    // The root's options: the route's screen as the one active component, and the parts of the /qapps root the components ask
    // for. Exposed as moqui.publicRootOptions for a test.
    const publicRootOptions = (config) => {
        const route = config.route || {};
        const basePath = route.basePath || "";
        return {
            data() {
                return {
                    activeComponent: null, failed: false, loading: 0, config: config, labels: config.labels || {}, context: config.context || {},
                    basePath: basePath, linkBasePath: basePath, appRootPath: config.appRootPath || "", currentPath: basePath,
                    currentPathList: [], currentParameters: queryOf(root.location ? root.location.search : ""), activeSubscreens: [],
                    navMenuList: [], moquiSessionToken: config.sessionToken || "", userId: null, formsVersion: 0, notifyHistoryList: []
                };
            },
            computed: {
                currentLinkPath() { return this.basePath; }
            },
            // the route's screen for inject: ["mScreen"], as the /qapps root gives it
            provide() {
                const vm = this;
                return { mScreen: { get url() { const search = moqui.objToSearch(vm.currentParameters); return vm.basePath + (search ? "?" + search : ""); },
                    get path() { return vm.basePath; }, get parameters() { return vm.currentParameters; }, containerId: null } };
            },
            methods: {
                // a path under the route stays here (its parameters reload the screen); any other URL leaves the page
                setUrl(url) {
                    if (url == null) return;
                    const href = typeof url === "string" ? url : moqui.makeHref(url);
                    const parsed = moqui.parseHref(href);
                    if (parsed.path === this.basePath || parsed.path === "") {
                        this.currentParameters = parsed.query || {};
                        // an island is part of a page whose URL is not its own
                        if (!isIsland(config) && root.history && root.history.replaceState) root.history.replaceState(null, "", href);
                        this.load();
                    } else root.location.assign(href);
                },
                setParameters(parameters) { this.currentParameters = Object.assign({}, this.currentParameters, parameters || {}); },
                getLinkPath(path) { return path; },
                getRoute() { return { path: this.basePath, matched: [{ path: this.basePath }], params: {}, query: this.currentParameters }; },
                addNotify(message, type) { this.notifyHistoryList.unshift({ message, type, time: Date.now() }); },
                reLoginCheckShow() { root.location.reload(); },
                setPreference() {},
                setScreenContext() {},
                removeScreenContext() {},
                addSubscreen() {},
                addContainer() {},
                removeContainer() {},
                hideContainer() {},
                loadContainer() {},
                reloadContainer() {},
                load() {
                    this.loading++;
                    const search = Object.keys(this.currentParameters).map((key) => encodeURIComponent(key) + "=" +
                        encodeURIComponent(this.currentParameters[key])).join("&");
                    moqui.loadComponent({ path: this.basePath, search: search, renderModes: route.renderModes || [] }, (comp) => {
                        this.loading = Math.max(0, this.loading - 1);
                        if (!comp || comp === moqui.NotFound) { this.failed = true; this.activeComponent = null; return; }
                        this.failed = false;
                        this.activeComponent = comp;
                    }, "public-screen-root");
                }
            },
            created() {
                moqui.sessionToken = this.moquiSessionToken;
                moqui.appRootPath = this.appRootPath;
            },
            mounted() {
                const container = root.document.getElementById(isIsland(config) ? config.mount : "public-root");
                if (container) container.style.display = "";
                this.load();
            }
        };
    };
    if (moqui) moqui.publicRootOptions = publicRootOptions;

    // What every app of the page shares, set once from the first config.
    let globalsSet = false;
    const setGlobals = (config) => {
        if (globalsSet) return;
        globalsSet = true;
        if (qvt) {
            qvt.applyRenders();
            qvt.config = config;
        }
        moqui.config = config;
        if (moqui.setNumberSeparators) moqui.setNumberSeparators(config.locale);
        if (moqui.l10n) {
            moqui.l10n.setLocale(config.locale);
            moqui.l10n.setTimeZone(config.timeZone);
        }
    };

    // One Vue app of a config, mounted at `selector`; its $route is its own root's.
    const mountApp = (config, template, selector) => {
        setGlobals(config);
        const options = publicRootOptions(config);
        options.template = template;
        const app = root.Vue.createApp(options);
        const quasarOptions = { config: Object.assign({ notify: { progress: true, closeBtn: "X", position: "top-right" } }, config.quasar),
            plugins: qvt && qvt.quasarPlugins ? qvt.quasarPlugins(root.Quasar) : { Notify: root.Quasar.Notify, LoadingBar: root.Quasar.LoadingBar, Dialog: root.Quasar.Dialog } };
        const langKey = String(config.quasarLang || "enUS").replace(/-([a-zA-Z])/g, (m, ch) => ch.toUpperCase()).replace("-", "");
        if (root.Quasar.Lang && root.Quasar.Lang[langKey]) quasarOptions.lang = root.Quasar.Lang[langKey];
        if (root.Quasar.IconSet && root.Quasar.IconSet.materialSymbolsOutlined) quasarOptions.iconSet = root.Quasar.IconSet.materialSymbolsOutlined;
        app.use(root.Quasar, quasarOptions);
        if (qvt) Object.keys(qvt.components).forEach((name) => { if (name !== "m-qapps-root") app.component(name, qvt.components[name]); });
        app.config.globalProperties.moqui = moqui;
        app.config.globalProperties.window = root;
        if (qvt && qvt.makeRouter) app.config.globalProperties.$router = qvt.makeRouter();
        let mounted = null;
        Object.defineProperty(app.config.globalProperties, "$route", { get: () => {
            const owner = mounted || moqui.webrootVue;
            return owner ? owner.getRoute() : { matched: [], params: {} };
        } });
        app.config.errorHandler = (err, instance, info) => { console.error("Vue error (" + info + ")", err); };
        mounted = app.mount(selector);
        return mounted;
    };

    const start = () => {
        const config = JSON.parse(root.document.getElementById("moqui-public-config").textContent);
        if (isIsland(config) && config.mount) return startIslands();
        moqui.webrootVue = mountApp(config, ROOT_TEMPLATE, "#public-root");
        return moqui.webrootVue;
    };

    // The islands of a page: each <script type="application/json" data-moqui-island="mountId"> with the config of the
    // island at that element (shell "island"), and #moqui-public-config when it says shell "island" with its mount.
    const islandConfigs = (doc) => {
        const found = [];
        const scripts = doc.querySelectorAll ? Array.from(doc.querySelectorAll('script[type="application/json"][data-moqui-island]')) : [];
        for (const script of scripts) {
            try {
                const config = Object.assign(JSON.parse(script.textContent), { shell: "island", mount: script.getAttribute("data-moqui-island") });
                if (config.mount && doc.getElementById(config.mount)) found.push(config);
            } catch (err) { console.error("A public island's config is not JSON", err); }
        }
        const page = doc.getElementById && doc.getElementById("moqui-public-config");
        if (page) {
            try {
                const config = JSON.parse(page.textContent);
                if (isIsland(config) && config.mount && doc.getElementById(config.mount) && !found.some((other) => other.mount === config.mount)) found.push(config);
            } catch (err) { console.error("The public config is not JSON", err); }
        }
        return found;
    };

    // Mount every island; one that fails is logged and the others still mount. The first is moqui.webrootVue, for the
    // components that ask the root.
    const startIslands = () => {
        const islands = {};
        for (const config of islandConfigs(root.document)) {
            try {
                islands[config.mount] = mountApp(config, ISLAND_TEMPLATE, "#" + config.mount);
                if (!moqui.webrootVue) moqui.webrootVue = islands[config.mount];
            } catch (err) { console.error("The public island " + config.mount + " did not mount", err); }
        }
        moqui.publicIslands = islands;
        return islands;
    };
    if (moqui) {
        moqui.publicIslandConfigs = islandConfigs;
        moqui.startPublicIslands = startIslands;
        moqui.publicIslandTemplate = ISLAND_TEMPLATE;
    }
    if (moqui) moqui.startPublic = start;

    const ready = root.document && root.document.getElementById && root.Vue && root.Quasar && moqui;
    if (ready && root.document.getElementById("moqui-public-config")) start();
    else if (ready && root.document.querySelector && root.document.querySelector("script[data-moqui-island]")) startIslands();
})(globalThis);
