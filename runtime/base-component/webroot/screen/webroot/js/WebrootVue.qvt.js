/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The client of the qvt render mode on Vue 3.5 (the build with the template compiler) and Quasar 2.35: the m-* components
 * the qvt macros write, and the root application that loads a screen as <path>.qvt, .qjs or .qvue and shows it. It needs
 * MoquiLib.js, MoquiL10n.js, vue.global.prod.js and quasar.umd.prod.js before it, takes its settings from the JSON block
 * <script type="application/json" id="moqui-config"> of the page shell, and has no build step and no other library.
 * js/README.md lists the components and the settings.
 *
 * moqui.qvt.components holds the component definitions, so a test can compile their templates without a browser. The
 * application starts by itself when the page has the settings block.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const qvt = { components: {}, config: null };
    moqui.qvt = qvt;

    const markRaw = (obj) => (root.Vue && root.Vue.markRaw ? root.Vue.markRaw(obj) : obj);
    const component = (name, definition) => { qvt.components[name] = definition; return definition; };
    // the components of a script loaded before this one (WebrootChat.js: the account menu's status)
    Object.keys(moqui.qvtComponents || {}).forEach((name) => component(name, moqui.qvtComponents[name]));

    moqui.urlExtensions = { js: "qjs", vue: "qvue", qvt: "qvt" };

    moqui.getQuasarColor = (bootstrapColor) => {
        // Quasar colors: primary, secondary, accent, dark, positive, negative, info, warning
        if (bootstrapColor === "success") return "positive";
        if (bootstrapColor === "danger") return "negative";
        return bootstrapColor;
    };
    // The classes of a tonal (soft tinted) button: the secondary kind of the three the macros write (unelevated primary, tonal, flat).
    moqui.tonalClass = (color, extra) => {
        const quasar = moqui.getQuasarColor(color);
        return ["m-btn-tonal", quasar && quasar !== "default" ? "text-" + quasar : "", extra || ""].filter((part) => part).join(" ");
    };
    // A menu image of type icon (menu-image-type="icon") is CSS classes when it has a space or starts with fa- (the baseline's
    // "fa fa-home"), else a Quasar icon name (sym_o_hub, or a Material Icons name), which m-menu-icon shows with q-icon.
    moqui.isIconClass = (image) => /\s/.test(image || "") || String(image || "").startsWith("fa-");

    const rootVm = () => moqui.webrootVue;

    /* ========== user preferences ========== */

    // The user's preferences in the page are config.preferences (the snapshot the server wrote). moqui.applyPreference(key, value)
    // keeps a new value there and applies at once one the shell reads: QUASAR_DARK (dark mode), QUASAR_LEFT_OPEN (the app drawer),
    // KEYMAP (the key overrides) and SHELL_DENSITY (comfortable or compact: the m-density-compact body class). A response to a request
    // that set preferences carries them (the X-Moqui-Preferences header, which moqui.request reads), and they are applied the same
    // way, so a Preferences screen that saves through its own transition needs nothing more; one that saves in the browser alone
    // calls moqui.applyPreference itself. Another key is only kept.
    moqui.preferenceAppliers = {
        QUASAR_DARK: (value, vm) => { if (vm && vm.$q && vm.$q.dark) vm.$q.dark.set(value === "true"); },
        QUASAR_LEFT_OPEN: (value, vm) => { if (vm && "leftOpen" in vm) vm.leftOpen = value === "true"; },
        KEYMAP: (value) => { if (!moqui.keymap.setOverrides(value === undefined ? null : value)) console.info("The KEYMAP preference is not JSON of {scope: {action: keys}}, ignored"); },
        SHELL_DENSITY: (value) => moqui.applyDensity(value)
    };
    moqui.applyPreference = (key, value) => {
        if (!key) return;
        const text = value === null || value === undefined ? null : String(value);
        const config = qvt.config || moqui.config;
        if (config) {
            if (!config.preferences) config.preferences = {};
            if (text === null || text === "") delete config.preferences[key];
            else config.preferences[key] = text;
            if (key === "SHELL_DENSITY") config.density = text || "comfortable";
        }
        const applier = moqui.preferenceAppliers[key];
        if (applier) applier(text, rootVm());
    };
    moqui.applyPreferences = (map) => { if (moqui.isPlainObject(map)) Object.keys(map).forEach((key) => moqui.applyPreference(key, map[key])); };
    moqui.onPreferences = moqui.applyPreferences;
    // SHELL_DENSITY: compact puts m-density-compact on the body (the style sheet tightens the page, lists, fields and cards);
    // comfortable, empty or anything else is the default.
    moqui.applyDensity = (value) => {
        const body = root.document && root.document.body;
        if (body && body.classList) body.classList.toggle("m-density-compact", value === "compact");
    };
    // SHELL_START_APP: the /qapps address with no path opens that app (config.startApp, set by the server only for an app the user
    // may open) in place of the app list.
    moqui.startPath = (pathname, linkBasePath, startApp) => {
        const base = linkBasePath || "/qapps";
        if (!startApp || (pathname !== base && pathname !== base + "/")) return pathname;
        return base + "/" + startApp;
    };
    const notify = (options) => {
        const vm = rootVm();
        if (vm && vm.$q) vm.$q.notify(options);
        else console.info(options.message);
    };
    const addNotify = (message, type, link, icon) => { if (rootVm()) rootVm().addNotify(message, type, link, icon); };

    /* ========== notify and error handling ========== */

    moqui.notifyOpts = { timeout: 1500, type: "positive" };
    moqui.notifyOptsInfo = { timeout: 5000, type: "info" };
    moqui.notifyOptsError = { timeout: 15000, type: "negative" };

    // Whether a link leaves the application: another scheme or host. A path, a relative link and a URL of this origin stay in the shell.
    moqui.isExternalLink = (link) => {
        const text = String(link || "").trim();
        if (/^\/\//.test(text)) return true;
        if (!/^[a-z][a-z0-9+.-]*:/i.test(text)) return false;
        const location = root.location;
        return !(location && location.origin && (text === location.origin || text.indexOf(location.origin + "/") === 0));
    };
    // Follow a link of a message or notification: an app link through the shell ($root.setUrl), an external one in a new window.
    moqui.openLink = (link) => {
        if (!link) return;
        const text = String(link).trim();
        if (moqui.isExternalLink(text)) {
            if (root.open) root.open(text, "_blank", "noopener");
            return;
        }
        const location = root.location;
        const path = location && location.origin && text.indexOf(location.origin) === 0 ? text.slice(location.origin.length) || "/" : text;
        if (rootVm() && rootVm().setUrl) rootVm().setUrl(path);
        else if (location) location.href = path;
    };
    // The Quasar Notify action of a message with a link: a button that follows it.
    moqui.linkNotifyAction = (link, linkText) => ({ label: linkText || (moqui.l10n ? moqui.l10n.localize("Open") : "Open"), color: "white", noCaps: true,
        handler: () => moqui.openLink(link) });

    // The messages, errors and validation errors of an answer as notifications. A message is a text or a map
    // {message, type, link, linkText}: with a link the notification has a button (linkText, "Open" by default) that follows it.
    moqui.notifyMessages = (messages, errors, validationErrors) => {
        let notified = false;
        const show = (options, message, type) => {
            notify(Object.assign({}, options, { message }));
            addNotify(message, type || options.type);
            notified = true;
        };
        if (messages) {
            (Array.isArray(messages) ? messages : [messages]).forEach((item) => {
                if (moqui.isPlainObject(item)) {
                    const type = moqui.getQuasarColor(item.type) || "info";
                    const options = Object.assign({}, moqui.notifyOptsInfo, { type, message: item.message });
                    if (item.link) options.actions = [moqui.linkNotifyAction(item.link, item.linkText)];
                    notify(options);
                    addNotify(item.message, type, item.link || undefined);
                    notified = true;
                } else if (item !== null && item !== undefined && item !== "") show(moqui.notifyOptsInfo, item, "info");
            });
        }
        if (errors) (Array.isArray(errors) ? errors : [errors]).forEach((item) => show(moqui.notifyOptsError, item, "negative"));
        if (validationErrors) (Array.isArray(validationErrors) ? validationErrors : [validationErrors]).forEach(moqui.notifyValidationError);
        if (validationErrors) notified = true;
        return notified;
    };

    moqui.notifyValidationError = (valError) => {
        let message = valError;
        if (moqui.isPlainObject(valError)) {
            message = valError.message;
            if (valError.fieldPretty && valError.fieldPretty.length) message = message + " (for field " + valError.fieldPretty + ")";
        }
        notify(Object.assign({}, moqui.notifyOptsError, { message }));
        addNotify(message, "negative");
    };

    // err is what moqui.request rejects with: { status, json, text, aborted, error }
    moqui.handleAjaxError = (err) => {
        if (!err || err.aborted) return;
        const respObj = err.json && moqui.isPlainObject(err.json) ? err.json : null;
        console.warn("request failed, status " + err.status + (err.statusText ? " " + err.statusText : ""));
        let notified = false;
        if (err.status === 401) notified = moqui.notifyMessages(null, "No user authenticated");
        else if (respObj) notified = moqui.notifyMessages(respObj.messageInfos, respObj.errors, respObj.validationErrors);
        else if (err.text && moqui.isString(err.text) && err.text.length && !/^\s*</.test(err.text)) notified = moqui.notifyMessages(err.text);
        if (err.status === 401) {
            // let the server remember the URL and send the user to login, or ask for the password again in a dialog
            if (rootVm() && rootVm().reLoginCheckShow) rootVm().reLoginCheckShow();
            else root.location.reload();
        } else if (err.status === 0) {
            const msg = "Could not connect to server";
            notify(Object.assign({}, moqui.notifyOptsError, { message: msg }));
            addNotify(msg, "negative");
        } else if (!notified && !(respObj && respObj.fieldErrorsPlaced)) {
            const errMsg = "Error: " + (err.statusText || err.status);
            notify(Object.assign({}, moqui.notifyOptsError, { message: errMsg }));
            addNotify(errMsg, "negative");
        }
    };

    moqui.notifyGrowl = (jsonObj) => {
        if (!jsonObj) return;
        const options = Object.assign({}, moqui.notifyOptsInfo, { type: moqui.getQuasarColor(jsonObj.type) || "info", message: jsonObj.title });
        if (jsonObj.link) options.actions = [moqui.linkNotifyAction(jsonObj.link, "View")];
        notify(options);
        addNotify(jsonObj.title, jsonObj.type, jsonObj.link, jsonObj.icon);
    };

    /* ========== component loading ========== */

    moqui.componentCache = new moqui.LruMap(50);

    moqui.NotFound = markRaw({ template: '<div id="current-page-root"><h4>Screen not found at {{$root.currentPath}}</h4></div>' });
    moqui.EmptyComponent = markRaw({ template: '<div id="current-page-root"><div class="spinner"><div>&nbsp;</div></div></div>' });

    // The screen a component was loaded as (js/README.md, Loading a screen): {url, path, parameters, containerId}, the path without its
    // query and its .qvt, .qvue or .qjs, the parameters its query. Each m-dynamic-container and m-dynamic-dialog provides it as mScreen
    // for the screen it loaded (so a tray tab, the quick-create dialog and a record panel too), and the root for the page, so a .qvue
    // screen reads its own query with inject: ["mScreen"] wherever it is. The members are getters over the loader, so a watch or a
    // template that reads one follows the next load.
    moqui.urlScreen = (urlOf, containerId) => {
        const parts = () => {
            const url = String(urlOf() || "");
            const quest = url.indexOf("?");
            const path = (quest >= 0 ? url.slice(0, quest) : url).replace(/\.(qvt|qvue|qjs)$/, "");
            return { url, path, search: quest >= 0 ? url.slice(quest + 1) : "" };
        };
        return {
            get url() { return parts().url; },
            get path() { return parts().path; },
            get parameters() { return moqui.searchToObj(parts().search); },
            containerId: containerId || null
        };
    };

    // A text of a merged qvt response as a component: the template of the screen in a div with the id of its container.
    // A script element is run by m-script and a link element is loaded by m-stylesheet, because Vue ignores both.
    moqui.mergedFragmentComponent = (text, divId) => {
        const templateText = text.replace(/<script/g, "<m-script").replace(/<\/script>/g, "</m-script>")
            .replace(/<link\b([^>]*?)\/?>/g, "<m-stylesheet$1></m-stylesheet>");
        return markRaw({ template: "<div" + (divId && divId.length > 0 ? ' id="' + divId + '"' : "") + ">" + templateText + "</div>" });
    };

    // How a qvt response is turned into a component, by the X-Moqui-Qvt response header. A merged fragment is a server-merged template
    // (the compiler build of Vue compiles it). A static template is a script that registers a render function, and its handler is
    // below with the rest of the server-static client. A handler may return the component or a promise of it.
    moqui.qvtHandlers = {
        "merged-fragment": (text, info) => {
            if (qvt.config && qvt.config.qvt && qvt.config.qvt.build === "runtime") {
                throw new Error("This page loads the runtime build of Vue and cannot compile a server-merged qvt response");
            }
            return moqui.mergedFragmentComponent(text, info.divId);
        }
    };

    // a .qvue component's <style>, once per page: the key is the text's length and a hash, compared as an attribute value and never
    // put in a selector (the text's first characters, with a line break or a quote, made an invalid selector and lost the style)
    const styleKey = (css) => {
        let hash = 0;
        for (let i = 0; i < css.length; i++) hash = (Math.imul(hash, 31) + css.charCodeAt(i)) | 0;
        return css.length + "-" + (hash >>> 0).toString(36);
    };
    const addStyle = (css, scoped) => {
        const key = styleKey(String(css));
        const head = root.document.head;
        if (Array.from(head.querySelectorAll("style[data-mq-style]")).some((el) => el.getAttribute("data-mq-style") === key)) return;
        const style = root.document.createElement("style");
        style.setAttribute("data-mq-style", key);
        style.textContent = scoped ? css : css;
        head.appendChild(style);
    };
    moqui.addComponentStyle = addStyle;

    // path: the screen path, extraPath, search and bodyParameters as the router has them. renderModes lists the render
    // modes the screen says it supports: qjs or qvue makes the request for that companion instead of qvt. Without them (a string
    // URL: an m-dynamic-container or m-dynamic-dialog, a tray tab, a quick-create kind) the request is .qvt with
    // X-Moqui-Client-Modes: qvue,qjs, and the server answers the first of those the screen's render-modes name, saying which in
    // X-Moqui-Qvt, so a .qvue screen loads as qvue wherever it is loaded, as the router picks it. A path ending in .qvt, .qvue or
    // .qjs is that mode.
    moqui.clientModesHeader = "qvue,qjs";
    moqui.loadComponent = (urlInfo, callback, divId) => {
        const jsExt = moqui.urlExtensions.js;
        const vueExt = moqui.urlExtensions.vue;
        const qvtExt = moqui.urlExtensions.qvt;
        let path;
        let extraPath;
        let search;
        let bodyParameters;
        let renderModes;
        if (typeof urlInfo === "string") {
            const questIdx = urlInfo.indexOf("?");
            if (questIdx > 0) {
                path = urlInfo.slice(0, questIdx);
                search = urlInfo.slice(questIdx + 1);
            } else path = urlInfo;
        } else {
            path = urlInfo.path;
            extraPath = urlInfo.extraPath;
            search = urlInfo.search;
            bodyParameters = urlInfo.bodyParameters;
            renderModes = urlInfo.renderModes;
        }
        // if Quasar says it is mobile then tell the server through the _uiType parameter
        const mobile = root.innerWidth <= 600 || (root.Quasar && root.Quasar.Platform.is.mobile);
        if (mobile && (!search || search.indexOf("_uiType") === -1)) search = (search ? search + "&" : "") + "_uiType=mobile";

        const use = { path, extraPath, search, divId };
        // A server-static template is cached as the means to make a component, since each use fetches its own data.
        const give = (comp) => callback(comp && comp.mqStatic ? comp.mqStatic(use) : comp);
        const cached = moqui.componentCache.get(path);
        if (cached) {
            give(cached);
            return { abort() {} };
        }

        let url = path;
        const modes = renderModes || [];
        let kind = qvtExt;
        let negotiate = false;
        if (url.slice(-vueExt.length) === vueExt) kind = vueExt;
        else if (url.slice(-jsExt.length) === jsExt) kind = jsExt;
        else if (url.slice(-qvtExt.length - 1) === "." + qvtExt) kind = qvtExt;
        else if (modes.indexOf(vueExt) >= 0) { url += "." + vueExt; kind = vueExt; }
        else if (modes.indexOf(jsExt) >= 0) { url += "." + jsExt; kind = jsExt; }
        else { url += "." + qvtExt; negotiate = !renderModes; }
        if (extraPath && extraPath.length > 0) url += "/" + extraPath;
        if (search && search.length > 0) url += "?" + search;

        const controller = new root.AbortController();
        const hasBody = bodyParameters && !moqui.isEmptyObject(bodyParameters);
        const options = { signal: controller.signal, accept: "text/html,application/javascript,*/*" };
        if (negotiate) options.headers = { "X-Moqui-Client-Modes": moqui.clientModesHeader };
        if (hasBody) { options.method = "POST"; options.params = bodyParameters; }
        console.info("loadComponent " + url + (divId ? " id " + divId : ""));

        const done = (comp, cacheable) => {
            if (cacheable) moqui.componentCache.put(path, comp);
            give(comp);
        };
        moqui.request(url, options).then((result) => {
            if (result.status === 205) {
                rootVm().setUrl(result.headers.get("X-Redirect-To"));
                return;
            }
            const text = result.text;
            if (!text) { callback(moqui.NotFound); return; }
            const cacheControl = result.headers.get("Cache-Control") || "";
            const cacheable = cacheControl.indexOf("max-age") >= 0;
            // the mode the server chose for a .qvt request that named the client modes
            const answered = negotiate ? result.headers.get("X-Moqui-Qvt") : null;
            if (answered === vueExt || answered === jsExt) kind = answered;
            if (result.json && moqui.isPlainObject(result.json) && kind === qvtExt) {
                if (result.json.screenUrl) rootVm().setUrl(result.json.screenUrl);
                else if (result.json.redirectUrl) root.location.replace(result.json.redirectUrl);
                else callback(moqui.NotFound);
                return;
            }
            if (kind === vueExt) {
                done(markRaw(moqui.parseVue(text, addStyle)), cacheable);
            } else if (kind === jsExt || text.slice(0, 7) === "define(") {
                const comp = markRaw(moqui.evalComponentJs(text));
                if (comp.template) done(comp, cacheable);
                else {
                    // the companion has the script and the template is the qvt of the same screen
                    const qvtUrl = (path.slice(-jsExt.length - 1) === "." + jsExt ? path.slice(0, -jsExt.length - 1) : path) + "." + qvtExt;
                    moqui.request(qvtUrl + (search ? "?" + search : ""), { signal: controller.signal, accept: "text/html" }).then((htmlResult) => {
                        comp.template = htmlResult.text;
                        done(comp, cacheable);
                    }, moqui.handleLoadError);
                }
            } else {
                const handlerName = result.headers.get("X-Moqui-Qvt") || "merged-fragment";
                const handler = moqui.qvtHandlers[handlerName];
                if (!handler) throw new Error("Unknown X-Moqui-Qvt response: " + handlerName);
                return Promise.resolve(handler(text, { path, url, divId, headers: result.headers, search })).then((comp) => done(markRaw(comp), cacheable));
            }
        }).catch((err) => {
            if (err instanceof Error) {
                console.error(err);
                moqui.notifyMessages(null, err.message);
                if (rootVm()) rootVm().loading = 0;
            } else moqui.handleLoadError(err);
        });
        return controller;
    };

    moqui.handleLoadError = (err) => {
        if (err && err.aborted) { console.warn("load aborted"); return; }
        if (rootVm()) rootVm().loading = 0;
        moqui.handleAjaxError(err);
    };

    // A validation error's line in errors, as the server writes it (Java's ValidationError.toStringPretty): the message, then "(" and the
    // pretty field, form and service names that are there, with no closing parenthesis.
    moqui.validationErrorText = (ve) => (ve.message || "") + "(" + (ve.fieldPretty ? "for field " + ve.fieldPretty : "") +
        (ve.formPretty ? " on form " + ve.formPretty : "") + (ve.serviceNamePretty ? " of service " + ve.serviceNamePretty : "");

    // The validation errors of a form's own fields go to the form (m-form fieldErrors), shown by the field. A copy of the response without
    // them, and without their text in errors, is what is left to notify. An error for another form or for no field stays in the copy.
    moqui.placeFieldErrors = (form, resp) => {
        if (!form || !resp || !moqui.isPlainObject(resp) || !Array.isArray(resp.validationErrors)) return resp;
        const formName = form.fields ? form.fields.moquiFormName : null;
        const placed = [];
        const rest = resp.validationErrors.filter((ve) => {
            if (!moqui.isPlainObject(ve) || !ve.field || (ve.form && formName && ve.form !== formName)) return true;
            if (!Object.prototype.hasOwnProperty.call(form.fields, ve.field)) return true;
            placed.push(ve);
            return false;
        });
        if (!placed.length) return resp;
        form.setFieldErrors(placed);
        const placedText = placed.map((ve) => ve.message).concat(placed.map(moqui.validationErrorText));
        const copy = Object.assign({}, resp, { validationErrors: rest, fieldErrorsPlaced: placed.length });
        if (moqui.isString(resp.errors)) {
            const lines = resp.errors.split("\n").filter((line) => placedText.indexOf(line) === -1);
            copy.errors = lines.length ? lines.join("\n") : null;
        } else if (Array.isArray(resp.errors)) copy.errors = resp.errors.filter((line) => placedText.indexOf(line) === -1);
        return copy;
    };

    /* ========== server-static screens ========== */

    // A screen sent as a data-free template (plans/QvtServerStatic.md) is a script that registers a render function in moqui.qvtStatic[path],
    // the server compiled it with Vue's prefixIdentifiers option so it needs no compiler and no eval. The screen's data is fetched from
    // <path>/actions, and each form-list's rows from <path>/actions/<form>. Expressions in the template read names through a scope chain.
    moqui.qvtStatic = moqui.qvtStatic || {};
    moqui.qvtStaticTags = moqui.qvtStaticTags || {};

    const hasOwn = (obj, key) => obj !== null && obj !== undefined && typeof obj === "object" && Object.prototype.hasOwnProperty.call(obj, key);
    const read = (source) => (typeof source === "function" ? source() : source);

    // A scope: a name is looked up in the extra map, then the map, then the scope around it, as the context stack finds a name. Each of the
    // three is a map or a function that gives one. $scope is the scope itself, for a name that is not an identifier.
    qvt.makeScope = (map, parent, extra) => {
        const scope = new Proxy({}, {
            get(target, key) {
                if (typeof key === "symbol") return undefined;
                if (key === "$scope") return scope;
                const extras = read(extra);
                if (hasOwn(extras, key)) return extras[key];
                const values = read(map);
                if (hasOwn(values, key)) return values[key];
                return parent ? parent[key] : undefined;
            },
            has(target, key) {
                if (typeof key !== "string") return false;
                return key === "$scope" || hasOwn(read(extra), key) || hasOwn(read(map), key) || (parent ? key in parent : false);
            },
            set(target, key, value) {
                const extras = read(extra);
                const values = read(map);
                if (hasOwn(extras, key)) extras[key] = value;
                else if (hasOwn(values, key) || !parent) { if (values && typeof values === "object") values[key] = value; }
                else parent[key] = value;
                return true;
            }
        });
        return scope;
    };

    // The utility calls a server-static template may make: pure functions that give what the server's do (moqui_lib::string).
    qvt.StringUtilities = {
        // A part equal to "id" in any case becomes "ID"; ".", "#", a capital and a digit start a part; a part that repeats the one before is dropped.
        camelCaseToPretty: (camel) => {
            const text = camel === null || camel === undefined ? "" : String(camel);
            const raw = [];
            let current = "";
            for (let i = 0; i < text.length; i++) {
                const ch = text.charAt(i);
                if (i > 0 && /[A-Z0-9.#]/.test(ch) && current) { raw.push(current); current = ""; }
                current += ch;
            }
            if (current) raw.push(current);
            const parts = [];
            let last = null;
            raw.forEach((piece) => {
                let part = piece.charAt(0) === "." || piece.charAt(0) === "#" ? piece.substring(1) : piece;
                if (!part) return;
                if (part.charAt(0) !== part.charAt(0).toUpperCase()) part = part.charAt(0).toUpperCase() + part.substring(1);
                if (part.toLowerCase() === "id") part = "ID";
                if (part === last) return;
                last = part;
                parts.push(part);
            });
            return parts.join(" ");
        }
    };

    // The Quasar color of a btn-type or a dialog type, as the macros map them: success is positive, danger negative, default none.
    qvt.quasarColor = (color) => (color === "success" ? "positive" : color === "danger" ? "negative" : color === "default" ? "" : color || "");

    // The helpers a template calls as $mq: the few things an expression needs that are not in the language.
    const NUMBER_TEXT = /^[-+]?\d+(\.\d+)?$/;
    qvt.helpers = {
        str: (value) => (value === null || value === undefined ? "" : String(value)),
        dflt: (value, fallback) => (value === null || value === undefined || value === "" ? fallback : value),
        nul: (value) => (value === undefined || value === "" ? null : value),
        // The number checks of a field's browser rules, the same the server writes into the rule text
        // ($mq.isStringNumber and the rest). They are the client's own, so the locale separators are
        // the user's.
        isStringNumber: (value) => moqui.isStringNumber(value),
        isStringInteger: (value) => moqui.isStringInteger(value),
        parseNumber: (value) => moqui.parseNumber(value),
        // The current value of a drop-down, check or radio: a list for several, else the first of a comma list, as the server reads it.
        cur: (value, multiple, none) => {
            let text = value === null || value === undefined || value === "" ? (none === undefined || none === null ? "" : String(none)) : value;
            if (Array.isArray(text)) return multiple ? text.map(String) : text.length ? String(text[0]) : "";
            text = String(text);
            if (text.charAt(0) === "[") text = text.substring(1, text.length - 1).replace(/ /g, "");
            const list = text ? text.split(",") : [];
            return multiple ? list : list.length ? list[0] : "";
        },
        // A value as text with a Java pattern. A decimal arrives as a string, so the pattern says what it is.
        fmt: (value, pattern, type) => qvt.formatValue(value, pattern, type),
        // The kind of a link or menu button by a btn-type that is an expression, as the macros write it (ScreenQvt.js btnKind):
        // primary is the filled primary button, any other value or none a tonal one in that color; btnText is the text color
        // class of a flat button (hidden-form-link).
        btnKind: (btnType, extra) => {
            const type = btnType === null || btnType === undefined ? "" : String(btnType).trim();
            if (type === "primary") return { color: "primary", class: extra || undefined };
            const color = qvt.quasarColor(type || "primary");
            return { class: ["m-btn-tonal", color ? "text-" + color : "", extra || ""].filter((part) => part).join(" ") };
        },
        btnText: (btnType) => {
            const color = qvt.quasarColor(btnType === null || btnType === undefined ? "primary" : String(btnType).trim() || "primary");
            return color ? "text-" + color : "";
        },
        // The classes of a row-col whose sizes are expressions, as the macros write them (ScreenQvt.js colOpen): each size given,
        // and the full width below the smallest one given.
        cols: (lg, md, sm, xs, plain) => {
            const size = (value) => (value === null || value === undefined || value === "null" ? "" : String(value).trim());
            const [l, m, s, x] = [size(lg), size(md), size(sm), size(xs)];
            return [plain ? "" : "q-px-xs", l ? "col-lg-" + l : "", m ? "col-md-" + m : l && !plain ? "col-md-12" : "",
                s ? "col-sm-" + s : (l || m) && !plain ? "col-sm-12" : "", x ? "col-xs-" + x : (l || m || s) && !plain ? "col-xs-12" : ""]
                .filter((part) => part).join(" ");
        },
        merge: (...maps) => {
            const out = {};
            maps.forEach((map) => { if (map && typeof map === "object") Object.keys(map).forEach((key) => { out[key] = map[key]; }); });
            return out;
        },
        // The entries of a map that have a value: not null, undefined or empty text.
        given: (map) => {
            const out = {};
            if (map && typeof map === "object") Object.keys(map).forEach((key) => {
                const value = map[key];
                if (value !== null && value !== undefined && value !== "") out[key] = value;
            });
            return out;
        },
        qs: (params, map) => {
            const merged = qvt.helpers.merge(map, params);
            const pairs = [];
            Object.keys(merged).forEach((key) => {
                const value = merged[key];
                if (value === null || value === undefined || value === "" || key === "moquiSessionToken") return;
                if (Array.isArray(value)) value.forEach((item) => pairs.push(encodeURIComponent(key) + "=" + encodeURIComponent(item)));
                else pairs.push(encodeURIComponent(key) + "=" + encodeURIComponent(value));
            });
            return pairs.join("&");
        },
        url: (path, params, map) => {
            const query = qvt.helpers.qs(params, map);
            return query ? path + (path.indexOf("?") >= 0 ? "&" : "?") + query : path;
        },
        // The url a ${} expression of a link gave, as the server render resolves one: a relative path against the screen's own path
        // (base, a directory: "Detail" under it, ".." its parent), with its query; an absolute path, an anchor and an external
        // URL as they are.
        link: (text, base) => {
            const value = text === null || text === undefined ? "" : String(text).trim();
            if (!value) return base || "";
            if (value.charAt(0) === "/" || value.charAt(0) === "#" || moqui.isExternalLink(value)) return value;
            const cut = value.search(/[?#]/);
            const path = cut < 0 ? value : value.slice(0, cut);
            const parts = String(base || "").split("/").filter((part) => part);
            path.split("/").forEach((part) => {
                if (part === "" || part === ".") return;
                if (part === "..") parts.pop();
                else parts.push(part);
            });
            return "/" + parts.join("/") + (cut < 0 ? "" : value.slice(cut));
        },
        // A text-map of a text: whether the map has the name, or with value true the entry itself.
        inMap: (map, name, value) => {
            const has = map !== null && map !== undefined && typeof map === "object" && Object.prototype.hasOwnProperty.call(map, name);
            return value ? (has ? map[name] : undefined) : has;
        },
        // The compare of an XML condition (moqui_lib::object::compare): both sides converted by the type, then the operator. A package holds a
        // decimal as text, so a numeric type compares numbers; Object compares numbers when both sides are numbers or numeric text, else text.
        compare: (field, operator, value, toField, format, type) => {
            const empty = (v) => v === null || v === undefined || v === "" || (Array.isArray(v) && !v.length) || (moqui.isPlainObject(v) && !Object.keys(v).length);
            let target = value === null || value === undefined ? toField : value;
            const kind = String(type || "Object");
            const numeric = /^(BigDecimal|Long|Integer|Double|Float|Number|java\.math\.BigDecimal|java\.lang\.(Long|Integer|Double))$/.test(kind);
            const isNum = (v) => typeof v === "number" || (typeof v === "string" && NUMBER_TEXT.test(v.trim()));
            const conv = (v) => {
                if (v === null || v === undefined || (typeof v === "string" && v === "" && kind !== "String")) return null;
                if (numeric) return Number(v);
                if (kind === "Boolean") return v === true || String(v) === "true";
                if (kind === "String") return String(v);
                return v;
            };
            let left = conv(field);
            let right = empty(target) ? (target === undefined ? null : target) : conv(target);
            if (kind === "Object" && isNum(left) && isNum(right)) { left = Number(left); right = Number(right); }
            const text = (v) => (v === null || v === undefined ? "" : String(v));
            const order = () => {
                if (left === null) return right === null ? 0 : 1;
                if (right === null) throw new Error("cannot compare a value to null");
                if (typeof left === "number" && typeof right === "number") return left < right ? -1 : left > right ? 1 : 0;
                const a = text(left);
                const b = text(right);
                return a < b ? -1 : a > b ? 1 : 0;
            };
            const full = (pattern) => new RegExp("^(?:" + text(pattern) + ")$");
            switch (operator || "equals") {
                case "less": return order() < 0;
                case "greater": return order() > 0;
                case "less-equals": return order() <= 0;
                case "greater-equals": return order() >= 0;
                case "contains": return text(left).indexOf(text(right)) >= 0;
                case "not-contains": return text(left).indexOf(text(right)) < 0;
                case "empty": return empty(field);
                case "not-empty": return !empty(field);
                case "matches": return full(right).test(text(left));
                case "not-matches": return !full(right).test(text(left));
                case "not-equals": return !(left === right || (left !== null && right !== null && typeof left === typeof right && String(left) === String(right)));
                default: return left === right || (left !== null && right !== null && typeof left === typeof right && String(left) === String(right));
            }
        },
        // The style of a field shown only while another field has a value.
        shown: (expected, actual) => {
            let wanted = expected;
            if (typeof wanted === "string" && wanted.indexOf(",") >= 0) wanted = wanted.split(",");
            const holds = Array.isArray(wanted) ? wanted.map(String).includes(String(actual)) : String(wanted) === String(actual);
            return { display: holds ? "" : "none" };
        }
    };

    qvt.formatValue = (value, pattern, type) => {
        if (value === null || value === undefined || value === "") return "";
        let kind = type === "date-time" ? "timestamp" : type;
        // No pattern: epoch millis text (a time stamp in the screen's data) is the time stamp, as l10n.rs epoch_millis_text
        if (!kind && !pattern && typeof value === "string" && /^\d{12,13}$/.test(value.trim())) {
            const millis = Number(value.trim());
            if (millis >= 1e12 && millis < 4102444800000) { kind = "timestamp"; value = millis; }
        }
        if (!kind && pattern && typeof value === "string") {
            if (NUMBER_TEXT.test(value.trim()) && /[#0]/.test(pattern) && !/[yMdHhms]/.test(pattern.replace(/'[^']*'/g, ""))) kind = "number";
            // epoch millis text with a date pattern (h:mm a as well as yyyy-MM-dd)
            else if (/^-?\d{9,}$/.test(value.trim()) && moqui.l10n.isDatePattern(pattern)) { kind = "timestamp"; value = Number(value.trim()); }
            else if (/^\d{4}-\d\d-\d\d/.test(value)) kind = /^\d{4}-\d\d-\d\d$/.test(value) ? "date" : "timestamp";
            else if (/^\d\d:\d\d/.test(value)) kind = "time";
        }
        try {
            return moqui.l10n.format(value, pattern || undefined, kind);
        } catch (err) {
            return String(value);
        }
    };

    // The ec of a server-static screen (the table in plans/QvtServerStatic.md). It is a window on the session and the formatter, not
    // the server's ec: any other member throws, so a screen that needs more than this has to prepare that data in its actions.
    const unavailable = (name) => new Error("ec." + name + " is not available in a server-static qvt screen; prepare it in the screen actions");
    qvt.makeEc = (config) => {
        const settings = config || {};
        const user = {
            userId: settings.userId || "", username: settings.username || "", locale: settings.locale || "", timeZone: settings.timeZone || "",
            userAccount: Object.assign({ partyId: "", emailAddress: "", userFullName: settings.userFullName || "", locale: settings.locale || "", timeZone: settings.timeZone || "" }, settings.userAccount),
            getPreference: (key) => {
                const preferences = settings.preferences || {};
                return Object.prototype.hasOwnProperty.call(preferences, key) ? preferences[key] : null;
            }
        };
        ["currentPassword", "currentPasswordSalt", "passwordSalt", "passwordHashType", "passwordBase64", "passwordHint"].forEach((name) => { delete user.userAccount[name]; });
        const l10n = {
            localize: (original) => moqui.l10n.localize(original),
            format: (value, pattern, type) => qvt.formatValue(value, pattern, type),
            formatCurrency: (amount, uomId, fractionDigits) => moqui.l10n.formatCurrency(amount, uomId, fractionDigits),
            formatCurrencyNoSymbol: (amount, uomId, fractionDigits) => moqui.l10n.formatCurrencyNoSymbol(amount, uomId, fractionDigits)
        };
        // The text is localized when it is shorter than 256 characters, as the server does. The expressions in it come as a table of
        // functions made by the compiled template, keyed by the expression text, so nothing here compiles or evaluates code.
        const expand = (text, expressions, localize) => {
            if (text === null || text === undefined || text === "") return "";
            let result = String(text);
            if (localize && result.length < 256) result = moqui.l10n.localize(result);
            if (result.indexOf("$") < 0 || result.charAt(result.length - 1) === "$") return result;
            let out = "";
            let i = 0;
            while (i < result.length) {
                if (result.charAt(i) === "$" && result.charAt(i + 1) === "{") {
                    let depth = 1;
                    let j = i + 2;
                    let quote = "";
                    while (j < result.length && depth > 0) {
                        const ch = result.charAt(j);
                        if (quote) { if (ch === "\\") j++; else if (ch === quote) quote = ""; }
                        else if (ch === "'" || ch === '"' || ch === "`") quote = ch;
                        else if (ch === "{") depth++;
                        else if (ch === "}") depth--;
                        j++;
                    }
                    if (depth === 0) {
                        const key = result.slice(i + 2, j - 1).trim();
                        const compute = expressions && expressions[key];
                        if (typeof compute !== "function") throw new Error("The expression ${" + key + "} is not part of this screen's template, so it cannot be evaluated in the browser");
                        const value = compute();
                        out += value === null || value === undefined ? "" : String(value);
                        i = j;
                        continue;
                    }
                }
                out += result.charAt(i);
                i++;
            }
            return out;
        };
        const members = {
            l10n, user, factory: { moquiVersion: settings.moquiVersion || "" },
            resource: { expand: (text, expressions) => expand(text, expressions, true), expandNoL10n: (text, expressions) => expand(text, expressions, false) }
        };
        return new Proxy(members, {
            get(target, key) {
                if (typeof key === "symbol" || key === "then" || key === "toJSON") return undefined;
                if (Object.prototype.hasOwnProperty.call(target, key)) return target[key];
                throw unavailable(String(key));
            },
            has: (target, key) => Object.prototype.hasOwnProperty.call(target, key)
        });
    };

    // The scope a screen is rendered with: its package, the host, and the instance for the names Vue itself gives a template.
    const rootScope = (vm, pkg) => {
        const fallback = new Proxy({}, { get: (target, key) => (typeof key === "string" && key.charAt(0) === "$" ? vm[key] : undefined), has: (target, key) => typeof key === "string" && key.charAt(0) === "$" });
        const host = { ec: qvt.ec(), $mq: qvt.helpers, StringUtilities: qvt.StringUtilities };
        return qvt.makeScope(pkg, fallback, host);
    };
    qvt.ec = () => {
        if (!qvt.hostEc) qvt.hostEc = qvt.makeEc(qvt.config || moqui.config || {});
        return qvt.hostEc;
    };

    // The component of one use of a static template. Its data is fetched when it is made, so a cached template never shows another request's values.
    qvt.staticComponent = (key, factory, use) => {
        const parameters = use && use.search ? moqui.searchToObj(use.search) : {};
        let render = null;
        return markRaw({
            name: "mStaticScreen",
            data() { return { pkg: null }; },
            provide() {
                const scope = rootScope(this, () => this.pkg);
                this.scopeObj = scope;
                return { mqScope: scope, mqScreen: { path: key, parameters } };
            },
            created() { this.mqCache = []; this.loadPackage(); },
            methods: {
                loadPackage() {
                    moqui.request(key + "/actions", { params: parameters }).then((result) => { this.pkg = result.json && moqui.isPlainObject(result.json) ? result.json : {}; }, moqui.handleLoadError);
                }
            },
            render() {
                const h = root.Vue.h;
                if (this.pkg === null) return h("div", { id: use && use.divId ? use.divId : undefined, class: "spinner" }, [h("div")]);
                if (render === null) render = factory(root.Vue);
                return h("div", { id: use && use.divId ? use.divId : undefined }, [render.call(this, this.scopeObj, this.mqCache)]);
            }
        });
    };

    // The handler of X-Moqui-Qvt: static-template. The body of the response is the script, which a script element loads (a page whose policy has no
    // unsafe-eval cannot evaluate fetched text), so the cache and the validator of the response serve the second request.
    moqui.qvtHandlers["static-template"] = (text, info) => new Promise((resolve, reject) => {
        const key = info.path;
        const tag = info.headers && info.headers.get ? info.headers.get("ETag") : null;
        const made = () => {
            const factory = moqui.qvtStatic[key];
            if (typeof factory !== "function") reject(new Error("The server-static template of " + key + " did not register"));
            else resolve({ mqStatic: (use) => qvt.staticComponent(key, factory, use) });
        };
        if (typeof moqui.qvtStatic[key] === "function" && tag && moqui.qvtStaticTags[key] === tag) { made(); return; }
        const script = root.document.createElement("script");
        script.src = key + "." + moqui.urlExtensions.qvt;
        script.onload = () => { if (tag) moqui.qvtStaticTags[key] = tag; script.remove(); made(); };
        script.onerror = () => { script.remove(); reject(new Error("Could not load the server-static template of " + key)); };
        root.document.head.appendChild(script);
    });

    /* ========== scope and static form-list ========== */

    // Gives its slot a scope over the map of a form-single or an entry: names are looked up in extra, then value, then the scope around it.
    component("m-scope", {
        name: "mScope",
        props: { value: null, extra: null },
        inject: { mqScope: { default: null } },
        provide() {
            const scope = qvt.makeScope(() => this.value, this.mqScope, () => this.extra);
            this.scopeObj = scope;
            return { mqScope: scope };
        },
        render() {
            const slot = this.$slots.default;
            return slot ? slot(this.scopeObj) : null;
        }
    });

    // A form-list of a static screen. It fetches <screen>/actions/<name>, which answers the list and its page keys, and writes the row slot for
    // each row with the row on the scope of the screen: the slot's _ctx. Sorting and paging change the request and fetch again.
    component("m-static-form-list", {
        name: "mStaticFormList",
        inject: { mqScope: { default: null }, mqScreen: { default: null } },
        props: { name: { type: String, required: true }, id: String, action: String, listName: String, multi: Boolean, skipForm: Boolean, skipHeader: Boolean,
            rowInitial: Function, hiddenInitial: Function, formLocation: String, findAction: String, defaultColumns: Array, columnFields: Array, columnsAction: String,
            headerInitial: Function, subFields: Array, specialRows: Object, selectionIdField: String, selectionParameter: String, selectionListMode: Boolean,
            views: [Array, String], board: Object, calendar: Object, tree: Object, peek: Object, viewsLocation: String },
        data() { return { pkg: null, searchObj: {}, loading: false, generation: 0 }; },
        computed: {
            idVal() { return this.id && this.id.length ? this.id : this.name; },
            listKey() { return this.listName || this.name; },
            // the rows drawn: the list, and the show-total row the form request answers as {list}Total after it
            rows() {
                const list = this.pkg ? this.pkg[this.listKey] : null;
                const total = this.pkg ? this.pkg[this.listKey + "Total"] : null;
                const rows = Array.isArray(list) ? list : [];
                return moqui.isPlainObject(total) ? rows.concat([Object.assign({ _moquiRowType: "total" }, total)]) : rows;
            },
            paginate() {
                const pkg = this.pkg;
                const name = this.listKey;
                if (!pkg || pkg[name + "Count"] === undefined) return null;
                const num = (suffix) => Number(pkg[name + suffix]);
                return { count: num("Count"), pageIndex: num("PageIndex"), pageSize: num("PageSize"), pageMaxIndex: num("PageMaxIndex"), pageRangeLow: num("PageRangeLow"), pageRangeHigh: num("PageRangeHigh") };
            }
        },
        methods: {
            fetchRows() {
                const screen = this.mqScreen || { path: this.$root.currentPath, parameters: {} };
                this.loading = true;
                const params = Object.assign({}, screen.parameters, this.searchObj);
                moqui.request(screen.path + "/actions/" + encodeURIComponent(this.name), { params }).then((result) => {
                    this.loading = false;
                    this.pkg = result.json && moqui.isPlainObject(result.json) ? result.json : {};
                    // the rows of another page or order are new: an m-form copies its fields-initial once, so a row is not patched in place
                    this.generation++;
                }, (err) => { this.loading = false; moqui.handleLoadError(err); });
            },
            screen() { return this.mqScreen || { path: this.$root.currentPath, parameters: {} }; },
            // The parameters of the current find: the screen's and the list's own, without the ones that only move through the pages.
            findParameters() {
                const out = Object.assign({}, this.screen().parameters, this.searchObj);
                ["pageIndex", "moquiFormName", "moquiSessionToken", "lastStandalone", "formListFindId"].forEach((key) => { delete out[key]; });
                Object.keys(out).forEach((key) => { if (out[key] === null || out[key] === undefined || out[key] === "") delete out[key]; });
                return out;
            },
            // What the nav slot of the template binds: the saved finds, the find and export URLs, and the current parameters.
            navProps() {
                const pkg = this.pkg || {};
                const screen = this.screen();
                const params = this.findParameters();
                const linkPath = (this.$root && this.$root.currentLinkPath) || screen.path;
                const query = (extra) => moqui.objToSearch(Object.assign({}, screen.parameters, this.searchObj, extra));
                const finds = Array.isArray(pkg.formListFinds) ? pkg.formListFinds : [];
                const activeFindId = pkg.formListFindId || "";
                return {
                    finds, activeFindId, defaultFindId: pkg.defaultFormListFindId || "",
                    // the active find of the list, its schedule if it has one, and the fields of the form that schedules it
                    activeFind: finds.find((find) => find.formListFindId === activeFindId) || null, activeScheduled: pkg.formListFindScheduled || null,
                    scheduleFields: () => ({ formListFindId: activeFindId, screenPath: screen.path, renderMode: "", cronSelected: "" }),
                    params, hasParams: Object.keys(params).length > 0, orderByField: this.searchObj.orderByField || pkg.orderByField || "",
                    findAction: linkPath, saveFindAction: this.findAction || "", screenPath: screen.path,
                    findUrl: (id) => linkPath + "?formListFindId=" + encodeURIComponent(id),
                    findFields: (id) => Object.assign({ formLocation: this.formLocation, formListFindId: id }, params),
                    newFindFields: () => Object.assign({ formLocation: this.formLocation, _findDescription: "" }, params),
                    exportUrl: (mode) => screen.path + "?" + query({ renderMode: mode, pageNoLimit: "true", lastStandalone: "true", saveFilename: this.name + "." + mode }),
                    // the page as a PDF, under /fop as Java's MoquiFopServlet
                    printUrl: () => "/fop" + screen.path + "?" + query({ pageNoLimit: "true" }),
                    columnsAction: this.columnsAction || "", columnsTree: this.columnsTree()
                };
            },
            // The columns shown: the user's from the package, else the template's, without the sub-list fields, which are the sub columns.
            shownColumns() {
                const chosen = this.pkg && this.pkg.mqColumns;
                const all = Array.isArray(chosen) && chosen.length ? chosen : this.defaultColumns || [];
                const sub = this.subFields || [];
                return all.map((column) => column.filter((field) => !sub.includes(field))).filter((column) => column.length);
            },
            // The sub-list fields in the user's columns (or each in a column of its own), for the sub-rows of an aggregated list.
            subColumns() {
                const sub = this.subFields || [];
                if (!sub.length) return [];
                const chosen = this.pkg && this.pkg.mqColumns;
                const all = Array.isArray(chosen) && chosen.length ? chosen : (this.defaultColumns || []).concat(sub.map((field) => [field]));
                return all.map((column) => column.filter((field) => sub.includes(field))).filter((column) => column.length);
            },
            // The scope of a sub-row: its entry over the row's scope, with the sub index, as Java's startFormListSubRow.
            subScope(entry, subIndex, count, rowScope) {
                const extra = { _formMap: entry };
                extra[this.listKey + "_sub_index"] = subIndex;
                extra[this.listKey + "_sub_has_next"] = subIndex + 1 < count;
                return qvt.makeScope(() => Object.assign({}, entry, extra), rowScope);
            },
            // What the column dialog edits: the fields not shown, then each column with its fields, labelled as the server-merged dialog has them.
            columnsTree() {
                const fields = this.columnFields || [];
                const labelOf = (id) => { const found = fields.find((field) => field.id === id); return found ? found.label : id; };
                const columns = this.shownColumns();
                const shown = new Set(columns.flat());
                const tree = [{ id: "hidden", label: moqui.l10n.localize("Do Not Display"), children: fields.filter((field) => !shown.has(field.id)).map((field) => ({ id: field.id, label: field.label })) }];
                columns.forEach((column, index) => tree.push({ id: "column_" + index, label: moqui.l10n.localize("Column") + " " + (index + 1), children: column.map((id) => ({ id, label: labelOf(id) })) }));
                return tree;
            },
            // The fields of a form that posts every row: each row's starting values under name_rowIndex, as the server-merged form has them.
            multiFields(formScope) {
                // Hidden parameters are posted once, unsuffixed. Row values are name_rowIndex, so they are not this map.
                const hidden = this.hiddenInitial ? (this.hiddenInitial(formScope) || {}) : {};
                const out = Object.assign({}, hidden, { moquiFormName: this.name, _isMulti: "true", pageIndex: String((this.pkg && this.pkg.pageIndex) || 0) });
                const order = this.searchObj.orderByField || (this.pkg && this.pkg.orderByField);
                if (order) out.orderByField = String(order);
                if (!this.rowInitial) return out;
                this.rows.forEach((row, rowIndex) => {
                    const initial = this.rowInitial(this.rowScope(rowIndex, formScope, null)) || {};
                    Object.keys(initial).forEach((key) => { if (initial[key] !== undefined) out[key + "_" + rowIndex] = initial[key]; });
                });
                return out;
            },
            // The scope of a row. In a multi form, formProps reads and writes the row's fields under name_rowIndex in the one form.
            rowScope(rowIndex, formScope, multiProps, formProps) {
                const extra = { rowIndex, _formMap: this.rows[rowIndex] };
                if (this.mqSelState) extra.mqSel = this.mqSelState;
                if (formProps) extra.formProps = formProps;
                if (multiProps) {
                    const suffix = "_" + rowIndex;
                    const fields = new Proxy({}, {
                        get: (target, key) => (typeof key === "string" ? multiProps.fields[key + suffix] : undefined),
                        set: (target, key, value) => { multiProps.fields[key + suffix] = value; return true; },
                        has: (target, key) => typeof key === "string" && (key + suffix) in multiProps.fields,
                        ownKeys: () => Object.keys(multiProps.fields).filter((key) => key.endsWith(suffix)).map((key) => key.slice(0, -suffix.length)),
                        getOwnPropertyDescriptor: (target, key) => (typeof key === "string" && (key + suffix) in multiProps.fields ? { enumerable: true, configurable: true, value: multiProps.fields[key + suffix] } : undefined)
                    });
                    extra.formProps = Object.assign({}, multiProps, { fields });
                }
                return qvt.makeScope(() => Object.assign({}, this.rows[rowIndex], extra), formScope);
            },
            setPageIndex(index) { this.searchObj = Object.assign({}, this.searchObj, { pageIndex: index }); this.fetchRows(); },
            sortBy(field) { this.searchObj = Object.assign({}, this.searchObj, { orderByField: field, pageIndex: 0 }); this.fetchRows(); },
            orderBy(field) {
                const current = this.searchObj.orderByField || (this.pkg && this.pkg.orderByField) || "";
                if (current !== field) return "";
                return field.charAt(0) === "-" ? "desc" : "asc";
            }
        },
        created() { this.searchObj = {}; this.fetchRows(); },
        render() {
            const V = root.Vue;
            const h = V.h;
            const formScope = qvt.makeScope(() => this.pkg || {}, this.mqScope);
            const slots = this.$slots;
            // With row-selection the table is in an m-checkbox-set (sel): the head starts with select-all and the menu of the row actions,
            // and each row with a checkbox, as Java's form-list; sel is not reactive state of this component.
            const build = (sel) => {
            this.mqSelState = sel;
            // the selection column is m-sel: as narrow as its checkbox, so it does not take the slack of the automatic table layout
            const selCell = (rowIndex) => {
                if (!sel) return null;
                const row = rowIndex === null ? null : this.rows[rowIndex];
                const id = row ? row[this.selectionIdField] : null;
                if (id === null || id === undefined || id === "") return h("div", { class: "td m-sel" }, "\u00a0");
                return h("div", { class: "td m-sel" }, [h(V.resolveComponent("q-checkbox"), { dense: true, size: "sm", modelValue: !!sel.checkboxStates[rowIndex],
                    "onUpdate:modelValue": (value, event) => { sel.clickCheckbox(event || {}, rowIndex); sel.checkboxStates[rowIndex] = value; } })]);
            };
            const selHead = sel ? h("div", { class: "th m-sel" }, [h("span", { class: "q-my-auto" }, [
                h(V.resolveComponent("q-checkbox"), { dense: true, size: "sm", modelValue: sel.checkboxAllState, "onUpdate:modelValue": sel.setCheckboxAllState }),
                h(V.resolveComponent("q-btn"), { dense: true, flat: true, round: true, size: "sm", icon: "more_vert", color: sel.checkboxStates && sel.checkboxStates.includes(true) ? "primary" : "grey-7" }, { default: () => [
                    h(V.resolveComponent("q-tooltip"), null, { default: () => moqui.l10n.localize("Row Actions") }),
                    h(V.resolveComponent("q-menu"), { anchor: "top left", self: "bottom left" }, { default: () => (slots.selection ? slots.selection(sel) : null) })] })])]) : null;
            const hs = { sortBy: this.sortBy, orderBy: this.orderBy, action: this.$root && this.$root.currentLinkPath, orderByField: this.searchObj.orderByField || (this.pkg && this.pkg.orderByField) || "" };
            let header = slots.header ? slots.header(hs) : null;
            if (this.defaultColumns) {
                // the head of each shown column: the hc_ slot of each of its fields, in the header find form when the template gives its starting values
                const ths = (formProps) => this.shownColumns().map((column) => h("div", { class: "th text-left" }, column.map((field) => (slots["hc_" + field] ? slots["hc_" + field]({ hs, formProps }) : null))));
                header = this.headerInitial
                    ? [h(V.resolveComponent("m-form-link"), { name: this.idVal + "_header", id: this.idVal + "_header", action: hs.action, fieldsInitial: this.headerInitial(formScope, hs) },
                        { default: (formProps) => ths(formProps) })]
                    : ths(null);
            }
            if (selHead) header = [selHead].concat(header || []);
            const columnsMode = !!this.defaultColumns;
            const columns = columnsMode ? this.shownColumns() : null;
            // In columns mode a row is a tr of a td for each column, each holding the rc_ slot of its fields, in a row form the component makes.
            const cellsOf = (scope) => columns.map((column) => h("div", { class: "td" }, column.map((field) => (slots["rc_" + field] ? slots["rc_" + field](scope) : null))));
            const columnsRow = (rowIndex, multiProps) => {
                const rowClass = this.rows[rowIndex] && this.rows[rowIndex]._moquiRowType === "total" ? "tr text-weight-bold" : "tr";
                if (multiProps || this.skipForm) return h("div", { class: rowClass, key: this.generation + "_" + rowIndex }, [selCell(rowIndex)].concat(cellsOf(this.rowScope(rowIndex, formScope, multiProps, null))));
                const initial = Object.assign({}, this.rowInitial ? this.rowInitial(this.rowScope(rowIndex, formScope, null, null)) : {},
                    { moquiFormName: this.name, pageIndex: String((this.pkg && this.pkg.pageIndex) || 0) });
                return h("div", { class: rowClass, key: this.generation + "_" + rowIndex }, [selCell(rowIndex), h(V.resolveComponent("m-form"), { name: this.idVal + "_" + rowIndex, id: this.idVal + "_" + rowIndex, action: this.action, fieldsInitial: initial },
                    { default: (formProps) => cellsOf(this.rowScope(rowIndex, formScope, null, formProps)) })]);
            };
            // The sub-rows of an aggregated row, under it: each entry of its aggregateSubList as a row of the sub columns.
            const subColumns = columnsMode ? this.subColumns() : null;
            const subRowsOf = (rowIndex) => {
                const entries = this.rows[rowIndex] && this.rows[rowIndex].aggregateSubList;
                if (!Array.isArray(entries) || !entries.length || !(columnsMode ? subColumns.length : slots.subrow)) return null;
                const parent = this.rowScope(rowIndex, formScope, null, null);
                const cells = (scope) => (columnsMode
                    ? subColumns.map((column) => h("div", { class: "col" }, column.map((field) => (slots["rc_" + field] ? slots["rc_" + field](scope) : null))))
                    : slots.subrow(scope));
                return h("div", { class: "tr sub-rows" }, [sel ? h("div", { class: "td m-sel" }, "\u00a0") : null, h("div", { class: "td" }, [h("div", { class: "q-pl-lg" },
                    entries.map((entry, subIndex) => h("div", { class: "row no-wrap q-col-gutter-sm" }, cells(this.subScope(entry, subIndex, entries.length, parent)))))])]);
            };
            const rowsOf = (multiProps) => this.rows.map((row, rowIndex) => h(V.Fragment, { key: this.generation + "_" + rowIndex },
                [columnsMode ? columnsRow(rowIndex, multiProps) : h(V.Fragment, slots.row ? slots.row(this.rowScope(rowIndex, formScope, multiProps)) : []), subRowsOf(rowIndex)]));
            const multiForm = this.multi && !this.skipForm;
            const rowNodes = multiForm ? [] : rowsOf(null);
            // The first, second and last rows: the map the form request answers for each (listFirst, ...), in a form of their own when the
            // template gives the transition, as Java's name_first form; outside a multi form, whose rows are the list's.
            const specialRow = (kind) => {
                const spec = this.specialRows && this.specialRows[kind];
                if (!spec || !this.pkg) return null;
                const map = this.pkg[this.listKey + kind.charAt(0).toUpperCase() + kind.slice(1)] || {};
                const scopeOf = (formProps) => qvt.makeScope(() => Object.assign({}, map, { _formMap: map }, formProps ? { formProps } : {}), formScope);
                const content = (scope) => (columnsMode
                    ? h("div", { class: "tr " + kind }, [sel ? h("div", { class: "td m-sel" }, "\u00a0") : null].concat(columns.map((column) => h("div", { class: "td" },
                        column.map((field) => (slots["rc" + kind.charAt(0) + "_" + field] ? slots["rc" + kind.charAt(0) + "_" + field](scope) : null))))))
                    : (slots["row_" + kind] ? slots["row_" + kind](scope) : null));
                if (!spec.action) return h(V.Fragment, { key: kind + "_" + this.generation }, [content(scopeOf(null))]);
                const initial = Object.assign({}, spec.initial ? spec.initial(scopeOf(null)) : {}, { moquiFormName: this.name, pageIndex: String(this.pkg.pageIndex || 0) });
                const order = this.searchObj.orderByField || this.pkg.orderByField;
                if (order) initial.orderByField = String(order);
                return h(V.resolveComponent("m-form"), { key: kind + "_" + this.generation, name: this.idVal + "_" + kind, id: this.idVal + "_" + kind, action: spec.action, fieldsInitial: initial },
                    { default: (formProps) => content(scopeOf(formProps)) });
            };
            const empty = [h("div", { class: "tr" }, [h("div", { class: "td" }, moqui.l10n.localize("No results found"))])];
            const body = multiForm
                ? h(V.resolveComponent("m-form"), { key: "multi_" + this.generation, name: this.idVal, id: this.idVal, action: this.action, fieldsInitial: this.multiFields(formScope),
                    parentCheckboxSet: sel || undefined, checkboxOptional: true },
                    { default: (multiProps) => (this.rows.length || this.loading ? rowsOf(multiProps) : empty) })
                : rowNodes.length || this.loading ? rowNodes : empty;
            // as Java: the titles of the sub-list columns once in the head
            const labelOf = (field) => { const found = (this.columnFields || []).find((item) => item.id === field); return found ? found.label : field; };
            const subHead = columnsMode
                ? (subColumns.length ? subColumns.map((column) => h("div", { class: "col" }, column.map(labelOf).join(" "))) : null)
                : (slots.subhead ? slots.subhead() : null);
            const subHeader = subHead ? h("div", { class: "tr sub-header" }, [h("div", { class: "th" }, [h("div", { class: "q-pl-lg" },
                [h("div", { class: "row no-wrap q-col-gutter-sm text-weight-bold" }, subHead)])])]) : null;
            const table = h("div", { class: "table q-table", id: this.idVal + "_table" }, [
                this.skipHeader ? null : h("div", { class: "thead" }, [h("div", { class: "tr" }, header), subHeader]),
                h("div", { class: "tbody" }, [specialRow("first"), specialRow("second")].concat(multiForm ? [body] : body, [specialRow("last")]))
            ]);
            return table;
            };
            // hideNav is on the screen package. It hides the tools and the page buttons, as the merged nav row does.
            const hideNav = !!(this.mqScope && this.mqScope.hideNav === "true");
            const nav = hideNav ? null : h("div", { class: "form-list-nav-row row items-center" }, [
                slots.nav ? h("div", { class: "row items-center q-gutter-xs" }, slots.nav(this.navProps())) : null,
                h(V.resolveComponent("q-space")),
                h(V.resolveComponent("m-form-paginate"), { paginate: this.paginate, formList: this }),
                h(V.resolveComponent("m-form-go-page"), { idVal: this.idVal, formList: this })
            ]);
            const table = this.selectionIdField
                ? h(V.resolveComponent("m-checkbox-set"), {
                    key: "sel_" + this.generation, checkboxCount: this.rows.length, checkboxParameter: this.selectionParameter || this.selectionIdField,
                    checkboxListMode: !!this.selectionListMode, checkboxValues: this.rows.map((row) => (row[this.selectionIdField] === undefined ? null : row[this.selectionIdField]))
                }, { default: (sel) => [build(sel)] })
                : build(null);
            // the page buttons again under a list of more than one page, after the table so they are not a row of its first column
            const bottomNav = !hideNav && this.paginate && this.paginate.pageMaxIndex > 0 && !this.skipHeader
                ? h("div", { class: "form-list-nav-row row items-center" }, [
                    h(V.resolveComponent("q-space")),
                    h(V.resolveComponent("m-form-go-page"), { idVal: this.idVal, formList: this }),
                    h(V.resolveComponent("m-form-paginate"), { paginate: this.paginate, formList: this })])
                : null;
            const list = h("div", { class: "q-my-sm q-table__container q-table__card q-table--horizontal-separator q-table--dense q-table--flat" }, [nav, table, bottomNav]);
            if (!this.board && !this.calendar && !this.tree && !this.peek) return list;
            // the board, calendar and tree views over the rows fetched, a card the card slot over the row's scope, a tree row the tree slot
            // over the scope of its row, which may be one the tree fetched (m-list-views)
            const viewSlots = { default: () => [list] };
            if (slots.card) viewSlots.card = (props) => slots.card(this.rowScope(props.index, formScope, null));
            if (slots.tree) {
                viewSlots.tree = (props) => slots.tree(qvt.makeScope(() => Object.assign({}, props.row, { rowIndex: "t" + (props.t ? props.t.key : ""), _formMap: props.row, mqTree: props }), formScope));
            }
            return h(V.resolveComponent("m-list-views"), { id: this.idVal + "_views", formName: this.name, formLocation: this.viewsLocation || this.formLocation,
                views: this.views, rows: this.rows, board: this.board, calendar: this.calendar, tree: this.tree, peek: this.peek, reload: this.fetchRows }, viewSlots);
        }
    });

    /* ========== inline components ========== */

    // A screen link: setUrl in the shell. An external href (another scheme or host, as a ${} url of a server-static screen can give)
    // is a plain anchor the browser follows.
    component("m-link", {
        name: "mLink",
        props: { href: { type: String, required: true }, loadId: String, confirmation: String },
        template: '<a :href="linkHref" @click="go" class="q-link"><slot></slot></a>',
        methods: {
            go(event) {
                if (this.external) {
                    if (this.confirmation && this.confirmation.length && !root.confirm(this.confirmation)) event.preventDefault();
                    return;
                }
                event.preventDefault();
                if (event.button !== 0) return;
                if (this.confirmation && this.confirmation.length && !root.confirm(this.confirmation)) return;
                if (this.loadId && this.loadId.length > 0) this.$root.loadContainer(this.loadId, this.href);
                else if (event.ctrlKey || event.metaKey) root.open(this.linkHref, "_blank");
                else this.$root.setUrl(this.linkHref);
            }
        },
        computed: {
            external() { return moqui.isExternalLink(this.href); },
            linkHref() { return this.external ? this.href : this.$root.getLinkPath(this.href); }
        }
    });

    component("m-script", {
        name: "mScript",
        props: { src: String, type: { type: String, default: "text/javascript" } },
        template: '<div :type="type" style="display:none;"><slot></slot></div>',
        created() { if (this.src && this.src.length > 0) moqui.loadScript(this.src); },
        mounted() {
            const innerText = this.$el.textContent;
            if (innerText && innerText.trim().length > 0) moqui.retryInlineScript(innerText, 1);
        }
    });

    component("m-stylesheet", {
        name: "mStylesheet",
        props: { href: { type: String, required: true }, rel: { type: String, default: "stylesheet" }, type: { type: String, default: "text/css" } },
        template: '<div :type="type" style="display:none;"></div>',
        created() { moqui.loadStylesheet(this.href, this.rel, this.type); }
    });

    /* ========== layout components ========== */

    component("m-container-box", {
        name: "mContainerBox",
        props: { type: { type: String, default: "default" }, title: String, initialOpen: { type: Boolean, default: true } },
        data() { return { isBodyOpen: this.initialOpen }; },
        computed: { boxClass() { const color = moqui.getQuasarColor(this.type); return color && color !== "default" ? "m-box-" + color : ""; } },
        template:
            '<q-card flat bordered class="q-ma-sm m-container-box" :class="boxClass">' +
                '<div class="m-box-head row items-center no-wrap q-py-xs q-px-sm" @click.self="toggleBody">' +
                    '<div v-if="title && title.length" class="text-subtitle1 text-weight-medium ellipsis cursor-pointer" @click="toggleBody">{{title}}</div>' +
                    '<slot name="header"></slot>' +
                    '<q-space></q-space>' +
                    '<slot name="toolbar"></slot>' +
                    '<q-btn flat round dense size="sm" color="grey-7" :icon="isBodyOpen ? \'expand_less\' : \'expand_more\'" @click="toggleBody"></q-btn>' +
                '</div>' +
                '<q-slide-transition><div v-show="isBodyOpen"><q-separator></q-separator><q-card-section><slot></slot></q-card-section></div></q-slide-transition>' +
            '</q-card>',
        methods: { toggleBody() { this.isBodyOpen = !this.isBodyOpen; } }
    });

    component("m-box-body", {
        name: "mBoxBody",
        props: { height: String },
        computed: { bodyStyle() { return this.height ? { "max-height": this.height + "px", "overflow-y": "auto" } : {}; } },
        template: '<div class="q-pa-xs" :style="bodyStyle"><slot></slot></div>'
    });

    component("m-dialog", {
        name: "mDialog",
        props: { draggable: { type: Boolean, default: true }, modelValue: { type: Boolean, default: false }, id: String, color: String, width: String, title: String },
        emits: ["update:modelValue", "onShow", "onHide"],
        template:
            '<q-dialog :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :id="id" @show="onShow" @hide="onHide" :maximized="$q.screen.lt.sm">' +
                '<q-card ref="dialogCard" flat bordered :style="{width:((width||760)+\'px\'),\'max-width\':($q.screen.lt.sm?\'100vw\':\'90vw\')}">' +
                    '<q-card-section ref="dialogHeader" class="row items-center no-wrap q-py-xs q-pr-xs non-selectable" :style="{cursor:(draggable?\'move\':\'default\')}">' +
                        '<div class="text-subtitle1 text-weight-medium ellipsis">{{title}}</div><q-space></q-space>' +
                        '<q-btn icon="close" flat round dense v-close-popup></q-btn>' +
                    '</q-card-section><q-separator></q-separator>' +
                    '<q-card-section ref="dialogBody" class="scroll" style="max-height:80vh;"><slot></slot></q-card-section>' +
                '</q-card>' +
            '</q-dialog>',
        methods: {
            onShow() {
                if (this.draggable && this.$refs.dialogHeader) this.$refs.dialogHeader.$el.addEventListener("mousedown", this.onGrab);
                this.focusFirst();
                this.$emit("onShow");
            },
            onHide() {
                if (this.draggable) {
                    root.document.removeEventListener("mousemove", this.onDrag);
                    root.document.removeEventListener("mouseup", this.onLetGo);
                    if (this.$refs.dialogHeader) this.$refs.dialogHeader.$el.removeEventListener("mousedown", this.onGrab);
                }
                this.$emit("onHide");
            },
            onDrag(e) {
                const target = this.$refs.dialogCard.$el;
                const styles = root.getComputedStyle(target);
                let newLeft = (parseInt(styles.left, 10) || 0) + e.movementX;
                let newTop = (parseInt(styles.top, 10) || 0) + e.movementY;
                const halfW = root.innerWidth / 2;
                const halfH = root.innerHeight / 2;
                const elW = target.offsetWidth / 2;
                const elH = target.offsetHeight / 2;
                newLeft = Math.max(-(halfW - elW - 10), Math.min(halfW - elW - 10, newLeft));
                newTop = Math.max(-(halfH - elH - 10), Math.min(halfH - elH - 10, newTop));
                target.style.left = newLeft + "px";
                target.style.top = newTop + "px";
            },
            onLetGo() {
                root.document.removeEventListener("mousemove", this.onDrag);
                root.document.removeEventListener("mouseup", this.onLetGo);
            },
            onGrab() {
                root.document.addEventListener("mousemove", this.onDrag);
                root.document.addEventListener("mouseup", this.onLetGo);
            },
            // the .default-focus field, else the first visible field of a form in the dialog
            focusFirst() {
                const body = this.$refs.dialogBody && this.$refs.dialogBody.$el;
                if (!body) return;
                const target = body.querySelector(".default-focus") ||
                    body.querySelector("form input:not([type=hidden]):not([type=submit]):not([type=button]), form textarea, form select");
                if (target && target.focus) target.focus();
            }
        }
    });

    // A container-dialog is a container of the root by its id (addContainer), so a form's background-hide-id (submit-hide-id) naming
    // it closes it; its content is the page's, so reload and load do nothing. tooltip is the button's (and the slot button's).
    component("m-container-dialog", {
        name: "mContainerDialog",
        props: { id: String, color: String, buttonText: String, buttonClass: String, title: String, width: String, openDialog: { type: Boolean, default: false },
            buttonIcon: { type: String, default: "open_in_new" }, tooltip: String },
        data() { return { isShown: false }; },
        template:
            '<span>' +
                '<span @click="show()"><slot name="button"><q-btn dense unelevated no-caps :icon="buttonIcon" :label="buttonText" :class="moqui.tonalClass(color, buttonClass)"></q-btn></slot>' +
                    '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip></span>' +
                '<m-dialog v-model="isShown" :id="id" :title="title" :color="color" :width="width"><slot></slot></m-dialog>' +
            '</span>',
        methods: { show() { this.isShown = true; }, hide() { this.isShown = false; }, reload() {}, load() {} },
        mounted() {
            if (this.id) this.$root.addContainer(this.id, this);
            if (this.openDialog) this.isShown = true;
        },
        beforeUnmount() { if (this.id) this.$root.removeContainer(this.id, this); }
    });

    component("m-dynamic-container", {
        name: "mDynamicContainer",
        props: { id: { type: String, required: true }, url: String },
        data() { return { curComponent: moqui.EmptyComponent, curUrl: "" }; },
        provide() { return { mScreen: moqui.urlScreen(() => this.curUrl, this.id) }; },
        template: '<component :is="curComponent" ref="screen"></component>',
        methods: {
            // The shell's refresh of a container it keeps (a tray tab, on a shellCounts push and the poll while the socket is down):
            // the screen's own shellRefresh() when its component has one, so it updates in place (an open chat room stays open),
            // else a reload.
            shellRefresh() {
                const screen = this.$refs.screen;
                if (screen && typeof screen.shellRefresh === "function") {
                    try { screen.shellRefresh(); } catch (err) { console.error("shellRefresh of " + this.id + " failed", err); }
                    return;
                }
                this.reload();
            },
            // the screen again: the content shown stays until the new one is there (a tray tab keeps its place); the same component
            // (a cached one) is made again by a moment without it
            reload() {
                const url = this.curUrl;
                if (!url) return;
                moqui.loadComponent(url, (comp) => {
                    if (this.curUrl !== url) return;
                    if (comp !== this.curComponent) { this.curComponent = comp; return; }
                    this.curComponent = moqui.EmptyComponent;
                    this.$nextTick(() => { this.curComponent = comp; });
                }, this.id);
            },
            load(url) { if (this.curUrl === url) this.reload(); else this.curUrl = url; }
        },
        watch: {
            curUrl(newUrl) {
                if (!newUrl || newUrl.length === 0) { this.curComponent = moqui.EmptyComponent; return; }
                moqui.loadComponent(newUrl, (comp) => { this.curComponent = comp; }, this.id);
            }
        },
        mounted() { this.$root.addContainer(this.id, this); this.curUrl = this.url; },
        beforeUnmount() { this.$root.removeContainer(this.id, this); }
    });

    component("m-dynamic-dialog", {
        name: "mDynamicDialog",
        props: { id: String, url: { type: String, required: true }, color: String, buttonText: String, buttonClass: String, title: String, width: String,
            openDialog: { type: Boolean, default: false }, dynamicParams: { type: Object, default: null }, buttonIcon: { type: String, default: "open_in_new" } },
        data() { return { curComponent: moqui.EmptyComponent, curUrl: "", loadedUrl: "", isShown: false }; },
        provide() { return { mScreen: moqui.urlScreen(() => this.loadedUrl, this.id) }; },
        template:
            '<span>' +
                '<q-btn dense unelevated no-caps :icon="buttonIcon" :label="buttonText" :class="moqui.tonalClass(color, buttonClass)" @click="isShown = true"></q-btn>' +
                '<m-dialog ref="dialog" v-model="isShown" :id="id" :title="title" :color="color" :width="width"><component :is="curComponent"></component></m-dialog>' +
            '</span>',
        methods: {
            reload() { if (this.isShown) { this.isShown = false; this.$nextTick(() => { this.isShown = true; }); } },
            load(url) { this.curUrl = url; },
            hide() { this.isShown = false; }
        },
        watch: {
            curUrl(newUrl) {
                if (!newUrl || newUrl.length === 0) { this.curComponent = moqui.EmptyComponent; this.loadedUrl = ""; return; }
                // values of other fields on the page, by element id, are added to the URL
                if (moqui.isPlainObject(this.dynamicParams)) {
                    const parts = [];
                    Object.keys(this.dynamicParams).forEach((key) => {
                        const el = root.document.getElementById(this.dynamicParams[key]);
                        const value = el && (el.value || (el.querySelector("select") || {}).value);
                        if (value && value.length) parts.push(encodeURIComponent(key) + "=" + encodeURIComponent(value));
                    });
                    if (parts.length) newUrl = newUrl + (newUrl.indexOf("?") > 0 ? "&" : "?") + parts.join("&");
                }
                this.loadedUrl = newUrl;
                moqui.loadComponent(newUrl, (comp) => {
                    this.curComponent = comp;
                    this.$nextTick(() => { if (this.$refs.dialog) this.$refs.dialog.focusFirst(); });
                }, this.id);
            },
            isShown(newShown) { this.curUrl = newShown ? this.url : ""; }
        },
        mounted() {
            this.$root.addContainer(this.id, this);
            if (this.openDialog) this.isShown = true;
        },
        beforeUnmount() { this.$root.removeContainer(this.id, this); }
    });

    component("m-tree-top", {
        name: "mTreeTop",
        template: '<ul :id="id" class="tree-list"><m-tree-item v-for="model in itemList" :key="model.id" :model="model" :top="top"></m-tree-item></ul>',
        props: { id: { type: String, required: true }, items: { type: [String, Array], required: true }, openPath: String, parameters: Object },
        data() { return { urlItems: null, currentPath: null }; },
        computed: {
            top() { return this; },
            itemList() { if (this.urlItems) return this.urlItems; return Array.isArray(this.items) ? this.items : []; }
        },
        mounted() {
            if (!moqui.isString(this.items)) return;
            this.currentPath = this.openPath;
            const params = Object.assign({ treeNodeId: "#", treeOpenPath: this.openPath }, this.parameters);
            moqui.request(this.items, { method: "POST", params }).then((result) => { this.urlItems = result.json; }, moqui.handleAjaxError);
        }
    });

    // A node of the tree. The server sends a node's children as a list when the node is on the open path, else as a boolean: true means
    // they load from the tree's URL when the node is opened, false that it is a leaf. The list drawn is the loaded one over the model's.
    component("m-tree-item", {
        name: "mTreeItem",
        template:
            '<li :id="model.id">' +
                '<i v-if="isFolder" @click="toggle" class="fa" :class="{\'fa-chevron-right\':!open, \'fa-chevron-down\':open}"></i>' +
                '<i v-else class="fa fa-square-o"></i>' +
                ' <span @click="setSelected">' +
                    '<m-link v-if="linkHref" :href="linkHref" :load-id="model.a_attr.loadId" :class="{\'text-success\':selected}">{{model.text}}</m-link>' +
                    '<span v-else :class="{\'text-success\':selected}">{{model.text}}</span>' +
                '</span>' +
                '<ul v-show="open" v-if="hasChildren"><m-tree-item v-for="child in children" :key="child.id" :model="child" :top="top"></m-tree-item></ul>' +
            '</li>',
        props: { model: Object, top: Object },
        data() { return { open: false, loadedChildren: null }; },
        computed: {
            linkHref() { const attrs = this.model.a_attr; return attrs ? attrs.urlText || attrs.href || "" : ""; },
            children() { return this.loadedChildren || this.model.children; },
            isFolder() { const children = this.children; if (!children) return false; return Array.isArray(children) ? children.length > 0 : true; },
            hasChildren() { return Array.isArray(this.children) && this.children.length > 0; },
            selected() { return this.top.currentPath === this.model.id; }
        },
        watch: {
            open(newVal) {
                if (!newVal) return;
                const url = this.top.items;
                if (this.children && moqui.isBoolean(this.children) && moqui.isString(url)) {
                    const liAttr = this.model.li_attr;
                    const params = Object.assign({ treeNodeId: this.model.id, treeNodeName: liAttr && liAttr.treeNodeName ? liAttr.treeNodeName : "", treeOpenPath: this.top.currentPath }, this.top.parameters);
                    // an answer that is not a list leaves the node a folder that fetches again on the next open
                    moqui.request(url, { method: "POST", params }).then((result) => { if (Array.isArray(result.json)) this.loadedChildren = result.json; }, moqui.handleAjaxError);
                }
            }
        },
        methods: {
            toggle() { if (this.isFolder) this.open = !this.open; },
            setSelected() { this.top.currentPath = this.model.id; this.open = true; }
        },
        mounted() { if (this.model.state && this.model.state.opened) this.open = true; }
    });

    /* ========== general field components ========== */

    // The options of an editable select from what its editable-load transition answered: a list of {value, label} (or key and
    // text, or plain values), or as Java's jEditable a map of key to label with the current key under "selected". Gives
    // {options, selected}; selected is undefined when the answer names none. Pure, for the node tests.
    moqui.editableOptions = (answer) => {
        const one = (item) => {
            if (moqui.isPlainObject(item)) {
                const value = item.value !== undefined ? item.value : item.key !== undefined ? item.key : item.id;
                const label = item.label !== undefined ? item.label : item.text !== undefined ? item.text : item.description;
                return { value: value === undefined || value === null ? "" : String(value), label: String(label === undefined || label === null ? value : label) };
            }
            return { value: String(item), label: String(item) };
        };
        if (Array.isArray(answer)) return { options: answer.map(one), selected: undefined };
        if (moqui.isPlainObject(answer)) {
            if (Array.isArray(answer.options)) return { options: answer.options.map(one), selected: answer.selected === undefined ? undefined : String(answer.selected) };
            const options = Object.keys(answer).filter((key) => key !== "selected").map((key) => ({ value: key, label: String(answer[key]) }));
            return { options, selected: answer.selected === undefined ? undefined : String(answer.selected) };
        }
        return { options: [], selected: undefined };
    };

    // Click to edit with the popup editor of Quasar. The text shows as the element named by label-type, and an empty one as the
    // placeholder (display-if-empty), so there is something to click. The widget type is textarea or text-line; select (the
    // options given, or loaded from load-url when the editor opens; value is the current key, else the option whose label is
    // the text); date or date-time (QDate, with QTime, in yyyy-MM-dd and yyyy-MM-dd HH:mm). With load-url and no select the
    // current value is fetched when the editor opens.
    component("m-editable", {
        name: "mEditable",
        props: { id: { type: String, required: true }, labelType: { type: String, default: "span" }, labelValue: { type: String, default: "" },
            url: { type: String, required: true }, urlParameters: { type: Object, default: () => ({}) },
            parameterName: { type: String, default: "value" }, widgetType: { type: String, default: "textarea" },
            loadUrl: String, loadParameters: Object, indicator: { type: String, default: "Saving" }, tooltip: { type: String, default: "Click to edit" },
            cancel: { type: String, default: "Cancel" }, submit: { type: String, default: "Save" }, placeholder: String,
            options: Array, value: String },
        data() {
            const text = moqui.htmlDecode(String(this.labelValue || "").replace(/<br\s*\/?>/gi, "\n"));
            let current = this.value;
            if (current === undefined || current === null) {
                const match = (this.options || []).find((option) => String(option.label) === text);
                current = match ? String(match.value) : text;
            }
            return { shown: this.labelValue || "", current, editValue: "", saving: false, loadedOptions: null, loadingOptions: false };
        },
        computed: {
            isEmpty() { return !String(this.shown || "").replace(/<[^>]*>/g, "").trim(); },
            isDate() { return this.widgetType === "date" || this.widgetType === "date-time"; },
            dateMask() { return this.widgetType === "date-time" ? "YYYY-MM-DD HH:mm" : "YYYY-MM-DD"; },
            selectOptions() { return (this.loadedOptions || this.options || []).map((option) => ({ value: String(option.value), label: String(option.label) })); }
        },
        template:
            '<component :is="labelType" :id="id" class="editable-label cursor-pointer" :class="{\'m-editable-empty\': isEmpty}" :title="tooltip">' +
                '<span v-if="isEmpty" class="m-muted m-editable-placeholder" v-text="placeholder || tooltip"></span><span v-else v-html="shown"></span>' +
                '<q-popup-edit v-model="editValue" buttons :label-set="submit" :label-cancel="cancel" @before-show="beforeShow" @save="save" v-slot="scope">' +
                    '<q-select v-if="widgetType === \'select\'" v-model="scope.value" :options="selectOptions" emit-value map-options dense outlined options-dense autofocus' +
                        ' :loading="saving || loadingOptions" class="m-editable-select"></q-select>' +
                    '<div v-else-if="isDate" class="row no-wrap q-gutter-sm m-editable-date">' +
                        '<q-date v-model="scope.value" :mask="dateMask" minimal flat></q-date>' +
                        '<q-time v-if="widgetType === \'date-time\'" v-model="scope.value" :mask="dateMask" format24h flat></q-time>' +
                    '</div>' +
                    '<q-input v-else v-model="scope.value" dense autofocus :type="widgetType === \'textarea\' ? \'textarea\' : \'text\'" :loading="saving" @keyup.enter="widgetType === \'textarea\' ? null : scope.set()"></q-input>' +
                '</q-popup-edit>' +
            '</component>',
        methods: {
            beforeShow() {
                if (this.widgetType === "select") {
                    this.editValue = this.current;
                    if (!this.loadUrl || !this.loadUrl.length) return;
                    this.loadingOptions = true;
                    const params = Object.assign({ currentValue: this.current }, this.loadParameters);
                    moqui.request(this.loadUrl, { method: "POST", params, accept: "application/json" }).then((result) => {
                        this.loadingOptions = false;
                        const loaded = moqui.editableOptions(result.json);
                        this.loadedOptions = loaded.options;
                        if (loaded.selected !== undefined) this.editValue = loaded.selected;
                    }, (err) => { this.loadingOptions = false; moqui.handleAjaxError(err); });
                    return;
                }
                // the label may hold markup, the editor starts from the text
                this.editValue = this.isEmpty ? "" : moqui.htmlDecode(String(this.shown).replace(/<br\s*\/?>/gi, "\n"));
                if (!this.loadUrl || !this.loadUrl.length) return;
                const params = Object.assign({ currentValue: this.editValue }, this.loadParameters);
                moqui.request(this.loadUrl, { method: "POST", params, accept: "application/json,text/plain" }).then((result) => {
                    const json = result.json;
                    const value = json !== undefined ? (Array.isArray(json) ? json[0] : json) : result.text;
                    if (value !== undefined && value !== null) this.editValue = moqui.isPlainObject(value) ? (value.value !== undefined ? value.value : value.label) : String(value);
                }, moqui.handleAjaxError);
            },
            save(value) {
                const text = value === null || value === undefined ? "" : String(value);
                const params = Object.assign({ parameterName: this.parameterName }, this.urlParameters, { [this.parameterName]: text });
                this.saving = true;
                moqui.request(this.url, { method: "POST", params }).then((result) => {
                    this.saving = false;
                    this.current = text;
                    // a select shows the label of the option chosen
                    const option = this.widgetType === "select" ? this.selectOptions.find((item) => item.value === text) : null;
                    this.shown = moqui.htmlEncode(option ? option.label : text).replace(/\n/g, "<br>");
                    if (result.json) moqui.notifyMessages(result.json.messageInfos, result.json.errors);
                }, (err) => { this.saving = false; moqui.handleAjaxError(err); });
            }
        }
    });

    /* ========== form components ========== */

    // The forms of the screens shown (m-form, m-form-link, m-form-list and the m-checkbox-set of a list's selection), while
    // mounted. The root's formsVersion changes with them, so its screenContext is gathered again.
    moqui.screenForms = [];
    const formsChanged = () => { if (moqui.webrootVue && typeof moqui.webrootVue.formsVersion === "number") moqui.webrootVue.formsVersion += 1; };
    moqui.registerScreenForm = (vm) => { if (vm && moqui.screenForms.indexOf(vm) < 0) { moqui.screenForms.push(vm); formsChanged(); } };
    moqui.unregisterScreenForm = (vm) => { const i = moqui.screenForms.indexOf(vm); if (i >= 0) { moqui.screenForms.splice(i, 1); formsChanged(); } };

    // The forms part of the root's screenContext, from the registered forms (vms), the page (dom: hasTable(name), the
    // <name>_table element of a list is on the page; holds(vm, name), that list is inside the vm's element), the current
    // parameters and the context's selection-form:
    // - forms: [{name, kind, fields}], kind form (m-form), find (m-form-link) or list. A list's row forms (<list>_<n>) fold
    //   into it (fields, rows: their count), its header find form (<list>_header) gives its findFields, and an m-form-list
    //   or a multi form (one form for all rows) is a list too. A list is known by its <list>_table element.
    // - filters: [{form, parameters}] or null, the current parameters with a value that are find fields of a list or a find
    //   form (also their _op, _not, _ic, _from, _thru and period companions).
    // - selection: {form, ids} or null, the values of the selected rows of the selection-form list (its m-checkbox-set,
    //   or the rows of an m-form-list with selection, as rows).
    const META_FIELDS = ["moquiFormName", "moquiSessionToken", "_isMulti", "pageIndex", "orderByField", "lastStandalone", "formListFindId"];
    const FIND_SUFFIXES = ["_op", "_not", "_ic", "_from", "_thru", "_period", "_poffset", "_pdate"];
    moqui.screenFormsContext = (vms, dom, parameters, selectionForm) => {
        const page = dom || { hasTable: () => false, holds: () => false };
        const forms = [];
        const byName = {};
        const entry = (name, kind) => {
            if (!byName[name]) { byName[name] = { name, kind, fields: [] }; forms.push(byName[name]); }
            if (kind === "list") byName[name].kind = "list";
            return byName[name];
        };
        const addAll = (target, names) => names.forEach((name) => { if (target.indexOf(name) < 0) target.push(name); });
        const fieldNames = (fields) => Object.keys(fields || {}).filter((name) => META_FIELDS.indexOf(name) < 0);
        const holders = [];
        (vms || []).forEach((vm) => {
            if (!vm) return;
            const type = vm.$options && vm.$options.name ? vm.$options.name : "";
            if (Array.isArray(vm.checkboxValues) && vm.checkboxValues.length && vm.getCheckboxValueArray) holders.push(vm);
            if (type === "mCheckboxSet") return;
            const attrs = vm.$attrs || {};
            const name = String(vm.name || attrs.name || attrs.id || "");
            if (!name) return;
            if (type === "mFormList") {
                const list = entry(name, "list");
                const rows = Array.isArray(vm.rowList) ? vm.rowList : [];
                list.rows = rows.length;
                addAll(list.fields, fieldNames(rows[0]));
                return;
            }
            const part = /^(.+)_(\d+|header|first|second|last|clr)$/.exec(name);
            if (part && page.hasTable(part[1])) {
                const list = entry(part[1], "list");
                if (part[2] === "header") { list.findFields = list.findFields || []; addAll(list.findFields, fieldNames(vm.fields)); }
                else if (/^\d+$/.test(part[2])) { list.rows = (list.rows || 0) + 1; addAll(list.fields, fieldNames(vm.fields)); }
                return;
            }
            const fields = vm.fields || {};
            if (fields._isMulti === "true" && page.hasTable(name)) {
                const list = entry(name, "list");
                const indexes = {};
                fieldNames(fields).forEach((key) => {
                    const match = /^(.+)_(\d+)$/.exec(key);
                    if (!match) return;
                    indexes[match[2]] = true;
                    if (list.fields.indexOf(match[1]) < 0) list.fields.push(match[1]);
                });
                list.rows = Object.keys(indexes).length;
                return;
            }
            addAll(entry(name, type === "mFormLink" ? "find" : "form").fields, fieldNames(fields));
        });
        const filters = [];
        const params = parameters || {};
        forms.forEach((form) => {
            const finds = form.kind === "list" ? form.findFields : form.kind === "find" ? form.fields : null;
            if (!finds || !finds.length) return;
            const found = {};
            Object.keys(params).forEach((key) => {
                const value = params[key];
                if (value === null || value === undefined || value === "" || META_FIELDS.indexOf(key) >= 0) return;
                const suffix = FIND_SUFFIXES.find((end) => key.endsWith(end) && finds.indexOf(key.slice(0, -end.length)) >= 0);
                if (finds.indexOf(key) >= 0 || suffix) found[key] = value;
            });
            if (Object.keys(found).length) filters.push({ form: form.name, parameters: found });
        });
        let selection = null;
        if (selectionForm) {
            selection = { form: selectionForm, ids: [] };
            const holder = holders.find((vm) => page.holds(vm, selectionForm));
            const formList = (vms || []).find((vm) => vm && vm.$options && vm.$options.name === "mFormList" && vm.name === selectionForm && vm.selectedRows);
            if (holder) selection.ids = holder.getCheckboxValueArray().map((value) => String(value));
            else if (formList) selection.rows = formList.selectedRows();
        }
        return { forms, filters: filters.length ? filters : null, selection };
    };
    // the page part of screenFormsContext in the browser
    moqui.screenFormsDom = () => {
        const doc = root.document;
        if (!doc || !doc.getElementById) return null;
        return {
            hasTable: (name) => !!doc.getElementById(name + "_table"),
            holds: (vm, name) => {
                const el = vm && vm.$el;
                const table = doc.getElementById(name + "_table");
                return !!(el && table && el.contains && el.contains(table));
            }
        };
    };

    // Selected rows of a list: a state for each row, the shift click range, and the parameters for the selected values.
    const checkboxSetMixin = {
        props: { checkboxCount: { type: Number, default: 100 }, checkboxParameter: String, checkboxListMode: Boolean, checkboxValues: Array },
        data() {
            return { checkboxAllState: false, checkboxLastIndex: null, checkboxLastChange: null, checkboxStates: new Array(this.checkboxCount).fill(false) };
        },
        methods: {
            setCheckboxAllState(newState) {
                this.checkboxAllState = newState;
                this.checkboxStates = this.checkboxStates.map(() => newState);
            },
            clickCheckbox(event, index) {
                if (event.shiftKey && this.checkboxLastIndex !== null && this.checkboxLastIndex !== index) {
                    const dir = index > this.checkboxLastIndex ? 1 : -1;
                    for (let i = this.checkboxLastIndex; i !== index; i += dir) this.checkboxStates[i] = this.checkboxLastChange;
                }
                this.checkboxLastIndex = index;
                // the click comes before the change of the box
                this.checkboxLastChange = !this.checkboxStates[index];
            },
            getCheckboxValueArray() {
                if (!this.checkboxValues) return [];
                const out = [];
                this.checkboxStates.forEach((state, i) => { if (state && this.checkboxValues[i]) out.push(this.checkboxValues[i]); });
                return out;
            },
            // formData has a set(name, value) method. Returns whether any row was selected.
            addCheckboxParameters(formData, parameter, listMode) {
                const parmName = parameter || this.checkboxParameter;
                const useList = listMode !== null && listMode !== undefined && listMode ? listMode : this.checkboxListMode;
                const values = this.getCheckboxValueArray();
                if (!values.length) return false;
                if (useList) formData.set(parmName, values.join(","));
                else {
                    values.forEach((value, i) => formData.set(parmName + "_" + i, value));
                    formData.set("_isMulti", "true");
                }
                return true;
            }
        },
        watch: {
            checkboxStates: { deep: true, handler(states) { this.checkboxAllState = states.length > 0 && states.every((state) => state); } }
        }
    };
    moqui.checkboxSetMixin = checkboxSetMixin;

    component("m-checkbox-set", {
        name: "mCheckboxSet",
        mixins: [checkboxSetMixin],
        template: '<span class="checkbox-set"><slot :checkboxAllState="checkboxAllState" :setCheckboxAllState="setCheckboxAllState"' +
            ' :checkboxStates="checkboxStates" :clickCheckbox="clickCheckbox" :addCheckboxParameters="addCheckboxParameters"></slot></span>',
        // the selected rows of a list are the screen context's selection
        mounted() { moqui.registerScreenForm(this); },
        beforeUnmount() { moqui.unregisterScreenForm(this); }
    });

    // The button a form was submitted with: the one clicked, else the one that has the focus.
    const submitButton = (el, clicked) => {
        const candidate = clicked || root.document.activeElement;
        if (candidate && candidate.matches && candidate.matches('button[type="submit"], input[type="submit"], input[type="image"]') && el.contains(candidate)) return candidate;
        return null;
    };
    const disableButton = (button) => {
        button.disabled = true;
        setTimeout(() => { button.disabled = false; }, 3000);
    };

    const appendFormValue = (formData, key, value, set) => {
        if (Array.isArray(value)) value.forEach((item) => appendFormValue(formData, key, item, false));
        else if (root.Blob && value instanceof root.Blob) formData.append(key, value);
        else if (set) formData.set(key, value === null || value === undefined ? "" : value);
        else formData.append(key, value === null || value === undefined ? "" : value);
    };

    // whether a transition's answer carries errors or validation errors
    moqui.responseHasErrors = (resp) => {
        if (!resp || !moqui.isPlainObject(resp)) return false;
        const some = (value) => (Array.isArray(value) || moqui.isString(value) ? value.length > 0 : !!value);
        return some(resp.errors) || some(resp.validationErrors);
    };

    // The answers of m-form's background submits, for the Assist hand-off (WebrootAssist.js, which reports the user's own save):
    // fn({form, ok, json, status}) after each answer, a refused one too (ok false, the error's json). moqui.onFormAnswer returns the
    // function that takes the listener back.
    moqui.formAnswerListeners = [];
    moqui.onFormAnswer = (fn) => {
        if (typeof fn !== "function") return () => {};
        moqui.formAnswerListeners.push(fn);
        return () => { const index = moqui.formAnswerListeners.indexOf(fn); if (index >= 0) moqui.formAnswerListeners.splice(index, 1); };
    };
    moqui.formAnswered = (vm, info) => {
        // once per submit: an answer whose handling failed is not told again as refused
        if (!vm || vm.answerTold) return;
        vm.answerTold = true;
        const answer = Object.assign({ form: vm }, info);
        moqui.formAnswerListeners.slice().forEach((fn) => {
            try { fn(answer); } catch (err) { console.error("form answer listener failed", err); }
        });
    };

    // The fields and the buttons of a form. The value a field starts with is the fields-initial, and a changed field is
    // tinted by the widget through fieldChanged.
    component("m-form", {
        name: "mForm",
        mixins: [checkboxSetMixin],
        // a dialog that closes when a form in it submits without errors and has no submit-hide-id (the shell's quick create), and
        // the wizard whose step this form may be (m-subscreens-wizard: its answer moves the wizard on)
        inject: { mSubmitHide: { default: null }, mWizard: { default: null } },
        props: { fieldsInitial: Object, action: { type: String, required: true }, method: { type: String, default: "POST" }, readOnly: { type: Boolean, default: false },
            submitMessage: String, submitReloadId: String, submitHideId: String, focusField: String, noValidate: Boolean, excludeEmptyFields: Boolean,
            parentCheckboxSet: Object, checkboxOptional: Boolean, bodyParameterNames: Array, validationErrorsInitial: Array },
        // submitted: after a background submit whose answer has no errors, with the answer (the JSON, or the text), so a .qvue host can react
        emits: ["submitted"],
        data() {
            return { fields: Object.assign({}, this.fieldsInitial), fieldsOriginal: Object.assign({}, this.fieldsInitial), buttonClicked: null,
                fieldErrors: {}, fieldErrorValues: {} };
        },
        provide() { return { mForm: this }; },
        watch: {
            // an error stays by its field until the value changes
            fields: {
                deep: true,
                handler() {
                    Object.keys(this.fieldErrors).forEach((name) => {
                        if (this.fields[name] !== this.fieldErrorValues[name]) delete this.fieldErrors[name];
                    });
                }
            }
        },
        created() {
            if (this.validationErrorsInitial && this.validationErrorsInitial.length) moqui.placeFieldErrors(this, { validationErrors: this.validationErrorsInitial });
        },
        template:
            '<q-form ref="qForm" @submit.prevent="submitForm" @reset.prevent="resetForm" autocapitalize="off" autocomplete="off">' +
                '<slot :fields="fields" :checkboxAllState="checkboxAllState" :setCheckboxAllState="setCheckboxAllState"' +
                    ' :checkboxStates="checkboxStates" :clickCheckbox="clickCheckbox" :addCheckboxParameters="addCheckboxParameters"' +
                    ' :blurSubmitForm="blurSubmitForm" :hasFieldsChanged="hasFieldsChanged" :fieldChanged="fieldChanged"></slot>' +
            '</q-form>',
        computed: { hasFieldsChanged() { return moqui.fieldValuesDiff(this.fields, this.fieldsOriginal); } },
        methods: {
            submitForm() {
                if (this.noValidate) this.submitGo();
                else this.$refs.qForm.validate().then((success) => {
                    if (success) this.submitGo();
                    else if (this.mWizard) this.mWizard.stepRefused(this);
                });
            },
            resetForm() { this.fields = Object.assign({}, this.fieldsOriginal); },
            blurSubmitForm() {
                if (this.hasFieldsChanged) this.submitForm();
                return true;
            },
            fieldChanged(name) { return moqui.fieldChanged(this.fields, this.fieldsOriginal, name); },
            setFieldErrors(list) {
                const errors = {};
                const values = {};
                list.forEach((ve) => {
                    errors[ve.field] = errors[ve.field] ? errors[ve.field] + "; " + ve.message : ve.message;
                    values[ve.field] = this.fields[ve.field];
                });
                this.fieldErrors = errors;
                this.fieldErrorValues = values;
            },
            submitGo() {
                this.fieldErrors = {};
                this.answerTold = false;
                const formEl = this.$refs.qForm.$el;
                const button = submitButton(formEl, this.buttonClicked);
                let btnName = null;
                let btnValue = null;
                if (button) {
                    if (button.hasAttribute("name")) { btnName = button.getAttribute("name"); btnValue = button.value; }
                    disableButton(button);
                }
                const formData = Object.keys(this.fields).length ? new root.FormData() : new root.FormData(formEl);
                Object.keys(this.fields).forEach((key) => appendFormValue(formData, key, this.fields[key], true));
                const remove = [];
                Array.from(formData.entries()).forEach(([fieldName, fieldValue]) => {
                    // an unfilled mask (underscores) is empty
                    if (moqui.isString(fieldValue) && fieldValue.startsWith("__")) formData.set(fieldName, "");
                    if (this.excludeEmptyFields && (fieldValue === "" || fieldValue === null || fieldValue === undefined)) remove.push(fieldName);
                });
                remove.forEach((name) => formData.delete(name));
                formData.set("moquiSessionToken", this.$root.moquiSessionToken);
                if (btnName) formData.set(btnName, btnValue);
                // A row action posts the selected rows. With none selected it does not post (Java's posted without them).
                if (this.parentCheckboxSet && this.parentCheckboxSet.addCheckboxParameters && !this.parentCheckboxSet.addCheckboxParameters(formData) && !this.checkboxOptional) {
                    if (button) button.disabled = false;
                    notify(Object.assign({}, moqui.notifyOptsInfo, { type: "warning", message: moqui.l10n ? moqui.l10n.localize("Select one or more rows first") : "Select one or more rows first" }));
                    return;
                }

                const root_ = this.$root;
                root_.loading++;
                if (this.mWizard && this.mWizard.isStepForm(this)) this.mWizard.stepSubmitting(this);
                let arrived = false;
                const method = this.method.toUpperCase();
                let target = this.$root.appRootPath + this.action;
                const init = { method, credentials: "same-origin", headers: { Accept: "application/json", "X-CSRF-Token": this.$root.moquiSessionToken } };
                if (method === "GET") {
                    const search = new root.URLSearchParams(formData).toString();
                    target += (target.indexOf("?") > 0 ? "&" : "?") + search;
                } else init.body = formData;
                root.fetch(target, init).then((response) => {
                    arrived = true;
                    root_.loading--;
                    root_.getCsrfToken(response);
                    const disposition = response.headers.get("Content-Disposition");
                    if (response.ok && disposition && (disposition.indexOf("attachment") !== -1 || disposition.indexOf("inline") !== -1)) {
                        return response.blob().then((blob) => {
                            const match = /filename[^;=\n]*=((['"]).*?\2|[^;\n]*)/.exec(disposition);
                            const filename = disposition.indexOf("attachment") !== -1 && match && match[1] ? match[1].replace(/['"]/g, "") : "";
                            const downloadUrl = root.URL.createObjectURL(blob);
                            if (filename) {
                                const a = root.document.createElement("a");
                                a.href = downloadUrl;
                                a.download = filename;
                                root.document.body.appendChild(a);
                                a.click();
                                a.remove();
                            } else root.location.href = downloadUrl;
                            setTimeout(() => root.URL.revokeObjectURL(downloadUrl), 100);
                        });
                    }
                    return response.text().then((text) => {
                        let json;
                        try { json = JSON.parse(text); } catch (err) { json = undefined; }
                        if (!response.ok) throw { status: response.status, statusText: response.statusText, text, json };
                        this.handleResponse(json !== undefined ? json : text);
                    });
                }).catch((err) => {
                    if (!arrived) root_.loading--;
                    // a refused save: the user fixes it and saves again at once (a good one keeps its button off for 3 s)
                    if (button) button.disabled = false;
                    moqui.formAnswered(this, { ok: false, json: err && moqui.isPlainObject(err.json) ? err.json : null, status: err && err.status !== undefined ? err.status : 0 });
                    if (err && err.json) err = Object.assign({}, err, { json: moqui.placeFieldErrors(this, err.json) });
                    moqui.handleLoadError(err && err.status !== undefined ? err : { status: 0, error: err, text: "" });
                    if (this.mWizard) this.mWizard.stepRefused(this);
                });
            },
            handleResponse(resp) {
                let notified = false;
                // the step form of a wizard: the wizard goes where the answer says (js/README.md, Wizard and stack panels)
                const wizard = this.mWizard && this.mWizard.isStepForm(this) ? this.mWizard : null;
                if (resp && moqui.isPlainObject(resp)) {
                    const rest = moqui.placeFieldErrors(this, resp);
                    notified = moqui.notifyMessages(rest.messageInfos, rest.errors, rest.validationErrors);
                    if (wizard) notified = true;
                    else if (resp.screenUrl && resp.screenUrl.length > 0) this.$root.setUrl(resp.screenUrl);
                    else if (resp.redirectUrl && resp.redirectUrl.length > 0) root.location.href = resp.redirectUrl;
                // an empty answer is a transition's type="none" response; other text that is not JSON is worth a warning
                } else if (resp) console.warn("m-form non-JSON response");
                if (this.submitHideId && this.submitHideId.length > 0) this.$root.hideContainer(this.submitHideId);
                else if (this.mSubmitHide && !moqui.responseHasErrors(resp)) this.mSubmitHide.hide();
                if (this.submitReloadId && this.submitReloadId.length > 0) this.$root.reloadContainer(this.submitReloadId);
                if (this.submitMessage && this.submitMessage.length) {
                    notify(Object.assign({}, moqui.notifyOpts, { message: this.submitMessage }));
                    addNotify(this.submitMessage, "success");
                } else if (!notified) notify(Object.assign({}, moqui.notifyOpts, { message: "Submit successful" }));
                if (wizard) wizard.stepAnswered(this, resp);
                if (!moqui.responseHasErrors(resp)) this.$emit("submitted", resp);
                moqui.formAnswered(this, { ok: !moqui.responseHasErrors(resp), json: moqui.isPlainObject(resp) ? resp : null, status: 200 });
            }
        },
        mounted() {
            if (this.mWizard) this.mWizard.addForm(this);
            const el = this.$refs.qForm.$el;
            if (this.focusField && this.focusField.length) {
                const target = el.querySelector('[name^="' + this.focusField + '"]');
                if (target) { target.classList.add("default-focus"); target.focus(); }
            }
            el.addEventListener("click", (event) => {
                const button = event.target.closest && event.target.closest('button[type="submit"], input[type="submit"], input[type="image"]');
                if (button) this.buttonClicked = button;
            });
            moqui.registerScreenForm(this);
        },
        beforeUnmount() {
            if (this.mWizard) this.mWizard.removeForm(this);
            moqui.unregisterScreenForm(this);
        }
    });

    // A form that goes to a screen: its fields become the query of the URL.
    component("m-form-link", {
        name: "mFormLink",
        props: { fieldsInitial: Object, action: { type: String, required: true }, focusField: String, noValidate: Boolean, bodyParameterNames: Array },
        data() { return { fields: Object.assign({}, this.fieldsInitial), fieldsOriginal: Object.assign({}, this.fieldsInitial) }; },
        provide() { return { mForm: this }; },
        template:
            '<q-form ref="qForm" @submit.prevent="submitForm" @reset.prevent="resetForm" autocapitalize="off" autocomplete="off">' +
                '<slot :clearForm="clearForm" :fields="fields" :hasFieldsChanged="hasFieldsChanged" :fieldChanged="fieldChanged"></slot></q-form>',
        computed: { hasFieldsChanged() { return moqui.fieldValuesDiff(this.fields, this.fieldsOriginal); } },
        methods: {
            submitForm() {
                if (this.noValidate) this.submitGo();
                else this.$refs.qForm.validate().then((success) => { if (success) this.submitGo(); });
            },
            submitGo() {
                const formEl = this.$refs.qForm.$el;
                const button = submitButton(formEl, null);
                let btnName = null;
                let btnValue = null;
                if (button) {
                    if (button.hasAttribute("name")) { btnName = button.getAttribute("name"); btnValue = button.value; }
                    disableButton(button);
                }
                const formData = Object.keys(this.fields).length ? new root.FormData() : new root.FormData(formEl);
                Object.keys(this.fields).forEach((key) => {
                    const value = this.fields[key];
                    if (!value || (moqui.isString(value) && value.startsWith("__"))) return;
                    appendFormValue(formData, key, value, true);
                });
                const extraList = [];
                const plainKeyList = [];
                const parms = [];
                let bodyParameters = null;
                Array.from(formData.entries()).forEach(([key, value]) => {
                    if (!moqui.isString(value) || value.trim().length === 0 || key === "moquiSessionToken" || key === "moquiFormName" || key.indexOf("[]") > 0) return;
                    if (key.indexOf("_op") > 0 || key.indexOf("_not") > 0 || key.indexOf("_ic") > 0) extraList.push({ name: key, value });
                    else {
                        plainKeyList.push(key);
                        if (this.bodyParameterNames && this.bodyParameterNames.indexOf(key) >= 0) {
                            bodyParameters = bodyParameters || {};
                            bodyParameters[key] = value;
                        } else parms.push(encodeURIComponent(key) + "=" + encodeURIComponent(value));
                    }
                });
                // the operator, not and ignore case values count only for a field that has a value
                extraList.forEach((extra) => {
                    const keyName = extra.name.substring(0, extra.name.indexOf("_"));
                    if (plainKeyList.indexOf(keyName) >= 0) parms.push(encodeURIComponent(extra.name) + "=" + encodeURIComponent(extra.value));
                });
                if (btnName && btnValue && btnValue.trim().length) parms.push(encodeURIComponent(btnName) + "=" + encodeURIComponent(btnValue));
                const parmStr = parms.join("&");
                const url = this.action.indexOf("?") > 0 ? this.action + "&" + parmStr : this.action + "?" + parmStr;
                this.$root.setUrl(url, bodyParameters);
            },
            resetForm() { this.fields = Object.assign({}, this.fieldsInitial); },
            clearForm() { this.fields = {}; },
            fieldChanged(name) { return moqui.fieldChanged(this.fields, this.fieldsOriginal, name); }
        },
        mounted() {
            if (this.focusField && this.focusField.length > 0) {
                const target = this.$refs.qForm.$el.querySelector('[name="' + this.focusField + '"]');
                if (target) { target.classList.add("default-focus"); target.focus(); }
            }
            moqui.registerScreenForm(this);
        },
        beforeUnmount() { moqui.unregisterScreenForm(this); }
    });

    component("m-form-paginate", {
        name: "mFormPaginate",
        props: { paginate: Object, formList: Object },
        template:
            '<div v-if="paginate && paginate.count > 1" class="row no-wrap items-center">' +
                '<span class="text-caption text-grey-7 q-mx-sm">{{paginate.pageRangeLow}}-{{paginate.pageRangeHigh}} / {{paginate.count}}</span>' +
                '<q-pagination v-if="paginate.pageMaxIndex > 0" :model-value="paginate.pageIndex + 1" @update:model-value="setIndex($event - 1)" :max="paginate.pageMaxIndex + 1" :max-pages="7"' +
                    ' direction-links boundary-links size="sm" flat active-design="unelevated" color="grey-8" active-color="primary"></q-pagination>' +
            '</div>',
        methods: {
            setIndex(newIndex) {
                if (this.formList) this.formList.setPageIndex(newIndex);
                else this.$root.setParameters({ pageIndex: newIndex });
            }
        }
    });

    component("m-form-go-page", {
        name: "mFormGoPage",
        props: { idVal: { type: String, required: true }, maxIndex: Number, formList: Object },
        data() { return { pageIndex: "" }; },
        template:
            '<q-form v-if="!formList || (formList.paginate && formList.paginate.pageMaxIndex > 4)" @submit.prevent="goPage" class="q-mx-xs">' +
                '<q-input dense outlined hide-bottom-space v-model="pageIndex" type="text" name="pageIndex" placeholder="Page #" style="width:96px;" input-class="text-center"' +
                    ' :rules="[val => /^\\d*$/.test(val) || \'digits only\', val => !val || ((formList && +val <= formList.paginate.pageMaxIndex + 1) || (maxIndex && +val <= maxIndex + 1)) || \'higher than max\']">' +
                    '<template v-slot:append><q-btn flat round dense size="sm" type="submit" icon="arrow_forward"></q-btn></template>' +
                '</q-input>' +
            '</q-form>',
        methods: {
            goPage() {
                if (!this.pageIndex) return;
                const newIndex = +this.pageIndex - 1;
                if (this.formList) this.formList.setPageIndex(newIndex);
                else this.$root.setParameters({ pageIndex: newIndex });
                this.$nextTick(() => { this.pageIndex = ""; });
            }
        }
    });

    component("m-form-column-config", {
        name: "mFormColumnConfig",
        // column entries have id, label and children
        props: { id: String, action: String, columnsInitial: { type: Array, required: true }, formLocation: { type: String, required: true }, findParameters: Object },
        data() { return { columns: moqui.deepCopy(this.columnsInitial) }; },
        template:
            '<m-form ref="mForm" :id="id" :action="action">' +
                '<q-list v-for="(column, columnIdx) in columns" :key="column.id" bordered dense class="q-mb-sm rounded-borders">' +
                    '<q-item-label header>{{column.label}}</q-item-label>' +
                    '<q-item v-for="(field, fieldIdx) in column.children" :key="field.id">' +
                        '<q-item-section side v-if="columnIdx !== 0"><q-btn flat round dense size="sm" icon="close" @click="hideField(columnIdx, fieldIdx)"><q-tooltip>Hide</q-tooltip></q-btn></q-item-section>' +
                        '<q-item-section><q-item-label>{{field.label}}</q-item-label></q-item-section>' +
                        '<q-item-section v-if="columnIdx === 0" side>' +
                            '<q-btn-dropdown dense flat no-caps label="Display" class="m-btn-tonal"><q-list dense>' +
                                '<q-item v-for="(toColumn, toColumnIdx) in columns.slice(1)" :key="toColumn.id" clickable v-close-popup>' +
                                    '<q-item-section @click="moveToCol(columnIdx, fieldIdx, toColumnIdx+1)">{{toColumn.label}}</q-item-section></q-item>' +
                                '<q-item clickable v-close-popup><q-item-section @click="moveToCol(columnIdx, fieldIdx, columns.length+1)">New Column</q-item-section></q-item>' +
                            '</q-list></q-btn-dropdown>' +
                        '</q-item-section>' +
                        '<q-item-section v-else side><q-btn-group flat>' +
                            '<q-btn :disabled="columnIdx <= 1" dense flat size="sm" icon="north" @click="moveToCol(columnIdx, fieldIdx, columnIdx-1)"></q-btn>' +
                            '<q-btn :disabled="fieldIdx === 0" dense flat size="sm" icon="expand_less" @click="moveInCol(columnIdx, fieldIdx, fieldIdx-1)"></q-btn>' +
                            '<q-btn :disabled="(fieldIdx + 1) === column.children.length" dense flat size="sm" icon="expand_more" @click="moveInCol(columnIdx, fieldIdx, fieldIdx+1)"></q-btn>' +
                            '<q-btn dense flat size="sm" icon="south" @click="moveToCol(columnIdx, fieldIdx, columnIdx+1)"></q-btn>' +
                        '</q-btn-group></q-item-section>' +
                    '</q-item>' +
                '</q-list>' +
                '<div class="q-my-md">' +
                    '<q-btn dense unelevated no-caps color="primary" @click.prevent="saveColumns()" label="Save Changes"></q-btn>' +
                    '<q-btn dense flat no-caps @click.prevent="resetColumns()" label="Undo Changes"></q-btn>' +
                    '<q-btn dense flat no-caps @click.prevent="resetToDefault()" label="Reset to Default"></q-btn>' +
                '</div>' +
            '</m-form>',
        methods: {
            moveInCol(columnIdx, fieldIdx, newFieldIdx) {
                const children = this.columns[columnIdx].children;
                children.splice(newFieldIdx, 0, children.splice(fieldIdx, 1)[0]);
            },
            moveToCol(columnIdx, fieldIdx, newColumnIdx) {
                const newColumn = newColumnIdx >= this.columns.length ? this.addColumn() : this.columns[newColumnIdx];
                newColumn.children.push(this.columns[columnIdx].children.splice(fieldIdx, 1)[0]);
            },
            addColumn() {
                const oldLength = this.columns.length;
                const lastCol = this.columns[oldLength - 1];
                this.columns.push({ id: lastCol.id.split("_")[0] + "_" + oldLength, label: lastCol.label.split(" ")[0] + " " + oldLength, children: [] });
                return this.columns[oldLength];
            },
            hideField(columnIdx, fieldIdx) {
                if (columnIdx === 0) return;
                this.columns[0].children.push(this.columns[columnIdx].children.splice(fieldIdx, 1)[0]);
            },
            resetColumns() { this.columns = moqui.deepCopy(this.columnsInitial); },
            saveColumns() {
                this.generalFormFields();
                const fields = this.$refs.mForm.fields;
                fields.SaveColumns = "SaveColumns";
                fields.columnsTree = JSON.stringify(this.columns);
                this.$refs.mForm.submitGo();
            },
            resetToDefault() {
                this.generalFormFields();
                this.$refs.mForm.fields.ResetColumns = "ResetColumns";
                this.$refs.mForm.submitGo();
            },
            generalFormFields() {
                const fields = this.$refs.mForm.fields;
                fields.formLocation = this.formLocation;
                if (this.findParameters) Object.keys(this.findParameters).forEach((key) => { fields[key] = this.findParameters[key]; });
                if (root.innerWidth <= 600 || root.Quasar.Platform.is.mobile) fields._uiType = "mobile";
            }
        }
    });

    // A list drawn by the client from rows it fetches. rows is a list, or the URL of the rows: the answer is a list or a
    // map that holds the list under the name of the form with its page keys (listName + Count, PageIndex, PageSize,
    // PageMaxIndex, PageRangeLow, PageRangeHigh). Without a URL of its own the rows of the form are fetched from
    // <current screen>/actions/<name>. The slots are headerForm, header (search, sortBy, orderBy), nav, rowForm (fields,
    // rowIndex), row (fields, rowIndex, selected), subRow (row, subRow). A column header calls sortBy(field), the page
    // buttons set the page, and rows can be selected when selection is true: selectedRows() are the selected rows.
    component("m-form-list", {
        name: "mFormList",
        props: { name: { type: String, required: true }, id: String, rows: { type: [String, Array], required: true }, search: Object, listName: String,
            action: String, multi: Boolean, skipForm: Boolean, skipHeader: Boolean, headerForm: Boolean, headerDialog: Boolean,
            savedFinds: Boolean, selectColumns: Boolean, allButton: Boolean, csvButton: Boolean, textButton: Boolean, pdfButton: Boolean,
            selection: Boolean, selectionField: String, columns: [String, Number] },
        data() { return { rowList: [], paginate: null, searchObj: {}, selected: [], loading: false }; },
        template:
            '<div>' +
                '<template v-if="!multi && !skipForm">' +
                    '<m-form v-for="(fields, rowIndex) in rowList" :key="rowIndex" :name="idVal+\'_\'+rowIndex" :id="idVal+\'_\'+rowIndex" :action="action" :fields-initial="fields">' +
                        '<slot name="rowForm" :fields="fields" :row-index="rowIndex"></slot></m-form></template>' +
                '<m-form v-if="multi && !skipForm" :name="idVal" :id="idVal" :action="action" :fields-initial="multiFields">' +
                    '<input type="hidden" name="moquiFormName" :value="name"><input type="hidden" name="_isMulti" value="true">' +
                    '<template v-for="(fields, rowIndex) in rowList" :key="rowIndex"><slot name="rowForm" :fields="fields" :row-index="rowIndex"></slot></template></m-form>' +
                '<div class="q-my-sm q-table__container q-table__card q-table--horizontal-separator q-table--dense q-table--flat" :class="{\'q-table--dark\':$q.dark.isActive}">' +
                    '<div class="form-list-nav-row row items-center">' +
                        '<slot name="navStart"></slot>' +
                        '<q-space></q-space>' +
                        '<m-form-paginate :paginate="paginate" :form-list="this"></m-form-paginate>' +
                        '<m-form-go-page :id-val="idVal" :form-list="this"></m-form-go-page>' +
                        '<q-btn v-if="csvButton" dense flat no-caps icon="download" type="a" :href="csvUrl" label="CSV"></q-btn>' +
                        '<slot name="nav"></slot>' +
                    '</div>' +
                    '<div class="table q-table" :id="idVal+\'_table\'">' +
                        '<div v-if="!skipHeader" class="thead">' +
                            '<m-form-link v-if="headerForm && !headerDialog" :name="idVal+\'_header\'" :id="idVal+\'_header\'" :action="$root.currentLinkPath" :fields-initial="searchObj" v-slot:default="formProps">' +
                                '<slot name="headerForm" :search="formProps.fields"></slot>' +
                            '</m-form-link>' +
                            '<div class="tr">' +
                                '<div v-if="selection" class="th m-sel"><q-checkbox dense size="sm" :model-value="allSelected" @update:model-value="selectAll"></q-checkbox></div>' +
                                '<slot name="header" :search="searchObj" :sort-by="sortBy" :order-by="orderBy"></slot>' +
                            '</div>' +
                        '</div>' +
                        '<div class="tbody">' +
                            '<template v-for="(fields, rowIndex) in rowList" :key="rowIndex">' +
                                '<div class="tr" :class="{selected:selected[rowIndex]}">' +
                                    '<div v-if="selection" class="td m-sel"><q-checkbox dense size="sm" :model-value="!!selected[rowIndex]" @update:model-value="selected[rowIndex] = $event"></q-checkbox></div>' +
                                    '<slot name="row" :fields="fields" :row-index="rowIndex" :selected="!!selected[rowIndex]"></slot>' +
                                '</div>' +
                                '<template v-if="fields.aggregateSubList && fields.aggregateSubList.length">' +
                                    '<div v-for="(subRow, subIndex) in fields.aggregateSubList" :key="rowIndex+\'_\'+subIndex" class="tr form-list-sub-row"><slot name="subRow" :row="fields" :sub-row="subRow" :sub-index="subIndex"></slot></div>' +
                                '</template>' +
                            '</template>' +
                            '<div v-if="!rowList.length && !loading" class="tr"><div class="td">No results found</div></div>' +
                        '</div>' +
                    '</div>' +
                    // the page buttons again under a list of more than one page: after the table, not a row squeezed into its first column
                    '<div v-if="paginate && paginate.pageMaxIndex > 0" class="form-list-nav-row row items-center">' +
                        '<q-space></q-space>' +
                        '<m-form-go-page :id-val="idVal" :form-list="this"></m-form-go-page>' +
                        '<m-form-paginate :paginate="paginate" :form-list="this"></m-form-paginate>' +
                    '</div>' +
                '</div>' +
            '</div>',
        computed: {
            idVal() { return this.id && this.id.length > 0 ? this.id : this.name; },
            allSelected() { return this.rowList.length > 0 && this.rowList.every((row, i) => this.selected[i]); },
            // the rows of a form that posts them all, with the number of the row after the name of each field
            multiFields() {
                const out = {};
                this.rowList.forEach((row, i) => Object.keys(row).forEach((key) => { out[key + "_" + i] = row[key]; }));
                return out;
            },
            csvUrl() {
                return this.$root.currentPath + "?" + moqui.objToSearch(Object.assign({}, this.searchObj, { renderMode: "csv", pageNoLimit: "true", lastStandalone: "true", saveFilename: this.name + ".csv" }));
            }
        },
        methods: {
            // the answer of a rows request: a list, or a map with the list and the page keys
            takeRows(data) {
                const listName = this.listName || this.name;
                let list = data;
                let paginate = null;
                if (moqui.isPlainObject(data)) {
                    list = data[listName] || data.list || data.rows || [];
                    if (data[listName + "Count"] !== undefined) {
                        paginate = {
                            count: Number(data[listName + "Count"]), pageIndex: Number(data[listName + "PageIndex"]), pageSize: Number(data[listName + "PageSize"]),
                            pageMaxIndex: Number(data[listName + "PageMaxIndex"]), pageRangeLow: Number(data[listName + "PageRangeLow"]), pageRangeHigh: Number(data[listName + "PageRangeHigh"])
                        };
                    }
                }
                this.rowList = Array.isArray(list) ? list : [];
                this.selected = this.rowList.map(() => false);
                this.paginate = paginate;
            },
            fetchRows() {
                if (Array.isArray(this.rows)) { this.takeRows(this.rows); return; }
                let url = this.rows;
                if (url.indexOf("/") === -1) url = this.$root.currentPath + "/actions/" + url;
                this.loading = true;
                moqui.request(url, { params: this.searchObj }).then((result) => {
                    this.loading = false;
                    const json = result.json;
                    this.takeRows(json);
                    const count = Number(result.headers.get("X-Total-Count"));
                    if (!this.paginate && count && !Number.isNaN(count)) {
                        const header = (name) => Number(result.headers.get(name));
                        this.paginate = { count, pageIndex: header("X-Page-Index"), pageSize: header("X-Page-Size"), pageMaxIndex: header("X-Page-Max-Index"),
                            pageRangeLow: header("X-Page-Range-Low"), pageRangeHigh: header("X-Page-Range-High") };
                    }
                }, (err) => { this.loading = false; moqui.handleAjaxError(err); });
            },
            setPageIndex(newIndex) {
                this.searchObj = Object.assign({}, this.searchObj, { pageIndex: newIndex });
                this.fetchRows();
            },
            // order by a field: the first call is ascending, the same field again descending
            sortBy(field) {
                const current = this.searchObj.orderByField || "";
                const next = current === field ? "-" + field : field;
                this.searchObj = Object.assign({}, this.searchObj, { orderByField: next, pageIndex: 0 });
                this.fetchRows();
            },
            orderBy(field) {
                const current = this.searchObj.orderByField || "";
                return current === field ? "asc" : current === "-" + field ? "desc" : "";
            },
            selectAll(state) { this.selected = this.rowList.map(() => state); },
            selectedRows() { return this.rowList.filter((row, i) => this.selected[i]); }
        },
        watch: {
            rows(newRows) { if (Array.isArray(newRows)) this.takeRows(newRows); else this.fetchRows(); },
            search() { this.searchObj = Object.assign({}, this.search); this.fetchRows(); }
        },
        mounted() {
            this.searchObj = Object.assign({}, this.search || this.$root.currentParameters);
            this.fetchRows();
            moqui.registerScreenForm(this);
        },
        beforeUnmount() { moqui.unregisterScreenForm(this); }
    });

    /* ========== form field widget components ========== */

    // What every widget takes from the form around it: the highlight of a changed field.
    const fieldMixin = {
        inject: { mForm: { default: null } },
        props: { bgColor: String },
        computed: {
            bg() {
                if (this.bgColor) return this.bgColor;
                const form = this.mForm;
                if (!form || !this.name || !form.fieldChanged(this.name)) return "";
                return this.$q.dark.isActive ? "blue-10" : "blue-1";
            },
            // the server's validation error for this field, put on the Quasar field; none leaves the rules in charge
            errorAttrs() {
                const form = this.mForm;
                const message = form && this.name && form.fieldErrors ? form.fieldErrors[this.name] : null;
                return message ? { error: true, errorMessage: message } : {};
            }
        }
    };

    // A value held here that follows the value given by the parent: the same widget works inside a form (v-model) and alone.
    const localValueMixin = {
        data() { return { local: this.modelValue }; },
        watch: { modelValue(value) { this.local = value; } },
        methods: { onInput(value) { this.local = value; this.$emit("update:modelValue", value); } }
    };

    // The values of other fields a widget depends on: the fields object of its form, else the page by element id.
    const dependsValues = (vm, dependsOn, parameters) => {
        const reqData = Object.assign({}, parameters);
        let hasAllParms = true;
        Object.keys(dependsOn || {}).forEach((doParm) => {
            let value;
            if (vm.fields) value = vm.fields[dependsOn[doParm]];
            else {
                const el = root.document.getElementById(dependsOn[doParm]);
                value = el && (el.value || (el.querySelector("select") || {}).value);
            }
            if (!value) hasAllParms = false;
            else reqData[doParm] = value;
        });
        return { reqData, hasAllParms };
    };

    component("m-field", {
        name: "mField",
        inheritAttrs: false,
        props: { label: String, bgColor: String, disable: Boolean, borderless: Boolean },
        template: '<q-field dense :outlined="!borderless" :borderless="borderless" stack-label hide-bottom-space :label="label" :bg-color="bgColor" :disable="disable" v-bind="$attrs" :model-value="1"><template v-slot:control><slot></slot></template></q-field>'
    });

    component("m-date-time", {
        name: "mDateTime",
        mixins: [fieldMixin, localValueMixin],
        props: { id: String, name: { type: String, required: true }, modelValue: String, type: { type: String, default: "date-time" }, label: String,
            size: String, format: String, tooltip: String, form: String, required: Boolean, rules: Array, disable: Boolean, autoYear: String,
            minuteStep: { type: Number, default: 5 } },
        emits: ["update:modelValue"],
        template:
            '<q-input dense outlined stack-label hide-bottom-space no-error-icon :label="label" :model-value="local" @update:model-value="onInput" :rules="allRules"' +
                    ' :mask="inputMask" fill-mask :for="id" :name="name" :form="form" :disable="disable" :size="sizeVal"' +
                    ' style="max-width:max-content;" :bg-color="bg" v-bind="errorAttrs">' +
                '<template v-slot:prepend v-if="hasDate">' +
                    '<q-icon name="event" size="xs" class="cursor-pointer">' +
                        '<q-popup-proxy ref="qDateProxy" cover transition-show="scale" transition-hide="scale">' +
                            '<q-date :model-value="local" @update:model-value="onDate" :mask="formatVal" today-btn></q-date>' +
                        '</q-popup-proxy>' +
                    '</q-icon>' +
                '</template>' +
                '<template v-slot:append v-if="hasTime">' +
                    '<q-icon name="access_time" size="xs" class="cursor-pointer">' +
                        '<q-popup-proxy ref="qTimeProxy" cover transition-show="scale" transition-hide="scale">' +
                            '<q-time :model-value="local" @update:model-value="onTime" :mask="formatVal" format24h now-btn></q-time>' +
                        '</q-popup-proxy>' +
                    '</q-icon>' +
                '</template>' +
                '<template v-slot:after><slot name="after"></slot></template>' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
            '</q-input>',
        computed: {
            hasDate() { return this.type === "date" || this.type === "date-time" || !this.type; },
            hasTime() { return this.type === "time" || this.type === "date-time" || !this.type; },
            formatVal() {
                if (this.format && this.format.length) return this.format;
                return this.type === "time" ? "HH:mm" : (this.type === "date" ? "YYYY-MM-DD" : "YYYY-MM-DD HH:mm");
            },
            inputMask() { return this.formatVal.replace(/\w/g, "#"); },
            sizeVal() { if (this.size && this.size.length) return this.size; return this.type === "time" ? "9" : (this.type === "date" ? "10" : "16"); },
            allRules() {
                const rules = this.rules ? this.rules.slice() : [];
                if (this.required) rules.unshift((val) => !!val || "Required");
                return rules;
            }
        },
        methods: {
            onDate(value) { this.onInput(value); if (this.$refs.qDateProxy) this.$refs.qDateProxy.hide(); },
            onTime(value) { this.onInput(value); if (this.$refs.qTimeProxy) this.$refs.qTimeProxy.hide(); }
        }
    });

    moqui.dateOffsets = [{ value: "0", label: "This" }, { value: "-1", label: "Last" }, { value: "1", label: "Next" }, { value: "-2", label: "-2" },
        { value: "2", label: "+2" }, { value: "-3", label: "-3" }, { value: "-4", label: "-4" }, { value: "-6", label: "-6" }, { value: "-12", label: "-12" }];
    moqui.datePeriods = [{ value: "day", label: "Day" }, { value: "7d", label: "7 Days" }, { value: "30d", label: "30 Days" }, { value: "week", label: "Week" },
        { value: "weeks", label: "Weeks" }, { value: "month", label: "Month" }, { value: "months", label: "Months" }, { value: "quarter", label: "Quarter" },
        { value: "year", label: "Year" }, { value: "7r", label: "+/-7d" }, { value: "30r", label: "+/-30d" }];

    component("m-date-period", {
        name: "mDatePeriod",
        props: { fields: { type: Object, required: true }, name: { type: String, required: true }, id: String, allowEmpty: Boolean,
            fromThruType: { type: String, default: "date" }, form: String, tooltip: String, label: String },
        data() { return { fromThruMode: false, dateOffsets: moqui.dateOffsets.slice(), datePeriods: moqui.datePeriods.slice(), fieldsOriginal: Object.assign({}, this.fields) }; },
        template:
            '<div v-if="fromThruMode" class="row">' +
                '<m-date-time :name="name+\'_from\'" :id="id+\'_from\'" :label="label+\' From\'" :form="form" :type="fromThruType" v-model="fields[name+\'_from\']"></m-date-time>' +
                '<q-icon class="q-my-auto" name="remove"></q-icon>' +
                '<m-date-time :name="name+\'_thru\'" :id="id+\'_thru\'" :label="label+\' Thru\'" :form="form" :type="fromThruType" v-model="fields[name+\'_thru\']">' +
                    '<template v-slot:after>' +
                        '<q-btn flat round dense size="sm" icon="calendar_view_day" @click="toggleMode"><q-tooltip>Period Select Mode</q-tooltip></q-btn>' +
                        '<q-btn flat round dense size="sm" icon="clear" @click="clearAll"><q-tooltip>Clear</q-tooltip></q-btn>' +
                    '</template>' +
                '</m-date-time>' +
            '</div>' +
            // Period mode is one labeled field: the base date, with the period and offset selects in a menu behind the button in its
            // append slot (as the find options of m-text-find), which reads the choice ("Last Month") once one is made.
            '<div v-else class="row"><q-input dense outlined stack-label hide-bottom-space :label="label" v-model="fields[name+\'_pdate\']"' +
                    ' mask="####-##-##" fill-mask :for="id" :name="name+\'_pdate\'" :form="form" style="max-width:max-content;"' +
                    ' :bg-color="changed(name+\'_pdate\')">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<template v-slot:prepend>' +
                    '<q-icon name="event" size="xs" class="cursor-pointer">' +
                        '<q-popup-proxy ref="qDateProxy" cover transition-show="scale" transition-hide="scale">' +
                            '<q-date v-model="fields[name+\'_pdate\']" mask="YYYY-MM-DD" today-btn @update:model-value="hideDate"></q-date>' +
                        '</q-popup-proxy>' +
                    '</q-icon>' +
                '</template>' +
                '<template v-slot:append>' +
                    '<q-btn flat dense no-caps size="sm" icon="event_repeat" :round="!periodText" :label="periodText || undefined" :color="periodText ? \'primary\' : \'grey-6\'">' +
                        '<q-tooltip>Period</q-tooltip>' +
                        '<q-menu class="m-date-period-options q-pa-sm"><div class="column q-gutter-y-xs">' +
                            '<q-select dense outlined options-dense emit-value map-options hide-bottom-space v-model="fields[name+\'_period\']"' +
                                ' :name="name+\'_period\'" :bg-color="changed(name+\'_period\')" stack-label label="Period" :options="datePeriods" :form="form" behavior="menu"></q-select>' +
                            '<q-select dense outlined options-dense emit-value map-options hide-bottom-space v-model="fields[name+\'_poffset\']"' +
                                ' :name="name+\'_poffset\'" :bg-color="changed(name+\'_poffset\')" stack-label label="Offset" :options="dateOffsets" :form="form" behavior="menu"></q-select>' +
                        '</div></q-menu>' +
                    '</q-btn>' +
                '</template>' +
                '<template v-slot:after>' +
                    '<q-btn flat round dense size="sm" icon="date_range" @click="toggleMode"><q-tooltip>Date Range Mode</q-tooltip></q-btn>' +
                    '<q-btn flat round dense size="sm" icon="clear" @click="clearAll"><q-tooltip>Clear</q-tooltip></q-btn>' +
                '</template>' +
            '</q-input></div>',
        computed: {
            // the choice as its labels, "Last Month"; empty until a period or an offset is chosen
            periodText() {
                const labelOf = (list, value) => { const found = list.find((option) => option.value === String(value)); return found ? found.label : String(value); };
                const parts = [];
                const offset = this.fields[this.name + "_poffset"];
                const period = this.fields[this.name + "_period"];
                if (offset !== undefined && offset !== null && offset !== "") parts.push(labelOf(this.dateOffsets, offset));
                if (period !== undefined && period !== null && period !== "") parts.push(labelOf(this.datePeriods, period));
                return parts.join(" ");
            }
        },
        methods: {
            toggleMode() { this.fromThruMode = !this.fromThruMode; },
            hideDate() { if (this.$refs.qDateProxy) this.$refs.qDateProxy.hide(); },
            clearAll() { ["_pdate", "_poffset", "_period", "_from", "_thru"].forEach((suffix) => { this.fields[this.name + suffix] = null; }); },
            changed(name) { return moqui.equalsOrPlaceholder(this.fields[name], this.fieldsOriginal[name]) ? "" : (this.$q.dark.isActive ? "blue-10" : "blue-1"); }
        },
        mounted() {
            const fromDate = this.fields[this.name + "_from"];
            const thruDate = this.fields[this.name + "_thru"];
            if ((fromDate && fromDate.length) || (thruDate && thruDate.length)) this.fromThruMode = true;
        }
    });

    // The display of a value: the text, or with label-wrapper a read-only field with the label. With value-url the text is fetched
    // from a transition when the fields it depends on change.
    component("m-display", {
        name: "mDisplay",
        props: { modelValue: [String, Number], display: String, valueUrl: String, valueParameters: Object, dependsOn: Object, dependsOptional: Boolean, valueLoadInit: Boolean,
            fields: Object, tooltip: String, label: String, labelWrapper: Boolean, name: String, id: String },
        emits: ["update:modelValue"],
        data() { return { curDisplay: this.display, loading: false }; },
        template:
            '<q-input v-if="labelWrapper" dense outlined readonly stack-label autogrow :model-value="displayValue" :label="label" :for="id" :name="name" :loading="loading">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
            '</q-input>' +
            '<span v-else :id="id">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip><slot></slot>' +
                '{{displayValue}}' +
            '</span>',
        computed: { displayValue() { return this.curDisplay && this.curDisplay.length ? this.curDisplay : this.modelValue; } },
        watch: { display(value) { this.curDisplay = value; } },
        methods: {
            populateFromUrl() {
                if (!this.valueUrl || !this.valueUrl.length) return;
                const { reqData, hasAllParms } = dependsValues(this, this.dependsOn, this.valueParameters);
                if (!hasAllParms && !this.dependsOptional) {
                    this.$emit("update:modelValue", null);
                    this.curDisplay = null;
                    return;
                }
                this.loading = true;
                moqui.request(this.valueUrl, { method: "POST", params: reqData, accept: "application/json,text/plain" }).then((result) => {
                    this.loading = false;
                    let newLabel = "";
                    let newValue = "";
                    let response = result.json;
                    if (Array.isArray(response) && response.length) response = response[0];
                    else if (moqui.isPlainObject(response) && Array.isArray(response.options) && response.options.length) response = response.options[0];
                    if (response && moqui.isPlainObject(response)) {
                        if (response.label !== undefined) newLabel = response.label;
                        if (response.value !== undefined) newValue = response.value;
                    }
                    if (!newLabel || !String(newLabel).length) newLabel = result.text;
                    if (!newValue || !String(newValue).length) newValue = result.text;
                    this.$emit("update:modelValue", String(newValue));
                    if (this.fields && this.name) this.fields[this.name + "_display"] = newLabel;
                    this.curDisplay = newLabel;
                }, (err) => { this.loading = false; moqui.handleAjaxError(err); });
            }
        },
        mounted() {
            if (!this.valueUrl || !this.valueUrl.length) return;
            if (this.fields) Object.keys(this.dependsOn || {}).forEach((doParm) => { this.$watch(() => this.fields[this.dependsOn[doParm]], () => this.populateFromUrl()); });
            if (this.valueLoadInit) this.populateFromUrl();
        }
    });

    // A drop-down over the options given, or fetched from options-url (all at once, or as the user types with server-search).
    // multiple makes the value a list. combo lets the user add a value that is not an option.
    component("m-drop-down", {
        name: "mDropDown",
        mixins: [fieldMixin],
        props: { modelValue: [Array, String, Number], options: { type: Array, default: () => [] }, combo: Boolean, allowEmpty: Boolean, multiple: Boolean,
            requiredManualSelect: Boolean, submitOnSelect: Boolean, optionsUrl: String, optionsParameters: Object, optionsLoadInit: Boolean, serverSearch: Boolean,
            serverDelay: { type: Number, default: 300 }, serverMinLength: { type: Number, default: 1 }, labelField: { type: String, default: "label" },
            valueField: { type: String, default: "value" }, dependsOn: Object, dependsOptional: Boolean, form: String, fields: Object, tooltip: String,
            label: String, name: String, id: String, disable: Boolean, onSelectGoTo: String },
        emits: ["update:modelValue"],
        data() { return { curOptions: this.options, allOptions: this.options, lastSearch: null, loading: false }; },
        // Selected text stays visible. fill-input hid a set value, so hide-selected is only for multiple
        // (Java WebrootVue.qvt.js m-drop-down).
        template:
            '<q-select ref="qSelect" :model-value="modelValue" @update:model-value="handleInput"' +
                    ' dense outlined options-dense use-input :use-chips="multiple" :hide-selected="multiple" :name="name" :for="id" :form="form"' +
                    ' :input-debounce="serverSearch ? serverDelay : 100" @filter="filterFn" :clearable="allowEmpty||multiple" :disable="disable"' +
                    ' :multiple="multiple" :emit-value="!onSelectGoTo" map-options behavior="menu" :new-value-mode="combo ? \'add-unique\' : undefined"' +
                    ' :rules="[val => allowEmpty||multiple||combo||val===\'\'||(val&&val.length)||\'Please select an option\']"' +
                    ' stack-label hide-bottom-space no-error-icon :label="label" :loading="loading" :bg-color="bg" :options="curOptions" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<template v-slot:no-option><q-item><q-item-section class="text-grey">No results</q-item-section></q-item></template>' +
                '<template v-if="$slots.after" v-slot:after><slot name="after"></slot></template>' +
            '</q-select>',
        watch: { options(list) { this.curOptions = list; this.allOptions = list; } },
        methods: {
            handleInput(value) {
                if (this.onSelectGoTo && this.onSelectGoTo.length) {
                    if (value && value[this.onSelectGoTo]) this.$root.setUrl(value[this.onSelectGoTo]);
                } else this.$emit("update:modelValue", value);
                this.maybeSubmit();
            },
            maybeSubmit() {
                if (this.submitOnSelect && this.mForm && this.mForm.submitForm) this.$nextTick(() => this.mForm.submitForm());
            },
            filterFn(search, doneFn, abortFn) {
                if (this.serverSearch) {
                    if (this.lastSearch === search || (this.serverMinLength && (search ? search.length : 0) < this.serverMinLength)) doneFn();
                    else {
                        this.lastSearch = search;
                        this.populateFromUrl({ term: search }, doneFn, abortFn);
                    }
                } else if (this.allOptions && this.allOptions.length) {
                    if (search && search.length) {
                        const needle = search.toLowerCase();
                        doneFn(() => { this.curOptions = this.allOptions.filter((v) => v.label && String(v.label).toLowerCase().indexOf(needle) > -1); });
                    } else if (this.curOptions.length === this.allOptions.length) doneFn();
                    else doneFn(() => { this.curOptions = this.allOptions; });
                } else if (this.optionsUrl && this.optionsUrl.length) {
                    this.populateFromUrl({}, doneFn, abortFn);
                } else {
                    console.error("m-drop-down " + this.name + " has no options and no options-url");
                    abortFn();
                }
            },
            processOptionList(list) {
                return (list || []).map((obj) => {
                    const valueVal = obj[this.valueField];
                    const labelVal = obj[this.labelField];
                    return Object.assign({}, obj, { value: valueVal || labelVal, label: labelVal || valueVal });
                });
            },
            populateFromUrl(params, doneFn, abortFn) {
                if (!this.optionsUrl || !this.optionsUrl.length) {
                    console.warn("In m-drop-down tried to populateFromUrl but no optionsUrl");
                    if (abortFn) abortFn();
                    return;
                }
                const { reqData, hasAllParms } = dependsValues(this, this.dependsOn, this.optionsParameters);
                if (params) { reqData.term = params.term || ""; reqData.pageIndex = (params.page || 1) - 1; }
                else if (this.serverSearch) { reqData.term = ""; reqData.pageIndex = 0; }
                if (!hasAllParms && !this.dependsOptional) {
                    this.curOptions = [];
                    this.allOptions = [];
                    if (abortFn) abortFn();
                    return;
                }
                this.loading = true;
                moqui.request(this.optionsUrl, { method: "POST", params: reqData }).then((result) => {
                    this.loading = false;
                    const data = result.json;
                    const list = Array.isArray(data) ? data : data && data.options;
                    if (!list) return;
                    const processed = this.processOptionList(list);
                    if (doneFn) doneFn(() => this.setNewOptions(processed));
                    else this.setNewOptions(processed);
                }, (err) => {
                    this.loading = false;
                    moqui.handleAjaxError(err);
                    if (abortFn) abortFn();
                });
            },
            setNewOptions(options) {
                this.curOptions = options;
                if (this.multiple && this.allOptions && this.allOptions.length && Array.isArray(this.modelValue) && this.modelValue.length) {
                    // keep the selected values in allOptions, at the end of the list, so that they still show their labels
                    const all = options.slice();
                    this.modelValue.forEach((curValue) => this.allOptions.forEach((curOption) => { if (curValue === curOption.value) all.push(curOption); }));
                    this.allOptions = all;
                } else {
                    this.allOptions = options;
                    this.checkCurrentValue(this.allOptions);
                }
            },
            // a value that is not in the new options is cleared, or becomes the first option when an empty value is not allowed
            checkCurrentValue(options) {
                const value = this.modelValue;
                const isArray = Array.isArray(value);
                let found = false;
                if (value && value.length && options) {
                    found = options.some((option) => (isArray ? value.includes(option.value) : option.value === value));
                }
                if (found) return;
                if (!this.allowEmpty && !this.multiple && options && options.length && options[0].value && (!this.requiredManualSelect || (!this.submitOnSelect && options.length === 1))) {
                    this.$emit("update:modelValue", options[0].value);
                } else this.$emit("update:modelValue", null);
            },
        },
        mounted() {
            if (this.serverSearch && !this.optionsUrl) console.error("m-drop-down in form " + this.form + " has no options-url but has server-search");
            if (this.optionsUrl && this.optionsUrl.length) {
                if (this.fields) {
                    Object.keys(this.dependsOn || {}).forEach((doParm) => {
                        this.$watch(() => this.fields[this.dependsOn[doParm]], () => {
                            // when what the options depend on changes, the value goes and the options are fetched again
                            this.$emit("update:modelValue", null);
                            this.populateFromUrl({ term: this.lastSearch });
                        });
                    });
                }
                if (this.optionsLoadInit) {
                    if (!this.serverSearch) this.populateFromUrl();
                    else if (this.modelValue && this.modelValue.length && moqui.isString(this.modelValue)) this.populateFromUrl({ term: this.modelValue });
                }
            }
            // with an empty value not allowed the first option is the value, but only for one option when the user has to choose
            if (!this.multiple && !this.allowEmpty && (!this.modelValue || !this.modelValue.length) && this.options && this.options.length &&
                (!this.requiredManualSelect || (!this.submitOnSelect && this.options.length === 1))) {
                this.$emit("update:modelValue", this.options[0].value);
            }
        }
    });

    component("m-text-line", {
        name: "mTextLine",
        mixins: [fieldMixin, localValueMixin],
        props: { modelValue: [String, Number], type: { type: String, default: "text" }, id: String, name: String, size: String, fields: Object, label: String,
            tooltip: String, prefix: String, disable: Boolean, mask: String, fillMask: String, reverseFillMask: Boolean, rules: Array, required: Boolean,
            requiredMessage: { type: String, default: "Required" }, maxlength: String, form: String, autocomplete: { type: String, default: "off" },
            defaultUrl: String, defaultParameters: Object, dependsOn: Object, dependsOptional: Boolean, defaultLoadInit: Boolean },
        emits: ["update:modelValue"],
        data() { return { loading: false }; },
        template:
            '<q-input dense outlined stack-label hide-bottom-space no-error-icon :label="label" :prefix="prefix" :model-value="local" @update:model-value="onInput" :type="type"' +
                    ' :for="id" :name="name" :size="size" :maxlength="maxlength" :form="form" :loading="loading" :rules="allRules" :disable="disable" :bg-color="bg"' +
                    ' :mask="mask" :fill-mask="fillMask" :reverse-fill-mask="reverseFillMask" autocapitalize="off" :autocomplete="autocomplete" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<template v-if="$slots.append" v-slot:append><slot name="append"></slot></template>' +
            '</q-input>',
        computed: {
            allRules() {
                const rules = this.rules ? this.rules.slice() : [];
                if (this.required) rules.unshift((val) => (val !== null && val !== undefined && String(val).length > 0) || this.requiredMessage);
                return rules;
            }
        },
        methods: {
            populateFromUrl() {
                if (!this.defaultUrl || !this.defaultUrl.length) return;
                const { reqData, hasAllParms } = dependsValues(this, this.dependsOn, this.defaultParameters);
                if (!hasAllParms && !this.dependsOptional) return;
                this.loading = true;
                moqui.request(this.defaultUrl, { method: "POST", params: reqData, accept: "text/plain,application/json" }).then((result) => {
                    this.loading = false;
                    if (result.text && result.text.length) this.onInput(result.text);
                }, (err) => { this.loading = false; moqui.handleAjaxError(err); });
            }
        },
        mounted() {
            if (!this.defaultUrl || !this.defaultUrl.length) return;
            if (this.fields) Object.keys(this.dependsOn || {}).forEach((doParm) => { this.$watch(() => this.fields[this.dependsOn[doParm]], () => this.populateFromUrl()); });
            if (this.defaultLoadInit) this.populateFromUrl();
        }
    });

    // A text area. editor-type html is the Quasar editor (QEditor); md is a text area with a preview drawn by Marked (and
    // sanitized by DOMPurify), both lazy. The html editor simplifies markup it does not understand. rich and rich-light are the
    // Tiptap editor m-rich-text (full and light), its mentions over mention-url and its record links over record-url.
    component("m-text-area", {
        name: "mTextArea",
        mixins: [fieldMixin, localValueMixin],
        props: { modelValue: String, name: String, id: String, label: String, editorType: String, rows: { type: String, default: "3" }, cols: String,
            readonly: Boolean, autogrow: Boolean, maxlength: String, disable: Boolean, form: String, tooltip: String, mentionUrl: String,
            mentionParameters: Object, recordUrl: String, recordParameters: Object },
        emits: ["update:modelValue"],
        data() {
            return { preview: false, previewHtml: "", toolbar: [["bold", "italic", "strike", "underline"], ["unordered", "ordered", "outdent", "indent"],
                ["quote", "link"], ["left", "center", "right", "justify"], ["undo", "redo"], ["viewsource"]] };
        },
        template:
            '<m-rich-text v-if="editorType === \'rich\' || editorType === \'rich-light\'" :light="editorType === \'rich-light\'" :model-value="local"' +
                ' @update:model-value="onInput" :name="name" :id="id" :label="label" :form="form" :disable="disable" :readonly="readonly" :tooltip="tooltip"' +
                ' :rows="rows" :mention-url="mentionUrl" :mention-parameters="mentionParameters" :record-url="recordUrl" :record-parameters="recordParameters"></m-rich-text>' +
            '<div v-else-if="editorType === \'html\'" class="m-text-area-editor">' +
                '<div v-if="label" class="text-caption text-grey-7">{{label}}</div>' +
                '<q-editor :model-value="local" @update:model-value="onInput" :toolbar="toolbar" dense min-height="12rem" :readonly="readonly" :disable="disable"></q-editor>' +
                '<input type="hidden" :name="name" :value="local" :form="form">' +
            '</div>' +
            '<div v-else-if="editorType === \'md\'" class="m-text-area-editor">' +
                '<q-input type="textarea" dense outlined stack-label hide-bottom-space :label="label" :model-value="local" @update:model-value="onInput" :name="name" :for="id" :rows="rows"' +
                    ' :readonly="readonly" :disable="disable" :form="form" :bg-color="bg" autogrow v-bind="errorAttrs">' +
                    '<template v-slot:after><q-btn flat round dense size="sm" :icon="preview ? \'edit\' : \'visibility\'" @click="togglePreview"><q-tooltip>Preview</q-tooltip></q-btn></template>' +
                '</q-input>' +
                '<div v-if="preview" class="q-pa-sm markdown-preview" v-html="previewHtml"></div>' +
            '</div>' +
            '<q-input v-else type="textarea" dense outlined stack-label hide-bottom-space no-error-icon :label="label" :model-value="local" @update:model-value="onInput" :name="name" :for="id" :rows="rows"' +
                ' :cols="cols" :readonly="readonly" :autogrow="autogrow" :maxlength="maxlength" :disable="disable" :form="form" :bg-color="bg" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
            '</q-input>',
        methods: {
            togglePreview() {
                this.preview = !this.preview;
                if (!this.preview) return;
                Promise.all([
                    moqui.loadScript("/libs/marked/marked.umd.min.js", null, () => !!root.marked),
                    moqui.loadScript("/libs/dompurify/purify.min.js", null, () => !!root.DOMPurify)
                ]).then(() => {
                    const html = root.marked.parse(this.local || "");
                    this.previewHtml = root.DOMPurify.sanitize(html);
                }, (err) => console.error(err));
            }
        }
    });

    component("m-check", {
        name: "mCheck",
        mixins: [fieldMixin, localValueMixin],
        props: { modelValue: [Array, String], name: String, id: String, label: String, options: { type: Array, default: () => [] }, disable: Boolean, form: String,
            tooltip: String, noWrapper: Boolean, trueValue: String, falseValue: String, singleLabel: String, toggle: Boolean },
        emits: ["update:modelValue"],
        template:
            '<q-toggle v-if="!options.length && toggle" dense size="xs" :model-value="local" @update:model-value="onInput" :true-value="trueValue" :false-value="falseValue"' +
                ' :label="singleLabel" :name="name" :id="id" :form="form" :disable="disable"></q-toggle>' +
            '<q-checkbox v-else-if="!options.length" dense size="xs" :model-value="local" @update:model-value="onInput" :true-value="trueValue" :false-value="falseValue"' +
                ' :label="singleLabel" :name="name" :id="id" :form="form" :disable="disable"></q-checkbox>' +
            '<m-field v-else-if="!noWrapper" borderless class="m-field-group" :label="label" :bg-color="bg" :disable="disable" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<q-option-group type="checkbox" inline dense size="sm" :options="options" :model-value="local" @update:model-value="onInput" :name="name" :id="id" :disable="disable"></q-option-group>' +
            '</m-field>' +
            '<q-option-group v-else type="checkbox" inline dense size="sm" :options="options" :model-value="local" @update:model-value="onInput" :name="name" :id="id" :disable="disable"></q-option-group>'
    });

    component("m-radio", {
        name: "mRadio",
        mixins: [fieldMixin, localValueMixin],
        props: { modelValue: String, name: String, id: String, label: String, options: { type: Array, default: () => [] }, disable: Boolean, form: String, tooltip: String },
        emits: ["update:modelValue"],
        template:
            '<m-field borderless class="m-field-group" :label="label" :bg-color="bg" :disable="disable" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<q-option-group type="radio" inline dense size="sm" :options="options" :model-value="local" @update:model-value="onInput" :name="name" :id="id" :disable="disable"></q-option-group>' +
            '</m-field>'
    });

    component("m-file", {
        name: "mFile",
        mixins: [fieldMixin],
        props: { modelValue: { type: null }, name: String, id: String, label: String, size: String, multiple: Boolean, accept: String, maxlength: String,
            disable: Boolean, form: String, tooltip: String },
        emits: ["update:modelValue"],
        template:
            '<q-file dense outlined stack-label hide-bottom-space no-error-icon :use-chips="multiple" :counter="multiple" :label="label" :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :name="name" :for="id"' +
                ' :size="size" :multiple="multiple" :accept="accept" :form="form" :disable="disable" :bg-color="bg" v-bind="errorAttrs">' +
                '<template v-slot:append><q-icon name="attach_file" size="xs"></q-icon></template>' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
            '</q-file>'
    });

    component("m-range-find", {
        name: "mRangeFind",
        props: { fields: { type: Object, required: true }, name: { type: String, required: true }, id: String, label: String, size: { type: String, default: "10" },
            maxlength: String, form: String, tooltip: String },
        template:
            '<div class="row">' +
                '<m-text-line :label="label + \' From\'" :name="name + \'_from\'" :id="id + \'_from\'" :size="size" :maxlength="maxlength" :form="form" :tooltip="tooltip" v-model="fields[name + \'_from\']"></m-text-line>' +
                '<m-text-line class="q-pl-xs" :label="label + \' Thru\'" :name="name + \'_thru\'" :id="id + \'_thru\'" :size="size" :maxlength="maxlength" :form="form" :tooltip="tooltip" v-model="fields[name + \'_thru\']"></m-text-line>' +
            '</div>'
    });

    // A find field: the text, with the options of the search (not, operator, ignore case) in the after slot of the input.
    component("m-text-find", {
        name: "mTextFind",
        props: { fields: { type: Object, required: true }, name: { type: String, required: true }, id: String, label: String, size: { type: String, default: "30" },
            maxlength: String, form: String, tooltip: String, hideOptions: { type: String, default: "false" } },
        data() {
            return { operators: [{ value: "equals", label: "Equals" }, { value: "like", label: "Like" }, { value: "contains", label: "Contains" },
                { value: "begins", label: "Begins With" }, { value: "empty", label: "Empty" }] };
        },
        computed: { optionsActive() { const op = this.fields[this.name + "_op"]; return this.fields[this.name + "_not"] === "Y" || (!!op && op !== "contains"); } },
        template:
            '<m-text-line :label="label" :name="name" :id="id" :size="size" :maxlength="maxlength" :form="form" :tooltip="tooltip" v-model="fields[name]">' +
                '<template v-if="hideOptions !== \'true\'" v-slot:append>' +
                    '<q-btn flat round dense size="sm" icon="tune" :color="optionsActive ? \'primary\' : \'grey-6\'">' +
                        '<q-tooltip>Find Options</q-tooltip>' +
                        '<q-menu class="m-text-find-options q-pa-sm"><div class="column q-gutter-y-xs">' +
                            '<q-select v-if="hideOptions !== \'operator\'" dense outlined options-dense emit-value map-options stack-label hide-bottom-space label="Operator" :name="name + \'_op\'" v-model="fields[name + \'_op\']" :options="operators" :form="form" behavior="menu"></q-select>' +
                            '<q-toggle v-if="hideOptions !== \'operator\'" dense size="sm" :name="name + \'_not\'" label="Not" true-value="Y" false-value="N" v-model="fields[name + \'_not\']" :form="form"></q-toggle>' +
                            '<q-toggle v-if="hideOptions !== \'ignore-case\'" dense size="sm" :name="name + \'_ic\'" label="Ignore Case" true-value="Y" false-value="N" v-model="fields[name + \'_ic\']" :form="form"></q-toggle>' +
                        '</div></q-menu>' +
                    '</q-btn>' +
                '</template>' +
            '</m-text-line>'
    });

    /* ========== lazy libraries ========== */

    component("m-chart", {
        name: "mChart",
        props: { config: { type: Object, required: true }, height: { type: String, default: "400px" }, width: { type: String, default: "100%" } },
        template: '<div class="chart-container" style="position:relative;" :style="{height:height,width:width}"><canvas ref="canvas"></canvas></div>',
        mounted() {
            moqui.loadScript("/libs/chartjs/chart.umd.min.js", null, () => !!root.Chart).then(() => {
                // the chart keeps its own copy of the config, a reactive object would be walked by Chart.js
                this.instance = new root.Chart(this.$refs.canvas, moqui.deepCopy(this.config));
            }, (err) => console.error("Error loading m-chart script: " + err));
        },
        watch: {
            config: {
                deep: true,
                handler(val) {
                    if (!this.instance) return;
                    if (val.type) this.instance.config.type = val.type;
                    if (val.data) this.instance.data = moqui.deepCopy(val.data);
                    if (val.options) this.instance.options = moqui.deepCopy(val.options);
                    this.instance.update();
                }
            }
        },
        beforeUnmount() { if (this.instance) { this.instance.destroy(); this.instance = null; } }
    });

    // The text of the element is the diagram. Mermaid 11 draws it with mermaid.run into an element that is not Vue's.
    component("m-mermaid", {
        name: "mMermaid",
        // The diagram comes in the text prop: the Vue compiler condenses the whitespace of slot content, and a diagram
        // needs its line breaks. The slot is kept for a single-line diagram or one written with explicit newlines.
        props: { text: String, config: { type: Object, default: () => ({ startOnLoad: false, securityLevel: "strict" }) }, height: { type: String, default: "400px" }, width: { type: String, default: "100%" } },
        template: '<div class="mermaid-container" :style="{height:height,width:width}"><pre ref="source" style="display:none;"><slot></slot></pre><div ref="target"></div></div>',
        mounted() {
            moqui.loadScript("/libs/mermaid/mermaid.min.js", null, () => !!root.mermaid).then(() => {
                const diagram = root.document.createElement("pre");
                diagram.className = "mermaid";
                diagram.textContent = (this.text || this.$refs.source.textContent).trim();
                this.$refs.target.appendChild(diagram);
                root.mermaid.initialize(Object.assign({ startOnLoad: false, securityLevel: "strict" }, this.config));
                return root.mermaid.run({ nodes: [diagram] });
            }).catch((err) => console.error("m-mermaid: " + err));
        }
    });

    /* ========== rich text: m-rich-text (Tiptap, lazy) ========== */

    // The pure part of m-rich-text, tested under Node (WebrootVue.qvt.records.test.js): the HTML of a mention and a record link
    // and the ones in a text, the editor's empty value, plain text from its HTML and HTML from plain text (the plain-text toggle),
    // and the answer of a mention or record search. The editor writes only what allow-html="safe" keeps (js/README.md, Rich text):
    // p, h1-h3, strong, em, s, u, code, pre, blockquote, ul, ol (start), li, br, hr, a (href, title), img (src, alt, title),
    // table, thead, tbody, tr, th, td (colspan, rowspan), the mention <span data-mention-id="id">@Label</span> and the record
    // <span data-record-entity="entity" data-record-id="id">#Label</span>.
    const htmlEscape = (text) => String(text === null || text === undefined ? "" : text).replace(/[&<>"']/g,
        (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[ch]);
    const htmlEntities = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: "\u00a0" };
    const htmlDecode = (text) => String(text || "").replace(/&(#x[0-9a-f]+|#[0-9]+|[a-z]+);/gi, (whole, code) => {
        if (code[0] === "#") {
            const point = code[1] === "x" || code[1] === "X" ? parseInt(code.slice(2), 16) : parseInt(code.slice(1), 10);
            return isNaN(point) ? whole : String.fromCodePoint(point);
        }
        const named = htmlEntities[code.toLowerCase()];
        return named === undefined ? whole : named;
    });
    const tagAttr = (tag, name) => {
        const match = new RegExp("\\s" + name + "\\s*=\\s*(\"([^\"]*)\"|'([^']*)'|([^\\s>]+))", "i").exec(tag);
        return match ? htmlDecode(match[2] !== undefined ? match[2] : match[3] !== undefined ? match[3] : match[4]) : null;
    };
    moqui.richText = {
        // a mention as the editor writes it: the id in data-mention-id, the label shown after the @
        mentionHtml(id, label) {
            return '<span data-mention-id="' + htmlEscape(id) + '">@' + htmlEscape(label || id) + "</span>";
        },
        // the mentions in an HTML text, each id once, in order: [{id, label}]
        mentions(html) {
            const out = [];
            const seen = {};
            const re = /<span\b([^>]*)>([\s\S]*?)<\/span>/gi;
            let match;
            while ((match = re.exec(String(html || "")))) {
                const id = tagAttr(match[1], "data-mention-id");
                if (id === null || id === "" || seen[id]) continue;
                seen[id] = true;
                out.push({ id, label: htmlDecode(match[2].replace(/<[^>]*>/g, "")).replace(/^@/, "") });
            }
            return out;
        },
        // a record link as the editor writes it: the entity and id in data attributes, the label shown after the #
        recordHtml(entity, id, label) {
            return '<span data-record-entity="' + htmlEscape(entity) + '" data-record-id="' + htmlEscape(id) + '">#' + htmlEscape(label || id) + "</span>";
        },
        // the record links in an HTML text, each id once, in order: [{entity, id, label}]; the span text already has the #
        records(html) {
            const out = [];
            const seen = {};
            const re = /<span\b([^>]*)>([\s\S]*?)<\/span>/gi;
            let match;
            while ((match = re.exec(String(html || "")))) {
                const id = tagAttr(match[1], "data-record-id");
                if (id === null || id === "" || seen[id]) continue;
                seen[id] = true;
                out.push({ entity: tagAttr(match[1], "data-record-entity") || "", id, label: htmlDecode(match[2].replace(/<[^>]*>/g, "")).replace(/^#/, "") });
            }
            return out;
        },
        // nothing but empty paragraphs, breaks and spaces (Tiptap's empty document is <p></p>)
        isEmpty(html) {
            const text = String(html || "");
            if (/<(img|hr|table)\b/i.test(text) || /data-mention-id|data-record-id/i.test(text)) return false;
            return htmlDecode(text.replace(/<[^>]*>/g, "")).replace(/[\s\u00a0]+/g, "") === "";
        },
        // the value of the field for the editor's HTML: empty for an empty document, so a required check and the change tint see it
        value(html) { return moqui.richText.isEmpty(html) ? "" : String(html); },
        // what the editor opens a value as: HTML as it is, and text with no markup (a note written before, a plain mail part) as
        // its paragraphs
        toHtml(value) {
            const text = String(value === null || value === undefined ? "" : value);
            return /<[a-zA-Z!/]/.test(text) ? text : moqui.richText.fromPlain(text);
        },
        // Plain text of the editor's HTML (the plain-text toggle, a plain mail part): paragraphs and blocks apart by a blank
        // line, list items on lines of their own with "- " or their number, a quote's lines after "> ", a link as its text and
        // (its address) when they differ, an image as its alt text, a mention as @Label, a record link as #Label (the span
        // text already has the hash), a table as cells joined by " | " with a dashed underline under a header row, entities decoded.
        toPlain(html) {
            const out = [];
            const lists = [];
            let quote = 0;
            let pre = 0;
            let breaks = 0;
            let lineStart = true;
            let prefix = "";
            const links = [];
            let table = null;
            const owe = (count) => { if (out.length) breaks = Math.max(breaks, count); };
            const put = (text) => {
                if (!text) return;
                if (breaks) { out.push("\n".repeat(breaks)); breaks = 0; lineStart = true; }
                if (lineStart) {
                    if (!pre) text = text.replace(/^ +/, "");
                    if (!text) return;
                    out.push("> ".repeat(quote) + prefix);
                    prefix = "";
                    lineStart = false;
                }
                out.push(text);
            };
            const tokens = String(html || "").match(/<!--[\s\S]*?-->|<\/?[a-zA-Z][^>]*>|[^<]+|</g) || [];
            tokens.forEach((token) => {
                if (token.startsWith("<!--")) return;
                const tagMatch = /^<(\/?)([a-zA-Z0-9]+)/.exec(token);
                if (!tagMatch) {
                    if (table) {
                        if (table.inCell) table.cell += htmlDecode(token).replace(/[\s\u00a0]+/g, " ");
                        return;
                    }
                    let text = htmlDecode(token);
                    if (!pre) text = text.replace(/[\s\u00a0]+/g, (space) => (space.indexOf("\u00a0") >= 0 ? "\u00a0" : " "));
                    if (pre) {
                        text.split("\n").forEach((line, index) => { if (index) { owe(1); breaks = Math.max(breaks, 1); } put(line); });
                    } else put(text);
                    return;
                }
                const closing = tagMatch[1] === "/";
                const tag = tagMatch[2].toLowerCase();
                if (tag === "table") {
                    if (!closing && !table) {
                        owe(2);
                        table = { rows: [], cells: null, cell: "", inCell: false, header: false };
                    } else if (closing && table) {
                        if (table.inCell && table.cells) table.cells.push(table.cell.replace(/[\s\u00a0]+/g, " ").trim());
                        if (table.cells) table.rows.push({ cells: table.cells, header: table.header });
                        const lines = [];
                        table.rows.forEach((row) => {
                            lines.push(row.cells.join(" | "));
                            if (row.header) lines.push(row.cells.map((cell) => "-".repeat(Math.max(3, cell.length))).join(" | "));
                        });
                        lines.forEach((line, index) => { if (index) owe(1); put(line); });
                        owe(2);
                        table = null;
                    }
                    return;
                }
                if (table) {
                    if (tag === "tr") {
                        if (!closing) { table.cells = []; table.header = false; table.inCell = false; table.cell = ""; }
                        else if (table.cells) {
                            if (table.inCell) { table.cells.push(table.cell.replace(/[\s\u00a0]+/g, " ").trim()); table.inCell = false; }
                            table.rows.push({ cells: table.cells, header: table.header });
                            table.cells = null;
                        }
                    } else if (tag === "td" || tag === "th") {
                        if (!closing) { table.inCell = true; table.cell = ""; if (tag === "th") table.header = true; }
                        else if (table.inCell && table.cells) {
                            table.cells.push(table.cell.replace(/[\s\u00a0]+/g, " ").trim());
                            table.inCell = false;
                            table.cell = "";
                        }
                    } else if (tag === "br" && table.inCell) table.cell += " ";
                    return;
                }
                const inList = lists.length > 0;
                if (/^(p|div|h[1-6])$/.test(tag)) owe(inList ? 1 : 2);
                else if (tag === "br") { if (out.length) breaks = Math.max(breaks, 1); }
                else if (tag === "blockquote") { owe(2); quote += closing ? -1 : 1; if (quote < 0) quote = 0; }
                else if (tag === "pre") { owe(2); pre += closing ? -1 : 1; if (pre < 0) pre = 0; }
                else if (tag === "ul" || tag === "ol") {
                    if (closing) lists.pop();
                    else lists.push(tag === "ol" ? (parseInt(tagAttr(token, "start"), 10) || 1) - 1 : null);
                    owe(lists.length ? 1 : 2);
                } else if (tag === "li" && !closing) {
                    owe(1);
                    const depth = Math.max(lists.length - 1, 0);
                    const top = lists.length - 1;
                    let marker = "- ";
                    if (top >= 0 && lists[top] !== null) { lists[top] += 1; marker = lists[top] + ". "; }
                    prefix = "  ".repeat(depth) + marker;
                    if (!out.length) lineStart = true;
                } else if (tag === "hr" && !closing) { owe(2); put("---"); owe(2); }
                else if (tag === "img" && !closing) put(tagAttr(token, "alt") || "");
                else if (tag === "a") {
                    if (!closing) links.push({ href: tagAttr(token, "href"), at: out.length });
                    else {
                        const link = links.pop();
                        if (link && link.href) {
                            const text = out.slice(link.at).join("").trim();
                            if (text !== link.href && text !== link.href.replace(/^mailto:/, "")) put(" (" + link.href + ")");
                        }
                    }
                }
            });
            return out.join("").replace(/[ \t]+$/gm, "").replace(/^\n+|\s+$/g, "");
        },
        // HTML of plain text: a paragraph for each run of lines, a <br> inside one, everything escaped; empty text is ""
        fromPlain(text) {
            const value = String(text || "").replace(/\r\n?/g, "\n").trim();
            if (!value) return "";
            return value.split(/\n[ \t]*\n+/).map((para) => "<p>" + para.split("\n").map(htmlEscape).join("<br>") + "</p>").join("");
        },
        // The plain-text toggle's way back. When plain is toPlain(originalHtml), the original comes back whole (formatting,
        // images, chips, tables). An edit is fromPlain, then each mention, record link and table of the original, in order,
        // replaces the first remaining copy of its own plain text. A second chip with the same label takes the next copy.
        // A piece whose text was deleted stays gone. Bold, lists, links and images follow fromPlain once the text changed.
        fromPlainKeeping(plain, originalHtml) {
            const original = String(originalHtml || "");
            const normalized = String(plain || "").replace(/\r\n?/g, "\n").replace(/[ \t]+$/gm, "").replace(/^\n+|\s+$/g, "");
            if (normalized === moqui.richText.toPlain(original)) return original;
            let html = moqui.richText.fromPlain(normalized);
            const pieces = [];
            const re = /<table\b[^>]*>[\s\S]*?<\/table>|<span\b[^>]*\bdata-mention-id\b[^>]*>[\s\S]*?<\/span>|<span\b[^>]*\bdata-record-id\b[^>]*>[\s\S]*?<\/span>/gi;
            let found;
            while ((found = re.exec(original))) pieces.push(found[0]);
            let from = 0;
            pieces.forEach((fragment) => {
                const token = moqui.richText.toPlain(fragment);
                if (!token) return;
                const needle = token.split("\n").map(htmlEscape).join("<br>");
                const wrapped = "<p>" + needle + "</p>";
                let at = html.indexOf(wrapped, from);
                let length = wrapped.length;
                if (at < 0) { at = html.indexOf(needle, from); length = needle.length; }
                if (at < 0) return;
                html = html.slice(0, at) + fragment + html.slice(at + length);
                from = at + fragment.length;
            });
            return html;
        },
        // The rows of a mention or record search answer: a list (or a map with it in options, optionList, mentions or rows) of
        // {id, label}, value and title or name also read, a plain string as both. A record row prefers recordId, and keeps
        // entity (or recordEntityName) when the row has one. At most limit (10), each id once.
        mentionItems(answer, limit) {
            const list = Array.isArray(answer) ? answer : moqui.isPlainObject(answer) ? (answer.options || answer.optionList || answer.mentions || answer.rows || []) : [];
            const out = [];
            const seen = {};
            list.forEach((row) => {
                if (out.length >= (limit || 10)) return;
                const item = moqui.isPlainObject(row) ? row : { id: row, label: row };
                const recordId = item.recordId;
                const id = recordId !== undefined && recordId !== null && recordId !== "" ? recordId : (item.id !== undefined && item.id !== null ? item.id : item.value);
                if (id === undefined || id === null || id === "" || seen[id]) return;
                seen[id] = true;
                const label = item.label || item.title || item.name || id;
                const found = { id: String(id), label: String(label), image: item.image || null };
                const entity = item.entity || item.recordEntityName;
                if (entity) found.entity = String(entity);
                out.push(found);
            });
            return out;
        },
        // A link a person typed: an address with a scheme as it is, an email address as mailto:, a bare host as https://
        linkHref(text) {
            const value = String(text || "").trim();
            if (!value) return "";
            if (/^[a-z][a-z0-9+.-]*:/i.test(value) || value.startsWith("/") || value.startsWith("#")) return value;
            if (/^[^\s@/]+@[^\s@/]+\.[^\s@/]+$/.test(value)) return "mailto:" + value;
            return "https://" + value;
        },
        // the buttons of the toolbar: light is the compact one of notes and chat
        toolbar(light, plainToggle) {
            const button = (key, icon, label) => ({ key, icon, label });
            const sep = (key) => ({ key, separator: true });
            if (light) {
                return [button("bold", "sym_o_format_bold", "Bold"), button("italic", "sym_o_format_italic", "Italic"), button("link", "sym_o_link", "Link"),
                    button("bulletList", "sym_o_format_list_bulleted", "Bulleted list"), button("code", "sym_o_code", "Code")];
            }
            const list = [button("block", "sym_o_title", "Paragraph style"), sep("s1"),
                button("bold", "sym_o_format_bold", "Bold"), button("italic", "sym_o_format_italic", "Italic"), button("strike", "sym_o_strikethrough_s", "Strikethrough"), sep("s2"),
                button("bulletList", "sym_o_format_list_bulleted", "Bulleted list"), button("orderedList", "sym_o_format_list_numbered", "Numbered list"), sep("s3"),
                button("link", "sym_o_link", "Link"), button("image", "sym_o_image", "Image"), button("table", "sym_o_table", "Table"), button("code", "sym_o_code", "Code"),
                button("codeBlock", "sym_o_code_blocks", "Code block"), button("blockquote", "sym_o_format_quote", "Quote"), sep("s4"),
                button("undo", "sym_o_undo", "Undo"), button("redo", "sym_o_redo", "Redo")];
            if (plainToggle) list.push(sep("s5"), button("plain", "sym_o_text_fields", "Plain text"));
            return list;
        }
    };

    // The editor's extensions: StarterKit (paragraphs, headings 1 to 3, bold, italic, strike, underline, code, code blocks,
    // quotes, lists, rules, breaks, history), Link (no target, class or rel of its own: the safe cleaning adds rel), Image by
    // URL (no data: URLs), the placeholder, a table (full editor only), the mention when the editor has a mention URL, the
    // record link when it has a record URL, and the keys of the component. With collab (m-coedit) the document is the Yjs
    // fragment "default" and the history is Yjs's (undo and redo of this editor's own changes), with the carets of the
    // others; StarterKit's history and trailing paragraph are off (each editor adding that paragraph would add one each).
    const richTextExtensions = (T, vm) => {
        const starter = { heading: { levels: [1, 2, 3] }, link: false, codeBlock: { HTMLAttributes: { class: null } } };
        if (vm.collab) Object.assign(starter, { undoRedo: false, trailingNode: false });
        const list = [
            T.StarterKit.configure(starter),
            T.Link.configure({ openOnClick: false, autolink: true, linkOnPaste: true, defaultProtocol: "https",
                HTMLAttributes: { target: null, rel: null, class: null } }),
            T.Image.configure({ inline: false, allowBase64: false }),
            T.Placeholder.configure({ placeholder: () => vm.placeholder || "" }),
            T.Extension.create({
                name: "mRichTextKeys",
                priority: 1000,
                addKeyboardShortcuts() {
                    return {
                        "Mod-Enter": () => { vm.submit(); return true; },
                        "Mod-k": () => { vm.ask("link"); return true; },
                        Enter: () => {
                            if (vm.enter !== "submit" || vm.mention.open) return false;
                            vm.submit();
                            return true;
                        }
                    };
                }
            })
        ];
        if (!vm.light && T.Table && T.TableRow && T.TableCell && T.TableHeader) {
            list.push(T.Table.configure({ resizable: false }), T.TableRow, T.TableHeader, T.TableCell);
        }
        if (vm.collab) {
            list.push(T.Collaboration.configure({ document: vm.collab.doc, field: "default" }),
                T.CollaborationCaret.configure({ provider: { awareness: vm.collab.awareness }, user: vm.collab.user }));
        }
        if (vm.mentionUrl) {
            const Mention = T.Mention.extend({
                // the mention in the HTML: <span data-mention-id="id">@Label</span>, and nothing the safe cleaning drops
                addAttributes() {
                    return {
                        id: { default: null, parseHTML: (el) => el.getAttribute("data-mention-id"), renderHTML: (attrs) => ({ "data-mention-id": attrs.id }) },
                        label: { default: null, parseHTML: (el) => (el.textContent || "").replace(/^@/, ""), renderHTML: () => ({}) },
                        mentionSuggestionChar: { default: "@", parseHTML: () => "@", renderHTML: () => ({}) }
                    };
                },
                parseHTML() { return [{ tag: "span[data-mention-id]" }]; },
                renderHTML({ node }) { return ["span", { "data-mention-id": node.attrs.id }, "@" + (node.attrs.label || node.attrs.id || "")]; }
            });
            list.push(Mention.configure({
                suggestion: { char: "@", debounce: 150, items: ({ query, signal }) => vm.searchMentions(query, signal), render: () => vm.mentionRenderer() }
            }));
        }
        if (vm.recordUrl) {
            const RecordMention = T.Mention.extend({
                name: "recordMention",
                // a record link, not a person: <span data-record-entity="entity" data-record-id="id">#Label</span>
                addAttributes() {
                    return {
                        id: { default: null, parseHTML: (el) => el.getAttribute("data-record-id"), renderHTML: () => ({}) },
                        entity: { default: null, parseHTML: (el) => el.getAttribute("data-record-entity"), renderHTML: () => ({}) },
                        label: { default: null, parseHTML: (el) => (el.textContent || "").replace(/^#/, ""), renderHTML: () => ({}) },
                        mentionSuggestionChar: { default: "#", parseHTML: () => "#", renderHTML: () => ({}) }
                    };
                },
                parseHTML() { return [{ tag: "span[data-record-id]" }]; },
                renderHTML({ node }) {
                    const attrs = { "data-record-id": node.attrs.id };
                    if (node.attrs.entity) attrs["data-record-entity"] = node.attrs.entity;
                    return ["span", attrs, "#" + (node.attrs.label || node.attrs.id || "")];
                }
            });
            list.push(RecordMention.configure({
                suggestion: {
                    char: "#",
                    pluginKey: T.PluginKey ? new T.PluginKey("recordSuggestion") : undefined,
                    debounce: 150,
                    items: ({ query, signal }) => vm.searchRecords(query, signal),
                    render: () => vm.mentionRenderer(),
                    command: ({ editor, range, props }) => {
                        editor.chain().focus().insertContentAt(range, [
                            { type: "recordMention", attrs: { id: props.id, label: props.label, entity: props.entity || null } },
                            { type: "text", text: " " }
                        ]).run();
                    }
                }
            }));
        }
        return list;
    };

    // A rich-text editor with the m-* model contract (v-model or model-value, a name, the change tint and the form's field
    // errors inside an m-form, a hidden input for a form that reads the DOM): Tiptap from /libs/tiptap, loaded when the first
    // editor mounts. The value is HTML ("" when empty). light is the compact toolbar; enter="submit" makes Enter submit the
    // form (and emit submit) with Shift+Enter a line break; Ctrl+Enter always submits; mention-url is searched with term for
    // the @ list and record-url for the # list (only when that URL is set, on light and full). The plain-text toggle
    // (v-model:plain, a boolean) shows toPlain in the textarea and keeps the HTML in the model (fromPlainKeeping on each
    // edit, so a save while the toggle is on still stores HTML). Paste, drop and the full toolbar's file button upload an
    // image to /richTextImage and insert the returned address; a data: image is uploaded the same way. When the library
    // does not load, the field is the html editor of m-text-area, with a control that tries the editor again; a later
    // editor that does load starts the ones still waiting. collab ({doc, awareness, user: {name, color}}, from m-coedit)
    // makes the document a shared Yjs one: no content of its own, no model, no plain-text toggle, and a failure emits
    // failed (for m-coedit's host to show its own field) instead of the html editor.
    let richTextCount = 0;
    // one load of the library for the page; a failure clears it so the next try starts again
    let tiptapLoad = null;
    const loadTiptap = () => tiptapLoad || (tiptapLoad = moqui.loadScript("/libs/tiptap/tiptap.min.js", null, () => !!root.Tiptap)
        .catch((error) => { tiptapLoad = null; throw error; }));
    // editors whose load or start failed, so a later success (or Try the editor again) can start them
    const failedRichTexts = [];
    const dropFailed = (vm) => {
        const index = failedRichTexts.indexOf(vm);
        if (index >= 0) failedRichTexts.splice(index, 1);
    };
    const rememberFailed = (vm) => { if (failedRichTexts.indexOf(vm) < 0) failedRichTexts.push(vm); };
    const wakeFailed = () => {
        failedRichTexts.slice().forEach((other) => {
            if (!other || other.unmountedNow || other.state !== "failed" || !root.Tiptap) return;
            other.state = "loading";
            other.$nextTick(() => {
                if (other.unmountedNow || other.state !== "loading" || !root.Tiptap) return;
                try {
                    other.start();
                    dropFailed(other);
                } catch (err) {
                    other.startFailed("the editor did not start, the html editor stands in: " + err);
                }
            });
        });
    };
    component("m-rich-text", {
        name: "mRichText",
        mixins: [fieldMixin, localValueMixin],
        props: { modelValue: String, name: String, id: String, label: String, form: String, disable: Boolean, readonly: Boolean, tooltip: String,
            light: Boolean, placeholder: String, rows: [String, Number], enter: { type: String, default: "newline" }, mentionUrl: String,
            mentionParameters: Object, recordUrl: String, recordParameters: Object, plain: Boolean, plainToggle: { type: Boolean, default: true }, autofocus: Boolean,
            collab: Object },
        emits: ["update:modelValue", "update:plain", "submit", "mention", "failed"],
        data() {
            richTextCount += 1;
            return { state: "loading", isPlain: this.plain, focused: false, plainFocused: false, htmlBeforePlain: "", plainText: "",
                active: {}, blockLabel: "Paragraph", asking: null, askValue: "", proposal: null,
                mention: { open: false, items: [], index: 0, loading: false }, uid: "m-rich-text-" + richTextCount };
        },
        template:
            '<div class="m-rich-text" :class="{\'m-rich-text-light\': light, \'m-rich-text-focused\': focused}" :data-state="state">' +
                '<div v-if="state === \'failed\' && !collab">' +
                    '<div class="row items-center q-mb-xs"><q-btn dense flat no-caps icon="sym_o_refresh" :label="t(\'Try the editor again\')" @click="retryEditor"></q-btn></div>' +
                    '<m-text-area editor-type="html" :model-value="local" @update:model-value="onInput" :name="name" :id="id"' +
                        ' :label="label" :form="form" :disable="disable" :readonly="readonly" :tooltip="tooltip"></m-text-area>' +
                '</div>' +
                '<template v-else>' +
                    '<div v-if="label" class="text-caption m-muted m-rich-text-label">{{label}}</div>' +
                    '<div class="m-rich-text-box" :class="[bg ? \'bg-\' + bg : \'\', errorAttrs.error ? \'m-rich-text-error\' : \'\']">' +
                        '<div v-if="!readonly" class="m-rich-text-toolbar row items-center">' +
                            '<template v-for="button in buttons" :key="button.key">' +
                                '<q-separator v-if="button.separator" vertical class="q-mx-xs"></q-separator>' +
                                '<q-btn-dropdown v-else-if="button.key === \'block\'" flat dense no-caps size="sm" :label="t(blockLabel)" :disable="!editable || isPlain"' +
                                        ' class="m-rich-text-block" content-class="m-rich-text-menu">' +
                                    '<q-list dense>' +
                                        '<q-item v-for="block in blocks" :key="block.key" clickable v-close-popup @click="run(block.key)" :active="blockLabel === block.label">' +
                                            '<q-item-section>{{t(block.label)}}</q-item-section></q-item>' +
                                    '</q-list>' +
                                '</q-btn-dropdown>' +
                                '<q-btn-dropdown v-else-if="button.key === \'table\'" flat dense size="sm" icon="sym_o_table" :disable="!editable || isPlain"' +
                                        ' class="m-rich-text-table" content-class="m-rich-text-menu" :aria-label="t(\'Table\')" @mousedown.prevent>' +
                                    '<q-list dense>' +
                                        '<q-item v-for="item in tableItems" :key="item.key" clickable v-close-popup @click="run(item.key)">' +
                                            '<q-item-section>{{t(item.label)}}</q-item-section></q-item>' +
                                    '</q-list>' +
                                '</q-btn-dropdown>' +
                                '<q-btn v-else flat dense size="sm" :icon="button.icon" :data-command="button.key" :aria-label="t(button.label)"' +
                                        ' :color="(button.key === \'plain\' ? isPlain : active[button.key]) ? \'primary\' : undefined"' +
                                        ' :class="{\'m-rich-text-on\': button.key === \'plain\' ? isPlain : active[button.key]}"' +
                                        ' :disable="button.key === \'plain\' ? (disable || state !== \'ready\') : (!editable || isPlain)" @mousedown.prevent @click="run(button.key)">' +
                                    '<q-tooltip>{{t(button.label)}}</q-tooltip></q-btn>' +
                            '</template>' +
                            // Assist's text actions (moqui.assist, js/WebrootAssist.js): a proposal below, applied only by a click
                            '<q-btn-dropdown v-if="textActions.length" flat dense size="sm" icon="sym_o_auto_awesome" :disable="!canEdit || (state !== \'ready\' && !isPlain)"' +
                                    ' class="m-rich-text-assist" content-class="m-rich-text-menu" data-assist-text-actions :aria-label="assistLabel" @mousedown.prevent>' +
                                '<q-list dense><q-item v-for="item in textActions" :key="item.key" clickable v-close-popup :data-assist-text-action="item.key" @click="textAction(item)">' +
                                    '<q-item-section>{{item.label}}</q-item-section></q-item></q-list>' +
                            '</q-btn-dropdown>' +
                        '</div>' +
                        '<input ref="imageFile" type="file" accept="image/png,image/gif,image/jpeg,image/webp" style="display:none" @change="onImageFile">' +
                        '<div v-if="asking" class="m-rich-text-ask row items-center no-wrap q-px-sm q-py-xs q-gutter-x-xs">' +
                            '<q-input ref="askInput" dense outlined hide-bottom-space class="col" v-model="askValue" :placeholder="asking === \'image\' ? \'https://\u2026/picture.png\' : \'https://\u2026\'"' +
                                ' :aria-label="t(asking === \'image\' ? \'Image address\' : \'Link address\')" @keydown.enter.prevent="applyAsk" @keydown.esc.prevent.stop="asking = null"></q-input>' +
                            '<q-btn v-if="asking === \'image\'" dense flat no-caps icon="sym_o_upload" :label="t(\'File\')" @click="pickImage"></q-btn>' +
                            '<q-btn dense unelevated no-caps class="m-btn-tonal" :label="t(\'Apply\')" @click="applyAsk"></q-btn>' +
                            '<q-btn v-if="asking === \'link\' && active.link" dense flat no-caps :label="t(\'Remove\')" @click="removeLink"></q-btn>' +
                            '<q-btn dense flat round size="sm" icon="sym_o_close" :aria-label="t(\'Close\')" @click="asking = null"></q-btn>' +
                        '</div>' +
                        '<div v-if="proposal" class="m-rich-text-proposal q-px-sm q-py-xs" data-assist-text-proposal :data-state="proposal.state" :data-action="proposal.key">' +
                            '<div v-if="proposal.state === \'loading\'" class="row items-center text-caption m-muted"><q-spinner-dots size="16px" class="q-mr-xs"></q-spinner-dots>{{proposal.label}}</div>' +
                            '<div v-else-if="proposal.state === \'failed\'" class="text-caption text-negative">{{proposal.error}}</div>' +
                            '<div v-else class="m-rich-text-proposal-text">{{proposal.text}}</div>' +
                            '<div class="row q-gutter-x-xs q-mt-xs">' +
                                '<template v-if="proposal.state === \'ready\'"><q-btn dense unelevated no-caps color="primary" :label="proposal.labels.replace" data-assist-text-apply="replace" @click="useProposal(\'replace\')"></q-btn>' +
                                    '<q-btn dense flat no-caps :label="proposal.labels.insertBelow" data-assist-text-apply="insert" @click="useProposal(\'insert\')"></q-btn></template>' +
                                '<q-btn dense flat no-caps :label="proposal.labels.discard" data-assist-text-apply="discard" @click="proposal = null"></q-btn>' +
                            '</div>' +
                        '</div>' +
                        '<textarea v-if="isPlain" ref="plainArea" :id="id ? id + \'_plain\' : null" class="m-rich-text-plain" :style="{minHeight: minHeight}" :value="plainText"' +
                            ' :readonly="readonly" :disabled="disable" @input="onPlainInput($event.target.value)" @keydown="plainKey"' +
                            ' @focus="plainFocused = true; focused = true" @blur="plainFocused = false; focused = false"></textarea>' +
                        '<div v-show="!isPlain" ref="content" class="m-rich-text-content" :style="{minHeight: minHeight}" @click="focusEnd"></div>' +
                        '<div v-if="state === \'loading\'" class="m-rich-text-loading m-muted text-caption q-px-sm">{{t(\'Loading the editor\')}}</div>' +
                    '</div>' +
                    '<div v-if="errorAttrs.error" class="text-negative text-caption q-px-sm">{{errorAttrs.errorMessage}}</div>' +
                    '<input type="hidden" :name="name" :value="local" :form="form">' +
                    '<teleport to="body">' +
                        '<div v-show="mention.open" ref="mentionMenu" class="m-rich-text-mentions" :id="uid + \'-mentions\'" role="listbox">' +
                            '<q-list dense>' +
                                '<q-item v-for="(item, index) in mention.items" :key="item.id" clickable :active="index === mention.index" role="option"' +
                                        ' :aria-selected="index === mention.index" @mousedown.prevent @click="pickMention(index)">' +
                                    '<q-item-section avatar class="m-rich-text-mention-avatar"><m-avatar :name="item.label" :image="item.image" size="22px"></m-avatar></q-item-section>' +
                                    '<q-item-section>{{item.label}}</q-item-section>' +
                                '</q-item>' +
                                '<q-item v-if="!mention.items.length"><q-item-section class="m-muted">{{t(mention.loading ? \'Searching\' : \'No matches\')}}</q-item-section></q-item>' +
                            '</q-list>' +
                        '</div>' +
                    '</teleport>' +
                '</template>' +
            '</div>',
        computed: {
            buttons() { return moqui.richText.toolbar(this.light, this.plainToggle && !this.light && !this.collab); },
            blocks() { return [{ key: "paragraph", label: "Paragraph" }, { key: "h1", label: "Heading 1" }, { key: "h2", label: "Heading 2" }, { key: "h3", label: "Heading 3" }]; },
            tableItems() { return [{ key: "insertTable", label: "Insert table" }, { key: "addRowAfter", label: "Add row" }, { key: "addColumnAfter", label: "Add column" },
                { key: "deleteRow", label: "Delete row" }, { key: "deleteColumn", label: "Delete column" }, { key: "deleteTable", label: "Delete table" }]; },
            canEdit() { return !this.disable && !this.readonly; },
            editable() { return this.state === "ready" && this.canEdit; },
            // Assist's text actions when the server offers them (bootstrap.features.textActions), never on a shared document
            textActions() { const assist = moqui.assist; return assist && assist.textActionsOn && !this.collab && assist.textActionsOn() ? assist.textActionList() : []; },
            assistLabel() { return moqui.assist ? moqui.assist.labels.textActions : "Assist"; },
            minHeight() {
                const rows = parseInt(this.rows, 10) || (this.light ? 2 : 6);
                return (Math.max(rows, this.light ? 1 : 2) * 1.5 + 0.6) + "em";
            }
        },
        watch: {
            modelValue(value) {
                if (this.collab) return;
                if (this.isPlain) {
                    // the textarea is the plain text; do not rewrite it from the model while it is focused
                    if (this.plainFocused) return;
                    const html = value || "";
                    const kept = moqui.richText.fromPlainKeeping(this.plainText, this.htmlBeforePlain);
                    if (html !== kept && html !== moqui.richText.value(kept)) {
                        this.htmlBeforePlain = html;
                        this.plainText = moqui.richText.toPlain(html);
                    }
                    return;
                }
                const editor = this.editor;
                if (!editor) return;
                if ((value || "") !== moqui.richText.value(editor.getHTML())) editor.commands.setContent(moqui.richText.toHtml(value), { emitUpdate: false });
            },
            plain(value) { if (value !== this.isPlain) this.togglePlain(); },
            canEdit(value) { if (this.editor) this.editor.setEditable(value); }
        },
        methods: {
            t(text) { return moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text; },
            start() {
                const T = root.Tiptap;
                if (this.editor) { this.editor.destroy(); this.editor = null; }
                const vm = this;
                this.editor = markRaw(new T.Editor({
                    element: this.$refs.content,
                    extensions: richTextExtensions(T, this),
                    // a shared document starts empty and takes the server's content through Yjs
                    content: this.collab ? "" : moqui.richText.toHtml(this.local),
                    editable: this.canEdit,
                    autofocus: this.autofocus ? "end" : false,
                    editorProps: {
                        attributes: Object.assign({ class: "m-rich-text-editor", role: "textbox", "aria-multiline": "true" },
                            this.id ? { id: this.id } : {}, this.label ? { "aria-label": this.label } : {}),
                        handlePaste(_view, event) {
                            if (!vm.takeImages(event.clipboardData)) return false;
                            event.preventDefault();
                            return true;
                        },
                        handleDrop(_view, event) {
                            if (!vm.takeImages(event.dataTransfer)) return false;
                            event.preventDefault();
                            return true;
                        }
                    },
                    onUpdate: ({ editor }) => {
                        if (this.isPlain || this.collab) return;
                        const html = moqui.richText.value(editor.getHTML());
                        if (html !== (this.local || "")) this.onInput(html);
                    },
                    onTransaction: () => this.readActive(),
                    onFocus: () => { this.focused = true; },
                    onBlur: () => { this.focused = false; }
                }));
                this.state = "ready";
                this.readActive();
            },
            readActive() {
                const editor = this.editor;
                if (!editor) return;
                const active = {};
                ["bold", "italic", "strike", "code", "link", "bulletList", "orderedList", "codeBlock", "blockquote"].forEach((name) => { active[name] = editor.isActive(name); });
                this.active = active;
                const level = [1, 2, 3].find((n) => editor.isActive("heading", { level: n }));
                this.blockLabel = level ? "Heading " + level : "Paragraph";
            },
            run(key) {
                const editor = this.editor;
                if (key === "plain") { this.togglePlain(); return; }
                if (!editor || !this.editable || this.isPlain) return;
                if (key === "link" || key === "image") { this.ask(key); return; }
                const chain = editor.chain().focus();
                const commands = {
                    bold: () => chain.toggleBold(), italic: () => chain.toggleItalic(), strike: () => chain.toggleStrike(), code: () => chain.toggleCode(),
                    bulletList: () => chain.toggleBulletList(), orderedList: () => chain.toggleOrderedList(), codeBlock: () => chain.toggleCodeBlock(),
                    blockquote: () => chain.toggleBlockquote(), undo: () => chain.undo(), redo: () => chain.redo(), paragraph: () => chain.setParagraph(),
                    h1: () => chain.toggleHeading({ level: 1 }), h2: () => chain.toggleHeading({ level: 2 }), h3: () => chain.toggleHeading({ level: 3 }),
                    insertTable: () => chain.insertTable({ rows: 3, cols: 3, withHeaderRow: true }), addRowAfter: () => chain.addRowAfter(),
                    addColumnAfter: () => chain.addColumnAfter(), deleteRow: () => chain.deleteRow(), deleteColumn: () => chain.deleteColumn(),
                    deleteTable: () => chain.deleteTable()
                };
                if (commands[key]) commands[key]().run();
            },
            // An Assist text action on the selection, or the whole field without one, as plain text: the proposal is shown under the
            // toolbar and applied (moqui.assist.proposalEdit, applyTextProposal) only by Replace or Insert below.
            textAction(item) {
                const assist = moqui.assist;
                const editor = this.editor;
                let source;
                if (this.isPlain) {
                    const area = this.$refs.plainArea;
                    const from = area ? area.selectionStart : 0;
                    const to = area ? area.selectionEnd : 0;
                    source = { selected: to > from, from, to, text: to > from ? this.plainText.slice(from, to) : this.plainText };
                } else if (editor) {
                    const sel = editor.state.selection;
                    source = { selected: !sel.empty, from: sel.from, to: sel.to, text: sel.empty ? moqui.richText.toPlain(this.local) : editor.state.doc.textBetween(sel.from, sel.to, "\n\n") };
                } else return;
                const labels = assist.labels;
                const token = assist.newId("t-");
                this.proposal = { token, key: item.key, state: "loading", label: labels.proposing, source, plain: this.isPlain, text: "", error: "", labels };
                const mine = () => this.proposal && this.proposal.token === token;
                assist.runTextAction(item.key, source.text, { field: this.label }).then((json) => {
                    if (mine()) Object.assign(this.proposal, { state: "ready", text: String(json.text || "") });
                }, (err) => { if (mine()) Object.assign(this.proposal, { state: "failed", error: (err && err.errors) || labels.errorTitle }); });
            },
            useProposal(mode) {
                const proposal = this.proposal;
                this.proposal = null;
                const assist = moqui.assist;
                if (!proposal || proposal.state !== "ready" || !assist) return;
                if (this.isPlain) { if (proposal.plain) this.onPlainInput(assist.applyTextProposal(this.plainText, proposal.source, proposal.text, mode)); return; }
                const editor = this.editor;
                if (!editor || !this.editable || proposal.plain) return;
                const doc = editor.state.doc;
                const src = proposal.source;
                const current = src.selected && src.to <= doc.content.size ? doc.textBetween(src.from, src.to, "\n\n") : null;
                const edit = assist.proposalEdit(src, proposal.text, mode, current);
                if (!edit.content) return;
                const chain = editor.chain().focus();
                if (edit.kind === "range") chain.insertContentAt({ from: edit.from, to: edit.to }, edit.content).run();
                else if (edit.kind === "all") chain.setContent(edit.content).run();
                else {
                    // below the block the selection ends in, else below the whole text
                    const end = doc.content.size;
                    const at = edit.kind === "after" ? (() => { const pos = doc.resolve(Math.min(edit.pos, end)); return pos.depth ? pos.after(1) : end; })() : end;
                    chain.insertContentAt(at, edit.content).run();
                }
            },
            // the address of a link or an image, asked in a line under the toolbar
            ask(kind) {
                if (!this.editor || !this.editable) return;
                this.asking = kind;
                this.askValue = kind === "link" ? (this.editor.getAttributes("link").href || "") : "";
                this.$nextTick(() => { if (this.$refs.askInput) this.$refs.askInput.focus(); });
            },
            applyAsk() {
                const editor = this.editor;
                const kind = this.asking;
                this.asking = null;
                if (!editor) return;
                const value = this.askValue.trim();
                if (kind === "image") {
                    if (/^(https?:)?\/\//i.test(value) || value.startsWith("/")) editor.chain().focus().setImage({ src: value }).run();
                    else if (value) moqui.notifyMessages(null, this.t("An image address starts with https:// or /"));
                    return;
                }
                const href = moqui.richText.linkHref(value);
                if (!href) { this.removeLink(); return; }
                if (editor.state.selection.empty && !editor.isActive("link")) {
                    editor.chain().focus().insertContent({ type: "text", text: value, marks: [{ type: "link", attrs: { href } }] }).run();
                } else editor.chain().focus().extendMarkRange("link").setLink({ href }).run();
            },
            removeLink() {
                this.asking = null;
                if (this.editor) this.editor.chain().focus().extendMarkRange("link").unsetLink().run();
            },
            togglePlain() {
                const editor = this.editor;
                if (!this.isPlain) {
                    this.htmlBeforePlain = this.local || "";
                    this.plainText = moqui.richText.toPlain(this.local);
                    this.isPlain = true;
                } else {
                    const html = moqui.richText.fromPlainKeeping(this.plainText, this.htmlBeforePlain);
                    this.isPlain = false;
                    if (editor) {
                        editor.commands.setContent(html, { emitUpdate: false });
                        this.onInput(moqui.richText.value(editor.getHTML()));
                    } else this.onInput(moqui.richText.value(html));
                }
                this.$emit("update:plain", this.isPlain);
            },
            // each edit writes the kept HTML into the model; the textarea stays the plain text
            onPlainInput(value) {
                this.plainText = value;
                this.onInput(moqui.richText.value(moqui.richText.fromPlainKeeping(value, this.htmlBeforePlain)));
            },
            // the plain string on screen, for a mail part; the model stays the HTML
            plainValue() { return this.isPlain ? this.plainText : moqui.richText.toPlain(this.local); },
            pickImage() { if (this.$refs.imageFile) this.$refs.imageFile.click(); },
            onImageFile(event) {
                const file = event.target.files && event.target.files[0];
                event.target.value = "";
                this.asking = null;
                if (file) this.uploadImage(file);
            },
            // paste or drop of an image file, or a data: image, uploads and inserts nothing of the original
            takeImages(data) {
                if (!data || !this.editable || this.isPlain) return false;
                const files = [];
                const list = data.files || [];
                for (let i = 0; i < list.length; i++) if (list[i] && /^image\//i.test(list[i].type || "")) files.push(list[i]);
                if (!files.length && data.items) {
                    for (let i = 0; i < data.items.length; i++) {
                        const item = data.items[i];
                        if (item.kind === "file" && /^image\//i.test(item.type || "")) {
                            const file = item.getAsFile && item.getAsFile();
                            if (file) files.push(file);
                        }
                    }
                }
                if (!files.length) {
                    const html = data.getData ? (data.getData("text/html") || "") : "";
                    const match = /src\s*=\s*["'](data:image\/[a-z0-9.+-]+;base64,[a-z0-9+/=\s]+)["']/i.exec(html);
                    const text = !match && data.getData ? String(data.getData("text/plain") || "").trim() : "";
                    const url = match ? match[1] : (/^data:image\/[a-z0-9.+-]+;base64,/i.test(text) ? text : "");
                    const file = url ? this.fileFromDataUrl(url) : null;
                    if (file) files.push(file);
                }
                if (!files.length) return false;
                files.forEach((file) => this.uploadImage(file));
                return true;
            },
            fileFromDataUrl(url) {
                const match = /^data:(image\/[a-z0-9.+-]+);base64,([a-z0-9+/=\s]+)$/i.exec(String(url || "").replace(/\s/g, ""));
                if (!match || typeof root.atob !== "function") return null;
                let binary;
                try { binary = root.atob(match[2]); } catch (err) { return null; }
                const bytes = new Uint8Array(binary.length);
                for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
                const subtype = match[1].slice(6).toLowerCase();
                const ext = subtype === "jpeg" ? "jpg" : subtype === "svg+xml" ? "svg" : (subtype.replace(/[^a-z0-9]/g, "") || "img");
                try { return new File([bytes], "image." + ext, { type: match[1].toLowerCase() }); } catch (err) { return null; }
            },
            uploadImage(file) {
                if (!file || !this.editor) { moqui.notifyMessages(null, this.t("The image was not saved")); return; }
                const body = new FormData();
                body.append("contentFile", file, file.name || "image.png");
                const alt = file.name || "";
                moqui.request("/richTextImage", { method: "POST", params: body }).then((result) => {
                    const url = result.json && result.json.url;
                    if (!url || !this.editor) { moqui.notifyMessages(null, this.t("The image was not saved")); return; }
                    this.editor.chain().focus().setImage({ src: url, alt: alt }).run();
                }, () => { moqui.notifyMessages(null, this.t("The image was not saved")); });
            },
            // the load or the start failed: the html editor stands in and waits for a later success, or with collab the host
            // is told (failed) and shows its own field
            startFailed(message) {
                this.state = "failed";
                if (this.collab) {
                    console.error("m-rich-text: " + message.replace(/, the html editor stands in/, ""));
                    this.$emit("failed", message);
                    return;
                }
                console.error("m-rich-text: " + message);
                rememberFailed(this);
            },
            retryEditor() {
                this.state = "loading";
                this.$nextTick(() => {
                    loadTiptap().then(() => {
                        if (this.unmountedNow) return;
                        try {
                            this.start();
                            dropFailed(this);
                            wakeFailed();
                        } catch (err) {
                            this.startFailed("the editor did not start, the html editor stands in: " + err);
                        }
                    }, (err) => {
                        if (this.unmountedNow) return;
                        this.startFailed("Tiptap did not load, the html editor stands in: " + err);
                    });
                });
            },
            plainKey(event) {
                if (event.key !== "Enter" || event.isComposing) return;
                if (event.ctrlKey || event.metaKey || (this.enter === "submit" && !event.shiftKey)) { event.preventDefault(); this.submit(); }
            },
            focusEnd(event) {
                if (this.editor && event.target === this.$refs.content) this.editor.commands.focus("end");
            },
            // Enter (with enter="submit") or Ctrl+Enter: the submit event, and the form's submit inside an m-form
            submit() {
                this.$emit("submit", this.local);
                if (this.mForm && this.mForm.submitForm) this.mForm.submitForm();
            },
            searchMentions(query, signal) {
                if (!this.mentionUrl) return Promise.resolve([]);
                const params = Object.assign({}, this.mentionParameters, { term: query || "" });
                return moqui.request(this.mentionUrl, { params, signal }).then((result) => moqui.richText.mentionItems(result.json),
                    (err) => {
                        if (!err || !err.aborted) console.info("m-rich-text: the mention search " + this.mentionUrl + " failed, status " + (err && err.status));
                        return [];
                    });
            },
            searchRecords(query, signal) {
                if (!this.recordUrl) return Promise.resolve([]);
                const params = Object.assign({}, this.recordParameters, { term: query || "" });
                return moqui.request(this.recordUrl, { params, signal }).then((result) => moqui.richText.mentionItems(result.json),
                    (err) => {
                        if (!err || !err.aborted) console.info("m-rich-text: the record search " + this.recordUrl + " failed, status " + (err && err.status));
                        return [];
                    });
            },
            // the renderer of the suggestion plugin: the list is this component's (teleported to the body), placed by the plugin
            mentionRenderer() {
                const vm = this;
                const show = (props) => {
                    vm.mentionProps = props;
                    vm.mention.items = props.items || [];
                    vm.mention.loading = !!props.loading;
                    if (vm.mention.index >= vm.mention.items.length) vm.mention.index = 0;
                };
                return {
                    onStart(props) {
                        show(props);
                        vm.mention.index = 0;
                        vm.mention.open = true;
                        vm.$nextTick(() => {
                            if (!vm.mention.open) return;
                            if (vm.mentionUnmount) vm.mentionUnmount();
                            vm.mentionUnmount = vm.$refs.mentionMenu && props.mount ? props.mount(vm.$refs.mentionMenu) : null;
                        });
                    },
                    onUpdate: show,
                    onKeyDown({ event }) {
                        if (!vm.mention.open) return false;
                        const count = vm.mention.items.length;
                        if (event.key === "ArrowDown" && count) { vm.mention.index = (vm.mention.index + 1) % count; return true; }
                        if (event.key === "ArrowUp" && count) { vm.mention.index = (vm.mention.index + count - 1) % count; return true; }
                        if ((event.key === "Enter" || event.key === "Tab") && count) { vm.pickMention(vm.mention.index); return true; }
                        return false;
                    },
                    onExit() {
                        vm.mention.open = false;
                        vm.mentionProps = null;
                        if (vm.mentionUnmount) { vm.mentionUnmount(); vm.mentionUnmount = null; }
                    }
                };
            },
            pickMention(index) {
                const item = this.mention.items[index];
                if (!item || !this.mentionProps) return;
                const picked = { id: item.id, label: item.label };
                if (item.entity) picked.entity = item.entity;
                this.mentionProps.command(picked);
                this.$emit("mention", item);
            }
        },
        created() {
            if (this.isPlain) {
                this.htmlBeforePlain = this.local || "";
                this.plainText = moqui.richText.toPlain(this.local);
            }
        },
        mounted() {
            loadTiptap().then(() => {
                if (this.unmountedNow) return;
                try {
                    this.start();
                    dropFailed(this);
                    wakeFailed();
                } catch (err) {
                    this.startFailed("the editor did not start, the html editor stands in: " + err);
                }
            }, (err) => {
                if (this.unmountedNow) return;
                this.startFailed("Tiptap did not load, the html editor stands in: " + err);
            });
        },
        beforeUnmount() {
            this.unmountedNow = true;
            dropFailed(this);
            if (this.mentionUnmount) { this.mentionUnmount(); this.mentionUnmount = null; }
            if (this.editor) { this.editor.destroy(); this.editor = null; }
        }
    });

    /* ========== collaborative editing: m-coedit (Yjs over the notification socket) ========== */

    // The pure part of m-coedit, tested under Node with a fake notification client (WebrootVue.qvt.coedit.test.js): base64 of
    // the binary Yjs updates (btoa in a browser, Buffer where there is none), the caret color of a user, and the provider,
    // the protocol of js/README.md (Collaborative editing) between a Y.Doc and its Awareness and the socket.
    const coeditColors = ["#1e88e5", "#e53935", "#43a047", "#8e24aa", "#fb8c00", "#00897b", "#d81b60", "#3949ab", "#6d4c41",
        "#00acc1", "#7cb342", "#f4511e"];
    moqui.coedit = {
        toBase64(bytes) {
            if (typeof root.btoa !== "function") return Buffer.from(bytes).toString("base64");
            let binary = "";
            for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
            return root.btoa(binary);
        },
        fromBase64(text) {
            if (typeof root.atob !== "function") return new Uint8Array(Buffer.from(String(text || ""), "base64"));
            const binary = root.atob(String(text || ""));
            const bytes = new Uint8Array(binary.length);
            for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
            return bytes;
        },
        // a color for a user id (or a name), the same each time; #rrggbb, the only form the caret extension takes
        userColor(id) {
            let hash = 0;
            for (const ch of String(id || "")) hash = (hash * 31 + ch.codePointAt(0)) >>> 0;
            return coeditColors[hash % coeditColors.length];
        }
    };

    // One editor's side of a shared document. options: client (a moqui.NotificationClient: send, registerListener, onState,
    // isOpen), docKey, doc (a Y.Doc), awareness (an Awareness of it), lib (the Tiptap global: Y and the awareness functions),
    // onStatus({state, peers, pending}), onAccess(canEdit), onSaved(message), onFallback({reason, code, message}), batchMs
    // (50: local updates in that time go as one message), joinTimeoutMs (20 s: a join no one answers on an open socket is a
    // fallback). A remote update is applied with the provider as its origin, so it is not sent back. Each open of the socket
    // joins again with the state vector, so what was edited offline merges; an update sent and not acknowledged before a
    // close is in that exchange too.
    class CoeditProvider {
        constructor(options) {
            this.options = options;
            this.client = options.client;
            this.doc = options.doc;
            this.awareness = options.awareness;
            this.lib = options.lib;
            this.docKey = options.docKey;
            this.clientId = options.clientId || "c" + Math.random().toString(36).slice(2, 10) + Date.now().toString(36);
            this.batchMs = options.batchMs === undefined ? 50 : options.batchMs;
            this.joined = false;
            this.joinedOnce = false;
            this.wasOpen = false;
            this.canEdit = false;
            this.failed = null;
            this.destroyed = false;
            this.peers = [];
            this.unacked = new Set();
            this.nextId = 0;
            this.queue = [];
            this.retries = 0;
            this.lastStatus = "";
            this.onMessage = (message) => this.receive(message);
            this.onDocUpdate = (update, origin) => {
                if (origin === this || this.destroyed) return;
                this.queue.push(update);
                if (!this.batchTimer) this.batchTimer = setTimeout(() => this.flush(), this.batchMs);
                this.tell();
            };
            // the local state changed (a caret, the user; the Awareness renews it every 15 s): sent once per batch
            this.onAwarenessUpdate = (changes, origin) => {
                if (origin === this || this.destroyed) return;
                const own = this.awareness.clientID;
                if (![].concat(changes.added, changes.updated, changes.removed).includes(own)) return;
                if (!this.awarenessTimer) this.awarenessTimer = setTimeout(() => { this.awarenessTimer = null; this.sendAwareness(); }, this.batchMs);
            };
        }
        start() {
            this.listenerOff = this.client.registerListener("collab:" + this.docKey, this.onMessage);
            this.stateOff = this.client.onState((state) => { if (state === "open") this.join(); else this.closed(); });
            this.doc.on("update", this.onDocUpdate);
            this.awareness.on("update", this.onAwarenessUpdate);
            if (this.client.isOpen()) this.join();
            else this.tell();
        }
        send(message) { this.client.send(Object.assign({ doc: this.docKey, client: this.clientId }, message)); }
        join() {
            if (this.destroyed || this.failed) return;
            this.joined = false;
            this.wasOpen = true;
            this.unacked.clear();
            this.dropQueue();
            this.send({ collab: "join", awareness: this.doc.clientID, sv: moqui.coedit.toBase64(this.lib.Y.encodeStateVector(this.doc)) });
            clearTimeout(this.joinTimer);
            this.joinTimer = setTimeout(() => {
                if (!this.joined && !this.destroyed && this.client.isOpen()) this.fail("timeout", null, "no answer to the join");
            }, this.options.joinTimeoutMs || 20000);
            this.tell();
        }
        // the socket closed: the others' carets go, and what is sent from here on waits for the next join's exchange
        closed() {
            if (this.destroyed) return;
            this.joined = false;
            this.unacked.clear();
            this.dropQueue();
            clearTimeout(this.joinTimer);
            const others = Array.from(this.awareness.getStates().keys()).filter((id) => id !== this.awareness.clientID);
            if (others.length) this.lib.removeAwarenessStates(this.awareness, others, this);
            this.peers = [];
            this.tell();
        }
        dropQueue() {
            clearTimeout(this.batchTimer);
            this.batchTimer = null;
            this.queue = [];
        }
        // the local updates of the batch as one message; nothing while not joined (the join's exchange carries them)
        flush() {
            this.batchTimer = null;
            const updates = this.queue.splice(0);
            if (updates.length && this.joined && this.canEdit && this.client.isOpen()) {
                this.sendUpdate(updates.length === 1 ? updates[0] : this.lib.Y.mergeUpdates(updates));
            }
            this.tell();
        }
        sendUpdate(update) {
            this.nextId += 1;
            this.unacked.add(this.nextId);
            this.send({ collab: "update", id: this.nextId, update: moqui.coedit.toBase64(update) });
        }
        sendAwareness() {
            if (!this.joined || this.destroyed || !this.client.isOpen()) return;
            const update = this.lib.encodeAwarenessUpdate(this.awareness, [this.awareness.clientID]);
            this.send({ collab: "awareness", update: moqui.coedit.toBase64(update) });
        }
        applyRemote(text) {
            try {
                this.lib.Y.applyUpdate(this.doc, moqui.coedit.fromBase64(text), this);
            } catch (err) {
                console.error("m-coedit: an update of " + this.docKey + " did not apply: " + err);
            }
        }
        // a frame on the topic; ack, error and joined of another client id (another editor of the same user) are not ours
        receive(message) {
            if (!message || this.destroyed || this.failed) return;
            const own = message.client === this.clientId;
            if (message.type === "joined") { if (own) this.onJoined(message); }
            else if (message.type === "update") { if (!own && message.update) this.applyRemote(message.update); }
            else if (message.type === "ack") {
                if (!own) return;
                this.unacked.delete(Number(message.id));
                this.retries = 0;
                this.tell();
            } else if (message.type === "awareness") {
                if (own || !message.update) return;
                try {
                    this.lib.applyAwarenessUpdate(this.awareness, moqui.coedit.fromBase64(message.update), this);
                } catch (err) {
                    console.error("m-coedit: an awareness update of " + this.docKey + " did not apply: " + err);
                }
            } else if (message.type === "peer") { if (!own) this.onPeer(message); }
            else if (message.type === "saved") { if (this.options.onSaved) this.options.onSaved(message); }
            else if (message.type === "error") { if (own) this.onError(message); }
        }
        // the server's diff against our state vector, then ours against its, when it holds anything (an empty update is 2 bytes)
        onJoined(message) {
            clearTimeout(this.joinTimer);
            this.joined = true;
            this.joinedOnce = true;
            this.canEdit = message.canEdit !== false;
            this.dropQueue();
            if (message.update) this.applyRemote(message.update);
            this.peers = (message.peers || []).filter((peer) => peer && peer.client !== this.clientId).map(peerOf);
            if (this.canEdit) {
                const diff = this.lib.Y.encodeStateAsUpdate(this.doc, message.sv ? moqui.coedit.fromBase64(message.sv) : undefined);
                if (diff.length > 2) this.sendUpdate(diff);
            }
            this.sendAwareness();
            if (this.options.onAccess) this.options.onAccess(this.canEdit);
            this.tell();
        }
        onPeer(message) {
            const peer = peerOf(message);
            this.peers = this.peers.filter((other) => other.client !== peer.client);
            if (message.joined !== false) this.peers.push(peer);
            else if (peer.awareness !== null) this.lib.removeAwarenessStates(this.awareness, [peer.awareness], this);
            this.tell();
        }
        // Before the join is answered any error is the end (the host shows its own field). After it, 403 is read-only, and
        // another code joins again (the exchange of state vectors makes the document whole), three times without an ack between.
        onError(message) {
            const code = Number(message.code) || 500;
            if (!this.joined) { this.fail("server", code, message.message); return; }
            if (code === 403) {
                this.canEdit = false;
                this.unacked.clear();
                this.dropQueue();
                if (this.options.onAccess) this.options.onAccess(false);
                this.tell();
                return;
            }
            this.retries += 1;
            if (this.retries > 3) { this.fail("server", code, message.message); return; }
            console.info("m-coedit: the server answered " + code + " for " + this.docKey + ", joining again: " + (message.message || ""));
            if (this.client.isOpen()) this.join();
        }
        fail(reason, code, text) {
            if (this.failed) return;
            this.failed = reason;
            clearTimeout(this.joinTimer);
            this.dropQueue();
            if (this.client.isOpen()) this.send({ collab: "leave" });
            this.tell();
            if (this.options.onFallback) this.options.onFallback({ reason, code: code || null, message: text || "" });
        }
        status() {
            let state;
            if (this.failed) state = "failed";
            else if (!this.joined) state = this.wasOpen && !this.client.isOpen() ? "offline" : "connecting";
            else if (!this.canEdit) state = "readonly";
            else state = this.unacked.size || this.queue.length ? "saving" : "synced";
            return { state, peers: this.peers.map((peer) => ({ client: peer.client, userId: peer.userId, name: peer.name })), pending: this.unacked.size };
        }
        tell() {
            const status = this.status();
            const key = JSON.stringify(status);
            if (key === this.lastStatus) return;
            this.lastStatus = key;
            if (this.options.onStatus) this.options.onStatus(status);
        }
        // the last batch goes, then leave; the doc, its awareness and the editor are the owner's to destroy
        destroy() {
            if (this.destroyed) return;
            if (this.queue.length) { clearTimeout(this.batchTimer); this.flush(); }
            this.destroyed = true;
            clearTimeout(this.joinTimer);
            clearTimeout(this.awarenessTimer);
            this.dropQueue();
            if (this.client.isOpen() && !this.failed) this.send({ collab: "leave" });
            if (this.listenerOff) this.listenerOff();
            if (this.stateOff) this.stateOff();
            this.doc.off("update", this.onDocUpdate);
            this.awareness.off("update", this.onAwarenessUpdate);
        }
    }
    const peerOf = (message) => ({ client: message.client, userId: message.userId || "", name: message.name || message.userId || "",
        awareness: message.awareness === undefined || message.awareness === null ? null : Number(message.awareness) });
    moqui.coedit.Provider = CoeditProvider;

    // m-rich-text on a shared document (js/README.md, Collaborative editing): doc-key names it ("WikiPage:100023"), and the
    // server seeds and renders it, so the editor never sets content of its own. The presence line shows the others (by user)
    // and the sync state. Emits status ({state: loading, connecting, synced, saving, offline, readonly or failed, peers:
    // [{client, userId, name}], pending}), saved (the server's saved frame: a version was made) and fallback ({reason: library,
    // socket, server or timeout, code, message}) when the host should show its own markdown field instead. Other attributes
    // (id, label, autofocus) go to the editor. html() is the editor's HTML now, for a preview.
    component("m-coedit", {
        name: "mCoedit",
        inheritAttrs: false,
        props: { docKey: { type: String, required: true }, placeholder: String, rows: [String, Number], light: Boolean, readonly: Boolean,
            userName: String, userColor: String },
        emits: ["status", "saved", "fallback"],
        data() { return { collab: null, status: { state: "loading", peers: [], pending: 0 }, canEdit: false, joinedOnce: false }; },
        template:
            '<div class="m-coedit" :data-state="status.state">' +
                '<div class="m-coedit-presence row items-center no-wrap" role="status">' +
                    '<m-avatar v-for="person in people" :key="person.key" class="m-coedit-peer" :name="person.name" :user-id="person.userId" size="22px" tooltip></m-avatar>' +
                    '<q-icon :name="stateIcon" size="16px" class="m-coedit-state-icon"></q-icon>' +
                    '<span class="m-coedit-state text-caption">{{t(stateLabel)}}</span>' +
                '</div>' +
                '<m-rich-text v-if="collab" ref="editor" v-bind="$attrs" :collab="collab" :light="light" :placeholder="placeholder" :rows="rows"' +
                    ' :readonly="readonly || (joinedOnce && !canEdit)" :disable="!joinedOnce" :plain-toggle="false" @failed="editorFailed"></m-rich-text>' +
            '</div>',
        computed: {
            // the others, once per user (two tabs of one person are one face), not the user here
            people() {
                const me = (this.$root && this.$root.userId) || "";
                const seen = {};
                const out = [];
                this.status.peers.forEach((peer) => {
                    const key = peer.userId || peer.client;
                    if ((me && peer.userId === me) || seen[key]) return;
                    seen[key] = true;
                    out.push({ key, userId: peer.userId, name: peer.name });
                });
                return out;
            },
            stateLabel() {
                return { loading: "Loading the editor", connecting: "Connecting", synced: "Up to date", saving: "Saving", offline: "Offline, the changes wait here",
                    readonly: "Read only", failed: "Not available" }[this.status.state] || "";
            },
            stateIcon() {
                return { loading: "sym_o_hourglass_empty", connecting: "sym_o_sync", synced: "sym_o_cloud_done", saving: "sym_o_cloud_upload",
                    offline: "sym_o_cloud_off", readonly: "sym_o_visibility", failed: "sym_o_error" }[this.status.state] || "sym_o_sync";
            }
        },
        watch: {
            docKey() {
                this.stop();
                this.collab = null;
                this.$nextTick(() => { this.dropDoc(); this.begin(); });
            }
        },
        methods: {
            t(text) { return moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text; },
            setStatus(status) {
                this.status = status;
                this.$emit("status", status);
            },
            fallback(reason, code, message) {
                if (this.fellBack) return;
                this.fellBack = true;
                this.stop();
                this.collab = null;
                this.setStatus({ state: "failed", peers: [], pending: 0 });
                this.$emit("fallback", { reason, code: code || null, message: message || "" });
            },
            editorFailed(message) { this.fallback("library", null, message); },
            html() {
                const editor = this.$refs.editor && this.$refs.editor.editor;
                return editor ? moqui.richText.value(editor.getHTML()) : "";
            },
            begin() {
                const client = this.$root && this.$root.notificationClient;
                this.fellBack = false;
                this.canEdit = false;
                this.joinedOnce = false;
                this.setStatus({ state: "loading", peers: [], pending: 0 });
                if (!client || typeof client.send !== "function") { this.fallback("socket", null, "no notification socket"); return; }
                loadTiptap().then(() => {
                    if (this.unmountedNow) return;
                    const T = root.Tiptap;
                    if (!T.Y || !T.Awareness || !T.Collaboration || !T.CollaborationCaret) { this.fallback("library", null, "the Tiptap bundle has no collaboration"); return; }
                    try {
                        this.open(T, client);
                    } catch (err) {
                        console.error("m-coedit: the shared document did not start: " + err);
                        this.fallback("library", null, String(err));
                    }
                }, (err) => {
                    if (this.unmountedNow) return;
                    console.error("m-coedit: Tiptap did not load: " + err);
                    this.fallback("library", null, String(err));
                });
            },
            open(T, client) {
                const rootVm = this.$root || {};
                const doc = new T.Y.Doc();
                const awareness = new T.Awareness(doc);
                const name = this.userName || rootVm.userFullName || rootVm.username || rootVm.userId || "";
                const color = /^#[0-9a-f]{6}$/i.test(this.userColor || "") ? this.userColor : moqui.coedit.userColor(rootVm.userId || name);
                this.ydoc = markRaw(doc);
                this.awareness = markRaw(awareness);
                this.provider = markRaw(new moqui.coedit.Provider({ client, docKey: this.docKey, doc, awareness, lib: T,
                    onStatus: (status) => { if (!this.fellBack) this.setStatus(status); },
                    onAccess: (canEdit) => { this.canEdit = canEdit; this.joinedOnce = true; },
                    onSaved: (message) => this.$emit("saved", message),
                    onFallback: (info) => this.fallback(info.reason, info.code, info.message) }));
                this.collab = markRaw({ doc, awareness, user: { name, color } });
                this.provider.start();
            },
            // leave (the provider sends its last batch first)
            stop() {
                if (this.provider) { this.provider.destroy(); this.provider = null; }
            },
            // after the editor is gone
            dropDoc() {
                if (this.awareness) { this.awareness.destroy(); this.awareness = null; }
                if (this.ydoc) { this.ydoc.destroy(); this.ydoc = null; }
            }
        },
        mounted() { this.begin(); },
        beforeUnmount() {
            this.unmountedNow = true;
            this.stop();
        },
        unmounted() { this.dropDoc(); }
    });

    /* ========== subscreens and the menu ========== */

    component("m-subscreens-tabs", {
        name: "mSubscreensTabs",
        data() { return { pathIndex: -1 }; },
        template:
            '<div v-if="subscreens.length > 1"><q-tabs dense no-caps align="left" narrow-indicator inline-label class="text-grey-7" active-color="primary" indicator-color="primary" :model-value="activeTab">' +
                '<q-tab v-for="tab in subscreens" :key="tab.name" :name="tab.name" :label="tab.title" :disable="tab.disableLink" @click.prevent="goTo(tab.pathWithParams)"></q-tab>' +
            '</q-tabs><q-separator class="q-mb-md"></q-separator></div>',
        methods: { goTo(pathWithParams) { this.$root.setUrl(this.$root.getLinkPath(pathWithParams)); } },
        computed: {
            subscreens() {
                if (!this.pathIndex || this.pathIndex < 0) return [];
                const navMenu = this.$root.navMenuList[this.pathIndex];
                return navMenu && navMenu.subscreens ? navMenu.subscreens : [];
            },
            activeTab() {
                const active = this.subscreens.find((tab) => tab.active);
                return active ? active.name : null;
            }
        },
        // the tabs need to be before the m-subscreens-active of the same screen
        mounted() { this.pathIndex = this.$root.activeSubscreens.length; }
    });
    qvt.components["m-subscreens-menu"] = qvt.components["m-subscreens-tabs"];

    component("m-subscreens-active", {
        name: "mSubscreensActive",
        data() { return { activeComponent: moqui.EmptyComponent, pathIndex: -1, pathName: null, searchUsed: null }; },
        template: '<component :is="activeComponent" style="height:100%;width:100%;"></component>',
        methods: {
            // a method and not a watch on pathName, so that it runs when the path is the same and the screen is not static
            loadActive() {
                const rootVm_ = this.$root;
                const curPathList = rootVm_.currentPathList;
                const pathIndex = this.pathIndex;
                const newPath = curPathList[pathIndex];
                const pathChanged = this.pathName !== newPath;
                this.pathName = newPath;
                // A cached (server-static) target screen is used again for new parameters: its data comes from them, so a find or a saved
                // find on the same path makes a new use. The screens above the target keep theirs.
                const isTarget = pathIndex === curPathList.length - 1;
                const searchNow = isTarget ? rootVm_.currentSearch || "" : "";
                const searchChanged = (this.searchUsed !== null && this.searchUsed !== searchNow) || (isTarget && rootVm_.reloadTarget === true);
                if (isTarget) rootVm_.reloadTarget = false;
                this.searchUsed = searchNow;
                if (!newPath || newPath.length === 0) {
                    this.activeComponent = moqui.EmptyComponent;
                    return true;
                }
                const fullPath = rootVm_.basePath + "/" + curPathList.slice(0, pathIndex + 1).join("/");
                if (!pathChanged && !searchChanged && moqui.componentCache.containsKey(fullPath)) return false;
                const urlInfo = { path: fullPath };
                if (pathIndex === curPathList.length - 1) {
                    const extra = rootVm_.extraPathList;
                    if (extra && extra.length > 0) urlInfo.extraPath = extra.join("/");
                }
                const search = rootVm_.currentSearch;
                if (search && search.length > 0) urlInfo.search = search;
                urlInfo.bodyParameters = rootVm_.bodyParameters;
                const navMenuItem = rootVm_.navMenuList[pathIndex + rootVm_.basePathSize];
                if (navMenuItem && navMenuItem.renderModes) urlInfo.renderModes = navMenuItem.renderModes;
                rootVm_.loading++;
                rootVm_.currentLoadRequest = moqui.loadComponent(urlInfo, (comp) => {
                    rootVm_.currentLoadRequest = null;
                    this.activeComponent = comp;
                    rootVm_.loading--;
                });
                return true;
            }
        },
        mounted() { this.$root.addSubscreen(this); }
    });

    component("m-subscreens-panel", {
        name: "mSubscreensPanel",
        template: '<div><m-subscreens-tabs></m-subscreens-tabs><m-subscreens-active></m-subscreens-active></div>'
    });

    /* ========== wizard and stack panels, and the split container ========== */

    const localized = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);

    // The URL of a step: its path with the current parameters, then its own, then those the step's answer gave (screenParameters),
    // without the session token and the form name. Pure, for the node tests.
    moqui.wizardStepUrl = (pathWithParams, current, answer) => {
        const text = String(pathWithParams || "");
        const at = text.indexOf("?");
        const path = at >= 0 ? text.substring(0, at) : text;
        const own = at >= 0 ? moqui.searchToObj(text.substring(at + 1)) : {};
        const merged = Object.assign({}, current || {}, own, answer || {});
        ["moquiSessionToken", "moquiFormName", "lastStandalone"].forEach((name) => delete merged[name]);
        Object.keys(merged).forEach((name) => { if (merged[name] === null || merged[name] === undefined || merged[name] === "") delete merged[name]; });
        const search = moqui.objToSearch(merged);
        return search ? path + "?" + search : path;
    };
    // Where a wizard goes when its step form's answer comes, from the answer, the current path, the steps and the active one:
    // {kind: "stay"} (errors, or a last step that names no screen), {kind: "redirect", url} (a URL off the screens), {kind: "url", url}
    // (a screen other than this step: the transition chose where to go), or {kind: "step", index, parameters} (the next step).
    moqui.wizardAfterAnswer = (answer, currentPathList, stepCount, activeIndex) => {
        if (moqui.responseHasErrors(answer)) return { kind: "stay" };
        const resp = answer && moqui.isPlainObject(answer) ? answer : {};
        if (resp.redirectUrl) return { kind: "redirect", url: resp.redirectUrl };
        const answerPath = Array.isArray(resp.screenPathList) ? resp.screenPathList : null;
        const current = currentPathList || [];
        const sameStep = !answerPath || (answerPath.length >= current.length &&
            current.every((name, index) => answerPath[answerPath.length - current.length + index] === name));
        if (!sameStep && resp.screenUrl) return { kind: "url", url: resp.screenUrl };
        if (activeIndex >= stepCount - 1) return resp.screenUrl ? { kind: "url", url: resp.screenUrl } : { kind: "stay" };
        return { kind: "step", index: activeIndex + 1, parameters: resp.screenParameters || {} };
    };

    // subscreens-panel type="wizard": a q-stepper with a step per subscreen (the menu titles, in menu order), the active subscreen
    // below it, and Back and Next. The current step is the subscreen in the URL. Next submits the step form, the first m-form of the
    // step that is not in a list or a dialog (the forms register through inject mWizard), and moves on when the answer has no errors
    // (moqui.wizardAfterAnswer); a step without a form moves on at once. The last step's button is finishText and only shows with a form.
    // While the step form posts (by Next, Enter or a button of its own) the step is covered by a spinner and Back and Next wait, until
    // the answer says to stay or the screen it goes to is there: the last step's submit (an import, a run) may take a while.
    component("m-subscreens-wizard", {
        name: "mSubscreensWizard",
        props: { id: String, finishText: String, nextText: String, backText: String },
        data() { return { pathIndex: -1, forms: [], submitting: false, hasForm: false }; },
        provide() { return { mWizard: this }; },
        template:
            '<div class="m-wizard" :id="id" :data-step="activeName" :data-step-form="hasForm ? \'true\' : \'false\'" :data-forms="forms.length">' +
                '<q-stepper v-if="steps.length" :model-value="activeName" flat bordered header-nav class="m-wizard-stepper" active-color="primary" done-color="primary"' +
                        ' :contracted="$q.screen.lt.sm" @update:model-value="headerTo">' +
                    '<q-step v-for="(step, index) in steps" :key="step.name" :name="step.name" :title="step.title" :done="index < activeIndex"' +
                        ' :header-nav="index < activeIndex" :prefix="step.imageType === \'icon\' && step.image ? undefined : String(index + 1)"' +
                        ' :icon="step.imageType === \'icon\' && step.image ? step.image : undefined" :data-step="step.name"></q-step>' +
                '</q-stepper>' +
                '<div ref="body" class="m-wizard-body"><m-subscreens-active></m-subscreens-active>' +
                    '<q-inner-loading :showing="submitting" class="m-wizard-working"><q-spinner color="primary" size="40px"></q-spinner>' +
                        '<div class="text-caption m-muted q-mt-sm" v-text="workingLabel"></div></q-inner-loading></div>' +
                '<div v-if="steps.length" class="m-wizard-nav row items-center q-gutter-sm">' +
                    '<q-btn v-if="activeIndex > 0" unelevated no-caps class="m-btn-tonal m-wizard-back" icon="arrow_back" :label="backLabel" :disable="submitting" @click="back"></q-btn>' +
                    '<q-space></q-space>' +
                    '<q-btn v-if="!isLast || hasForm" unelevated no-caps color="primary" class="m-wizard-next" :icon-right="isLast ? \'check\' : \'arrow_forward\'"' +
                        ' :label="isLast ? finishLabel : nextLabel" :loading="submitting" @click="next"></q-btn>' +
                '</div>' +
            '</div>',
        computed: {
            steps() {
                if (this.pathIndex < 0) return [];
                const navMenu = this.$root.navMenuList[this.pathIndex];
                return navMenu && navMenu.subscreens ? navMenu.subscreens.filter((step) => !step.disableLink) : [];
            },
            activeIndex() { return Math.max(0, this.steps.findIndex((step) => step.active)); },
            activeName() { const step = this.steps[this.activeIndex]; return step ? step.name : null; },
            isLast() { return this.activeIndex >= this.steps.length - 1; },
            nextLabel() { return this.nextText || localized("Next"); },
            backLabel() { return this.backText || localized("Back"); },
            finishLabel() { return this.finishText || localized("Finish"); },
            workingLabel() { return localized("Working..."); }
        },
        methods: {
            addForm(form) { if (this.forms.indexOf(form) < 0) this.forms.push(form); this.formsChanged(); },
            removeForm(form) { const at = this.forms.indexOf(form); if (at >= 0) this.forms.splice(at, 1); this.formsChanged(); },
            // whether the step has a form, once the DOM of a step that came or went is in place (the step form is found in the DOM)
            formsChanged() { this.$nextTick(() => { this.hasForm = !!this.stepForm(); }); },
            // the first form of the step in document order that is not a list row form and not in a dialog (a dialog is teleported)
            stepForm() {
                const body = this.$refs.body;
                if (!body) return null;
                const forms = this.forms.filter((form) => form.$el && body.contains(form.$el) && !form.$el.closest("table, .q-table, .m-form-list, .q-dialog"));
                forms.sort((a, b) => (a.$el.compareDocumentPosition(b.$el) & 4 ? -1 : 1));
                return forms[0] || null;
            },
            isStepForm(form) { return this.stepForm() === form; },
            goTo(index, answerParameters, onComplete) {
                const step = this.steps[index];
                if (!step) { if (onComplete) onComplete(); return; }
                this.$root.setUrl(this.$root.getLinkPath(moqui.wizardStepUrl(step.pathWithParams, this.$root.currentParameters, answerParameters)), null, onComplete);
            },
            next() {
                if (this.submitting) return;
                const form = this.stepForm();
                if (!form) { this.goTo(this.activeIndex + 1, null); return; }
                this.submitting = true;
                form.submitForm();
            },
            back() { if (!this.submitting) this.goTo(this.activeIndex - 1, null); },
            stepSubmitting() { this.submitting = true; },
            headerTo(name) {
                const index = this.steps.findIndex((step) => step.name === name);
                if (index >= 0 && index < this.activeIndex) this.goTo(index, null);
            },
            stepRefused() { this.submitting = false; },
            // the spinner stays until the screen the answer names is there (a redirect leaves the page)
            stepAnswered(form, resp) {
                const way = moqui.wizardAfterAnswer(resp, this.$root.currentPathList, this.steps.length, this.activeIndex);
                const done = () => { this.submitting = false; };
                if (way.kind === "redirect") root.location.href = way.url;
                else if (way.kind === "url") this.$root.setUrl(way.url, null, done);
                else if (way.kind === "step") this.goTo(way.index, way.parameters, done);
                else done();
            }
        },
        // before the m-subscreens-active inside registers, as m-subscreens-tabs is mounted before its sibling
        beforeMount() { this.pathIndex = this.$root.activeSubscreens.length; }
    });

    // subscreens-panel type="stack": the first subscreen in menu order is the bottom of the stack and the others open over it, with a
    // breadcrumb back to it; no tabs.
    component("m-subscreens-stack", {
        name: "mSubscreensStack",
        props: { id: String },
        data() { return { pathIndex: -1 }; },
        template:
            '<div class="m-stack" :id="id">' +
                '<q-breadcrumbs v-if="stacked" class="m-stack-crumbs q-mb-sm" active-color="grey-8">' +
                    '<q-breadcrumbs-el icon="arrow_back" :label="base.title" class="cursor-pointer text-primary" @click="goTo(base)"></q-breadcrumbs-el>' +
                    '<q-breadcrumbs-el :label="active.title" class="text-weight-medium"></q-breadcrumbs-el>' +
                '</q-breadcrumbs>' +
                '<m-subscreens-active></m-subscreens-active>' +
            '</div>',
        computed: {
            subscreens() {
                if (this.pathIndex < 0) return [];
                const navMenu = this.$root.navMenuList[this.pathIndex];
                return navMenu && navMenu.subscreens ? navMenu.subscreens : [];
            },
            base() { return this.subscreens[0] || null; },
            active() { return this.subscreens.find((item) => item.active) || null; },
            stacked() { return !!(this.base && this.active && this.active.name !== this.base.name); }
        },
        methods: { goTo(item) { this.$root.setUrl(this.$root.getLinkPath(item.pathWithParams)); } },
        beforeMount() { this.pathIndex = this.$root.activeSubscreens.length; }
    });

    // The layout of a container-split from the viewport, its width and the attributes: stacked below 1024 px (wide is false) or when
    // the side and the main pane's minimum do not fit; the side width kept within [min, max]. Pure, for the node tests.
    moqui.splitLayout = (options) => {
        const sideWidth = Number(options.sideWidth) > 0 ? Number(options.sideWidth) : 320;
        const minMain = Number(options.minMain) > 0 ? Number(options.minMain) : 480;
        const width = Number(options.width) || 0;
        const min = Math.min(200, sideWidth);
        const max = width > 0 ? Math.max(min, width - minMain) : Number.MAX_SAFE_INTEGER;
        const stacked = !options.wide || (width > 0 && width < min + minMain);
        const saved = Number(options.saved);
        const side = Math.round(Math.min(Math.max(saved > 0 ? saved : sideWidth, min), max));
        return { stacked, side, min, max: width > 0 ? max : Math.max(sideWidth, min) * 3 };
    };

    // The height of a split's panes side by side: the height attribute when given, else the viewport below the split's top (less
    // a margin, at least 360 px), so the side list and the main pane each scroll in their own pane and the page does not; none
    // when stacked, where the page scrolls. Pure, for the node tests.
    moqui.splitHeight = (options) => {
        if (options.stacked) return undefined;
        if (options.height) return String(options.height);
        const viewport = Number(options.viewport) || 0;
        if (viewport <= 0) return undefined;
        const top = Math.max(0, Number(options.top) || 0);
        return Math.max(360, Math.round(viewport - top - (Number(options.margin) >= 0 ? Number(options.margin) : 16))) + "px";
    };

    // container-split (style-guide §3.8): a q-splitter with the side at side-width px from 1024 px, draggable between 200 px and what
    // leaves min-main to the main pane, its width kept per id in the session and the user's preferences (SPLIT_WIDTH_<id>); stacked
    // below that, where a selection in the URL (select-parameter) shows the main pane with a back button that clears it, and none
    // shows the side. The panes keep their content across the change, so a form in them keeps its values.
    component("m-container-split", {
        name: "mContainerSplit",
        props: { id: String, sideWidth: { type: [Number, String], default: 320 }, minMain: { type: [Number, String], default: 480 },
            sidePosition: { type: String, default: "left" }, selectParameter: String, backText: String, height: String },
        data() { return { width: 0, side: null, observer: null, saveTimer: null, viewTop: 0, viewport: 0, onResize: null }; },
        template:
            '<div ref="root" class="m-container-split" :id="id" :data-stacked="layout.stacked ? \'true\' : \'false\'"' +
                    ' :class="{\'m-split-stacked\': layout.stacked, \'m-split-right\': right, \'m-split-selected\': selected}">' +
                '<q-splitter :model-value="layout.side" @update:model-value="drag" unit="px" :limits="[layout.min, layout.max]" :reverse="right"' +
                        ' :disable="layout.stacked" :style="paneHeight ? {height: paneHeight} : undefined" separator-class="m-split-separator">' +
                    '<template v-slot:before>' +
                        '<div v-if="right" v-show="showMain" class="m-split-main">' +
                            '<q-btn v-if="backShown" flat dense no-caps icon="arrow_back" :label="backLabel" class="m-split-back" @click="back"></q-btn>' +
                            '<slot name="main"></slot></div>' +
                        '<div v-else v-show="showSide" class="m-split-side"><slot name="side"></slot></div>' +
                    '</template>' +
                    '<template v-slot:after>' +
                        '<div v-if="right" v-show="showSide" class="m-split-side"><slot name="side"></slot></div>' +
                        '<div v-else v-show="showMain" class="m-split-main">' +
                            '<q-btn v-if="backShown" flat dense no-caps icon="arrow_back" :label="backLabel" class="m-split-back" @click="back"></q-btn>' +
                            '<slot name="main"></slot></div>' +
                    '</template>' +
                '</q-splitter>' +
            '</div>',
        computed: {
            right() { return this.sidePosition === "right"; },
            storageKey() { return this.id ? "SPLIT_WIDTH_" + this.id : null; },
            layout() {
                return moqui.splitLayout({ wide: !!(this.$q && this.$q.screen && this.$q.screen.gt.sm), width: this.width, sideWidth: this.sideWidth,
                    minMain: this.minMain, saved: this.side });
            },
            selected() {
                if (!this.selectParameter) return false;
                const value = (this.$root.currentParameters || {})[this.selectParameter];
                return value !== undefined && value !== null && String(value) !== "";
            },
            paneHeight() { return moqui.splitHeight({ stacked: this.layout.stacked, height: this.height, top: this.viewTop, viewport: this.viewport }); },
            showSide() { return !this.layout.stacked || !this.selectParameter || !this.selected; },
            showMain() { return !this.layout.stacked || !this.selectParameter || this.selected; },
            backShown() { return this.layout.stacked && !!this.selectParameter && this.selected; },
            backLabel() { return this.backText || localized("Back"); }
        },
        methods: {
            drag(value) {
                this.side = Math.round(value);
                if (!this.storageKey) return;
                try { root.sessionStorage.setItem("moqui." + this.storageKey, String(this.side)); } catch (err) { /* no storage */ }
                // the preference once the dragging stops
                if (this.saveTimer) clearTimeout(this.saveTimer);
                this.saveTimer = setTimeout(() => {
                    this.saveTimer = null;
                    if (this.$root.setPreference && this.$root.userId) this.$root.setPreference(this.storageKey, String(this.side));
                }, 800);
            },
            // where the split starts in the page and the viewport's height, for the panes' height
            measure() {
                const el = this.$refs.root;
                if (!el || !el.getBoundingClientRect) return;
                this.viewTop = el.getBoundingClientRect().top + (root.scrollY || root.pageYOffset || 0);
                this.viewport = root.innerHeight || 0;
            },
            back() {
                const current = Object.assign({}, this.$root.currentParameters || {});
                delete current[this.selectParameter];
                const search = moqui.objToSearch(current);
                this.$root.setUrl(this.$root.currentLinkPath + (search ? "?" + search : ""));
            }
        },
        created() {
            if (!this.storageKey) return;
            let saved = null;
            try { saved = root.sessionStorage.getItem("moqui." + this.storageKey); } catch (err) { saved = null; }
            if (!saved && qvt.config && qvt.config.preferences) saved = qvt.config.preferences[this.storageKey] || null;
            if (Number(saved) > 0) this.side = Number(saved);
        },
        mounted() {
            const el = this.$refs.root;
            this.width = el.clientWidth;
            this.measure();
            if (root.ResizeObserver) {
                this.observer = new root.ResizeObserver(() => { this.width = el.clientWidth; this.measure(); });
                this.observer.observe(el);
            }
            this.onResize = () => this.measure();
            if (root.addEventListener) root.addEventListener("resize", this.onResize);
        },
        beforeUnmount() {
            if (this.observer) this.observer.disconnect();
            if (this.saveTimer) clearTimeout(this.saveTimer);
            if (this.onResize && root.removeEventListener) root.removeEventListener("resize", this.onResize);
        }
    });

    component("m-menu-nav-item", {
        name: "mMenuNavItem",
        props: { menuIndex: Number },
        template:
            '<q-expansion-item v-if="navMenuItem && navMenuItem.subscreens && navMenuItem.subscreens.length" :model-value="true" :content-inset-level="0.3"' +
                    ' switch-toggle-side dense dense-toggle expand-icon-class="text-grey-6" expanded-icon="arrow_drop_down" :to="navMenuItem.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="navMenuItem" active></m-menu-item-content></template>' +
                '<template v-slot:default><m-menu-subscreen-item v-for="(subscreen, ssIndex) in navMenuItem.subscreens" :key="subscreen.name" :menu-index="menuIndex" :subscreen-index="ssIndex"></m-menu-subscreen-item></template>' +
            '</q-expansion-item>' +
            '<q-expansion-item v-else-if="navMenuItem && navMenuItem.savedFinds && navMenuItem.savedFinds.length" :model-value="true" :content-inset-level="0.3"' +
                    ' switch-toggle-side dense dense-toggle expand-icon-class="text-grey-6" expanded-icon="arrow_drop_down" :to="navMenuItem.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="navMenuItem" active></m-menu-item-content></template>' +
                '<template v-slot:default><q-expansion-item v-for="(savedFind, ssIndex) in navMenuItem.savedFinds" :key="savedFind.name"' +
                        ' :model-value="false" switch-toggle-side dense dense-toggle expand-icon-class="text-grey-6" expand-icon="chevron_right" :to="savedFind.pathWithParams">' +
                    '<template v-slot:header><m-menu-item-content :menu-item="savedFind" :active="savedFind.active"></m-menu-item-content></template>' +
                '</q-expansion-item></template>' +
            '</q-expansion-item>' +
            '<q-expansion-item v-else-if="navMenuItem && menuIndex < (navMenuLength - 1)" :model-value="true" :content-inset-level="0.3"' +
                    ' switch-toggle-side dense dense-toggle expand-icon-class="text-grey-6" expanded-icon="arrow_drop_down" :to="navMenuItem.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="navMenuItem" active></m-menu-item-content></template>' +
                '<template v-slot:default><m-menu-nav-item :menu-index="menuIndex + 1"></m-menu-nav-item></template>' +
            '</q-expansion-item>' +
            '<q-expansion-item v-else-if="navMenuItem" :model-value="false" switch-toggle-side dense dense-toggle expand-icon-class="text-grey-6" expand-icon="arrow_right" :to="navMenuItem.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="navMenuItem" active></m-menu-item-content></template>' +
            '</q-expansion-item>',
        computed: {
            navMenuItem() { return this.$root.navMenuList[this.menuIndex]; },
            navMenuLength() { return this.$root.navMenuList.length; }
        }
    });

    component("m-menu-subscreen-item", {
        name: "mMenuSubscreenItem",
        props: { menuIndex: Number, subscreenIndex: Number },
        template:
            '<m-menu-nav-item v-if="subscreen.active" :menu-index="menuIndex + 1"></m-menu-nav-item>' +
            '<q-expansion-item v-else :model-value="false" switch-toggle-side dense dense-toggle expand-icon-class="text-grey-6" expand-icon="arrow_right" :to="subscreen.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="subscreen"></m-menu-item-content></template>' +
            '</q-expansion-item>',
        computed: { subscreen() { return this.$root.navMenuList[this.menuIndex].subscreens[this.subscreenIndex]; } }
    });

    component("m-menu-item-content", {
        name: "mMenuItemContent",
        props: { menuItem: Object, active: Boolean },
        template:
            '<q-item-section v-if="menuItem.image" avatar>' +
                '<m-menu-icon v-if="menuItem.imageType === \'icon\'" :name="menuItem.image" size="xs"></m-menu-icon>' +
                '<img v-else :src="menuItem.image" :alt="menuItem.title" height="14" class="invertible">' +
            '</q-item-section>' +
            '<q-item-section :class="{\'text-primary text-weight-medium\':active}">{{menuItem.title}}</q-item-section>'
    });

    // The image of a menu entry whose imageType is icon, in the drawer, the breadcrumbs and the screen history: q-icon for a Quasar
    // icon name, an i element with the classes for CSS classes (moqui.isIconClass).
    component("m-menu-icon", {
        name: "mMenuIcon",
        props: { name: String, size: String },
        template: '<i v-if="isClass" :class="name"></i><q-icon v-else :name="name" :size="size"></q-icon>',
        computed: { isClass() { return moqui.isIconClass(this.name); } }
    });

    /* ========== status ========== */

    // How a status-bar draws what org.moqui.impl.StatusServices.get#StatusBar answered (style-guide §3.3, §5.2): each stage a chip,
    // the current one filled in its color, a done one tinted with a check, one to come outlined and muted; each transition a
    // button, the next stage the filled primary action, the others tonal (negative for those that end badly), with the
    // confirmation "Label: title?". Pure, for the node tests; m-status-bar binds what it gives.
    moqui.statusBarView = (bar, options) => {
        const opts = options || {};
        const data = bar && typeof bar === "object" ? bar : {};
        const color = (value) => (value === null || value === undefined || value === "" ? "grey-7" : String(value));
        const stages = (Array.isArray(data.stages) ? data.stages : []).filter((stage) => stage && stage.statusId).map((stage) => {
            const state = stage.state === "current" || stage.state === "done" ? stage.state : "todo";
            const chip = state === "current" ? { color: color(stage.color), "text-color": "white", class: "q-ma-none text-weight-medium m-status-current" } :
                state === "done" ? { icon: "sym_o_check", class: "q-ma-none m-tonal text-grey-8 m-status-done" } :
                    { outline: true, class: "q-ma-none text-grey-7 m-status-todo" };
            return { statusId: String(stage.statusId), label: String(stage.label || stage.statusId), state, chip };
        });
        const title = opts.title ? String(opts.title) : "";
        const buttons = (Array.isArray(data.transitions) ? data.transitions : []).filter((tr) => tr && tr.toStatusId).map((tr) => {
            const label = String(tr.label || tr.toStatusId);
            const button = tr.isNext ? { color: "primary", class: "m-status-next" } :
                { class: "m-btn-tonal " + (tr.isDanger ? "text-negative" : "text-primary") };
            return { toStatusId: String(tr.toStatusId), label, isNext: !!tr.isNext, isDanger: !!tr.isDanger, button,
                confirm: opts.confirm === false ? null : label + (title ? ": " + title : "") + "?" };
        });
        return { current: data.current || null, stages, buttons };
    };

    // The status-bar element: the stage chips and, with an action (the transition), the buttons that post it with the new status
    // in parameterName and the parameters, each in an m-form, so the answer (the screen again, with the new status) is followed
    // as any form's. The server-merged macros write the bar's data in; a server-static screen gives the status and the bar is
    // asked of the root screen's statusBar transition (barUrl), again when the status changes.
    component("m-status-bar", {
        name: "mStatusBar",
        props: { id: String, bar: Object, barUrl: { type: String, default: "/statusBar" }, statusId: String, statusTypeId: String, statusFlowId: String,
            stages: [Array, String], allowedStatusIds: [Array, String], readOnly: Boolean, action: String, parameters: Object,
            parameterName: { type: String, default: "statusId" }, title: String, confirm: { type: Boolean, default: true } },
        data() { return { loaded: null }; },
        template:
            '<div class="m-status-bar row items-center" :id="id" :data-status="view.current ? view.current.statusId : null">' +
                '<div class="m-status-stages row items-center">' +
                    '<template v-for="(stage, index) in view.stages" :key="stage.statusId + \':\' + index">' +
                        '<q-icon v-if="index" name="sym_o_chevron_right" size="xs" class="m-muted m-status-sep"></q-icon>' +
                        '<q-chip dense square :ripple="false" v-bind="stage.chip" :data-stage="stage.statusId" :data-state="stage.state">{{ stage.label }}</q-chip>' +
                    '</template>' +
                '</div>' +
                '<div v-if="action && view.buttons.length" class="m-status-actions row items-center">' +
                    '<m-form v-for="item in view.buttons" :key="item.toStatusId" :action="action" :fields-initial="fieldsOf(item)" class="m-status-form">' +
                        '<q-btn dense no-caps unelevated type="submit" v-bind="item.button" :label="item.label" :data-confirm="item.confirm"' +
                            ' :data-to-status="item.toStatusId"></q-btn>' +
                    '</m-form>' +
                '</div>' +
            '</div>',
        computed: {
            view() { return moqui.statusBarView(this.bar || this.loaded, { title: this.title, confirm: this.confirm }); },
            requestKey() {
                if (this.bar) return null;
                const list = (value) => (Array.isArray(value) ? value.join(",") : value || "");
                return [this.statusId || "", this.statusTypeId || "", this.statusFlowId || "", list(this.stages), this.readOnly ? "ro" : list(this.allowedStatusIds)].join("|");
            }
        },
        watch: { requestKey() { this.load(); } },
        methods: {
            fieldsOf(item) { return Object.assign({}, this.parameters || {}, { [this.parameterName || "statusId"]: item.toStatusId }); },
            load() {
                if (this.bar || !this.statusId) { this.loaded = null; return; }
                const list = (value) => (Array.isArray(value) ? value.join(",") : value || "");
                const params = { statusId: this.statusId, statusTypeId: this.statusTypeId || "", statusFlowId: this.statusFlowId || "", stages: list(this.stages) };
                if (this.readOnly) params.readOnly = "true";
                else if (this.allowedStatusIds !== undefined && this.allowedStatusIds !== null) params.allowedStatusIds = list(this.allowedStatusIds);
                const base = this.$root && this.$root.appRootPath ? this.$root.appRootPath : "";
                moqui.request(base + this.barUrl, { params }).then((result) => { this.loaded = result.json || null; }, moqui.handleAjaxError);
            }
        },
        mounted() { this.load(); }
    });

    /* ========== list views: a form-list as a board or a calendar ========== */

    // The views of a form-list (style-guide §3.6, §3.7, §8.2; form-list views, view-board, view-calendar): the same rows, filters,
    // saved finds and page as the list, shown as cards in columns by a field or on a calendar. The rows are the list's page (the
    // server-merged macros write them in, an m-static-form-list hands over the ones it fetched), so a filter, a sort, a saved find or
    // another page changes what both views show. The helpers are pure, for the node tests (WebrootVue.qvt.views.test.js);
    // m-list-views draws them. Times are wall clock in the user's time zone as UTC day numbers and minutes, so the browser's own zone
    // never counts: a row's time stamp (epoch millis) is read in the zone, and a moved one is posted back as epoch millis.
    const lvText = (value) => (value === null || value === undefined ? "" : String(value));
    const lvLocalize = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
    const LV_DAY_MS = 86400000;
    const LV_PALETTE = ["primary", "secondary", "accent", "info", "positive", "warning", "negative"];
    const lvFormatters = {};
    const lvFormatter = (tz) => {
        const key = tz || "";
        if (lvFormatters[key]) return lvFormatters[key];
        const options = { hourCycle: "h23", year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric" };
        let made;
        try { made = new Intl.DateTimeFormat("en-US", Object.assign({ timeZone: tz || undefined }, options)); } catch (err) { made = new Intl.DateTimeFormat("en-US", options); }
        lvFormatters[key] = made;
        return made;
    };
    moqui.listViews = {
        icons: { list: "sym_o_view_list", board: "sym_o_view_kanban", calendar: "sym_o_calendar_month", tree: "sym_o_account_tree" },
        modeIcons: { month: "sym_o_calendar_view_month", week: "sym_o_calendar_view_week", day: "sym_o_calendar_view_day", list: "sym_o_view_agenda" },
        palette: LV_PALETTE,
        // The views offered: the views attribute (a comma list or a list) without the ones that have no element, else list and each
        // element there is; list stays in the list wherever it is named.
        views(views, board, calendar, tree) {
            const has = { list: true, board: !!board, calendar: !!calendar, tree: !!tree };
            const named = (Array.isArray(views) ? views : lvText(views).split(",")).map((name) => lvText(name).trim()).filter((name) => has[name]);
            const out = named.filter((name, index) => named.indexOf(name) === index);
            if (out.length) return out;
            return ["list"].concat(board ? ["board"] : [], calendar ? ["calendar"] : [], tree ? ["tree"] : []);
        },
        // The board as grouped by field (view-board group-fields): the board itself for its own group-field, else the board with that
        // field, the group's columns and flow (a status field's), and none of the default field's column-order, status type and
        // parameter name.
        boardFor(board, field) {
            if (!board || !field || field === board.groupField) return board;
            const group = (board.groups || []).find((entry) => entry.field === field);
            if (!group) return board;
            return Object.assign({}, board, { groupField: field, columns: group.columns || null, flow: group.flow || null, columnOrder: [], statusTypeId: null,
                statusFlowId: null, parameterName: null });
        },
        // The find of the list's screen without its paging and the client's own keys, for a request of the views (range-find,
        // all-rows, a tree's level); without the names in drop (a tree's parent field and its operators)
        findParams(shown, drop) {
            const out = {};
            const paging = ["pageIndex", "pageSize", "pageNoLimit", "moquiFormName", "moquiSessionToken", "lastStandalone", "renderMode", "view", "peekId", "treeLevel"];
            const dropped = moqui.listViews.list(drop);
            Object.keys(shown || {}).forEach((name) => {
                if (paging.includes(name) || shown[name] === null || shown[name] === undefined) return;
                if (dropped.some((field) => name === field || name.indexOf(field + "_") === 0)) return;
                out[name] = shown[name];
            });
            return out;
        },
        // the rows of a form request's answer: the list's (listName), else the first list in it, else the answer when it is a list
        answerRows(json, listName) {
            if (Array.isArray(json)) return json;
            if (!json || typeof json !== "object") return [];
            if (listName && Array.isArray(json[listName])) return json[listName];
            const found = Object.values(json).find(Array.isArray);
            return Array.isArray(found) ? found : [];
        },
        // a list attribute: a list, or comma text
        list(value) {
            if (Array.isArray(value)) return value.map(lvText).map((item) => item.trim()).filter((item) => item);
            return lvText(value).split(",").map((item) => item.trim()).filter((item) => item);
        },
        isStatusField(name) { return /^statusId$|StatusId$/.test(lvText(name)); },
        // a row's text for a field: what the list shows (its _display text) when the row has one, else the value
        text(row, field) {
            if (!row || !field) return "";
            const shown = row[field + "_display"];
            return lvText(shown !== undefined && shown !== null && shown !== "" ? shown : row[field]);
        },
        number(value) {
            if (value === null || value === undefined || value === "") return 0;
            const number = typeof value === "number" ? value : parseFloat(String(value).replace(/[^0-9.eE+-]/g, ""));
            return isNaN(number) ? 0 : number;
        },
        // the key fields of a row as parameters, for the transition a move posts
        keyParams(row, keyFields) {
            const out = {};
            moqui.listViews.list(keyFields).forEach((name) => { if (row && row[name] !== undefined && row[name] !== null) out[name] = lvText(row[name]); });
            return out;
        },
        // a semantic color for a value: the map's (status colors, a color-map attribute), else one of the brand palette by the value's hash,
        // so the same value always has the same color
        colorOf(value, colors) {
            const key = lvText(value);
            if (colors && colors[key]) return String(colors[key]);
            if (!key) return "grey-7";
            let hash = 0;
            for (let i = 0; i < key.length; i++) hash = (hash * 31 + key.charCodeAt(i)) | 0;
            return LV_PALETTE[Math.abs(hash) % LV_PALETTE.length];
        },
        // A color's text class on a tinted chip, and whether white text reads on it filled (warning is light).
        lightColor(color) { return /^(warning|yellow|amber|lime|grey-[1-4]|white)/.test(lvText(color)); },

        // The columns of a board: the known columns in order (the status flow's, or column-order's, with their labels and colors),
        // then a column for each other value the rows have, in the order of the rows; each with its rows, count and sum. moved holds the
        // value of a card moved here, until the list loads again.
        // The controls of a card whose click is their own, not the card's: links, buttons, form fields and labels, Quasar's buttons,
        // fields, toggles and clickable chips, an in-place edit (m-editable), anything with a button-like role or marked data-card-control.
        controlSelector: "a, button, input, select, textarea, label, summary, [contenteditable='true'], [contenteditable=''], [role='button'], " +
            "[role='checkbox'], [role='switch'], [role='combobox'], [role='link'], [role='menuitem'], .q-btn, .q-field, .q-toggle, .q-checkbox, " +
            ".q-radio, .q-chip--clickable, .editable-label, .q-popup-edit, [data-card-control]",
        // whether a click on target (inside card) was on one of the card's controls
        onControl(target, card) {
            if (!target || typeof target.closest !== "function") return false;
            const hit = target.closest(moqui.listViews.controlSelector);
            if (!hit || hit === card) return false;
            return !card || typeof card.contains !== "function" || card.contains(hit);
        },
        boardColumns(items, board, known, moved) {
            const lv = moqui.listViews;
            const cfg = board || {};
            const group = cfg.groupField;
            const given = Array.isArray(known) ? known : [];
            const columns = [];
            const byValue = {};
            const labelInRows = (value) => {
                const hit = (items || []).find((item) => lvText(item.row[group]) === value);
                return hit ? lv.text(hit.row, group) : "";
            };
            const add = (value, label, color) => {
                const key = lvText(value);
                if (byValue[key]) return byValue[key];
                const column = { value: key, label: label || labelInRows(key) || key || lvLocalize("None"), color: color || "", items: [], count: 0, sum: 0 };
                byValue[key] = column;
                columns.push(column);
                return column;
            };
            const knownOf = (value) => given.find((column) => lvText(column.value) === value);
            const order = lv.list(cfg.columnOrder);
            if (order.length) order.forEach((value) => { const found = knownOf(value); add(value, found && found.label, found && found.color); });
            else given.forEach((column) => add(column.value, column.label, column.color));
            (items || []).forEach((item) => {
                const over = moved && moved[item.index];
                const value = lvText(over && over.group !== undefined ? over.group : item.row[group]);
                // a value outside column-order: the label and color the server sent for it (a status's description), else the row's text
                const found = byValue[value] ? null : knownOf(value);
                const column = byValue[value] || add(value, (found && found.label) || lv.text(item.row, group), (found && found.color) || "");
                column.items.push(item);
                column.count++;
                if (cfg.sumField) column.sum += lv.number(item.row[cfg.sumField]);
            });
            return columns;
        },

        // ---- time: UTC day numbers and minutes of the wall clock in a zone ----
        dayOf(year, month, day) { return Math.floor(Date.UTC(year, month - 1, day) / LV_DAY_MS); },
        ymd(day) { const d = new Date(day * LV_DAY_MS); return { y: d.getUTCFullYear(), m: d.getUTCMonth() + 1, d: d.getUTCDate(), dow: d.getUTCDay() }; },
        dayKey(day) { const p = moqui.listViews.ymd(day); return p.y + "-" + String(p.m).padStart(2, "0") + "-" + String(p.d).padStart(2, "0"); },
        wall(millis, tz) {
            const parts = {};
            lvFormatter(tz).formatToParts(new Date(Number(millis))).forEach((part) => { parts[part.type] = part.value; });
            let hour = Number(parts.hour);
            if (hour === 24) hour = 0;
            return { day: moqui.listViews.dayOf(Number(parts.year), Number(parts.month), Number(parts.day)), minutes: hour * 60 + Number(parts.minute) };
        },
        // the epoch millis of a wall clock time in a zone (the later of the two in a fall-back hour, the shifted one in a spring-forward gap)
        millis(day, minutes, tz) {
            const target = day * LV_DAY_MS + minutes * 60000;
            let guess = target;
            for (let i = 0; i < 3; i++) {
                const shown = moqui.listViews.wall(guess, tz);
                const diff = target - (shown.day * LV_DAY_MS + shown.minutes * 60000);
                if (diff === 0) return guess;
                guess += diff;
            }
            return guess;
        },
        today(tz, now) { return moqui.listViews.wall(now === undefined ? Date.now() : now, tz); },
        // A value of a row as a time: epoch millis (a time stamp as the server sends one) in the zone, a date (yyyy-MM-dd) as a date
        // only, a wall clock text (yyyy-MM-dd HH:mm) as it is, other text with a zone by Date.parse; null for nothing.
        parseTime(value, tz) {
            const lv = moqui.listViews;
            if (value === null || value === undefined || value === "") return null;
            if (typeof value === "number") return Object.assign(lv.wall(value, tz), { dateOnly: false });
            const text = String(value).trim();
            if (/^-?\d{9,}$/.test(text)) return Object.assign(lv.wall(Number(text), tz), { dateOnly: false });
            let match = /^(\d{4})-(\d\d)-(\d\d)$/.exec(text);
            if (match) return { day: lv.dayOf(+match[1], +match[2], +match[3]), minutes: 0, dateOnly: true };
            match = /^(\d{4})-(\d\d)-(\d\d)[T ](\d\d):(\d\d)(?::\d\d(?:\.\d+)?)?$/.exec(text);
            if (match) return { day: lv.dayOf(+match[1], +match[2], +match[3]), minutes: +match[4] * 60 + +match[5], dateOnly: false };
            const parsed = Date.parse(text);
            return isNaN(parsed) ? null : Object.assign(lv.wall(parsed, tz), { dateOnly: false });
        },
        // what a moved time posts: a date for a date-only value, else epoch millis of the wall clock in the zone
        timeOut(abs, dateOnly, tz) {
            const day = Math.floor(abs / 1440);
            return dateOnly ? moqui.listViews.dayKey(day) : String(moqui.listViews.millis(day, abs - day * 1440, tz));
        },
        // wall clock text for a form field (m-date-time): yyyy-MM-dd HH:mm, or yyyy-MM-dd
        timeText(abs, dateOnly) {
            const day = Math.floor(abs / 1440);
            const minutes = abs - day * 1440;
            return moqui.listViews.dayKey(day) + (dateOnly ? "" : " " + String(Math.floor(minutes / 60)).padStart(2, "0") + ":" + String(minutes % 60).padStart(2, "0"));
        },
        weekStart(day, firstDay) {
            const dow = moqui.listViews.ymd(day).dow;
            return day - ((dow - (firstDay || 0) + 7) % 7);
        },
        monthStart(day) { const p = moqui.listViews.ymd(day); return moqui.listViews.dayOf(p.y, p.m, 1); },
        // the weeks of the month of anchor: from the week of its first day through the week of its last
        monthWeeks(anchor, firstDay) {
            const lv = moqui.listViews;
            const p = lv.ymd(anchor);
            const first = lv.dayOf(p.y, p.m, 1);
            const last = lv.dayOf(p.m === 12 ? p.y + 1 : p.y, p.m === 12 ? 1 : p.m + 1, 1) - 1;
            const weeks = [];
            for (let start = lv.weekStart(first, firstDay); start <= last; start += 7) {
                const days = [];
                for (let i = 0; i < 7; i++) days.push(start + i);
                weeks.push(days);
            }
            return weeks;
        },
        // a month, week or day later or earlier
        shift(anchor, mode, by) {
            const lv = moqui.listViews;
            if (mode === "week") return anchor + 7 * by;
            if (mode === "day") return anchor + by;
            const p = lv.ymd(anchor);
            const months = p.y * 12 + (p.m - 1) + by;
            const year = Math.floor(months / 12);
            const month = months - year * 12 + 1;
            const length = lv.dayOf(month === 12 ? year + 1 : year, month === 12 ? 1 : month + 1, 1) - lv.dayOf(year, month, 1);
            return lv.dayOf(year, month, Math.min(p.d, length));
        },
        // The days a calendar mode shows ({first, last}): the month's weeks, the week, the day, or for the list (agenda) the anchor's month.
        visibleDays(mode, anchor, firstDay) {
            const lv = moqui.listViews;
            if (mode === "week") { const start = lv.weekStart(anchor, firstDay); return { first: start, last: start + 6 }; }
            if (mode === "day") return { first: anchor, last: anchor };
            if (mode === "month") { const weeks = lv.monthWeeks(anchor, firstDay); return { first: weeks[0][0], last: weeks[weeks.length - 1][6] }; }
            const p = lv.ymd(anchor);
            return { first: lv.monthStart(anchor), last: lv.dayOf(p.m === 12 ? p.y + 1 : p.y, p.m === 12 ? 1 : p.m + 1, 1) - 1 };
        },
        // The finds of a calendar's dates (view-calendar range-find): the list's find with the start and end field range added
        // ({field}_from and {field}_thru, as search-form-inputs reads them) and no paging, a day wider on each side (a date field
        // reads a time stamp in UTC), as epoch millis. With an end field two: what overlaps the dates (starts before their end and ends
        // after their start), and what starts in them (an event with no end); else what starts in them.
        rangeFinds(cal, days, tz) {
            const lv = moqui.listViews;
            const from = String(lv.millis(days.first - 1, 0, tz));
            const thru = String(lv.millis(days.last + 2, 0, tz) - 1);
            const starts = { pageNoLimit: "true" };
            starts[cal.startField + "_from"] = from;
            starts[cal.startField + "_thru"] = thru;
            if (!cal.endField) return [starts];
            const overlaps = { pageNoLimit: "true" };
            overlaps[cal.startField + "_thru"] = thru;
            overlaps[cal.endField + "_from"] = from;
            return [overlaps, starts];
        },
        // The rows of a range's finds as the calendar's items, each once (by its key fields, else all its values): a row on the list's
        // page is that item (its index, so its card_{index} slot, and its values as the page has them), another is an item of its own
        // with an index from 100000.
        rangeItems(pageItems, lists, keyFields) {
            const keys = moqui.listViews.list(keyFields);
            const keyOf = (row) => JSON.stringify(keys.length ? keys.map((name) => lvText(row[name])) : row);
            const page = {};
            (pageItems || []).forEach((item) => { page[keyOf(item.row)] = item; });
            const seen = new Set();
            const out = [];
            let next = 100000;
            (lists || []).forEach((rows) => (Array.isArray(rows) ? rows : []).forEach((row) => {
                if (!row || typeof row !== "object" || row._moquiRowType) return;
                const key = keyOf(row);
                if (seen.has(key)) return;
                seen.add(key);
                out.push(page[key] || { index: next++, row });
            }));
            return out;
        },

        // The events of a calendar: each row with a start, its title, color and resource; start and end are minutes from the epoch day 0 of
        // the wall clock (end exclusive). An all-day event (a date-only start, or the all-day field Y) covers whole days: a date-only end is the
        // last day, a time stamp end at midnight the day after. A timed event without an end is an hour long. moved and preview hold where a
        // drag put an event ({start, end, resource} in the same minutes).
        events(items, cal, tz, moved, preview) {
            const lv = moqui.listViews;
            const cfg = cal || {};
            const out = [];
            (items || []).forEach((item) => {
                const row = item.row;
                const start = lv.parseTime(row[cfg.startField], tz);
                if (!start) return;
                const end = cfg.endField ? lv.parseTime(row[cfg.endField], tz) : null;
                const flag = cfg.allDayField ? lvText(row[cfg.allDayField]) : "";
                const allDay = start.dateOnly || flag === "Y" || flag === "true";
                let from = start.day * 1440 + (allDay ? 0 : start.minutes);
                let thru;
                if (allDay) {
                    const endDay = end ? (end.dateOnly || end.minutes > 0 ? end.day + 1 : end.day) : start.day + 1;
                    thru = Math.max(endDay, start.day + 1) * 1440;
                } else {
                    thru = end ? end.day * 1440 + end.minutes : from + 60;
                    if (thru <= from) thru = from + 30;
                }
                let resource = cfg.resourceField ? lvText(row[cfg.resourceField]) : null;
                const over = (preview && preview.index === item.index ? preview : null) || (moved && moved[item.index]) || null;
                if (over) {
                    if (over.start !== undefined) from = over.start;
                    if (over.end !== undefined) thru = over.end;
                    if (over.resource !== undefined && over.resource !== null) resource = over.resource;
                }
                const colorValue = cfg.colorField ? row[cfg.colorField] : null;
                out.push({ index: item.index, key: String(item.index), row, title: lv.text(row, cfg.titleField) || lvLocalize("(no title)"), allDay, start: from, end: thru,
                    startDateOnly: start.dateOnly, endDateOnly: end ? end.dateOnly : start.dateOnly, resource,
                    color: cfg.colorField ? lv.colorOf(colorValue, cfg.colors) : "primary" });
            });
            return out.sort((a, b) => a.start - b.start || b.end - a.end || a.index - b.index);
        },
        // The places of the timed events of one day column: each in a lane of its group of overlapping events, lanes the group's width.
        layoutDay(list) {
            const sorted = (list || []).slice().sort((a, b) => a.start - b.start || b.end - a.end);
            const out = [];
            let group = [];
            let lanes = [];
            let groupEnd = -Infinity;
            const close = () => { group.forEach((entry) => { entry.lanes = lanes.length; }); group = []; lanes = []; };
            sorted.forEach((ev) => {
                if (group.length && ev.start >= groupEnd) { close(); groupEnd = -Infinity; }
                let lane = lanes.findIndex((laneEnd) => laneEnd <= ev.start);
                if (lane < 0) { lane = lanes.length; lanes.push(ev.end); } else lanes[lane] = ev.end;
                const entry = { ev, lane, lanes: 1 };
                group.push(entry);
                out.push(entry);
                groupEnd = Math.max(groupEnd, ev.end);
            });
            close();
            return out;
        },
        // What a moved or resized event posts: its key fields, the transition's parameters, and the start and end as the row had them (a
        // date for a date-only value, else epoch millis; a date-only end is the last day), the resource when it changed.
        moveParams(ev, cal, tz, change) {
            const lv = moqui.listViews;
            const cfg = cal || {};
            const out = Object.assign({}, cfg.parameters || {}, lv.keyParams(ev.row, cfg.keyFields));
            out[cfg.startParameter || cfg.startField] = lv.timeOut(change.start, ev.startDateOnly, tz);
            if (cfg.endField) out[cfg.endParameter || cfg.endField] = ev.endDateOnly ? lv.timeOut(change.end - 1, true, tz) : lv.timeOut(change.end, false, tz);
            if (cfg.resourceField && change.resource !== undefined && change.resource !== null && change.resource !== ev.resource) {
                out[cfg.resourceParameter || cfg.resourceField] = change.resource;
            }
            return out;
        },
        snap(minutes, step) { return Math.round(minutes / step) * step; }
    };

    // The board and calendar of a form-list (the views attribute and the view-board and view-calendar elements), around the list. The list
    // is the default slot: in the board and calendar its rows are hidden and its nav row (find options, saved finds, the page buttons) and
    // the head's find fields stay. A card is the card_{index} slot the server-merged macros write for each row, else the card slot with
    // {index, row} (an m-static-form-list gives it over the row's scope). The view is the URL's view parameter, else the user's choice
    // (the LIST_VIEW_{form} preference, set on a switch), else the first. A card dragged to another column posts the board's transition
    // with the row's key fields and the new value; a refusal puts it back with the answer's message. The calendar shows month, week, day
    // and list (agenda) of the rows by their start (and end), colors by the color field, a drag moves an event and its lower edge
    // resizes it (the transition with the new start and end, and resource), a click on empty time opens the quick create (the
    // create-transition's form, or the create-screen) with the time in it. On a phone the board scrolls sideways and the calendar
    // offers day and list. A board with group-fields has a Group by choice (kept for the tab), and with all-rows it asks the screen's
    // form request for every row of the find while it shows. The tree is m-list-tree over the same rows (a tree_{index} slot per row of
    // the page, else the tree slot). With a peek-url and from 1280 px, the Peek toggle puts a pane at the right of the views: a click on
    // a row, a card or a tree row (not on a control) loads its detail there with peek=true, Up and Down move to the next row, Escape
    // closes it; the key is in the URL as peekId, and the pane's width is kept for the tab.
    let listViewsCount = 0;
    component("m-list-views", {
        name: "mListViews",
        inject: { mScreen: { default: null } },
        props: { id: String, formName: String, formLocation: String, views: [Array, String], view: String, rows: { type: Array, default: () => [] },
            board: Object, calendar: Object, tree: Object, peek: Object, infoUrl: { type: String, default: "/listViewColumns" }, reload: Function },
        data() {
            return { current: "list", uid: "mlv" + (++listViewsCount), moved: {}, dirty: false, info: null, infoKey: "", dragging: null, dragEnded: 0,
                calMode: "", anchor: 0, drag: null, preview: null, selection: null, pop: { open: false, index: null, when: "" }, anchorRect: { left: 0, top: 0, width: 0, height: 0 },
                create: { open: false, initial: {}, url: "", dateType: "date-time" }, hourPx: 44, nowAbs: 0, range: { key: "", seq: 0, rows: null },
                groupBy: "", all: { key: "", seq: 0, rows: null }, peekOn: false, peekKey: "", peekWidth: 0, treeKeys: [] };
        },
        template:
            '<div class="m-list-views" :id="id" :class="[\'m-list-views--\' + current, {\'m-list-views--peek\': peekShown}]" :data-view="current">' +
                '<div v-if="viewOptions.length > 1 || peekAvailable" class="m-list-views-bar row items-center no-wrap">' +
                    '<q-btn-dropdown v-if="current === \'board\' && groupOptions.length > 1" flat dense no-caps class="m-board-group" :label="labels.groupBy + \': \' + groupLabel">' +
                        '<q-list dense><q-item v-for="option in groupOptions" :key="option.field" clickable v-close-popup :active="option.field === boardView.groupField"' +
                                ' :data-group="option.field" @click="setGroup(option.field)"><q-item-section>{{option.label}}</q-item-section></q-item></q-list>' +
                    '</q-btn-dropdown>' +
                    '<q-space></q-space>' +
                    '<q-btn v-if="peekAvailable" flat dense no-caps icon="sym_o_right_panel_open" class="m-peek-toggle q-mr-sm" :class="{\'text-primary\': peekOn}"' +
                            ' :label="labels.peek" :aria-pressed="peekOn ? \'true\' : \'false\'" @click="setPeek(!peekOn)"><q-tooltip>{{labels.peekTip}}</q-tooltip></q-btn>' +
                    '<q-btn-toggle v-if="viewOptions.length > 1" :model-value="current" @update:model-value="setView" dense unelevated no-caps toggle-color="primary" class="m-list-views-toggle" :options="viewOptions">' +
                        '<template v-slot:list><q-tooltip>{{labels.list}}</q-tooltip></template>' +
                        '<template v-slot:board><q-tooltip>{{labels.board}}</q-tooltip></template>' +
                        '<template v-slot:calendar><q-tooltip>{{labels.calendar}}</q-tooltip></template>' +
                        '<template v-slot:tree><q-tooltip>{{labels.tree}}</q-tooltip></template>' +
                    '</q-btn-toggle>' +
                '</div>' +
                '<div ref="body" class="m-list-views-body" :class="{\'row no-wrap\': peekShown}">' +
                '<div class="m-list-views-content" :class="{\'col\': peekShown}" @click="onPeekClick">' +
                '<div class="m-list-views-list"><slot></slot></div>' +
                // ---- the tree ----
                '<m-list-tree v-if="current === \'tree\' && tree" ref="tree" :tree="tree" :items="items" :pref-key="prefKey" :find="findOf" :post="post"' +
                        ' :peekable="peekShown" :selected="peekShown ? peekKey : \'\'" :peek-field="peek ? peek.keyField : \'\'" @select="(row) => peekSelect(peekKeyOf(row))"' +
                        ' @moved="reloadRows" @focusrow="(row) => treeFocus(peekKeyOf(row))" @keys="(rows) => { treeKeys = rows.map(peekKeyOf); }">' +
                    '<template v-for="name in treeSlotNames()" :key="name" v-slot:[name]="props"><slot :name="name" v-bind="props"></slot></template>' +
                '</m-list-tree>' +
                // ---- the board ----
                '<div v-if="current === \'board\'" ref="board" class="m-board row no-wrap" :class="{\'m-board--dragging\': !!dragging}">' +
                    '<div v-for="column in columns" :key="column.value" class="m-board-col column no-wrap" :data-value="column.value"' +
                            ' :class="dragging ? (column.value === dragging.from ? \'m-board-col--from\' : (dragAllowed(column.value) ? \'m-board-col--target\' : \'m-board-col--blocked\')) : \'\'">' +
                        '<div class="m-board-col-head row items-center no-wrap">' +
                            '<span class="m-board-col-dot" :class="\'bg-\' + (column.color || \'grey-6\')"></span>' +
                            '<span class="m-board-col-label ellipsis">{{column.label}}</span>' +
                            '<q-badge class="m-board-col-count m-tonal q-ml-xs" :class="column.color ? \'text-\' + column.color : \'text-grey-8\'">{{column.count}}</q-badge>' +
                            '<q-space></q-space>' +
                            '<span v-if="board.sumField" class="m-board-col-sum m-muted">{{sumText(column.sum)}}</span>' +
                        '</div>' +
                        '<div class="m-board-cards col" :data-value="column.value">' +
                            '<div v-for="item in column.items" :key="item.index" class="m-board-card" :data-index="item.index" @click="openCard($event, item)"' +
                                    ' :class="{\'m-board-card--selected\': peekShown && !!peekKey && peekKey === peekKeyOf(item.row)}">' +
                                '<slot :name="\'card_\' + item.index" :index="item.index" :row="item.row"><slot name="card" :index="item.index" :row="item.row">' +
                                    '<div class="m-card-title">{{cardTitle(item.row)}}</div></slot></slot>' +
                            '</div>' +
                        '</div>' +
                        '<div v-if="!column.count" class="m-board-empty m-muted">{{labels.noCards}}</div>' +
                    '</div>' +
                    '<div v-if="!columns.length" class="m-board-empty m-muted q-pa-md">{{labels.noCards}}</div>' +
                '</div>' +
                // ---- the calendar ----
                '<div v-if="current === \'calendar\'" class="m-calendar" :class="{\'m-calendar--dragging\': !!drag && drag.moved}">' +
                    '<div class="m-calendar-bar row items-center no-wrap">' +
                        '<q-btn unelevated dense no-caps class="m-btn-tonal q-px-sm" :label="labels.today" @click="goToday"></q-btn>' +
                        '<template v-if="calMode !== \'list\'">' +
                            '<q-btn flat dense round icon="chevron_left" class="m-cal-prev" @click="shift(-1)"><q-tooltip>{{labels.previous}}</q-tooltip></q-btn>' +
                            '<q-btn flat dense round icon="chevron_right" class="m-cal-next" @click="shift(1)"><q-tooltip>{{labels.next}}</q-tooltip></q-btn>' +
                        '</template>' +
                        '<div class="m-calendar-title ellipsis q-ml-sm" :class="phone ? \'text-subtitle1 text-weight-medium\' : \'text-h6\'">{{rangeTitle}}</div>' +
                        '<q-space></q-space>' +
                        '<q-btn-toggle v-if="modeOptions.length > 1" :model-value="calMode" @update:model-value="setMode" dense no-caps unelevated toggle-color="primary" class="m-cal-modes" :options="modeOptions"></q-btn-toggle>' +
                    '</div>' +
                    // month
                    '<q-card v-if="calMode === \'month\'" flat bordered class="m-cal-month">' +
                        '<div class="row no-wrap m-cal-dow"><div v-for="name in weekdayNames" :key="name" class="col text-center text-caption m-muted">{{name}}</div></div>' +
                        '<div v-for="week in monthWeeks" :key="week.key" class="row no-wrap m-cal-week">' +
                            '<div v-for="cell in week.days" :key="cell.day" class="col m-cal-cell" :class="{\'m-cal-cell--out\': !cell.inMonth, \'m-cal-cell--sel\': selection && selection.day === cell.day}"' +
                                    ' :data-day="cell.day" data-zone="month" @pointerdown="onCellDown($event, cell.day, \'month\', null)">' +
                                '<div class="m-cal-daynum"><span :class="{\'m-cal-today\': cell.isToday}" @pointerdown.stop @click.stop="goDay(cell.day)">{{cell.dayNum}}</span></div>' +
                                '<div v-for="ev in cell.items.slice(0, cell.limit)" :key="ev.key" class="m-cal-chip ellipsis" :class="chipClass(ev)" :data-index="ev.index"' +
                                        ' @pointerdown.stop="onEventDown($event, ev, \'move\')">' +
                                    '<template v-if="ev.allDay">{{ev.title}}</template>' +
                                    '<template v-else><span class="m-cal-dot" :class="\'bg-\' + ev.color"></span><span class="m-cal-chip-time">{{ev.shortTime}}</span> {{ev.title}}</template>' +
                                '</div>' +
                                '<div v-if="cell.items.length > cell.limit" class="m-cal-more text-primary" @pointerdown.stop @click.stop="goDay(cell.day)">+{{cell.items.length - cell.limit}} {{labels.more}}</div>' +
                            '</div>' +
                        '</div>' +
                    '</q-card>' +
                    // week and day: the time grid, columns of days or, for a day with a resource field, of resources
                    '<q-card v-else-if="calMode === \'week\' || calMode === \'day\'" flat bordered class="m-cal-grid">' +
                        '<div class="row no-wrap m-cal-head">' +
                            '<div class="m-cal-gutter"></div>' +
                            '<div v-for="col in gridCols" :key="col.key" class="col m-cal-head-cell text-center ellipsis" :class="{\'m-cal-head-cell--today\': col.isToday}" @click="col.resource === null ? goDay(col.day) : null">' +
                                '<div class="text-caption" :class="col.isToday ? \'text-primary text-weight-medium\' : \'m-muted\'">{{col.top}}</div><div class="m-cal-head-label">{{col.label}}</div></div>' +
                        '</div>' +
                        '<div class="row no-wrap m-cal-allday">' +
                            '<div class="m-cal-gutter text-caption m-muted">{{labels.allDay}}</div>' +
                            '<div v-for="col in gridCols" :key="col.key" class="col m-cal-allday-cell" :data-day="col.day" :data-res="col.resource" data-zone="allday"' +
                                    ' @pointerdown="onCellDown($event, col.day, \'allday\', col.resource)">' +
                                '<div v-for="ev in col.allDay" :key="ev.key" class="m-cal-chip ellipsis" :class="chipClass(ev)" :data-index="ev.index" @pointerdown.stop="onEventDown($event, ev, \'move\')">{{ev.title}}</div>' +
                            '</div>' +
                        '</div>' +
                        '<div ref="calScroll" class="m-cal-scroll">' +
                            '<div class="row no-wrap m-cal-hours" :style="{height: (24 * hourPx) + \'px\'}">' +
                                '<div class="m-cal-gutter relative-position">' +
                                    '<div v-for="hour in 23" :key="hour" class="m-cal-hour-label text-caption m-muted" :style="{top: (hour * hourPx - 8) + \'px\'}">{{hourLabel(hour)}}</div>' +
                                '</div>' +
                                '<div v-for="col in gridCols" :key="col.key" class="col m-cal-col" :style="{backgroundSize: \'100% \' + hourPx + \'px\'}" :data-day="col.day" :data-res="col.resource" data-zone="time"' +
                                        ' @pointerdown="onCellDown($event, col.day, \'time\', col.resource)">' +
                                    '<div v-for="place in col.timed" :key="place.ev.key" class="m-cal-event" :class="eventClass(place.ev)" :style="eventStyle(place)" :data-index="place.ev.index"' +
                                            ' @pointerdown.stop="onEventDown($event, place.ev, \'move\')">' +
                                        '<div class="m-cal-event-title ellipsis">{{place.ev.title}}</div>' +
                                        '<div v-if="(place.ev.end - place.ev.start) >= 45" class="m-cal-event-time ellipsis">{{place.ev.timeText}}</div>' +
                                        '<div v-if="canMove && !phone" class="m-cal-resize" @pointerdown.stop="onEventDown($event, place.ev, \'resize\')"></div>' +
                                    '</div>' +
                                    '<div v-if="selection && selection.zone === \'time\' && selection.col === col.key" class="m-cal-selection" :style="spanStyle(selection.start, selection.end, col.day)">{{selectionText}}</div>' +
                                    '<div v-if="col.isToday" class="m-cal-now" :style="{top: (nowMinutes / 60 * hourPx) + \'px\'}"></div>' +
                                '</div>' +
                            '</div>' +
                        '</div>' +
                    '</q-card>' +
                    // list: the agenda of the rows, by day
                    '<q-card v-else flat bordered class="m-cal-agenda">' +
                        '<template v-for="(group, groupIndex) in agenda" :key="group.day">' +
                            '<q-separator v-if="groupIndex > 0"></q-separator>' +
                            '<div class="m-cal-agenda-day text-subtitle2 q-px-md q-pt-sm" :class="group.isToday ? \'text-primary\' : \'\'">{{group.label}}</div>' +
                            '<q-list dense class="q-pb-xs">' +
                                '<q-item v-for="ev in group.items" :key="ev.key" clickable class="m-cal-agenda-item" :data-index="ev.index" @click="openEvent($event, ev)">' +
                                    '<q-item-section side class="m-cal-agenda-time text-caption">{{ev.allDay ? labels.allDay : (phone ? ev.shortTime : ev.timeText)}}</q-item-section>' +
                                    '<q-item-section avatar class="m-cal-agenda-dot"><span class="m-cal-dot" :class="\'bg-\' + ev.color"></span></q-item-section>' +
                                    '<q-item-section><q-item-label>{{ev.title}}</q-item-label><q-item-label v-if="ev.resourceLabel" caption>{{ev.resourceLabel}}</q-item-label></q-item-section>' +
                                '</q-item>' +
                            '</q-list>' +
                        '</template>' +
                        '<div v-if="!agenda.length" class="q-pa-md m-muted">{{labels.noEvents}}</div>' +
                    '</q-card>' +
                    // the event's card, by the event clicked
                    '<div :id="uid + \'_anchor\'" class="m-cal-anchor" :style="{left: anchorRect.left + \'px\', top: anchorRect.top + \'px\', width: anchorRect.width + \'px\', height: anchorRect.height + \'px\'}"></div>' +
                    '<q-menu v-model="pop.open" :target="\'#\' + uid + \'_anchor\'" no-parent-event anchor="bottom left" self="top left" class="m-cal-pop">' +
                        '<div v-if="pop.index !== null" class="m-cal-pop-body q-pa-sm">' +
                            '<div class="text-caption m-muted q-mb-xs">{{pop.when}}</div>' +
                            '<slot :name="\'card_\' + pop.index" :index="pop.index" :row="popRow"><slot name="card" :index="pop.index" :row="popRow">' +
                                '<div class="m-card-title">{{cardTitle(popRow)}}</div></slot></slot>' +
                        '</div>' +
                    '</q-menu>' +
                    // the quick create
                    '<m-dialog v-model="create.open" :title="labels.newEvent" width="520">' +
                        '<template v-if="create.open">' +
                            '<m-dynamic-container v-if="create.url" :id="uid + \'_create\'" :url="create.url"></m-dynamic-container>' +
                            '<m-form v-else :action="calendar.createTransition" :fields-initial="create.initial" @submitted="afterCreate" v-slot="formProps">' +
                                '<m-text-line v-if="calendar.titleField" v-model="formProps.fields[calendar.titleField]" :name="calendar.titleField" :id="uid + \'_ctitle\'" :label="labels.title" required></m-text-line>' +
                                '<div class="row q-col-gutter-sm q-mt-xs">' +
                                    '<div class="col-12 col-sm-6"><m-date-time v-model="formProps.fields[startName]" :name="startName" :id="uid + \'_cstart\'" :type="create.dateType" :label="labels.starts"></m-date-time></div>' +
                                    '<div v-if="calendar.endField" class="col-12 col-sm-6"><m-date-time v-model="formProps.fields[endName]" :name="endName" :id="uid + \'_cend\'" :type="create.dateType" :label="labels.ends"></m-date-time></div>' +
                                '</div>' +
                                '<div class="row justify-end q-mt-md"><q-btn unelevated dense no-caps color="primary" type="submit" class="q-px-md m-cal-create-submit" :label="labels.create"></q-btn></div>' +
                            '</m-form>' +
                        '</template>' +
                    '</m-dialog>' +
                '</div>' +
                '</div>' +
                // ---- the peek pane ----
                '<div v-if="peekShown" class="m-peek-sep" role="separator" aria-orientation="vertical" @pointerdown="onPeekResize"></div>' +
                '<div v-if="peekShown" class="m-peek-pane" :style="{width: peekWidth ? peekWidth + \'px\' : \'55%\'}">' +
                    '<div class="m-peek-bar row items-center no-wrap">' +
                        '<q-space></q-space>' +
                        '<q-btn v-if="peekKey" flat dense no-caps icon="sym_o_open_in_full" class="m-peek-full" :label="labels.openFull" @click="openFull"></q-btn>' +
                        '<q-btn flat dense round icon="sym_o_close" class="m-peek-close" @click="peekSelect(\'\')"><q-tooltip>{{labels.close}}</q-tooltip></q-btn>' +
                    '</div>' +
                    '<m-dynamic-container v-if="peekSrc" :key="peekSrc" :id="uid + \'_peek\'" :url="peekSrc"></m-dynamic-container>' +
                    '<div v-else class="m-peek-empty m-muted q-pa-md">{{labels.peekEmpty}}</div>' +
                '</div>' +
                '</div>' +
            '</div>',
        computed: {
            lv() { return moqui.listViews; },
            labels() {
                const l = lvLocalize;
                return { list: l("List"), board: l("Board"), calendar: l("Calendar"), today: l("Today"), previous: l("Previous"), next: l("Next"), allDay: l("All day"),
                    more: l("more"), noCards: l("No cards"), noEvents: l("Nothing to show"), newEvent: l("New"), title: l("Title"), starts: l("Starts"), ends: l("Ends"),
                    create: l("Create"), month: l("Month"), week: l("Week"), day: l("Day"), agenda: l("List"), tree: l("Tree"), groupBy: l("Group by"), peek: l("Peek"),
                    peekTip: l("Open a row beside the list"), openFull: l("Open full"), close: l("Close"), peekEmpty: l("Select a row to see it here") };
            },
            phone() { return !!(this.$q && this.$q.screen && this.$q.screen.lt.sm); },
            viewList() { return moqui.listViews.views(this.views, this.board, this.calendar, this.tree); },
            // ---- group by (view-board group-fields) and all-rows ----
            boardView() { return moqui.listViews.boardFor(this.board, this.groupBy); },
            groupOptions() { return (this.board && this.board.groups) || []; },
            groupLabel() { const found = this.groupOptions.find((option) => option.field === this.boardView.groupField); return found ? found.label : ""; },
            allOn() { return !!(this.board && this.board.allRows && this.board.rowsUrl); },
            boardItems() { return this.allOn && this.all.rows ? this.all.rows : this.items; },
            // ---- peek (form-list peek-url): from 1280 px ----
            peekAvailable() { return !!(this.peek && this.peek.url) && !!(this.$q && this.$q.screen && this.$q.screen.width >= 1280); },
            peekShown() { return this.peekAvailable && this.peekOn; },
            peekSrc() {
                if (!this.peekShown || !this.peekKey) return "";
                // the screen's own path, as the calendar's create-screen (the shell's link path is for the address bar)
                const params = Object.assign({}, this.peek.parameters || {}, { [this.peek.keyField]: this.peekKey, peek: "true" });
                return this.peek.url + "?" + moqui.objToSearch(params);
            },
            viewOptions() { return this.viewList.map((name) => ({ value: name, icon: moqui.listViews.icons[name], slot: name, attrs: { "aria-label": this.labels[name], "data-view": name } })); },
            timeZone() { const config = qvt.config || moqui.config || {}; return config.timeZone || (this.$root && this.$root.timeZone) || ""; },
            locale() { const config = qvt.config || moqui.config || {}; return lvText(config.locale || (this.$root && this.$root.locale) || "").replace("_", "-") || undefined; },
            prefKey() { return "LIST_VIEW_" + (this.formLocation || this.formName || ""); },
            peekPref() { return "LIST_PEEK_" + (this.formLocation || this.formName || ""); },
            // the rows the views show: the list's page without its total row, each with its index (the card_{index} slot)
            items() { return (this.rows || []).map((row, index) => ({ index, row: row || {} })).filter((item) => !item.row._moquiRowType); },
            // ---- board ----
            columns() {
                if (!this.board) return [];
                const known = this.boardView.columns || (this.info && this.info.columns) || [];
                return moqui.listViews.boardColumns(this.boardItems, this.boardView, known, this.moved);
            },
            flow() { return (this.boardView && this.boardView.flow) || (this.info && this.info.transitions) || null; },
            canMove() { return !!(this.calendar && this.calendar.transition); },
            // the calendar's rows: those of the dates shown when it asks for them (range-find), else the list's page
            rangeOn() { return !!(this.calendar && this.calendar.rangeFind && this.calendar.rangeUrl); },
            calItems() { return this.rangeOn && this.range.rows ? this.range.rows : this.items; },
            // ---- calendar ----
            calendarColors() { return Object.assign({}, this.info && this.info.colors, this.calendar && this.calendar.colors); },
            events() {
                if (!this.calendar) return [];
                const cfg = Object.assign({}, this.calendar, { colors: this.calendarColors });
                const lv = moqui.listViews;
                const resources = this.resourceLabels;
                return lv.events(this.calItems, cfg, this.timeZone, this.moved, this.preview).map((ev) => Object.assign(ev, {
                    timeText: this.timeRange(ev), shortTime: ev.allDay ? "" : this.clock(ev.start - Math.floor(ev.start / 1440) * 1440),
                    resourceLabel: ev.resource !== null && ev.resource !== undefined ? (resources[ev.resource] || ev.resource) : ""
                }));
            },
            resourceLabels() {
                const out = {};
                const field = this.calendar && this.calendar.resourceField;
                if (!field) return out;
                (this.calendar.resources || []).forEach((res) => { out[lvText(res.value)] = res.label || lvText(res.value); });
                this.calItems.forEach((item) => { const value = lvText(item.row[field]); if (value && !out[value]) out[value] = moqui.listViews.text(item.row, field); });
                return out;
            },
            modeOptions() {
                const lv = moqui.listViews;
                let modes = lv.list(this.calendar && this.calendar.modes);
                if (!modes.length) modes = ["month", "week", "day", "list"];
                modes = modes.filter((mode) => lv.modeIcons[mode]);
                if (this.phone) { const small = modes.filter((mode) => mode === "day" || mode === "list"); if (small.length) modes = small; }
                return modes.map((mode) => ({ value: mode, label: this.phone ? undefined : this.labels[mode === "list" ? "agenda" : mode], icon: this.phone ? lv.modeIcons[mode] : undefined,
                    attrs: { "data-mode": mode } }));
            },
            firstDay() { const lang = this.$q && this.$q.lang && this.$q.lang.date; return lang && lang.firstDayOfWeek !== undefined ? Number(lang.firstDayOfWeek) : 0; },
            // today and the time now in the user's zone, again each minute (nowAbs)
            todayDay() { return moqui.listViews.today(this.timeZone, this.nowAbs || undefined).day; },
            nowMinutes() { return moqui.listViews.today(this.timeZone, this.nowAbs || undefined).minutes; },
            weekdayNames() {
                const out = [];
                const start = moqui.listViews.weekStart(this.anchor || this.todayDay, this.firstDay);
                for (let i = 0; i < 7; i++) out.push(this.dayFormat(start + i, this.phone ? { weekday: "narrow" } : { weekday: "short" }));
                return out;
            },
            monthWeeks() {
                const lv = moqui.listViews;
                const month = lv.ymd(this.anchor).m;
                const byDay = this.eventsByDay;
                return lv.monthWeeks(this.anchor, this.firstDay).map((days) => ({
                    key: days[0],
                    days: days.map((day) => ({ day, dayNum: lv.ymd(day).d, inMonth: lv.ymd(day).m === month, isToday: day === this.todayDay,
                        items: byDay[day] || [], limit: this.phone ? 2 : 3 }))
                }));
            },
            // the events on each day they touch, all-day first
            eventsByDay() {
                const out = {};
                this.events.forEach((ev) => {
                    const first = Math.floor(ev.start / 1440);
                    const last = Math.floor((ev.end - 1) / 1440);
                    for (let day = first; day <= last && day - first < 62; day++) (out[day] = out[day] || []).push(ev);
                });
                Object.keys(out).forEach((day) => out[day].sort((a, b) => (b.allDay ? 1 : 0) - (a.allDay ? 1 : 0) || a.start - b.start));
                return out;
            },
            // the columns of the time grid: the days of the week, or the day; a day with a resource field has a column per resource
            gridCols() {
                const lv = moqui.listViews;
                const resourceField = this.calendar && this.calendar.resourceField;
                const cols = [];
                if (this.calMode === "day" && resourceField) {
                    const values = Object.keys(this.resourceLabels);
                    this.events.forEach((ev) => { if (ev.resource && !values.includes(ev.resource)) values.push(ev.resource); });
                    (values.length ? values : [""]).forEach((value) => cols.push({ key: "r" + value, day: this.anchor, resource: value, isToday: this.anchor === this.todayDay,
                        top: this.dayFormat(this.anchor, { weekday: "short", day: "numeric" }), label: value ? (this.resourceLabels[value] || value) : lvLocalize("None") }));
                } else {
                    const start = this.calMode === "week" ? lv.weekStart(this.anchor, this.firstDay) : this.anchor;
                    for (let i = 0; i < (this.calMode === "week" ? 7 : 1); i++) {
                        const day = start + i;
                        cols.push({ key: "d" + day, day, resource: null, isToday: day === this.todayDay, top: this.dayFormat(day, { weekday: "short" }), label: String(lv.ymd(day).d) });
                    }
                }
                // each column's all-day events, and its timed ones in their lanes, cut to the day (from and to: minutes in the day)
                return cols.map((col) => {
                    const dayStart = col.day * 1440;
                    const inCol = this.events.filter((ev) => (col.resource === null || lvText(ev.resource) === col.resource) && ev.start < dayStart + 1440 && ev.end > dayStart);
                    const spans = inCol.filter((ev) => !ev.allDay).map((ev) => ({ ev, start: Math.max(ev.start, dayStart) - dayStart, end: Math.min(ev.end, dayStart + 1440) - dayStart }));
                    const timed = lv.layoutDay(spans).map((place) => ({ ev: place.ev.ev, from: place.ev.start, to: place.ev.end, lane: place.lane, lanes: place.lanes }));
                    return Object.assign(col, { allDay: inCol.filter((ev) => ev.allDay), timed });
                });
            },
            agenda() {
                const groups = [];
                const byDay = {};
                this.events.forEach((ev) => {
                    const day = Math.floor(ev.start / 1440);
                    if (!byDay[day]) { byDay[day] = { day, label: this.dayFormat(day, { weekday: "long", month: "long", day: "numeric", year: "numeric" }), isToday: day === this.todayDay, items: [] }; groups.push(byDay[day]); }
                    byDay[day].items.push(ev);
                });
                return groups;
            },
            rangeTitle() {
                const lv = moqui.listViews;
                if (this.calMode === "month") return this.dayFormat(this.anchor, { month: "long", year: "numeric" });
                if (this.calMode === "day") return this.dayFormat(this.anchor, { weekday: "long", month: "long", day: "numeric", year: "numeric" });
                if (this.calMode === "week") {
                    const start = lv.weekStart(this.anchor, this.firstDay);
                    return this.dayFormat(start, { month: "short", day: "numeric" }) + " – " + this.dayFormat(start + 6, { month: "short", day: "numeric", year: "numeric" });
                }
                return this.labels.agenda;
            },
            rowsKey() { try { return JSON.stringify(this.rows || []); } catch (err) { return String(Math.random()); } },
            popRow() {
                if (this.pop.index === null) return {};
                const item = this.calItems.find((entry) => entry.index === this.pop.index) || this.items.find((entry) => entry.index === this.pop.index);
                return item ? item.row : {};
            },
            selectionText() { return this.selection ? this.clock(this.selection.start) + " – " + this.clock(this.selection.end) : ""; },
            startName() { return this.calendar ? this.calendar.startParameter || this.calendar.startField : ""; },
            endName() { return this.calendar ? this.calendar.endParameter || this.calendar.endField : ""; }
        },
        watch: {
            current() { this.afterViewChange(); },
            calMode() { this.saveCalendar(); this.$nextTick(this.scrollToMorning); this.loadRange(false); },
            anchor() { this.saveCalendar(); this.loadRange(false); },
            // other rows (another page, a reload): the moves are in them now. The server-merged rows are a literal of the screen's template,
            // a new array whenever the screen draws again, so their text tells a change
            rowsKey() { this.moved = {}; this.loadInfo(); this.loadRange(true); this.loadAll(true); this.$nextTick(this.markPeekRow); },
            peekKey() { this.markPeekRow(); this.peekUrlParam(); },
            peekShown() { this.$nextTick(this.markPeekRow); }
        },
        created() {
            const lv = moqui.listViews;
            const views = this.viewList;
            const params = (this.mScreen && this.mScreen.parameters) || (this.$root && this.$root.currentParameters) || {};
            const pref = ((qvt.config || moqui.config || {}).preferences || {})[this.prefKey];
            let stored = null;
            try { stored = root.sessionStorage.getItem("moqui." + this.prefKey); } catch (err) { stored = null; }
            this.current = [params.view, this.view, stored, pref].find((name) => name && views.includes(name)) || views[0];
            // the calendar's mode and day, kept for the tab while it is open; else the first mode, and today unless the rows are elsewhere
            let saved = null;
            try { saved = JSON.parse(root.sessionStorage.getItem("moqui." + this.prefKey + "_cal") || "null"); } catch (err) { saved = null; }
            const modes = this.modeOptions.map((option) => option.value);
            this.calMode = saved && modes.includes(saved.mode) ? saved.mode : modes[0] || "month";
            if (saved && typeof saved.anchor === "number") this.anchor = saved.anchor;
            else {
                const today = lv.today(this.timeZone).day;
                const days = this.events.map((ev) => Math.floor(ev.start / 1440));
                const month = lv.monthStart(today);
                this.anchor = !days.length || days.some((day) => lv.monthStart(day) === month) ? today : days[0];
            }
            // the board's grouping, kept for the tab; the peek pane as the user left it (LIST_PEEK_), its width for the tab, the URL's row
            let group = null;
            try { group = root.sessionStorage.getItem("moqui." + this.prefKey + "_group"); } catch (err) { group = null; }
            if (group && this.groupOptions.some((option) => option.field === group)) this.groupBy = group;
            if (this.peek) {
                let peekOn = null;
                let width = null;
                try { peekOn = root.sessionStorage.getItem("moqui." + this.peekPref); width = root.sessionStorage.getItem("moqui." + this.peekPref + "_width"); } catch (err) { peekOn = null; }
                if (peekOn === null) peekOn = ((qvt.config || moqui.config || {}).preferences || {})[this.peekPref] || null;
                this.peekOn = peekOn === "true";
                if (Number(width) > 0) this.peekWidth = Number(width);
                if (params.peekId) { this.peekKey = String(params.peekId); this.peekOn = true; }
            }
            this.loadInfo();
        },
        mounted() {
            this.nowTimer = setInterval(() => { this.nowAbs = Date.now(); }, 60000);
            this.afterViewChange();
            this.onPeekKeyBound = (event) => this.onPeekKey(event);
            if (this.peek && root.document) root.document.addEventListener("keydown", this.onPeekKeyBound);
            this.$nextTick(this.markPeekRow);
        },
        updated() { if (this.current === "board") this.bindSortables(); },
        beforeUnmount() {
            clearInterval(this.nowTimer);
            this.unbindSortables();
            this.endPointer();
            if (this.onPeekKeyBound && root.document) root.document.removeEventListener("keydown", this.onPeekKeyBound);
        },
        methods: {
            setView(name) {
                if (!name || name === this.current) return;
                // back to the list after a card or event moved: the list again from the server, so it has what changed
                const reloadList = name === "list" && this.dirty;
                this.current = name;
                try { root.sessionStorage.setItem("moqui." + this.prefKey, name); } catch (err) { /* no storage */ }
                const config = qvt.config || moqui.config;
                if (config) config.preferences = Object.assign({}, config.preferences, { [this.prefKey]: name });
                if (this.$root && this.$root.setPreference && this.$root.userId) this.$root.setPreference(this.prefKey, name);
                if (reloadList) this.reloadRows();
            },
            afterViewChange() {
                this.pop.open = false;
                if (this.current === "board") this.$nextTick(this.bindSortables);
                else this.unbindSortables();
                if (this.current === "calendar") { this.$nextTick(this.scrollToMorning); this.loadRange(false); }
                if (this.current === "board") this.loadAll(false);
                this.$nextTick(this.markPeekRow);
            },
            // the list's find: the screen's parameters without the paging (moqui.listViews.findParams), less the names in drop
            findOf(drop) {
                const shown = (this.mScreen && this.mScreen.parameters) || (this.$root && this.$root.currentParameters) || {};
                return moqui.listViews.findParams(shown, drop);
            },
            // a GET of a path of the app (a form request, a transition) answering rows: the list's (listName), else the first list in it
            askRows(path, params, listName) {
                const prefix = this.$root && this.$root.appRootPath ? this.$root.appRootPath : "";
                return moqui.request(prefix + path, { params }).then((result) => moqui.listViews.answerRows(result.json, listName));
            },
            // range-find: the rows of the dates shown, asked of the screen's form request (the list's find with the start and end field range
            // added, no paging, with the list's other parameters) when other dates show or the list changed; the last answer asked for wins,
            // and until one comes (or when it fails) the list's page shows
            loadRange(force) {
                if (!this.rangeOn || this.current !== "calendar") return;
                const lv = moqui.listViews;
                const cal = this.calendar;
                const base = this.findOf();
                const finds = lv.rangeFinds(cal, lv.visibleDays(this.calMode, this.anchor, this.firstDay), this.timeZone).map((find) => Object.assign({}, base, find));
                const key = JSON.stringify([cal.rangeUrl, finds]);
                if (!force && key === this.range.key) return;
                const seq = this.range.seq + 1;
                this.range = Object.assign({}, this.range, { key, seq });
                Promise.all(finds.map((params) => this.askRows(cal.rangeUrl, params, cal.listName))).then((lists) => {
                    if (seq === this.range.seq) this.range = Object.assign({}, this.range, { rows: lv.rangeItems(this.items, lists, cal.keyFields) });
                }, (err) => {
                    if (seq === this.range.seq) this.range = Object.assign({}, this.range, { key: "" });
                    moqui.handleAjaxError(err);
                });
            },
            // all-rows: every row of the list's find for the board, asked of the screen's form request with no paging (a pageSize would
            // be kept as the user's page size), the first max-rows shown, when the board shows or the list changed; the last answer asked
            // for wins, and until one comes the list's page shows
            loadAll(force) {
                if (!this.allOn || this.current !== "board") return;
                const board = this.board;
                const params = Object.assign(this.findOf(), { pageNoLimit: "true" });
                const key = JSON.stringify([board.rowsUrl, params]);
                if (!force && key === this.all.key) return;
                const seq = this.all.seq + 1;
                this.all = Object.assign({}, this.all, { key, seq });
                this.askRows(board.rowsUrl, params, board.listName).then((list) => {
                    if (seq !== this.all.seq) return;
                    this.all = Object.assign({}, this.all, { rows: moqui.listViews.rangeItems(this.items, [list.slice(0, board.maxRows || 500)], board.keyFields) });
                    this.loadInfo();
                }, (err) => {
                    if (seq === this.all.seq) this.all = Object.assign({}, this.all, { key: "" });
                    moqui.handleAjaxError(err);
                });
            },
            reloadRows() {
                this.dirty = false;
                if (this.reload) { this.reload(); return; }
                const screen = this.mScreen;
                if (screen && screen.containerId && this.$root.reloadContainer) this.$root.reloadContainer(screen.containerId);
                else if (this.$root && this.$root.reloadSubscreens) this.$root.reloadSubscreens();
            },
            // the columns of a status board and the colors of a status calendar (or the labels of column-order values), asked once for the
            // values the rows have when the server did not write them in (a server-static screen, a status field chosen to group by)
            loadInfo() {
                const board = this.board ? this.boardView : null;
                const cal = this.calendar;
                const lv = moqui.listViews;
                const wantBoard = board && !board.columns && (board.statusTypeId || lv.isStatusField(board.groupField) || lv.list(board.columnOrder).length);
                const wantColors = cal && cal.colorField && !cal.colors && lv.isStatusField(cal.colorField);
                if (!wantBoard && !wantColors) return;
                const values = new Set(lv.list(board && board.columnOrder));
                (board ? this.boardItems : this.items).forEach((item) => {
                    if (wantBoard) values.add(lvText(item.row[board.groupField]));
                });
                this.items.forEach((item) => {
                    if (wantColors) values.add(lvText(item.row[cal.colorField]));
                });
                values.delete("");
                const params = { values: Array.from(values).join(","), statusTypeId: (board && board.statusTypeId) || "", statusFlowId: (board && board.statusFlowId) || "",
                    status: wantBoard ? (lv.isStatusField(board.groupField) || board.statusTypeId ? "true" : "false") : "true" };
                const key = JSON.stringify(params);
                if (key === this.infoKey) return;
                this.infoKey = key;
                const base = this.$root && this.$root.appRootPath ? this.$root.appRootPath : "";
                moqui.request(base + this.infoUrl, { params }).then((result) => { this.info = result.json || null; }, moqui.handleAjaxError);
            },
            // POST a transition with parameters; true when the answer has no errors (its messages shown), false after showing what was wrong
            post(action, params) {
                const token = this.$root ? this.$root.moquiSessionToken : "";
                const body = Object.assign({}, params, { moquiSessionToken: token });
                const base = this.$root && this.$root.appRootPath ? this.$root.appRootPath : "";
                return moqui.request(base + action, { method: "POST", params: body, headers: token ? { "X-CSRF-Token": token } : {} }).then((result) => {
                    const json = result.json;
                    if (json && moqui.responseHasErrors(json)) { moqui.notifyMessages(json.messageInfos, json.errors, json.validationErrors); return false; }
                    if (json) moqui.notifyMessages(json.messageInfos);
                    this.dirty = true;
                    return true;
                }, (err) => { moqui.handleAjaxError(err); return false; });
            },
            cardTitle(row) { const field = (this.board && this.board.titleField) || (this.calendar && this.calendar.titleField); return moqui.listViews.text(row, field); },
            sumText(sum) {
                try { return moqui.l10n && moqui.l10n.format ? moqui.l10n.format(sum, (this.board && this.board.sumFormat) || "#,##0.##") : String(sum); } catch (err) { return String(sum); }
            },
            // ---- board ----
            dragAllowed(value) {
                if (!this.dragging) return false;
                const flow = this.flow;
                if (!flow || !moqui.listViews.isStatusField(this.boardView.groupField)) return true;
                return (flow[this.dragging.from] || []).includes(value);
            },
            // the board grouped by another of its group-fields: its columns asked for when it is a status field the server gave none of
            setGroup(field) {
                if (!field || field === this.boardView.groupField) return;
                this.groupBy = field === this.board.groupField ? "" : field;
                this.moved = {};
                this.info = null;
                this.infoKey = "";
                try { root.sessionStorage.setItem("moqui." + this.prefKey + "_group", field); } catch (err) { /* no storage */ }
                this.loadInfo();
            },
            // a click on a card opens it: the link of its title (the first link of the card), unless the click was on a control of the
            // card (a link, a button, a field, an in-place edit: moqui.listViews.onControl) or ended a drag; in the peek pane while it shows
            openCard(event, item) {
                if (Date.now() - this.dragEnded < 350) return;
                const target = event.target;
                if (moqui.listViews.onControl(target, event.currentTarget)) return;
                if (this.peekShown && item) { this.peekSelect(this.peekKeyOf(item.row)); return; }
                const card = event.currentTarget;
                const link = card && card.querySelector ? card.querySelector(".m-card-title a[href], a[href]") : null;
                if (link) link.click();
            },
            bindSortables() {
                const board = this.$refs.board;
                if (!board || !this.board || !this.board.transition) return;
                const lists = Array.from(board.querySelectorAll(".m-board-cards"));
                this.sortables = (this.sortables || []).filter((sortable) => lists.includes(sortable.el) || (sortable.destroy(), false));
                const bind = () => {
                    lists.forEach((el) => {
                        if (this.sortables.some((sortable) => sortable.el === el)) return;
                        this.sortables.push(root.Sortable.create(el, {
                            group: this.uid, sort: false, draggable: ".m-board-card", forceFallback: true, fallbackTolerance: 4, delay: 150, delayOnTouchOnly: true,
                            animation: 120, ghostClass: "m-board-card--ghost", chosenClass: "m-board-card--chosen", dragClass: "m-board-card--drag",
                            onStart: (evt) => { this.dragging = { from: evt.from.dataset.value, index: Number(evt.item.dataset.index) }; },
                            onEnd: (evt) => {
                                const from = evt.from.dataset.value;
                                const to = evt.to.dataset.value;
                                const index = Number(evt.item.dataset.index);
                                // the card back where the page has it; the model moves it
                                if (evt.from !== evt.to && evt.item.parentNode === evt.to) {
                                    evt.to.removeChild(evt.item);
                                    evt.from.insertBefore(evt.item, evt.from.children[evt.oldIndex] || null);
                                }
                                this.dragging = null;
                                this.dragEnded = Date.now();
                                if (from !== to) this.moveCard(index, to);
                            }
                        }));
                    });
                };
                if (root.Sortable) bind();
                else moqui.loadScript("/libs/sortablejs/Sortable.min.js", null, () => !!root.Sortable).then(() => { if (this.current === "board") bind(); }, (err) => console.warn("SortableJS did not load", err));
            },
            unbindSortables() {
                (this.sortables || []).forEach((sortable) => { try { sortable.destroy(); } catch (err) { /* gone with its element */ } });
                this.sortables = [];
            },
            moveCard(index, to) {
                const item = this.boardItems.find((entry) => entry.index === index);
                if (!item || !this.board.transition) return;
                const before = this.moved[index];
                this.moved = Object.assign({}, this.moved, { [index]: Object.assign({}, before, { group: to }) });
                const params = Object.assign({}, this.board.parameters || {}, moqui.listViews.keyParams(item.row, this.board.keyFields));
                // the new value under the field's name (parameter-name for group-field) and, with group-fields, the field as groupField
                const view = this.boardView;
                params[view.parameterName || view.groupField] = to;
                if (this.groupOptions.length) params.groupField = view.groupField;
                this.post(this.board.transition, params).then((ok) => {
                    if (ok) return;
                    const moved = Object.assign({}, this.moved);
                    if (before) moved[index] = before; else delete moved[index];
                    this.moved = moved;
                });
            },
            // ---- tree: the slots m-list-tree draws a row with, passed on ----
            treeSlotNames() { return Object.keys(this.$slots || {}).filter((name) => name === "tree" || name.indexOf("tree_") === 0); },
            // ---- peek ----
            peekKeyOf(row) { return row && this.peek ? lvText(row[this.peek.keyField]) : ""; },
            setPeek(on) {
                const value = on ? "true" : "false";
                this.peekOn = !!on;
                if (!on) this.peekSelect("");
                try { root.sessionStorage.setItem("moqui." + this.peekPref, value); } catch (err) { /* no storage */ }
                const config = qvt.config || moqui.config;
                if (config) config.preferences = Object.assign({}, config.preferences, { [this.peekPref]: value });
                if (this.$root && this.$root.setPreference && this.$root.userId) this.$root.setPreference(this.peekPref, value);
            },
            peekSelect(key) { this.peekKey = key ? String(key) : ""; },
            // the selected key in the URL as peekId, so a peeked row is a link; not for a list in a container, whose URL is its own
            peekUrlParam() {
                const rootVm = this.$root;
                if ((this.mScreen && this.mScreen.containerId) || !rootVm || !rootVm.currentParameters || !root.history || !root.history.replaceState) return;
                const params = Object.assign({}, rootVm.currentParameters);
                if (this.peekKey) params.peekId = this.peekKey; else delete params.peekId;
                if (String(params.peekId || "") === String(rootVm.currentParameters.peekId || "")) return;
                rootVm.currentParameters = params;
                root.history.replaceState(root.history.state, "", rootVm.currentLinkUrl);
            },
            // a click on a list row, not on a control of it, opens the row in the peek pane
            onPeekClick(event) {
                if (!this.peekShown || this.current !== "list") return;
                const target = event.target;
                const rowEl = target && target.closest ? target.closest("[data-row]") : null;
                if (!rowEl || rowEl.closest(".m-list-views") !== this.$el || moqui.listViews.onControl(target, rowEl)) return;
                const row = this.rows[Number(rowEl.getAttribute("data-row"))];
                if (row && !row._moquiRowType) this.peekSelect(this.peekKeyOf(row));
            },
            // the list's row of the peeked key marked selected (the list's own rows are the server's, so by their class)
            markPeekRow() {
                const el = this.$el;
                if (!el || !el.querySelectorAll) return;
                el.querySelectorAll(".m-list-views-list [data-row]").forEach((rowEl) => {
                    if (rowEl.closest(".m-list-views") !== el) return;
                    const row = this.rows[Number(rowEl.getAttribute("data-row"))];
                    rowEl.classList.toggle("selected", this.peekShown && !!this.peekKey && !!row && this.peekKeyOf(row) === this.peekKey);
                });
            },
            // the keys of the rows in the order the view shows them, for Up and Down
            peekOrder() {
                if (this.current === "tree") return this.treeKeys;
                if (this.current === "board") return [].concat(...this.columns.map((column) => column.items.map((item) => this.peekKeyOf(item.row))));
                return this.items.map((item) => this.peekKeyOf(item.row));
            },
            // Up and Down move the peek pane through the rows, Escape closes it; not while typing, in a menu or dialog, or with a modifier
            onPeekKey(event) {
                if (!this.peekShown || event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
                const target = event.target;
                if (target && target.closest && target.closest("input, textarea, select, [contenteditable='true'], [contenteditable=''], .q-menu, .q-dialog, .m-list-tree")) return;
                if (event.key === "Escape" && this.peekKey) { event.preventDefault(); this.peekSelect(""); return; }
                if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                const keys = this.peekOrder().filter((key) => key);
                if (!keys.length) return;
                const at = keys.indexOf(this.peekKey);
                const next = at < 0 ? (event.key === "ArrowDown" ? 0 : keys.length - 1) : at + (event.key === "ArrowDown" ? 1 : -1);
                if (next < 0 || next >= keys.length) return;
                event.preventDefault();
                this.peekSelect(keys[next]);
            },
            // the tree's focused row follows into the pane once a row is peeked
            treeFocus(key) { if (this.peekShown && this.peekKey && key) this.peekSelect(key); },
            openFull() {
                const params = Object.assign({}, this.peek.parameters || {}, { [this.peek.keyField]: this.peekKey });
                const url = this.peek.url + "?" + moqui.objToSearch(params);
                if (this.$root && this.$root.setUrl) this.$root.setUrl(this.$root.getLinkPath ? this.$root.getLinkPath(url) : url);
            },
            // the separator dragged: the pane's width, kept for the tab, between 320 px and what leaves the list 480 px
            onPeekResize(event) {
                const body = this.$refs.body;
                if (!body || !body.getBoundingClientRect) return;
                event.preventDefault();
                const rect = body.getBoundingClientRect();
                const move = (moved) => { this.peekWidth = Math.round(Math.max(320, Math.min(rect.right - moved.clientX, rect.width - 480))); };
                const up = () => {
                    root.removeEventListener("pointermove", move);
                    root.removeEventListener("pointerup", up);
                    try { root.sessionStorage.setItem("moqui." + this.peekPref + "_width", String(this.peekWidth)); } catch (err) { /* no storage */ }
                };
                root.addEventListener("pointermove", move);
                root.addEventListener("pointerup", up);
            },
            // ---- calendar ----
            setMode(mode) { if (mode) { this.calMode = mode; this.pop.open = false; } },
            saveCalendar() { try { root.sessionStorage.setItem("moqui." + this.prefKey + "_cal", JSON.stringify({ mode: this.calMode, anchor: this.anchor })); } catch (err) { /* no storage */ } },
            shift(by) { this.anchor = moqui.listViews.shift(this.anchor, this.calMode, by); },
            goToday() { this.anchor = this.todayDay; },
            goDay(day) { this.anchor = day; if (this.modeOptions.some((option) => option.value === "day")) this.calMode = "day"; },
            scrollToMorning() {
                const el = this.$refs.calScroll;
                // the time grid starts at seven in the morning, once for each grid shown
                if (el && el !== this.scrolledEl) { el.scrollTop = 7 * this.hourPx - 10; this.scrolledEl = el; }
            },
            dayFormat(day, options) {
                try { return new Intl.DateTimeFormat(this.locale, Object.assign({ timeZone: "UTC" }, options)).format(new Date(day * LV_DAY_MS)); } catch (err) { return moqui.listViews.dayKey(day); }
            },
            clock(minutes) {
                const m = ((minutes % 1440) + 1440) % 1440;
                try { return new Intl.DateTimeFormat(this.locale, { timeZone: "UTC", hour: "numeric", minute: "2-digit" }).format(new Date(m * 60000)); } catch (err) { return String(Math.floor(m / 60)).padStart(2, "0") + ":" + String(m % 60).padStart(2, "0"); }
            },
            hourLabel(hour) {
                try { return new Intl.DateTimeFormat(this.locale, { timeZone: "UTC", hour: "numeric" }).format(new Date(hour * 3600000)); } catch (err) { return String(hour); }
            },
            timeRange(ev) {
                const startDay = Math.floor(ev.start / 1440);
                const endDay = Math.floor((ev.end - 1) / 1440);
                if (ev.allDay) return startDay === endDay ? this.labels.allDay : this.dayFormat(startDay, { month: "short", day: "numeric" }) + " – " + this.dayFormat(endDay, { month: "short", day: "numeric" });
                const from = this.clock(ev.start);
                const to = this.clock(ev.end);
                return startDay === Math.floor(ev.end / 1440) || ev.end % 1440 === 0 ? from + " – " + to : from + " – " + this.dayFormat(Math.floor(ev.end / 1440), { month: "short", day: "numeric" }) + " " + to;
            },
            chipClass(ev) { return ev.allDay ? ["m-cal-chip--filled", "bg-" + ev.color, moqui.listViews.lightColor(ev.color) ? "text-dark" : "text-white"] : []; },
            eventClass(ev) { return ["bg-" + ev.color, moqui.listViews.lightColor(ev.color) ? "text-dark" : "text-white", { "m-cal-event--moving": this.preview && this.preview.index === ev.index }]; },
            eventStyle(place) {
                const width = 100 / place.lanes;
                return { top: (place.from / 60 * this.hourPx) + "px", height: Math.max((place.to - place.from) / 60 * this.hourPx - 2, 16) + "px",
                    left: "calc(" + (place.lane * width) + "% + 1px)", width: "calc(" + width + "% - 3px)" };
            },
            spanStyle(start, end, day) {
                const from = start - day * 1440;
                return { top: (from / 60 * this.hourPx) + "px", height: Math.max((end - start) / 60 * this.hourPx - 2, 14) + "px" };
            },
            // the zone under the pointer: {zone, day, resource, minutes} for a time column (minutes from its top), day and resource for the others
            zoneAt(event) {
                const el = root.document.elementFromPoint ? root.document.elementFromPoint(event.clientX, event.clientY) : null;
                const zoneEl = el && el.closest ? el.closest("[data-zone]") : null;
                if (!zoneEl || !this.$el.contains(zoneEl)) return null;
                const out = { zone: zoneEl.dataset.zone, day: Number(zoneEl.dataset.day), resource: zoneEl.dataset.res === undefined ? null : zoneEl.dataset.res };
                if (out.zone === "time") {
                    const rect = zoneEl.getBoundingClientRect();
                    out.minutes = Math.max(0, Math.min(1440, (event.clientY - rect.top) / this.hourPx * 60));
                }
                return out;
            },
            onEventDown(event, ev, kind) {
                if (event.button !== undefined && event.button !== 0) return;
                const at = this.zoneAt(event);
                this.drag = { kind, ev, x: event.clientX, y: event.clientY, moved: false, target: event.currentTarget,
                    grab: at && at.zone === "time" ? at.day * 1440 + at.minutes - ev.start : 0, grabDay: at ? at.day : Math.floor(ev.start / 1440) };
                this.startPointer();
            },
            onCellDown(event, day, zone, resource) {
                if (event.button !== undefined && event.button !== 0) return;
                if (!this.calendar || !(this.calendar.createTransition || this.calendar.createScreen)) return;
                const at = this.zoneAt(event);
                const start = zone === "time" && at ? day * 1440 + Math.floor((at.minutes || 0) / 30) * 30 : day * 1440;
                this.drag = { kind: "create", zone, day, resource, x: event.clientX, y: event.clientY, moved: false, start, target: event.currentTarget };
                this.selection = { zone, day, col: (resource !== null && resource !== undefined ? "r" + resource : "d" + day), start, end: zone === "time" ? start + 60 : start + 1440 };
                this.startPointer();
            },
            startPointer() {
                if (!this.pointerMove) {
                    this.pointerMove = (event) => this.onPointerMove(event);
                    this.pointerUp = (event) => this.onPointerUp(event);
                    // the browser took the pointer (a touch that scrolls the page): nothing is moved or made
                    this.pointerCancel = () => { this.endPointer(); this.drag = null; this.preview = null; this.selection = null; };
                }
                root.addEventListener("pointermove", this.pointerMove);
                root.addEventListener("pointerup", this.pointerUp);
                root.addEventListener("pointercancel", this.pointerCancel);
            },
            endPointer() {
                if (!this.pointerMove) return;
                root.removeEventListener("pointermove", this.pointerMove);
                root.removeEventListener("pointerup", this.pointerUp);
                root.removeEventListener("pointercancel", this.pointerCancel);
            },
            onPointerMove(event) {
                const drag = this.drag;
                if (!drag) return;
                if (!drag.moved && Math.abs(event.clientX - drag.x) + Math.abs(event.clientY - drag.y) < 5) return;
                if (drag.kind !== "create" && !this.canMove) return;
                drag.moved = true;
                if (event.preventDefault) event.preventDefault();
                const at = this.zoneAt(event);
                if (!at) return;
                const lv = moqui.listViews;
                if (drag.kind === "create") {
                    if (drag.zone !== "time" || at.zone !== "time" || at.day !== drag.day) return;
                    const minutes = at.day * 1440 + lv.snap(at.minutes, 30);
                    this.selection = Object.assign({}, this.selection, { start: Math.min(drag.start, minutes), end: Math.max(drag.start + 30, minutes) });
                    return;
                }
                const ev = drag.ev;
                const length = ev.end - ev.start;
                if (drag.kind === "resize") {
                    if (at.zone !== "time") return;
                    const end = Math.max(ev.start + 15, at.day * 1440 + lv.snap(at.minutes, 15));
                    this.preview = { index: ev.index, start: ev.start, end };
                } else if (at.zone === "time" && !ev.allDay) {
                    const start = Math.max(at.day * 1440, Math.min(at.day * 1440 + 1440 - 15, at.day * 1440 + lv.snap(at.minutes - drag.grab, 15)));
                    this.preview = { index: ev.index, start, end: start + length, resource: at.resource };
                } else if ((at.zone === "month" || (at.zone === "allday" && ev.allDay)) && !isNaN(at.day)) {
                    const shift = (at.day - drag.grabDay) * 1440;
                    this.preview = { index: ev.index, start: ev.start + shift, end: ev.end + shift, resource: at.zone === "allday" ? at.resource : undefined };
                }
            },
            onPointerUp(event) {
                this.endPointer();
                const drag = this.drag;
                this.drag = null;
                if (!drag) return;
                if (drag.kind === "create") {
                    const selection = this.selection;
                    if (selection) this.openCreate(selection, drag.resource);
                    return;
                }
                const preview = this.preview;
                if (!drag.moved || !preview) {
                    this.preview = null;
                    if (drag.kind !== "resize") this.openEvent(event, drag.ev, drag.target);
                    return;
                }
                this.preview = null;
                const ev = drag.ev;
                if (preview.start === ev.start && preview.end === ev.end && (preview.resource === undefined || preview.resource === ev.resource)) return;
                this.moveEvent(ev, preview);
            },
            moveEvent(ev, change) {
                const before = this.moved[ev.index];
                const move = { start: change.start, end: change.end };
                if (change.resource !== undefined && change.resource !== null) move.resource = change.resource;
                this.moved = Object.assign({}, this.moved, { [ev.index]: move });
                const params = moqui.listViews.moveParams(ev, this.calendar, this.timeZone, change);
                this.post(this.calendar.transition, params).then((ok) => {
                    if (ok) return;
                    const moved = Object.assign({}, this.moved);
                    if (before) moved[ev.index] = before; else delete moved[ev.index];
                    this.moved = moved;
                });
            },
            openEvent(event, ev, target) {
                const el = target || (event && event.currentTarget) || null;
                const rect = el && el.getBoundingClientRect ? el.getBoundingClientRect() : { left: event.clientX || 0, top: event.clientY || 0, width: 0, height: 0 };
                this.anchorRect = { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
                this.pop = { open: false, index: ev.index, when: ev.timeText + (ev.allDay ? "" : ", " + this.dayFormat(Math.floor(ev.start / 1440), { weekday: "short", month: "short", day: "numeric" })) };
                this.$nextTick(() => { this.pop.open = true; });
            },
            // the quick create: the create-screen in the dialog with the start and end (and resource) as parameters, else the form that posts the
            // create-transition with the title, start and end
            openCreate(selection, resource) {
                const cal = this.calendar;
                const lv = moqui.listViews;
                // a day without a time: all day with an all-day field (its first and last day), else nine to ten
                const allDay = selection.zone !== "time";
                const dateOnly = allDay && !!cal.allDayField;
                let start = selection.start;
                let end = selection.end;
                if (allDay && !dateOnly) { start += 9 * 60; end = start + 60; }
                const values = {};
                values[this.startName] = lv.timeText(start, dateOnly);
                if (cal.endField) values[this.endName] = lv.timeText(dateOnly ? end - 1440 : end, dateOnly);
                if (cal.allDayField) values[cal.allDayParameter || cal.allDayField] = allDay ? "Y" : "N";
                if (cal.resourceField && resource !== null && resource !== undefined && resource !== "") values[cal.resourceParameter || cal.resourceField] = resource;
                const fixed = Object.assign({}, cal.createParameters || {});
                this.selection = null;
                if (cal.createScreen) {
                    const search = moqui.objToSearch(Object.assign({}, fixed, values));
                    this.create = { open: true, initial: {}, url: cal.createScreen + (search ? (cal.createScreen.indexOf("?") >= 0 ? "&" : "?") + search : ""), dateType: "date-time" };
                    return;
                }
                const initial = Object.assign({}, fixed, values);
                if (cal.titleField) initial[cal.titleField] = "";
                this.create = { open: true, initial, url: "", dateType: dateOnly ? "date" : "date-time" };
            },
            afterCreate() {
                this.create = Object.assign({}, this.create, { open: false });
                if (this.reload) this.reload();
                this.loadRange(true);
            }
        }
    });

    /* ========== list views: a form-list as a tree ========== */

    // The tree of a form-list (view-tree; style-guide §3.15): the rows known (the list's page, the levels fetched as rows open, the
    // ancestors the find left out) as {key: {key, row, index, parent, muted, order}}, index the row's place on the page (its tree_{index}
    // slot) or null. The helpers are pure, for the node tests (WebrootVue.qvt.views.test.js); m-list-tree draws them.
    moqui.listTree = {
        // the key of a row: its first key field's value as text
        keyOf(row, keyFields) {
            const name = moqui.listViews.list(keyFields)[0];
            return name && row ? lvText(row[name]) : "";
        },
        // the parent of a known row as the tree places it: its parent when that is known, else the top ("")
        placed(known, key) {
            const parent = known[key] ? lvText(known[key].parent) : "";
            return parent && known[parent] && parent !== key ? parent : "";
        },
        // the keys under each parent ("" the top) in order
        childrenOf(known) {
            const out = {};
            Object.keys(known).forEach((key) => {
                const parent = moqui.listTree.placed(known, key);
                (out[parent] = out[parent] || []).push(key);
            });
            Object.keys(out).forEach((parent) => out[parent].sort((a, b) => known[a].order - known[b].order));
            return out;
        },
        // The rows shown, depth first from the top: a row's children when it is open, each with its level (1 at the top), its place among
        // its siblings and their count; after an open row's children what extra(key) gives (a loading, error or more row) at their level.
        visible(known, children, isOpen, extra) {
            const out = [];
            const seen = new Set();
            const walk = (parent, level) => {
                const keys = children[parent] || [];
                keys.forEach((key, index) => {
                    if (seen.has(key)) return;
                    seen.add(key);
                    out.push({ kind: "row", key, level, posinset: index + 1, setsize: keys.length });
                    if (!isOpen(key)) return;
                    walk(key, level + 1);
                    (extra ? extra(key) : []).forEach((kind) => out.push({ kind, key, level: level + 1 }));
                });
            };
            walk("", 1);
            return out;
        },
        // the parents the known rows name that are not known (what ancestor-transition is asked for)
        missing(known) {
            const out = [];
            Object.keys(known).forEach((key) => {
                const parent = lvText(known[key].parent);
                if (parent && !known[parent] && out.indexOf(parent) < 0) out.push(parent);
            });
            return out;
        },
        // whether key is under (or is) top, by the known parents
        within(known, key, top) {
            const seen = new Set();
            let at = key;
            while (at && !seen.has(at)) {
                if (at === top) return true;
                seen.add(at);
                at = known[at] ? lvText(known[at].parent) : "";
            }
            return false;
        },
        // Where a row goes for a key (indent under the sibling above as its last child, outdent to after its parent, up and down among its
        // siblings), as the parent and the previous sibling a move posts (empty for the top and the first); null when it cannot move so.
        move(known, children, key, how) {
            const node = known[key];
            if (!node) return null;
            const parent = moqui.listTree.placed(known, key);
            const siblings = children[parent] || [];
            const at = siblings.indexOf(key);
            const own = lvText(node.parent);
            if (how === "indent") {
                if (at < 1) return null;
                const under = siblings[at - 1];
                const kids = (children[under] || []).filter((other) => other !== key);
                return { parent: under, previous: kids.length ? kids[kids.length - 1] : "" };
            }
            if (how === "outdent") return own && known[own] ? { parent: lvText(known[own].parent), previous: own } : null;
            if (how === "up") return at >= 1 ? { parent: own, previous: at >= 2 ? siblings[at - 2] : "" } : null;
            if (how === "down") return at >= 0 && at < siblings.length - 1 ? { parent: own, previous: siblings[at + 1] } : null;
            return null;
        },
        // A drop of key on the row target: before or after it (zone), or into it as its last child; after an open row with children is its
        // first child. null onto itself or into its own branch.
        drop(known, children, key, target, zone, targetOpen) {
            if (!known[key] || !known[target] || moqui.listTree.within(known, target, key)) return null;
            const kidsOf = (parent) => (children[parent] || []).filter((other) => other !== key);
            if (zone === "into") { const kids = kidsOf(target); return { parent: target, previous: kids.length ? kids[kids.length - 1] : "" }; }
            if (zone === "after" && targetOpen && kidsOf(target).length) return { parent: target, previous: "" };
            const parent = lvText(known[target].parent);
            if (zone === "after") return { parent, previous: target };
            const siblings = kidsOf(moqui.listTree.placed(known, target));
            const at = siblings.indexOf(target);
            return { parent, previous: at >= 1 ? siblings[at - 1] : "" };
        },
        // the parent and the previous sibling a row has now, to tell a move that changes nothing
        place(known, children, key) {
            const siblings = children[moqui.listTree.placed(known, key)] || [];
            const at = siblings.indexOf(key);
            return { parent: known[key] ? lvText(known[key].parent) : "", previous: at >= 1 ? siblings[at - 1] : "" };
        },
        // the indent of a level: 20 px a level to the sixth (12 px to the fourth on a phone), then its number shown
        indent(level, phone) {
            const cap = phone ? 4 : 6;
            return { px: (Math.min(level, cap) - 1) * (phone ? 12 : 20), deep: level > cap };
        },
        // The rows open at a level (Expand to level n: the rows above level n that have children; all for "all") over the known rows;
        // hasChildren(key) tells a row that may have children
        openTo(children, level, hasChildren) {
            const max = level === "all" ? Infinity : Number(level) || 1;
            const out = {};
            const walk = (parent, depth) => (children[parent] || []).forEach((key) => {
                if (depth >= max || !hasChildren(key) || out[key]) return;
                out[key] = true;
                walk(key, depth + 1);
            });
            walk("", 1);
            return out;
        }
    };

    // The tree view of m-list-views (view-tree, style-guide §3.15), in the list's look: a div table with the list's columns (each row of
    // the page its tree_{index} slot, the list's cells; else the tree slot, else the text of its fields) and the roll-up columns at the
    // right, a totals row of the top-level rows. A row opens to its children (asked of children-transition with the list's find; 200 shown
    // and a Show more row for the next 200), under it with a skeleton row while they come and an error row with Retry if they fail. The
    // rows open are the branches that hold a row of the page until the user opens or closes one, then those kept for the tab; Expand to
    // level n opens the rows above
    // it (the level in the URL as treeLevel). The page's rows whose parents are not known are placed under their ancestors, asked of
    // ancestor-transition and shown muted. With a transition a row is dragged (from anywhere but a control of it) before, after or into
    // another, or moved by Tab, Shift+Tab, Alt+Up and Alt+Down; each posts the key fields, the parent field and previousSiblingId and asks
    // the list to load again. The keys are the treegrid's.
    component("m-list-tree", {
        name: "mListTree",
        inject: { mScreen: { default: null } },
        props: { tree: { type: Object, required: true }, items: { type: Array, default: () => [] }, prefKey: String, find: Function, post: Function,
            peekable: Boolean, selected: String, peekField: String },
        emits: ["select", "moved", "focusrow", "keys"],
        data() {
            return { known: {}, loaded: {}, loading: {}, failed: {}, more: {}, order: 0, openState: null, level: "", focusKey: "", armed: "", dragKey: "",
                dropAt: null, hover: { key: "", since: 0 }, capped: false };
        },
        template:
            '<div class="m-list-tree" :class="{\'m-list-tree--dragging\': !!dragKey}">' +
                '<div class="m-tree-bar row items-center no-wrap">' +
                    '<q-btn-dropdown flat dense no-caps icon="sym_o_unfold_more" class="m-tree-expand" :label="labels.expand">' +
                        '<q-list dense><q-item v-for="option in levelOptions" :key="option.value" clickable v-close-popup :active="level === option.value"' +
                                ' :data-level="option.value" @click="expandTo(option.value)"><q-item-section>{{option.label}}</q-item-section></q-item></q-list>' +
                    '</q-btn-dropdown>' +
                '</div>' +
                '<div class="q-my-sm q-table__container q-table__card q-table--horizontal-separator q-table--dense q-table--flat"' +
                        ' :class="{\'q-table--dark\': $q.dark.isActive, \'q-table__card--dark\': $q.dark.isActive, \'q-dark\': $q.dark.isActive}">' +
                    '<div class="table q-table m-tree-table" role="treegrid" :aria-label="labels.tree">' +
                        '<div class="thead"><div class="tr" role="row">' +
                            '<div v-for="(column, c) in tree.columns" :key="\'h\' + c" class="th" role="columnheader" :class="column.align === \'right\' ? \'text-right\' : \'text-left\'">{{column.label}}</div>' +
                            '<div v-for="rollup in rollups" :key="\'r\' + rollup.name" class="th text-right m-tree-rollup" role="columnheader">{{rollup.label}}</div>' +
                        '</div></div>' +
                        '<div ref="body" class="tbody" @keydown="onKey">' +
                            '<template v-for="entry in rows" :key="entry.id">' +
                                '<div v-if="entry.kind === \'row\'" class="tr m-tree-row" role="row" :data-tree-key="entry.key" :tabindex="entry.key === focusShown ? 0 : -1"' +
                                        ' :aria-level="entry.level" :aria-setsize="entry.setsize" :aria-posinset="entry.posinset" :aria-expanded="entry.parent ? String(entry.open) : null"' +
                                        ' :aria-selected="entry.selected ? \'true\' : null" :class="entry.classes" :draggable="canMove && !entry.muted && armed === entry.key ? \'true\' : null"' +
                                        ' @click="onRowClick($event, entry)" @focus="focusKey = entry.key" @pointerdown="arm($event, entry)"' +
                                        ' @dragstart="onDragStart($event, entry)" @dragover="onDragOver($event, entry)" @drop="onDrop($event, entry)" @dragend="onDragEnd">' +
                                    '<slot :name="\'tree_\' + entry.index" :t="entry.t" :row="entry.row"><slot name="tree" :t="entry.t" :row="entry.row">' +
                                        '<div v-for="(column, c) in tree.columns" :key="c" class="td" :class="column.align === \'right\' ? \'text-right\' : null">' +
                                            '<div v-if="c === 0" class="m-tree-cell row no-wrap items-center"><m-tree-lead :t="entry.t"></m-tree-lead><div class="m-tree-label">{{cellText(entry.row, column)}}</div></div>' +
                                            '<template v-else>{{cellText(entry.row, column)}}</template>' +
                                        '</div>' +
                                    '</slot></slot>' +
                                    '<div v-for="rollup in rollups" :key="rollup.name" class="td text-right m-tree-rollup" :class="{\'m-muted\': !entry.parent}">{{rollupText(entry.row, rollup.name)}}</div>' +
                                '</div>' +
                                '<div v-else class="tr m-tree-extra" :data-extra="entry.kind">' +
                                    '<div class="td"><div class="m-tree-cell row no-wrap items-center" :style="{paddingLeft: (indentOf(entry.level).px + 24) + \'px\'}">' +
                                        '<q-skeleton v-if="entry.kind === \'loading\'" type="text" width="160px" height="20px"></q-skeleton>' +
                                        '<template v-else-if="entry.kind === \'error\'"><span class="text-negative">{{labels.failed}}</span>' +
                                            '<q-btn flat dense no-caps color="primary" class="q-ml-sm m-tree-retry" :label="labels.retry" @click="fetchLevel(entry.key)"></q-btn></template>' +
                                        '<q-btn v-else flat dense no-caps color="primary" class="m-tree-more" :label="labels.more" @click="more[entry.key] = shownOf(entry.key) + 200"></q-btn>' +
                                    '</div></div>' +
                                    '<div v-for="n in fillers" :key="n" class="td"></div>' +
                                '</div>' +
                            '</template>' +
                            '<div v-if="!rows.length" class="tr"><div class="td m-muted">{{labels.empty}}</div><div v-for="n in fillers" :key="n" class="td"></div></div>' +
                            '<div v-if="rollups.length && rows.length" class="tr m-tree-total text-weight-bold">' +
                                '<div class="td">{{totalLabel}}</div>' +
                                '<div v-for="n in tree.columns.length - 1" :key="\'f\' + n" class="td"></div>' +
                                '<div v-for="rollup in rollups" :key="rollup.name" class="td text-right m-tree-rollup">{{totalText(rollup.name)}}</div>' +
                            '</div>' +
                        '</div>' +
                    '</div>' +
                '</div>' +
            '</div>',
        computed: {
            labels() {
                const l = lvLocalize;
                return { expand: l("Expand"), tree: l("Tree"), failed: l("Could not load these rows"), retry: l("Retry"), more: l("Show 200 more"),
                    empty: l("Nothing to show"), open: l("Open"), close: l("Close"), all: l("All"), level: l("Level"), total: l("Total") };
            },
            lt() { return moqui.listTree; },
            phone() { return !!(this.$q && this.$q.screen && this.$q.screen.lt.sm); },
            keyField() { return moqui.listViews.list(this.tree.keyFields)[0] || ""; },
            canMove() { return !!this.tree.transition; },
            rollups() { return this.tree.rollups || []; },
            fillers() { return Math.max(0, (this.tree.columns || []).length + this.rollups.length - 1); },
            levelOptions() {
                return ["1", "2", "3"].map((value) => ({ value, label: this.labels.level + " " + value })).concat([{ value: "all", label: this.labels.all }]);
            },
            children() { return moqui.listTree.childrenOf(this.known); },
            // the rows of the page by key, for a fetched row that is on the page (its slot)
            pageIndex() {
                const out = {};
                this.items.forEach((item) => { const key = moqui.listTree.keyOf(item.row, this.tree.keyFields); if (key) out[key] = item.index; });
                return out;
            },
            itemsKey() { try { return JSON.stringify(this.items.map((item) => item.row)); } catch (err) { return String(Math.random()); } },
            rows() {
                const lt = moqui.listTree;
                // a level shows 200 rows, and 200 more for each Show more (the top level is the list's page)
                const shown = {};
                Object.keys(this.children).forEach((key) => { shown[key] = key ? this.children[key].slice(0, this.shownOf(key)) : this.children[key]; });
                const extra = (key) => (this.loading[key] ? ["loading"] : this.failed[key] ? ["error"] : (this.children[key] || []).length > this.shownOf(key) ? ["more"] : []);
                return lt.visible(this.known, shown, (key) => this.isOpen(key), extra).map((entry, at) => {
                    if (entry.kind !== "row") return Object.assign(entry, { id: entry.kind + ":" + entry.key + ":" + at });
                    const node = this.known[entry.key];
                    const parent = this.hasChildren(entry.key);
                    const open = parent && this.isOpen(entry.key);
                    const selected = !!this.selected && !!this.peekField && lvText(node.row[this.peekField]) === this.selected;
                    const indent = lt.indent(entry.level, this.phone);
                    const drop = this.dropAt && this.dropAt.key === entry.key ? this.dropAt.zone : "";
                    return Object.assign(entry, { id: "row:" + entry.key, row: node.row, index: node.index, muted: node.muted, parent, open, selected,
                        classes: { "m-tree-row--parent": parent, "m-tree-row--muted": node.muted, selected, "m-tree-row--focus": entry.key === this.focusKey,
                            "m-tree-row--dragged": entry.key === this.dragKey, ["m-tree-drop-" + drop]: !!drop },
                        t: { key: entry.key, level: entry.level, parent, open, loading: !!this.loading[entry.key], indent: indent.px, deep: indent.deep,
                            openLabel: this.labels.open, closeLabel: this.labels.close, toggle: () => this.toggle(entry.key) } });
                });
            },
            focusShown() {
                const shown = this.rows.filter((entry) => entry.kind === "row");
                return shown.some((entry) => entry.key === this.focusKey) ? this.focusKey : (shown[0] ? shown[0].key : "");
            },
            topKeys() { return this.children[""] || []; },
            totalLabel() { return this.labels.total + " (" + this.topKeys.length + ")"; }
        },
        watch: {
            itemsKey() { this.rebuild(); },
            rows: { handler(rows) { this.$emit("keys", rows.filter((entry) => entry.kind === "row").map((entry) => entry.row)); }, immediate: true }
        },
        created() {
            const params = (this.mScreen && this.mScreen.parameters) || (this.$root && this.$root.currentParameters) || {};
            let stored = null;
            try { stored = JSON.parse(root.sessionStorage.getItem("moqui." + this.prefKey + "_tree") || "null"); } catch (err) { stored = null; }
            if (Array.isArray(stored)) { this.openState = {}; stored.forEach((key) => { this.openState[String(key)] = true; }); }
            if (params.treeLevel && this.levelOptions.some((option) => option.value === String(params.treeLevel))) this.level = String(params.treeLevel);
            this.rebuild();
        },
        methods: {
            indentOf(level) { return moqui.listTree.indent(level, this.phone); },
            shownOf(key) { return this.more[key] || 200; },
            // ---- the rows known ----
            add(row, muted, reorder) {
                const key = moqui.listTree.keyOf(row, this.tree.keyFields);
                if (!key) return false;
                const had = this.known[key];
                const index = this.pageIndex[key] === undefined ? null : this.pageIndex[key];
                // a row of the page or of a level keeps its place and is not muted; an ancestor does not take the place of either
                if (had && muted) return false;
                this.known[key] = { key, row: had && had.index !== null && index !== null ? had.row : row, index, parent: lvText(row[this.tree.parentField]), muted: !!muted,
                    order: had && !reorder ? had.order : ++this.order };
                return !had;
            },
            rebuild() {
                this.known = {};
                this.loaded = {};
                this.loading = {};
                this.failed = {};
                this.more = {};
                this.items.forEach((item) => this.add(item.row, false, true));
                this.loadAncestors(0);
                this.fetchOpen();
                if (this.level) this.applyLevel();
            },
            // the parents of the known rows that are not known, from ancestor-transition (ids), again for the ones its answer names
            loadAncestors(round) {
                const missing = moqui.listTree.missing(this.known);
                if (!this.tree.ancestorUrl || !missing.length || round > 8) return;
                this.ask(this.tree.ancestorUrl, Object.assign({}, this.tree.childrenParameters, { ids: missing.join(",") })).then((list) => {
                    let added = 0;
                    list.forEach((row) => { if (row && typeof row === "object" && this.add(row, true, true)) added++; });
                    if (added) this.loadAncestors(round + 1);
                }, (err) => console.warn("The ancestors of the tree did not load", err));
            },
            ask(path, params) {
                const prefix = this.$root && this.$root.appRootPath ? this.$root.appRootPath : "";
                return moqui.request(prefix + path, { params }).then((result) => moqui.listViews.answerRows(result.json, this.tree.listName));
            },
            // One level: the children of key, with the list's find without the parent field's own find, no paging (a pageSize would be kept
            // as the user's page size); the rows of the answer in its order.
            fetchLevel(key) {
                if (!this.tree.childrenUrl || this.loading[key]) return Promise.resolve();
                const tree = this.tree;
                const params = Object.assign(this.find ? this.find([tree.parentField]) : {}, tree.childrenParameters || {},
                    { [tree.parentField]: key, pageNoLimit: "true" });
                this.loading[key] = true;
                this.failed[key] = false;
                return this.ask(tree.childrenUrl, params).then((list) => {
                    list.forEach((row) => { if (row && typeof row === "object" && !row._moquiRowType) this.add(row, false, true); });
                    this.loaded[key] = true;
                    this.loading[key] = false;
                    // the rows under it that a level chosen opens, else the ones kept open
                    if (this.level) this.applyLevel(); else this.fetchOpen();
                }, (err) => {
                    this.loading[key] = false;
                    this.failed[key] = true;
                    console.warn("The rows under " + key + " did not load", err);
                });
            },
            // the children of the open rows that are not shown yet: each open row with no children known (or fewer than its child count)
            fetchOpen() {
                if (!this.openState) return;
                const count = this.tree.childCountField;
                Object.keys(this.openState).forEach((key) => {
                    const node = this.known[key];
                    if (!this.openState[key] || !node || this.loaded[key] || this.loading[key] || this.failed[key]) return;
                    const knownKids = (this.children[key] || []).length;
                    if (count ? moqui.listViews.number(node.row[count]) > knownKids : knownKids === 0) this.fetchLevel(key);
                });
            },
            // ---- open and closed ----
            hasChildren(key) {
                const node = this.known[key];
                if (!node) return false;
                if ((this.children[key] || []).length) return true;
                const count = this.tree.childCountField;
                if (count) return moqui.listViews.number(node.row[count]) > 0;
                return !this.loaded[key];
            },
            // open: as the user left it, else the branches that hold a known row
            isOpen(key) { return this.openState ? !!this.openState[key] : (this.children[key] || []).length > 0; },
            saveOpen() {
                const keys = Object.keys(this.openState || {}).filter((key) => this.openState[key]);
                try { root.sessionStorage.setItem("moqui." + this.prefKey + "_tree", JSON.stringify(keys)); } catch (err) { /* no storage */ }
            },
            // a row opened or closed by hand (or by a move): the level chosen no longer says what is open
            setOpen(key, open) {
                this.level = "";
                if (!this.openState) {
                    const now = {};
                    Object.keys(this.known).forEach((other) => { if (this.isOpen(other)) now[other] = true; });
                    this.openState = now;
                }
                this.openState[key] = !!open;
                this.saveOpen();
                if (open && !this.loaded[key] && this.hasChildren(key)) this.fetchLevel(key);
            },
            toggle(key) { this.setOpen(key, !this.isOpen(key)); },
            // Expand to level n: the rows above it open, their levels asked for as they come (Expand all stops past 500 rows)
            expandTo(value) {
                this.level = value;
                this.capped = false;
                this.applyLevel();
                this.levelParam();
            },
            applyLevel() {
                const level = this.level;
                if (!level) return;
                this.openState = moqui.listTree.openTo(this.children, level, (key) => this.hasChildren(key));
                this.saveOpen();
                const big = Object.keys(this.known).length > 500;
                if (big && level === "all" && !this.capped) {
                    this.capped = true;
                    moqui.notifyMessages(lvLocalize("Expand all stopped at 500 rows"));
                }
                Object.keys(this.openState).forEach((key) => {
                    if (!big && !this.loaded[key] && !this.loading[key] && !this.failed[key]) this.fetchLevel(key);
                });
            },
            // the level chosen in the URL as treeLevel; not for a list in a container
            levelParam() {
                const rootVm = this.$root;
                if ((this.mScreen && this.mScreen.containerId) || !rootVm || !rootVm.currentParameters || !root.history || !root.history.replaceState) return;
                rootVm.currentParameters = Object.assign({}, rootVm.currentParameters, { treeLevel: this.level });
                root.history.replaceState(root.history.state, "", rootVm.currentLinkUrl);
            },
            // ---- cells ----
            cellText(row, column) { return (column.fields || []).map((field) => moqui.listViews.text(row, field)).filter((text) => text).join(" "); },
            rollupText(row, name) {
                const shown = row ? row[name + "_display"] : null;
                if (shown !== undefined && shown !== null && shown !== "") return lvText(shown);
                const value = row ? row[name] : null;
                if (value === null || value === undefined || value === "") return "";
                // a number formatted, other text (done of total, 6/8) as it is
                return typeof value === "number" || /^-?\d+(\.\d+)?$/.test(String(value)) ? this.number(Number(value)) : lvText(value);
            },
            number(value) {
                try { return moqui.l10n && moqui.l10n.format ? moqui.l10n.format(value, "#,##0.##") : String(value); } catch (err) { return String(value); }
            },
            // the totals row: the top-level rows' roll-ups, so nothing counts twice; none for a roll-up that is not a number (done of total)
            totalText(name) {
                const values = this.topKeys.map((key) => this.known[key].row[name]).filter((value) => value !== null && value !== undefined && value !== "");
                if (values.some((value) => typeof value !== "number" && !/^-?\d+(\.\d+)?$/.test(String(value)))) return "";
                return this.number(values.reduce((sum, value) => sum + Number(value), 0));
            },
            // ---- rows: click, keys ----
            onRowClick(event, entry) {
                if (moqui.listViews.onControl(event.target, event.currentTarget)) return;
                this.focusKey = entry.key;
                if (this.peekable) this.$emit("select", entry.row);
            },
            rowEl(key) {
                const body = this.$refs.body;
                if (!body) return null;
                return Array.from(body.querySelectorAll("[data-tree-key]")).find((el) => el.getAttribute("data-tree-key") === key) || null;
            },
            focusRow(key) {
                if (!key) return;
                this.focusKey = key;
                const entry = this.rows.find((other) => other.kind === "row" && other.key === key);
                if (entry) this.$emit("focusrow", entry.row);
                this.$nextTick(() => { const el = this.rowEl(key); if (el) el.focus(); });
            },
            // the treegrid's keys on the focused row (not in a control of it): Up, Down, Home, End, Right, Left, Enter, and the moves
            onKey(event) {
                const rowEl = event.target && event.target.closest ? event.target.closest("[data-tree-key]") : null;
                if (!rowEl || rowEl !== event.target) return;
                const key = rowEl.getAttribute("data-tree-key");
                const shown = this.rows.filter((entry) => entry.kind === "row");
                const at = shown.findIndex((entry) => entry.key === key);
                const entry = shown[at];
                if (!entry) return;
                const go = (other) => { event.preventDefault(); if (other) this.focusRow(other.key); };
                if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) { event.preventDefault(); this.moveBy(entry, event.key === "ArrowUp" ? "up" : "down"); return; }
                if (event.altKey || event.ctrlKey || event.metaKey) return;
                if (event.key === "ArrowDown") go(shown[at + 1]);
                else if (event.key === "ArrowUp") go(shown[at - 1]);
                else if (event.key === "Home") go(shown[0]);
                else if (event.key === "End") go(shown[shown.length - 1]);
                else if (event.key === "ArrowRight") {
                    event.preventDefault();
                    if (entry.parent && !entry.open) this.setOpen(key, true);
                    else if (entry.open && shown[at + 1] && shown[at + 1].level > entry.level) this.focusRow(shown[at + 1].key);
                } else if (event.key === "ArrowLeft") {
                    event.preventDefault();
                    if (entry.open) this.setOpen(key, false);
                    else { const up = moqui.listTree.placed(this.known, key); if (up) this.focusRow(up); }
                } else if (event.key === "Enter") {
                    const link = rowEl.querySelector("a[href]");
                    event.preventDefault();
                    if (link) link.click();
                    else if (this.peekable) this.$emit("select", entry.row);
                } else if (event.key === "Tab" && this.canMove && !entry.muted) {
                    event.preventDefault();
                    this.moveBy(entry, event.shiftKey ? "outdent" : "indent");
                }
            },
            // ---- moves ----
            // a row's level known (its children fetched when it may have some), for a move that makes the row its last child
            levelKnown(key) { return this.loaded[key] || !this.hasChildren(key) ? Promise.resolve() : this.fetchLevel(key); },
            moveBy(entry, how) {
                if (!this.canMove || entry.muted) return;
                const lt = moqui.listTree;
                const siblings = this.children[lt.placed(this.known, entry.key)] || [];
                const under = how === "indent" ? siblings[siblings.indexOf(entry.key) - 1] : null;
                (under ? this.levelKnown(under) : Promise.resolve()).then(() => this.postMove(entry.key, lt.move(this.known, this.children, entry.key, how)));
            },
            postMove(key, target) {
                if (!target) return;
                const lt = moqui.listTree;
                const now = lt.place(this.known, this.children, key);
                if (now.parent === target.parent && now.previous === target.previous) return;
                const tree = this.tree;
                const params = Object.assign({}, tree.parameters || {}, moqui.listViews.keyParams(this.known[key].row, tree.keyFields),
                    { [tree.parentField]: target.parent || "", previousSiblingId: target.previous || "" });
                // the new parent open when the list loads again
                if (target.parent) this.setOpen(target.parent, true);
                this.post(tree.transition, params).then((ok) => { if (ok) this.$emit("moved"); });
            },
            // a drag starts from a row's own area, not from a control of it (a field keeps its text selection)
            arm(event, entry) { this.armed = this.canMove && !entry.muted && !moqui.listViews.onControl(event.target, event.currentTarget) ? entry.key : ""; },
            onDragStart(event, entry) {
                if (this.armed !== entry.key) { event.preventDefault(); return; }
                this.dragKey = entry.key;
                if (event.dataTransfer) { event.dataTransfer.effectAllowed = "move"; try { event.dataTransfer.setData("text/plain", entry.key); } catch (err) { /* IE */ } }
            },
            // the zone of a row under the pointer: its upper quarter before it, its lower quarter after it, the middle into it
            zoneOf(event) {
                const rect = event.currentTarget.getBoundingClientRect();
                const y = event.clientY - rect.top;
                return y < rect.height * 0.25 ? "before" : y > rect.height * 0.75 ? "after" : "into";
            },
            onDragOver(event, entry) {
                if (!this.dragKey) return;
                const zone = this.zoneOf(event);
                if (!moqui.listTree.drop(this.known, this.children, this.dragKey, entry.key, zone, entry.open)) { this.dropAt = null; return; }
                event.preventDefault();
                if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
                if (!this.dropAt || this.dropAt.key !== entry.key || this.dropAt.zone !== zone) this.dropAt = { key: entry.key, zone };
                // a closed row held over for 600 ms opens
                if (zone === "into" && entry.parent && !entry.open) {
                    if (this.hover.key !== entry.key) this.hover = { key: entry.key, since: Date.now() };
                    else if (Date.now() - this.hover.since > 600) { this.hover = { key: "", since: 0 }; this.setOpen(entry.key, true); }
                }
            },
            onDrop(event, entry) {
                const key = this.dragKey;
                const zone = this.zoneOf(event);
                const open = entry.open;
                event.preventDefault();
                this.onDragEnd();
                if (!key) return;
                (zone === "into" ? this.levelKnown(entry.key) : Promise.resolve())
                    .then(() => this.postMove(key, moqui.listTree.drop(this.known, this.children, key, entry.key, zone, open)));
            },
            onDragEnd() { this.dragKey = ""; this.dropAt = null; this.armed = ""; this.hover = { key: "", since: 0 }; }
        }
    });

    // The indent, number past the deepest level and chevron of a tree row (m-list-tree), first in its first cell; t is the row's tree
    // props: level, parent (it has or may have children), open, loading, indent (px), deep, toggle. The guide lines of the open branches
    // above it are the indent's background, a line at the middle of each level's chevron.
    component("m-tree-lead", {
        name: "mTreeLead",
        props: { t: Object },
        template:
            '<span v-if="t" class="m-tree-lead row no-wrap items-center">' +
                '<span class="m-tree-indent" :style="{width: t.indent + \'px\'}"></span>' +
                '<span v-if="t.deep" class="m-tree-depth m-muted text-caption">{{t.level}}</span>' +
                '<q-btn v-if="t.parent" flat dense round size="sm" tabindex="-1" icon="sym_o_chevron_right" class="m-tree-chevron" :class="{\'m-tree-chevron--open\': t.open}"' +
                    ' :loading="t.loading" :aria-label="t.open ? t.closeLabel : t.openLabel" @click.stop="t.toggle()"></q-btn>' +
                '<span v-else class="m-tree-gutter"></span>' +
            '</span>'
    });

    /* ========== elapsed time ========== */

    // The time since a moment as h:mm (the hours past 24 as they are), 0:00 for a moment to come; empty for none. from is epoch millis
    // (a number or its text) or a date and time text. Pure, for the node tests.
    moqui.elapsedText = (from, now) => {
        if (from === null || from === undefined || from === "") return "";
        const text = String(from).trim();
        const start = from instanceof Date ? from.getTime() : /^-?\d+$/.test(text) ? Number(text) : Date.parse(text.replace(/^(\d{4}-\d\d-\d\d) /, "$1T"));
        if (!isFinite(start)) return "";
        const minutes = Math.max(0, Math.floor(((now === undefined ? Date.now() : now) - start) / 60000));
        return Math.floor(minutes / 60) + ":" + String(minutes % 60).padStart(2, "0");
    };

    // m-elapsed (design-projects §15): <m-elapsed :from="…"> the time since from as h:mm, ticking as each minute passes, for a running
    // timer; in an XML screen through render-mode.
    component("m-elapsed", {
        name: "mElapsed",
        props: { from: [Number, String, Date] },
        data() { return { now: Date.now() }; },
        template: '<span class="m-elapsed">{{text}}</span>',
        computed: { text() { return moqui.elapsedText(this.from, this.now); } },
        watch: { from() { this.now = Date.now(); this.schedule(); } },
        mounted() { this.schedule(); },
        beforeUnmount() { clearTimeout(this.timer); },
        methods: {
            // the next tick when the next minute since from passes
            schedule() {
                clearTimeout(this.timer);
                const start = Number(this.from);
                const since = isFinite(start) && start > 0 ? (Date.now() - start) % 60000 : 0;
                this.timer = setTimeout(() => { this.now = Date.now(); this.schedule(); }, Math.max(1000, 60000 - since));
            }
        }
    });

    /* ========== markdown ========== */

    // Markdown text as HTML (m-markdown, the markdown-view element): GitHub flavored markdown by Marked (libs/marked), cleaned by DOMPurify
    // (libs/dompurify), both loaded on first use. Each heading gets an id from its text (headingPrefix, md- by default, then the words, made
    // unique), and the outline is emitted; a link goes through the resolver (the resolveLink prop: (href, info) => a new href, an object
    // {href, external, className, title, missing}, or nothing for the default), else a relative link is resolved from basePath; a link that
    // leaves the application (another scheme or host) opens in a new tab with noopener noreferrer, an application path goes through the shell
    // (setUrl, or a new tab with Ctrl or Cmd), #id scrolls to the heading. wikiLinks expands [[path]] and [[path|text]] to wiki:path links first,
    // outside code, as index#WikiPageLinks reads them, so a resolver sees wiki:path. A table is wrapped in a scrolling q-table container and
    // code blocks and tables take their tint from the text color, so both read in dark mode. The helpers are pure (node tests in
    // WebrootVue.qvt.markdown.test.js).
    const mdAttr = (value) => String(value === null || value === undefined ? "" : value).replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const mdDecode = (text) => String(text || "").replace(/<[^>]*>/g, "").replace(/&(#x[0-9a-f]+|#\d+|amp|lt|gt|quot|apos|nbsp|#39);/gi, (whole, code) => {
        const named = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", "#39": "'" };
        const key = code.toLowerCase();
        if (named[key] !== undefined) return named[key];
        const number = key.charAt(1) === "x" ? parseInt(key.slice(2), 16) : parseInt(key.slice(1), 10);
        return isNaN(number) ? whole : String.fromCodePoint(number);
    });
    const MD_WIKI_LINK = /\[\[([^\]|\n]+)(?:\|([^\]\n]+))?\]\]/g;
    moqui.markdown = {
        // [[path]] and [[path|text]] as markdown links to wiki:path, outside fenced code and code spans
        expandWikiLinks(text) {
            const out = [];
            let fence = null;
            String(text || "").split("\n").forEach((line) => {
                const open = line.match(/^ {0,3}(`{3,}|~{3,})/);
                if (fence) {
                    if (open && open[1].charAt(0) === fence.charAt(0) && open[1].length >= fence.length) fence = null;
                    out.push(line);
                    return;
                }
                if (open) { fence = open[1]; out.push(line); return; }
                out.push(line.split(/(`[^`\n]*`)/).map((piece, index) => (index % 2 ? piece : piece.replace(MD_WIKI_LINK, (all, path, label) =>
                    "[" + String(label || path).replace(/[[\]]/g, "").trim() + "](wiki:" + String(path).trim().split("/").map((part) => encodeURIComponent(part.trim())).join("/") + ")"))).join(""));
            });
            return out.join("\n");
        },
        // the words of a heading as an id: letters and digits, the rest one dash, at most 60 characters
        slug(text) {
            return mdDecode(text).toLowerCase().replace(/[^\p{L}\p{N}]+/gu, "-").replace(/^-+|-+$/g, "").substring(0, 60) || "section";
        },
        // what a link is: anchor (#id), mail (mailto:, tel:), wiki (wiki:), external (another scheme or host), path (/...), relative, or none
        linkKind(href) {
            const text = String(href || "").trim();
            if (!text) return "none";
            if (text.charAt(0) === "#") return "anchor";
            if (/^(mailto|tel):/i.test(text)) return "mail";
            if (/^wiki:/i.test(text)) return "wiki";
            if (/^\/\//.test(text) || /^[a-z][a-z0-9+.-]*:/i.test(text)) return moqui.isExternalLink(text) ? "external" : "path";
            if (text.charAt(0) === "/") return "path";
            return "relative";
        },
        // a relative link from a base path, a directory ("a/b" from "/x/y" is "/x/y/a/b"; ".." its parent), its query and anchor kept
        resolvePath(href, basePath) {
            const text = String(href || "").trim();
            const cut = text.search(/[?#]/);
            const path = cut < 0 ? text : text.slice(0, cut);
            const parts = String(basePath || "").split("/").filter((part) => part);
            path.split("/").forEach((part) => {
                if (part === "" || part === ".") return;
                if (part === "..") parts.pop();
                else parts.push(part);
            });
            return "/" + parts.join("/") + (cut < 0 ? "" : text.slice(cut));
        },
        // a link of the text as the page shows it: {href, kind, external, className, title}; the resolver first, then the base path
        resolve(href, options, info) {
            const md = moqui.markdown;
            const opts = options || {};
            let out = { href: String(href || ""), kind: md.linkKind(href), className: "", title: "" };
            if (typeof opts.resolveLink === "function") {
                let given = null;
                try { given = opts.resolveLink(out.href, Object.assign({ kind: out.kind }, info)); } catch (err) { console.warn("m-markdown: the link resolver failed for " + out.href, err); }
                if (typeof given === "string") out = { href: given, kind: md.linkKind(given), className: "", title: "" };
                else if (given && typeof given === "object" && given.href !== undefined) {
                    out = { href: String(given.href), kind: md.linkKind(given.href), className: given.className || (given.missing ? "text-negative" : ""), title: given.title || "" };
                    if (given.external !== undefined) out.kind = given.external ? "external" : (out.kind === "external" ? "path" : out.kind);
                }
            }
            if (out.kind === "relative" && opts.basePath) out = Object.assign(out, { href: md.resolvePath(out.href, opts.basePath), kind: "path" });
            out.external = out.kind === "external";
            return out;
        },
        // {html, outline}: the markdown with heading ids and resolved links, tables in their containers; not cleaned yet (sanitize)
        render(marked, text, options) {
            const md = moqui.markdown;
            const opts = options || {};
            const prefix = opts.headingPrefix === undefined || opts.headingPrefix === null ? "md-" : String(opts.headingPrefix);
            const used = new Set();
            const outline = [];
            const instance = new marked.Marked({ gfm: true });
            instance.use({
                renderer: {
                    heading(token) {
                        const inner = this.parser.parseInline(token.tokens);
                        const plain = mdDecode(this.parser.parseInline(token.tokens, this.parser.textRenderer)).trim();
                        let id = prefix + md.slug(plain);
                        while (used.has(id)) id += "-1";
                        used.add(id);
                        outline.push({ id, level: token.depth, text: plain });
                        return "<h" + token.depth + ' id="' + mdAttr(id) + '">' + inner + "</h" + token.depth + ">\n";
                    },
                    link(token) {
                        const inner = this.parser.parseInline(token.tokens);
                        const link = md.resolve(token.href, opts, { text: mdDecode(inner) });
                        let attrs = ' href="' + mdAttr(link.href) + '"';
                        if (token.title || link.title) attrs += ' title="' + mdAttr(link.title || token.title) + '"';
                        if (link.className) attrs += ' class="' + mdAttr(link.className) + '"';
                        if (link.kind === "external") attrs += ' target="_blank" rel="noopener noreferrer"';
                        else if (link.kind === "anchor") attrs += ' data-md-anchor="' + mdAttr(prefix + md.slug(decodeURIComponent(link.href.slice(1)))) + '"';
                        else if (link.kind === "path") attrs += " data-md-link";
                        return "<a" + attrs + ">" + inner + "</a>";
                    },
                    image(token) {
                        const link = md.resolve(token.href, opts, { image: true });
                        return '<img src="' + mdAttr(link.href) + '" alt="' + mdAttr(token.text) + '"' + (token.title ? ' title="' + mdAttr(token.title) + '"' : "") + ">";
                    },
                    // escapeHtml (a text that never needs raw HTML, such as an agent's answer): raw HTML is shown as text
                    html(token) {
                        return opts.escapeHtml ? mdAttr(token.text !== undefined ? token.text : token.raw) : false;
                    }
                }
            });
            let html;
            try {
                const tokens = instance.lexer(opts.wikiLinks ? md.expandWikiLinks(text) : String(text || ""));
                // a first heading that is the title the page shows already
                const lead = tokens.findIndex((token) => token.type !== "space");
                if (opts.stripTitle && lead >= 0 && tokens[lead].type === "heading" && tokens[lead].depth === 1 &&
                    mdDecode(tokens[lead].text).replace(/[*_`~]/g, "").trim() === String(opts.stripTitle).trim()) tokens.splice(lead, 1);
                html = instance.parser(tokens).replace(/<table>/g, '<div class="m-md-table q-table__container q-table--flat q-table--bordered q-table--dense"><table class="q-table">')
                    .replace(/<\/table>/g, "</table></div>");
            } catch (err) { console.warn("m-markdown: the text did not render", err); html = "<pre>" + mdAttr(text) + "</pre>"; }
            return { html, outline };
        },
        // the HTML cleaned: no script, style, form or frame, no event attribute; a link's target and rel, heading ids and the data
        // attributes of the links stay
        sanitize(purify, html) {
            return purify.sanitize(html, { ADD_ATTR: ["target"], FORBID_TAGS: ["style", "form", "input", "button", "textarea", "select"], FORBID_ATTR: ["style"] });
        }
    };

    let markdownLoad = null;
    const loadMarkdown = () => markdownLoad || (markdownLoad = Promise.all([
        moqui.loadScript("/libs/marked/marked.umd.min.js", null, () => !!root.marked),
        moqui.loadScript("/libs/dompurify/purify.min.js", null, () => !!root.DOMPurify)
    ]).catch((err) => { markdownLoad = null; throw err; }));
    component("m-markdown", {
        name: "mMarkdown",
        props: { id: String, text: String, basePath: String, resolveLink: Function, headingPrefix: { type: String, default: "md-" }, stripTitle: String, wikiLinks: Boolean },
        // outline: the headings [{id, level, text}] once rendered; rendered: the root element; link: {href, event, anchor, preventDefault()}
        // before an application link is followed through the shell (preventDefault: the host follows it)
        emits: ["outline", "rendered", "link"],
        data() { return { html: "", failed: false, loaded: false }; },
        template:
            '<div class="m-markdown" :id="id" @click="onClick">' +
                '<div v-if="failed" class="m-markdown-plain">{{text}}</div>' +
                '<div v-else class="m-markdown-body" v-html="html"></div>' +
            '</div>',
        computed: { renderKey() { return [this.text || "", this.basePath || "", this.headingPrefix, this.stripTitle || "", this.wikiLinks ? "w" : "", this.resolveLink ? "r" : ""].join("\u0000"); } },
        watch: { renderKey() { this.render(); } },
        mounted() { this.render(); },
        methods: {
            render() {
                const text = this.text;
                loadMarkdown().then(() => {
                    if (text !== this.text) return;
                    const md = moqui.markdown;
                    const out = md.render(root.marked, text, { basePath: this.basePath, resolveLink: this.resolveLink, headingPrefix: this.headingPrefix, stripTitle: this.stripTitle, wikiLinks: this.wikiLinks });
                    this.failed = false;
                    this.html = md.sanitize(root.DOMPurify, out.html);
                    this.$emit("outline", out.outline);
                    this.$nextTick(() => this.$emit("rendered", this.$el));
                }, (err) => { console.warn("m-markdown: Marked or DOMPurify did not load", err); this.failed = true; });
            },
            scrollTo(id) {
                const el = this.$el && this.$el.querySelector ? this.$el.querySelector('[id="' + String(id).replace(/["\\]/g, "") + '"]') : null;
                if (el && el.scrollIntoView) el.scrollIntoView({ behavior: "smooth", block: "start" });
            },
            onClick(event) {
                const anchor = event.target && event.target.closest ? event.target.closest("a[href]") : null;
                if (!anchor || !this.$el.contains(anchor)) return;
                if (anchor.hasAttribute("data-md-anchor")) { event.preventDefault(); this.scrollTo(anchor.getAttribute("data-md-anchor")); return; }
                if (!anchor.hasAttribute("data-md-link") || event.ctrlKey || event.metaKey || event.shiftKey || event.button) return;
                event.preventDefault();
                const href = anchor.getAttribute("href");
                // a host that opens the link itself (a wiki page in place) calls preventDefault on what link gives it
                const link = { href, event, anchor, prevented: false, preventDefault() { this.prevented = true; } };
                this.$emit("link", link);
                if (link.prevented) return;
                if (this.$root && this.$root.setUrl) this.$root.setUrl(href);
                else root.location.href = href;
            }
        }
    });

    /* ========== code ========== */

    // Source code with line numbers (m-code, the code-view element): highlight.js (libs/highlightjs: its core and the languages of
    // refresh.mjs's HLJS_LANGS) loaded on first use. The language is the one given, else the file name's extension, else plain text:
    // never guessed, so a large file costs no detection. The code shows at once as plain lines and is highlighted when the library
    // has loaded; a file over 512 KB stays plain. Lines in highlightLines ("3,5-7") are marked and the first is scrolled to. The line
    // numbers are drawn by CSS (data-line), so a copy takes only the code. The helpers are pure (node tests in WebrootVue.qvt.code.test.js).
    const codeEscape = (text) => String(text === null || text === undefined ? "" : text).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    moqui.code = {
        // file extensions (and whole names) to highlight.js languages
        extensions: {
            java: "java", groovy: "groovy", gradle: "groovy", gvy: "groovy", kt: "kotlin", kts: "kotlin",
            xml: "xml", xsd: "xml", xsl: "xml", xslt: "xml", html: "xml", htm: "xml", xhtml: "xml", svg: "xml", ftl: "xml", qvt: "xml", qvue: "xml", vue: "xml",
            js: "javascript", mjs: "javascript", cjs: "javascript", jsx: "javascript", ts: "typescript", mts: "typescript", tsx: "typescript",
            rs: "rust", py: "python", sql: "sql", yml: "yaml", yaml: "yaml", json: "json", md: "markdown", markdown: "markdown",
            css: "css", scss: "css", sh: "bash", bash: "bash", zsh: "bash", diff: "diff", patch: "diff", txt: "plaintext"
        },
        // the highlight.js language of a file name or path; plaintext when the extension is not known
        languageOf(fileName) {
            const name = String(fileName || "").split("/").pop().toLowerCase();
            if (!name) return "plaintext";
            if (name === "dockerfile" || name === "makefile") return "bash";
            const dot = name.lastIndexOf(".");
            return (dot >= 0 && moqui.code.extensions[name.slice(dot + 1)]) || "plaintext";
        },
        // "3,5-7" as the set of those line numbers
        parseLines(text) {
            const lines = new Set();
            String(text || "").split(",").forEach((part) => {
                const match = /^\s*(\d+)\s*(?:-\s*(\d+)\s*)?$/.exec(part);
                if (!match) return;
                const from = Number(match[1]), to = match[2] ? Number(match[2]) : from;
                for (let line = Math.min(from, to); line <= Math.max(from, to) && line - Math.min(from, to) < 100000; line++) lines.add(line);
            });
            return lines;
        },
        // highlighted HTML split into lines, each with its own spans: a span open at a line's end is closed there and opened again on the
        // next line, so a comment or string over several lines keeps its color on each
        splitLines(html) {
            const lines = [];
            const open = [];
            let current = "";
            const re = /(<span[^>]*>)|(<\/span>)|(\n)|([^<\n]+|<)/g;
            let match;
            while ((match = re.exec(String(html || ""))) !== null) {
                if (match[1]) { open.push(match[1]); current += match[1]; }
                else if (match[2]) { open.pop(); current += match[2]; }
                else if (match[3]) { lines.push(current + "</span>".repeat(open.length)); current = open.join(""); }
                else current += match[4];
            }
            lines.push(current + "</span>".repeat(open.length));
            return lines;
        },
        // plain lines, escaped
        plainLines(text) { return codeEscape(text).split("\n"); },
        // text without the trailing newline a file ends with
        trimEnd(text) { return String(text === null || text === undefined ? "" : text).replace(/\r\n?/g, "\n").replace(/\n+$/, ""); }
    };
    let highlightLoad = null;
    const loadHighlight = () => highlightLoad || (highlightLoad = moqui.loadScript("/libs/highlightjs/highlight.min.js", null, () => !!root.hljs)
        .catch((err) => { highlightLoad = null; throw err; }));
    component("m-code", {
        name: "mCode",
        props: { id: String, text: String, language: String, fileName: String, lineNumbers: { type: Boolean, default: true }, highlightLines: String },
        data() { return { highlighted: null }; },
        template:
            '<div class="m-code" :id="id" :class="{\'m-code-numbered\': lineNumbers}">' +
                '<table class="m-code-table"><tbody>' +
                    '<tr v-for="(line, index) in lines" :key="index" :class="{\'m-code-marked\': marked.has(index + 1)}" :data-line-row="index + 1">' +
                        '<td v-if="lineNumbers" class="m-code-number" :data-line="index + 1"></td>' +
                        '<td class="m-code-line"><code class="hljs" v-html="line || \' \'"></code></td>' +
                    '</tr>' +
                '</tbody></table>' +
            '</div>',
        computed: {
            code() { return moqui.code.trimEnd(this.text); },
            lang() { return this.language || moqui.code.languageOf(this.fileName); },
            lines() { return this.highlighted || moqui.code.plainLines(this.code); },
            marked() { return moqui.code.parseLines(this.highlightLines); }
        },
        watch: { code() { this.render(); }, lang() { this.render(); }, highlightLines() { this.$nextTick(() => this.scrollToMark()); } },
        mounted() { this.render(); this.$nextTick(() => this.scrollToMark()); },
        methods: {
            render() {
                this.highlighted = null;
                const code = this.code, lang = this.lang;
                if (lang === "plaintext" || !code || code.length > 512 * 1024) return;
                loadHighlight().then(() => {
                    if (code !== this.code || lang !== this.lang || !root.hljs.getLanguage(lang)) return;
                    try { this.highlighted = moqui.code.splitLines(root.hljs.highlight(code, { language: lang, ignoreIllegals: true }).value); }
                    catch (err) { console.warn("m-code: " + lang + " did not highlight", err); }
                }, (err) => console.warn("m-code: highlight.js did not load", err));
            },
            scrollToMark() {
                const first = Array.from(this.marked).sort((a, b) => a - b)[0];
                const row = first && this.$el && this.$el.querySelector ? this.$el.querySelector('[data-line-row="' + first + '"]') : null;
                if (row && row.scrollIntoView) row.scrollIntoView({ block: "center" });
            }
        }
    });

    /* ========== dashboards ========== */

    // The delta of a number-card: its sign (a leading + or -, else the number's; 0 or no number is none) and, against
    // deltaGood (up, the default: a rise is good; down: a fall is), its tone and arrow. ScreenHtml.js reads it the same way.
    moqui.numberCardDelta = (delta, deltaGood) => {
        const text = String(delta === null || delta === undefined ? "" : delta).trim();
        if (!text || text === "null") return null;
        const number = parseFloat(text.replace(/[^0-9.\-+]/g, ""));
        const sign = /^[-\u2212]/.test(text) ? -1 : /^\+/.test(text) ? 1 : (isNaN(number) ? 0 : Math.sign(number));
        const good = deltaGood === "down" ? sign < 0 : sign > 0;
        return { sign, tone: sign === 0 ? "neutral" : good ? "good" : "bad",
            icon: sign > 0 ? "sym_o_trending_up" : sign < 0 ? "sym_o_trending_down" : "sym_o_trending_flat" };
    };

    // The number-card element (style-guide §8.2): a q-card flat bordered with the title as text-overline, the value (text-h5 for
    // size md, h6 for sm, h4 for lg), the delta in the tone of numberCardDelta and the icon; with href the whole card is a link,
    // m-link for a screen (the shell's setUrl) and a plain anchor when external.
    component("m-number-card", {
        name: "mNumberCard",
        props: { title: String, value: [String, Number], delta: [String, Number], deltaGood: { type: String, default: "up" },
            icon: String, href: String, external: Boolean, size: { type: String, default: "md" } },
        computed: {
            deltaInfo() { return moqui.numberCardDelta(this.delta, this.deltaGood); },
            deltaClass() {
                const tone = this.deltaInfo ? this.deltaInfo.tone : "neutral";
                return tone === "good" ? "text-positive" : tone === "bad" ? "text-negative" : "m-muted";
            },
            valueClass() { return this.size === "sm" ? "text-h6" : this.size === "lg" ? "text-h4" : "text-h5"; },
            shownValue() { return this.value === null || this.value === undefined || this.value === "" || this.value === "null" ? "\u2013" : String(this.value); },
            wrapper() { return this.href ? (this.external ? "a" : "m-link") : "div"; }
        },
        template:
            '<component :is="wrapper" :href="href || undefined" class="m-number-card-wrap" :class="{\'m-number-card-link\': !!href}">' +
                '<q-card flat bordered class="m-number-card" :class="\'m-number-card-\' + size">' +
                    '<q-card-section class="row no-wrap items-start q-pa-md">' +
                        '<div class="col m-number-card-body">' +
                            '<div class="text-overline m-number-card-title ellipsis">{{title}}</div>' +
                            '<div class="m-number-card-value" :class="valueClass">{{shownValue}}</div>' +
                            '<div v-if="deltaInfo" class="text-caption row no-wrap items-center m-number-card-delta" :class="deltaClass">' +
                                '<q-icon :name="deltaInfo.icon" size="xs" class="q-mr-xs"></q-icon><span>{{delta}}</span>' +
                            '</div>' +
                        '</div>' +
                        '<m-menu-icon v-if="icon" :name="icon" size="sm" class="m-number-card-icon"></m-menu-icon>' +
                    '</q-card-section>' +
                '</q-card>' +
            '</component>'
    });

    // The screen context of the suite shell (the context element of a screen): while mounted it registers recordEntity, recordId,
    // title and selectionForm with the root, again when one changes, and takes them back when unmounted. The root's screenContext
    // is the latest registration of the screens shown (a dialog's over the page's).
    let screenContextCount = 0;
    component("m-screen-context", {
        name: "mScreenContext",
        props: { recordEntity: String, recordId: [String, Number], title: String, selectionForm: String },
        template: '<span style="display:none;"></span>',
        data() { screenContextCount += 1; return { contextKey: "screenContext" + screenContextCount }; },
        computed: {
            value() {
                const text = (value) => (value === null || value === undefined || value === "null" ? "" : String(value));
                return { recordEntity: text(this.recordEntity), recordId: text(this.recordId), title: text(this.title), selectionForm: text(this.selectionForm) };
            }
        },
        watch: { value(newValue) { if (this.$root.setScreenContext) this.$root.setScreenContext(this.contextKey, newValue); } },
        mounted() { if (this.$root.setScreenContext) this.$root.setScreenContext(this.contextKey, this.value); },
        unmounted() { if (this.$root.removeScreenContext) this.$root.removeScreenContext(this.contextKey); }
    });

    /* ========== records: the record panels, avatars and record cards ========== */

    // Where m-chatter or m-record-files loads its panel (the chatter and record-files elements, IronStone R5), tested under Node:
    // the subscreen the theme names (shellData.shell chatterScreen or recordFilesScreen) as the current app root mounts it
    // (shellData.apps[].recordScreens[kind]: its link, null when that app has no such subscreen the user may open), as a
    // /apps/... path with .qvue or .qjs when the screen renders that way, and the record and the hints as the query. Without a
    // URL, reason says why (to log once), and wait that the shell data is not there yet.
    moqui.recordPanelUrl = (shellData, appName, kind, record, basePath, linkBasePath) => {
        if (!shellData) return { url: null, wait: true };
        const themeName = (shellData.shell || {})[kind + "Screen"];
        if (!themeName) return { url: null, reason: "the theme names no " + kind + " screen" };
        if (!appName) return { url: null, reason: "there is no current app root" };
        const app = (shellData.apps || []).find((item) => item.name === appName);
        const screen = app && app.recordScreens ? app.recordScreens[kind] : null;
        if (!screen || !screen.url) return { url: null, reason: "app " + appName + " has no " + themeName + " screen the user may open" };
        const info = record || {};
        if (!info.recordEntity || !info.recordId) return { url: null };
        let path = String(screen.url);
        if (linkBasePath && (path === linkBasePath || path.indexOf(linkBasePath + "/") === 0)) path = (basePath || "") + path.slice(linkBasePath.length);
        const modes = screen.renderModes || [];
        if (modes.indexOf(moqui.urlExtensions.vue) >= 0) path += "." + moqui.urlExtensions.vue;
        else if (modes.indexOf(moqui.urlExtensions.js) >= 0) path += "." + moqui.urlExtensions.js;
        const query = { recordEntity: info.recordEntity, recordId: info.recordId, appRoot: appName };
        if (info.defaultTab) query.defaultTab = info.defaultTab;
        if (info.hide) query.hide = info.hide;
        return { url: path + "?" + moqui.objToSearch(query) };
    };

    // The chatter and record-files elements: the panel loaded into an m-dynamic-container (id, for submit-reload-id), as its own
    // request, so the app's authorization applies to it. Nothing (logged once) when the theme names no screen, the page is in no
    // app, or the app does not mount the screen. The panel gets the record and hints from its query, and a .qvue panel, which
    // does not see the query of its own request, from inject: ['mRecordPanel'] ({kind, recordEntity, recordId, appRoot,
    // defaultTab, hide, containerId}).
    const recordPanelLogged = {};
    let recordPanelCount = 0;
    const recordPanel = (kind, name) => ({
        name,
        props: { id: String, recordEntity: [String, Number], recordId: [String, Number], defaultTab: String, hide: String },
        data() { recordPanelCount += 1; return { containerId: this.id || "m-" + kind + "-" + recordPanelCount }; },
        provide() { return { mRecordPanel: root.Vue ? root.Vue.computed(() => this.panel) : null }; },
        template: '<div v-if="loadUrl" :class="\'m-record-panel m-record-panel-\' + kind"><m-dynamic-container ref="container" :id="containerId" :url="loadUrl"></m-dynamic-container></div>',
        computed: {
            kind() { return kind; },
            record() {
                const text = (value) => (value === null || value === undefined || value === "null" ? "" : String(value));
                return { recordEntity: text(this.recordEntity), recordId: text(this.recordId), defaultTab: text(this.defaultTab), hide: text(this.hide) };
            },
            appName() { return this.$root.currentAppName || ""; },
            target() { return moqui.recordPanelUrl(this.$root.shellData, this.appName, kind, this.record, this.$root.basePath, this.$root.linkBasePath); },
            loadUrl() { return this.target.url || null; },
            panel() { return Object.assign({ kind, appRoot: this.appName, containerId: this.containerId }, this.record); }
        },
        watch: {
            target: {
                immediate: true,
                handler(target) {
                    if (!target.reason || recordPanelLogged[target.reason]) return;
                    recordPanelLogged[target.reason] = true;
                    console.info(name + " shows nothing: " + target.reason);
                }
            },
            loadUrl(url, oldUrl) {
                const container = this.$refs.container;
                if (url && oldUrl && container) container.load(url);
            }
        }
    });
    component("m-chatter", recordPanel("chatter", "mChatter"));
    component("m-record-files", recordPanel("recordFiles", "mRecordFiles"));

    // An avatar (design §19): the face image, else the initials of the name on a color of the name, else a person icon. The
    // presence dot is drawn with presence (online, away, dnd, offline), or for user-id with the presence the chat store has
    // (moqui.chat, WebrootChat.js), which it asks for.
    moqui.avatar = {
        // the first letters of the first and the last word ("Ada Lovelace" AL, "ada.lovelace@example.com" AL), one for one word
        initials(name) {
            const text = String(name || "").trim().replace(/@.*$/, "");
            if (!text) return "";
            let words = text.split(/\s+/).filter((word) => word);
            if (words.length === 1) words = words[0].split(/[._-]+/).filter((word) => word);
            const letter = (word) => Array.from(word.replace(/^[^\p{L}\p{N}]+/u, ""))[0] || "";
            const first = letter(words[0] || "");
            const last = words.length > 1 ? letter(words[words.length - 1]) : "";
            return (first + last).toUpperCase();
        },
        // a brand color for a name, the same each time
        color(name) {
            const colors = ["primary", "secondary", "accent", "positive", "info", "deep-purple", "teal", "indigo", "brown", "blue-grey"];
            let hash = 0;
            for (const ch of String(name || "")) hash = (hash * 31 + ch.codePointAt(0)) >>> 0;
            return colors[hash % colors.length];
        },
        // the face image of a user id, from the theme's STRT_AVATAR_URL (qvt.config.avatarUrl). The template must contain
        // ${userId}. Empty without a template or a user id, so the initials show. A 404 still falls back to initials.
        imageUrl(userId, template) {
            const id = String(userId || "").trim();
            const text = String(template || "");
            if (!id || text.indexOf("${userId}") < 0) return "";
            return text.split("${userId}").join(encodeURIComponent(id));
        }
    };
    component("m-avatar", {
        name: "mAvatar",
        props: { name: String, image: String, size: { type: String, default: "32px" }, color: String, textColor: { type: String, default: "white" },
            presence: String, userId: String, square: Boolean, rounded: Boolean, tooltip: Boolean },
        data() { return { failedImage: "" }; },
        computed: {
            shownPresence() { return this.presence || (this.userId && moqui.chat ? moqui.chat.presenceOf(this.userId) : ""); },
            // an image given on the component, else the theme's face-image URL for user-id (none: the initials)
            resolvedImage() {
                if (this.image) return this.image;
                const config = (typeof qvt !== "undefined" && qvt && qvt.config) || moqui.config || {};
                return moqui.avatar.imageUrl(this.userId, config.avatarUrl);
            },
            showImage() { return !!this.resolvedImage && this.failedImage !== this.resolvedImage; },
            initials() { return moqui.avatar.initials(this.name); },
            background() { return this.showImage ? undefined : this.color || moqui.avatar.color(this.name); }
        },
        watch: { userId(id) { if (id && moqui.chat) moqui.chat.watchUsers([id]); } },
        mounted() { if (this.userId && !this.presence && moqui.chat) moqui.chat.watchUsers([this.userId]); },
        template:
            '<q-avatar class="m-avatar" :size="size" :color="background" :text-color="showImage ? undefined : textColor" :square="square" :rounded="rounded"' +
                    ' :aria-label="name || undefined">' +
                '<img v-if="showImage" :src="resolvedImage" :alt="name || \'\'" @error="failedImage = resolvedImage">' +
                '<span v-else-if="initials" class="m-avatar-initials">{{initials}}</span>' +
                '<q-icon v-else name="sym_o_person"></q-icon>' +
                '<span v-if="shownPresence" class="m-avatar-presence" :class="\'m-presence-\' + shownPresence" :title="shownPresence"></span>' +
                '<q-tooltip v-if="tooltip && name">{{name}}</q-tooltip>' +
            '</q-avatar>'
    });

    // A record card (design §19 Record cards): the card JSON a host's card-url answers for {recordEntity, recordId} (BOSS: a
    // transition over mantle.GeneralServices.lookup#ById), or the card prop, as m-record-card shows it:
    //   { title, typeLabel, icon, id, entity, url, status: {label, color} | "label", fields: [{label, value}] }
    // or that object under card. At most two fields with a value are shown; the title falls back to the id.
    moqui.recordCard = {
        normalize(data, entity, id) {
            if (!moqui.isPlainObject(data)) return null;
            const card = moqui.isPlainObject(data.card) ? data.card : data;
            const text = (value) => (value === null || value === undefined ? "" : String(value));
            let status = null;
            if (moqui.isPlainObject(card.status) && card.status.label) status = { label: text(card.status.label), color: text(card.status.color) || "primary" };
            else if (card.status || card.statusLabel) status = { label: text(card.statusLabel || card.status), color: text(card.statusColor) || "primary" };
            const fields = (Array.isArray(card.fields) ? card.fields : []).filter((field) => moqui.isPlainObject(field) && text(field.value) !== "")
                .slice(0, 2).map((field) => ({ label: text(field.label), value: text(field.value) }));
            const recordId = text(card.id || id);
            return { entity: text(card.entity || entity), id: recordId, title: text(card.title) || recordId, typeLabel: text(card.typeLabel || card.type),
                icon: text(card.icon) || "sym_o_description", status, fields, url: text(card.url) || null };
        }
    };
    moqui.recordCardCache = new moqui.LruMap(100);
    component("m-record-card", {
        name: "mRecordCard",
        props: { recordEntity: String, recordId: [String, Number], cardUrl: String, cardParameters: Object, card: Object, dense: Boolean },
        data() { return { loaded: null, failed: false }; },
        computed: {
            info() {
                if (this.card) return moqui.recordCard.normalize(this.card, this.recordEntity, this.recordId);
                return this.loaded;
            },
            requestKey() {
                return this.cardUrl && this.recordId ? this.cardUrl + "|" + (this.recordEntity || "") + "|" + this.recordId + "|" + JSON.stringify(this.cardParameters || {}) : "";
            }
        },
        template:
            '<q-card flat bordered class="m-record-card">' +
                '<q-item v-if="info" :dense="dense" class="m-record-card-item">' +
                    '<q-item-section avatar class="m-record-card-icon"><m-menu-icon :name="info.icon" size="sm"></m-menu-icon></q-item-section>' +
                    '<q-item-section class="m-record-card-body">' +
                        '<q-item-label class="ellipsis text-weight-medium">' +
                            '<m-link v-if="info.url" :href="info.url" class="m-link-text">{{info.title}}</m-link><span v-else>{{info.title}}</span></q-item-label>' +
                        '<q-item-label caption class="ellipsis">{{info.typeLabel}}<span v-if="info.typeLabel && info.id"> · </span>{{info.id}}</q-item-label>' +
                        '<q-item-label v-for="field in info.fields" :key="field.label" caption class="ellipsis"><span class="m-muted">{{field.label}}</span> {{field.value}}</q-item-label>' +
                    '</q-item-section>' +
                    '<q-item-section v-if="info.status" side top><q-chip dense square :class="\'m-tonal text-\' + info.status.color">{{info.status.label}}</q-chip></q-item-section>' +
                '</q-item>' +
                '<q-item v-else-if="failed || !requestKey" :dense="dense" class="m-record-card-item"><q-item-section class="m-muted">{{recordEntity}} {{recordId}}</q-item-section></q-item>' +
                '<q-item v-else :dense="dense" class="m-record-card-item"><q-item-section avatar><q-skeleton type="QAvatar" size="24px"></q-skeleton></q-item-section>' +
                    '<q-item-section><q-skeleton type="text" width="60%"></q-skeleton><q-skeleton type="text" width="40%"></q-skeleton></q-item-section></q-item>' +
            '</q-card>',
        watch: { requestKey: { immediate: true, handler() { this.load(); } } },
        methods: {
            // one request per card URL and record for the page (a timeline repeats a record); a failure shows the entity and id
            load() {
                const key = this.requestKey;
                if (this.card || !key) return;
                let pending = moqui.recordCardCache.get(key);
                if (!pending) {
                    const params = Object.assign({}, this.cardParameters, { recordEntity: this.recordEntity || "", recordId: String(this.recordId) });
                    pending = moqui.request(this.cardUrl, { params }).then((result) => moqui.recordCard.normalize(result.json, this.recordEntity, this.recordId));
                    moqui.recordCardCache.put(key, pending);
                }
                this.failed = false;
                pending.then((info) => {
                    if (key !== this.requestKey) return;
                    if (info) this.loaded = info; else this.failed = true;
                }, (err) => {
                    moqui.recordCardCache.remove(key);
                    if (key !== this.requestKey) return;
                    console.info("m-record-card: " + this.cardUrl + " answered status " + (err && err.status) + " for " + key);
                    this.failed = true;
                });
            }
        }
    });

    /* ========== m-html-view: an email or HTML body in a sandboxed frame ========== */

    // The pure part of m-html-view (IronStone design-drawers-commons §12 reading pane, §19), tested under Node with a small
    // fake DOM (WebrootVue.qvt.htmlview.test.js). The body is cleaned with DOMPurify (libs/dompurify, loaded with the first
    // view), then walked by process: images by imageSource (data: kept, cid: through the host's map, remote ones only when
    // allowed, a placeholder otherwise, tracking pixels dropped), links by linkHref (http, https, mailto, tel; a new window
    // with rel noopener noreferrer), the CSS of style attributes and style blocks by css (remote url() only when allowed,
    // no @import, no expression() or behaviors, no fixed positions), and the reply history at the end (Gmail, Outlook,
    // Apple Mail, Thunderbird and Yahoo markers, or a last blockquote) collapsed into a <details> behind a "..." summary,
    // which opens without a script. srcdoc makes the frame's document with its content security policy: no script, no
    // connection, images only data: (and http and https when remote images are allowed), styles inline.
    const hvEscape = (text) => String(text === null || text === undefined ? "" : text).replace(/[&<>"']/g,
        (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[ch]);
    const HV_QUOTE_CLASSES = ["gmail_quote", "gmail_extra", "yahoo_quoted", "moz-cite-prefix", "protonmail_quote", "zmail_extra"];
    const HV_RUN_IDS = ["divRplyFwdMsg", "appendonsend", "mail-editor-reference-message-container"];
    const HV_ORIGINAL = /^\s*-{2,}\s*(Original Message|Forwarded message|Ursprüngliche Nachricht|Message d'origine)\s*-{2,}/i;
    const HV_ATTRIBUTION = /^\s*(On\s.+\swrote:|Am\s.+\sschrieb.*:|Le\s.+\sa écrit\s*:|El\s.+\sescribió:)\s*$/i;
    moqui.htmlView = {
        // a Content-ID as a key of the host's map: without its angle brackets, lower case
        cidKey(id) { return String(id || "").trim().replace(/^cid:/i, "").replace(/^<|>$/g, "").toLowerCase(); },
        // where an image may come from: {src, remote, blocked}; src null drops it (a relative path, javascript:, file:)
        imageSource(src, options) {
            const opts = options || {};
            const text = String(src || "").trim();
            if (/^data:image\/(png|gif|jpe?g|webp|bmp|svg\+xml|x-icon|avif);/i.test(text)) return { src: text, remote: false, blocked: false };
            if (/^cid:/i.test(text)) {
                const map = opts.cidMap || {};
                const found = map[moqui.htmlView.cidKey(text)];
                return found ? { src: found, remote: false, blocked: false } : { src: null, remote: false, blocked: false };
            }
            const absolute = text.indexOf("//") === 0 ? "https:" + text : text;
            if (/^https?:\/\//i.test(absolute)) return opts.allowRemote ? { src: absolute, remote: true, blocked: false } : { src: null, remote: true, blocked: true };
            return { src: null, remote: false, blocked: false };
        },
        // a link the frame may open: http, https, mailto and tel, and an anchor in the message; null removes the link
        linkHref(href) {
            const text = String(href || "").trim();
            if (!text) return null;
            if (text.charAt(0) === "#") return text;
            if (/^(https?:\/\/|mailto:|tel:)/i.test(text)) return text;
            if (text.indexOf("//") === 0) return "https:" + text;
            return null;
        },
        // CSS as the frame may use it: {css, blocked} with remote url() only when allowed (counted when not), no @import,
        // no expression(), behavior or -moz-binding, no fixed positions; backslash escapes are read first, so none hides
        // a url()
        css(text, allowRemote) {
            let blocked = 0;
            let css = String(text || "").replace(/\\([0-9a-f]{1,6})\s?|\\(.)/gi, (whole, hex, ch) => {
                if (ch !== undefined) return ch;
                const point = parseInt(hex, 16);
                return point > 0 && point <= 0x10ffff ? String.fromCodePoint(point) : "";
            });
            css = css.replace(/@import[^;]*;?/gi, "").replace(/expression\s*\(/gi, "(").replace(/(behavior|-moz-binding)\s*:[^;}]*/gi, "");
            css = css.replace(/position\s*:\s*fixed/gi, "position:static");
            css = css.replace(/url\(\s*(['"]?)([^'")]*)\1\s*\)/gi, (whole, quote, target) => {
                const url = target.trim();
                if (/^data:/i.test(url) && !/^data:[^,]*(script|html)/i.test(url)) return whole;
                if (/^(https?:)?\/\//i.test(url)) {
                    if (allowRemote) return "url(" + quote + url + quote + ")";
                    blocked++;
                    return "none";
                }
                return "none";
            });
            return { css, blocked };
        },
        // a designed message (its own colors or backgrounds) keeps a light card in dark mode; a plain one follows the theme
        isDesigned(html) {
            return /\bbgcolor\s*=|\sbackground\s*=|background(-color|-image)?\s*:|(^|[\s;"'{])color\s*:|<font[^>]*\scolor\s*=/i.test(String(html || ""));
        },
        escape: hvEscape,
        // text already escaped, with its addresses and URLs as links that open in a new window (a URL ends before a
        // space, a quote or an angle bracket, and its trailing punctuation stays text)
        autolink(escaped) {
            return String(escaped || "").replace(/\b(?:https?:\/\/|www\.)(?:(?!&(?:quot|#39|lt|gt);)[^\s<>"'])+|[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g, (whole) => {
                let url = whole;
                let tail = "";
                const trailing = /[.,;:!?)\]}]+$/.exec(url);
                if (trailing) { tail = trailing[0]; url = url.slice(0, -tail.length); }
                if (!url) return whole;
                const plainUrl = url.replace(/&amp;/g, "&");
                const href = /^www\./i.test(plainUrl) ? "https://" + plainUrl : (/^https?:\/\//i.test(plainUrl) ? plainUrl : "mailto:" + plainUrl);
                return '<a href="' + hvEscape(href) + '" target="_blank" rel="noopener noreferrer">' + url + "</a>" + tail;
            });
        },
        // the index of the first line of the reply history at the end of a text (quoted lines, an attribution or an
        // "Original Message" line before them), or -1
        quoteStart(lines) {
            for (let index = 0; index < lines.length; index++) if (HV_ORIGINAL.test(lines[index])) return index;
            let end = lines.length - 1;
            while (end >= 0 && !lines[end].trim()) end--;
            if (end < 0 || !/^\s*>/.test(lines[end])) return -1;
            let start = end;
            while (start > 0 && (/^\s*>/.test(lines[start - 1]) || !lines[start - 1].trim())) start--;
            while (start < end && !lines[start].trim()) start++;
            let before = start - 1;
            while (before >= 0 && !lines[before].trim()) before--;
            if (before >= 0 && HV_ATTRIBUTION.test(lines[before])) start = before;
            else if (before >= 1 && HV_ATTRIBUTION.test(lines[before - 1] + " " + lines[before])) start = before - 1;
            return start;
        },
        summary() { return '<summary class="m-hv-more" title="Show trimmed content" aria-label="Show trimmed content">•••</summary>'; },
        // a text/plain body as HTML: escaped, with links, the reply history at the end collapsed
        textToHtml(text, collapseQuotes) {
            const lines = String(text || "").replace(/\r\n?/g, "\n").split("\n");
            const start = collapseQuotes === false ? -1 : moqui.htmlView.quoteStart(lines);
            const block = (part) => moqui.htmlView.autolink(hvEscape(part.join("\n")));
            if (start < 0) return '<div class="m-hv-plain">' + block(lines) + "</div>";
            const shown = lines.slice(0, start);
            while (shown.length && !shown[shown.length - 1].trim()) shown.pop();
            return '<div class="m-hv-plain">' + block(shown) + "</div>" + '<details class="m-hv-quote">' + moqui.htmlView.summary() +
                '<div class="m-hv-plain">' + block(lines.slice(start)) + "</div></details>";
        },
        // whether a node shows nothing: no text but spaces, no image (a tracking pixel aside)
        isEmptyContent(node) {
            if (!node) return true;
            if (node.nodeType === 3) return !String(node.textContent || "").trim();
            if (node.nodeType !== 1) return true;
            if (String(node.textContent || "").trim()) return false;
            const stack = [node];
            while (stack.length) {
                const el = stack.pop();
                if (String(el.tagName || "").toLowerCase() === "img") {
                    const width = parseInt(el.getAttribute("width") || "", 10), height = parseInt(el.getAttribute("height") || "", 10);
                    if (!(width <= 2 && height <= 2)) return false;
                }
                for (const child of Array.from(el.childNodes || [])) if (child.nodeType === 1) stack.push(child);
            }
            return true;
        },
        // whether nothing shows after a node, up to the root
        isTrailing(node, rootNode) {
            let current = node;
            while (current && current !== rootNode) {
                for (let next = current.nextSibling; next; next = next.nextSibling) if (!moqui.htmlView.isEmptyContent(next)) return false;
                current = current.parentNode;
            }
            return true;
        },
        // what kind of reply history an element starts: "element" (it is the quote), "run" (it and what follows), or null
        quoteKind(el) {
            if (!el || el.nodeType !== 1) return null;
            const tag = String(el.tagName || "").toLowerCase();
            const classes = String(el.getAttribute("class") || "").split(/\s+/);
            const id = String(el.getAttribute("id") || "");
            if (HV_RUN_IDS.indexOf(id) >= 0) return "run";
            if (tag === "hr" && el.nextSibling && el.nextSibling.nodeType === 1 && HV_RUN_IDS.indexOf(String(el.nextSibling.getAttribute("id") || "")) >= 0) return "run";
            if (tag === "blockquote" || classes.some((name) => HV_QUOTE_CLASSES.indexOf(name) >= 0)) return "element";
            if ((tag === "div" || tag === "p") && HV_ORIGINAL.test(String(el.textContent || "")) && String(el.textContent || "").length < 2000) return "run";
            return null;
        },
        // the reply history at the end of a cleaned body, wrapped in <details>; the count of blocks collapsed
        collapse(rootNode, doc) {
            const found = [];
            const walk = (node) => {
                for (const child of Array.from(node.childNodes || [])) {
                    if (child.nodeType !== 1) continue;
                    const kind = moqui.htmlView.quoteKind(child);
                    if (kind) {
                        found.push({ node: child, kind });
                        // a run takes the siblings after it
                        if (kind === "run") return;
                        continue;
                    }
                    walk(child);
                }
            };
            walk(rootNode);
            let count = 0;
            for (const item of found) {
                const parent = item.node.parentNode;
                if (!parent) continue;
                const nodes = [item.node];
                if (item.kind === "run") for (let next = item.node.nextSibling; next; next = next.nextSibling) nodes.push(next);
                if (!moqui.htmlView.isTrailing(nodes[nodes.length - 1], rootNode)) continue;
                const details = doc.createElement("details");
                details.setAttribute("class", "m-hv-quote");
                const summary = doc.createElement("summary");
                summary.setAttribute("class", "m-hv-more");
                summary.setAttribute("title", "Show trimmed content");
                summary.setAttribute("aria-label", "Show trimmed content");
                summary.textContent = "•••";
                details.appendChild(summary);
                parent.insertBefore(details, item.node);
                for (const node of nodes) details.appendChild(node);
                count++;
            }
            return count;
        },
        // the cleaned body made safe for the frame: images, links, style attributes and the reply history; {blocked}
        process(rootNode, doc, options) {
            const opts = options || {};
            let blocked = 0;
            const elements = [];
            const gather = (node) => {
                for (const child of Array.from(node.childNodes || [])) if (child.nodeType === 1) { elements.push(child); gather(child); }
            };
            gather(rootNode);
            const size = (el, name) => {
                const value = parseInt(el.getAttribute(name) || "", 10);
                if (!isNaN(value)) return value;
                const style = /(?:^|;)\s*NAME\s*:\s*(\d+)px/i.source.replace("NAME", name);
                const match = new RegExp(style, "i").exec(String(el.getAttribute("style") || ""));
                return match ? parseInt(match[1], 10) : null;
            };
            for (const el of elements) {
                const tag = String(el.tagName || "").toLowerCase();
                if (el.getAttribute("style") !== null && el.getAttribute("style") !== undefined) {
                    const cleaned = moqui.htmlView.css(el.getAttribute("style"), opts.allowRemote);
                    blocked += cleaned.blocked;
                    el.setAttribute("style", cleaned.css);
                }
                const background = el.getAttribute("background");
                if (background !== null && background !== undefined) {
                    const source = moqui.htmlView.imageSource(background, opts);
                    if (source.blocked) blocked++;
                    if (source.src) el.setAttribute("background", source.src); else el.removeAttribute("background");
                }
                if (tag === "img") {
                    const source = moqui.htmlView.imageSource(el.getAttribute("src"), opts);
                    const width = size(el, "width"), height = size(el, "height");
                    const pixel = width !== null && height !== null && width <= 2 && height <= 2;
                    if (source.src) { el.setAttribute("src", source.src); continue; }
                    if (source.blocked && !pixel) blocked++;
                    // a pixel, or an image with nothing to show and nothing to say, is left out
                    if (pixel || !el.parentNode || (!source.blocked && !el.getAttribute("alt"))) { if (el.parentNode) el.parentNode.removeChild(el); continue; }
                    const holder = doc.createElement("span");
                    holder.setAttribute("class", "m-hv-blocked");
                    holder.setAttribute("title", source.blocked ? "Image hidden" : "Image not available");
                    const dims = [];
                    if (width) dims.push("width:" + Math.min(width, 2000) + "px");
                    if (height) dims.push("height:" + Math.min(height, 2000) + "px");
                    if (dims.length) holder.setAttribute("style", dims.join(";"));
                    holder.textContent = el.getAttribute("alt") || "";
                    el.parentNode.replaceChild(holder, el);
                    continue;
                }
                if (tag === "a" || tag === "area") {
                    const href = moqui.htmlView.linkHref(el.getAttribute("href"));
                    if (href === null) el.removeAttribute("href");
                    else {
                        el.setAttribute("href", href);
                        if (href.charAt(0) !== "#") { el.setAttribute("target", "_blank"); el.setAttribute("rel", "noopener noreferrer"); }
                    }
                } else if (el.getAttribute("target") !== null && el.getAttribute("target") !== undefined) {
                    el.removeAttribute("target");
                }
            }
            const collapsed = opts.collapseQuotes === false ? 0 : moqui.htmlView.collapse(rootNode, doc);
            return { blocked, collapsed };
        },
        // DOMPurify's options: HTML only (no SVG or MathML), no forms, frames, plugins, media, scripts or document-level
        // elements, no srcset, poster or ping (which reach the network on their own), no data- attributes of the sender
        purifyOptions: {
            USE_PROFILES: { html: true }, RETURN_DOM: true, FORCE_BODY: true, ALLOW_DATA_ATTR: false,
            FORBID_TAGS: ["style", "form", "input", "button", "select", "option", "optgroup", "textarea", "iframe", "frame", "frameset",
                "object", "embed", "applet", "base", "link", "meta", "audio", "video", "source", "track", "script", "noscript", "template",
                "dialog", "portal", "title"],
            FORBID_ATTR: ["srcset", "poster", "ping", "formaction", "action", "autofocus", "srcdoc"]
        },
        // the style blocks of an HTML message (the head's included), as one cleaned CSS text, and the rest of it
        splitStyles(html, allowRemote) {
            let blocked = 0;
            const parts = [];
            const rest = String(html || "").replace(/<style[^>]*>([\s\S]*?)<\/style\s*>/gi, (whole, css) => {
                const cleaned = moqui.htmlView.css(css, allowRemote);
                blocked += cleaned.blocked;
                parts.push(cleaned.css.replace(/<\/?style/gi, ""));
                return "";
            });
            return { css: parts.join("\n"), html: rest, blocked };
        },
        // an HTML message cleaned for the frame (a browser, DOMPurify loaded): {content, css, blocked, designed}
        sanitize(html, purify, options) {
            const opts = options || {};
            const styles = moqui.htmlView.splitStyles(html, opts.allowRemote);
            const body = purify.sanitize(styles.html, moqui.htmlView.purifyOptions);
            const done = moqui.htmlView.process(body, body.ownerDocument, opts);
            return { content: body.innerHTML, css: styles.css, blocked: styles.blocked + done.blocked, designed: moqui.htmlView.isDesigned(html) };
        },
        // the frame's content security policy: nothing but inline styles and images (data:, and http and https when
        // remote images are allowed), no script, connection, form, frame or base
        csp(allowRemote) {
            const images = allowRemote ? "data: https: http:" : "data:";
            return "default-src 'none'; img-src " + images + "; style-src 'unsafe-inline'; font-src " + images +
                "; base-uri 'none'; form-action 'none'; frame-src 'none'";
        },
        // the frame's document: its policy, no referrer, the theme (dark for a plain message in dark mode, a light card
        // for a designed one), the message's own styles, then the content in #m-hv-root, which the host measures
        srcdoc(parts) {
            const p = parts || {};
            const dark = !!p.dark && !p.designed;
            const font = p.font || "Roboto, -apple-system, 'Segoe UI', Helvetica, Arial, sans-serif";
            const base = [
                ":root { color-scheme: " + (dark ? "dark" : "light") + "; }",
                "html, body { margin: 0; padding: 0; overflow: hidden; background: " + (p.designed && p.dark ? "#ffffff" : "transparent") + "; }",
                "body { font-family: " + font + "; font-size: " + (p.fontSize || "14px") + "; line-height: 1.5; color: " +
                    (dark ? "#e3e3e3" : "#1f1f1f") + "; overflow-wrap: anywhere; }",
                "#m-hv-root { display: flow-root; overflow-x: auto; padding: " + (p.designed && p.dark ? "12px" : "0") + "; }",
                "img { max-width: 100%; }",
                "a[href] { color: " + (dark ? "#8ab4f8" : "#1a73e8") + "; }",
                "a:not([href]) { color: inherit; text-decoration: none; }",
                ".m-hv-plain { white-space: pre-wrap; font-family: inherit; }",
                "blockquote { margin: 0 0 0 .8ex; padding-left: 1ex; border-left: 2px solid " + (dark ? "#5f6368" : "#dadce0") + "; }",
                "details.m-hv-quote { margin: 8px 0 0; }",
                "summary.m-hv-more { display: inline-block; list-style: none; cursor: pointer; padding: 0 8px; line-height: 14px; font-size: 11px;" +
                    " letter-spacing: 1px; border-radius: 7px; color: " + (dark ? "#e3e3e3" : "#3c4043") + "; background: " + (dark ? "#3c4043" : "#e8eaed") + "; }",
                "summary.m-hv-more::-webkit-details-marker { display: none; }",
                "details.m-hv-quote[open] > summary.m-hv-more { margin-bottom: 6px; }",
                ".m-hv-blocked { display: inline-block; vertical-align: middle; max-width: 100%; min-width: 16px; min-height: 16px; overflow: hidden;" +
                    " box-sizing: border-box; font-size: 11px; color: #80868b; background: " + (dark ? "#2d2e30" : "#f1f3f4") + "; border: 1px dashed #bdc1c6; }"
            ].join("\n");
            return "<!DOCTYPE html><html><head><meta charset=\"utf-8\">" +
                "<meta http-equiv=\"Content-Security-Policy\" content=\"" + hvEscape(moqui.htmlView.csp(!!p.allowRemote)) + "\">" +
                "<meta name=\"referrer\" content=\"no-referrer\">" +
                "<style>" + base + "</style>" + (p.css ? "<style>" + String(p.css).replace(/<\/?style/gi, "") + "</style>" : "") +
                "</head><body><div id=\"m-hv-root\">" + (p.content || "") + "</div></body></html>";
        }
    };

    // The host's cid: images as data: URLs, as the frame shows no other: a data: URL as it is, a URL of this origin
    // fetched with the session (once per page for each, at most 5 MB), anything else left out.
    const hvCidCache = new moqui.LruMap(50);
    const hvCidData = (map) => {
        const out = {};
        const pending = [];
        for (const key of Object.keys(map || {})) {
            const url = String(map[key] || "");
            const id = moqui.htmlView.cidKey(key);
            if (!url || !id) continue;
            if (/^data:image\//i.test(url)) { out[id] = url; continue; }
            let absolute;
            try { absolute = new root.URL(url, root.location.href); } catch (err) { continue; }
            if (absolute.origin !== root.location.origin) continue;
            let loading = hvCidCache.get(absolute.href);
            if (!loading) {
                loading = root.fetch(absolute.href, { credentials: "same-origin" }).then((response) => {
                    if (!response.ok) throw new Error("status " + response.status);
                    return response.blob();
                }).then((blob) => new Promise((resolve, reject) => {
                    if (blob.size > 5 * 1024 * 1024 || !/^image\//.test(blob.type)) { resolve(null); return; }
                    const reader = new root.FileReader();
                    reader.onload = () => resolve(String(reader.result));
                    reader.onerror = () => reject(reader.error);
                    reader.readAsDataURL(blob);
                }));
                hvCidCache.put(absolute.href, loading);
            }
            pending.push(loading.then((data) => { if (data) out[id] = data; }, (err) => {
                hvCidCache.remove(absolute.href);
                console.info("m-html-view: the inline image " + key + " did not load: " + err);
            }));
        }
        return Promise.all(pending).then(() => out);
    };

    // An email or HTML body (design §12 reading pane): html (or text, a text/plain body), cleaned and shown in a frame
    // sandboxed without scripts (allow-same-origin, so the host measures it, and allow-popups-to-escape-sandbox, so its
    // links open in a normal window). Remote images are hidden until "Show images" (the show-images event) or allow-remote
    // (a trusted sender); remote-images says how many were hidden. cid-map is {contentId: url} for the inline images.
    // The reply history at the end is collapsed (collapse-quotes). dark is auto (the theme), on or off. The frame is as
    // tall as its content.
    component("m-html-view", {
        name: "mHtmlView",
        props: { html: String, text: String, allowRemote: Boolean, cidMap: Object, collapseQuotes: { type: Boolean, default: true },
            imagesBar: { type: Boolean, default: true }, dark: { type: String, default: "auto" }, title: { type: String, default: "Message" } },
        emits: ["remote-images", "show-images", "loaded"],
        data() { return { showRemote: false, srcdoc: "", blocked: 0, designed: false, state: "loading", height: 0 }; },
        computed: {
            remoteAllowed() { return this.allowRemote || this.showRemote; },
            isDark() {
                if (this.dark === "on") return true;
                if (this.dark === "off") return false;
                return !!(this.$q && this.$q.dark && this.$q.dark.isActive);
            },
            source() { return [this.html || "", this.text || "", this.remoteAllowed, this.isDark, this.collapseQuotes, JSON.stringify(this.cidMap || {})]; },
            frameStyle() {
                return { height: (this.height || 24) + "px", colorScheme: this.isDark && !this.designed ? "dark" : "light" };
            }
        },
        template:
            '<div class="m-html-view" :data-state="state" :data-blocked="blocked">' +
                '<div v-if="blocked && !remoteAllowed && imagesBar" class="m-html-view-images row items-center no-wrap">' +
                    '<q-icon name="sym_o_hide_image" size="xs" class="q-mr-sm"></q-icon>' +
                    '<span class="col m-muted">{{t("Images from this sender are hidden")}}</span>' +
                    '<q-btn flat dense no-caps color="primary" class="m-html-view-show" :label="t(\'Show images\')" @click="showImages"></q-btn>' +
                '</div>' +
                '<iframe ref="frame" class="m-html-view-frame" :class="{\'m-html-view-card\': designed && isDark}" :title="title"' +
                    ' sandbox="allow-same-origin allow-popups allow-popups-to-escape-sandbox" referrerpolicy="no-referrer"' +
                    ' :srcdoc="srcdoc" :style="frameStyle" @load="onLoad"></iframe>' +
            '</div>',
        watch: { source() { this.render(); } },
        mounted() {
            this.renderCount = 0;
            if (root.ResizeObserver) {
                this.widthObserver = new root.ResizeObserver(() => this.measure());
                this.widthObserver.observe(this.$el);
            }
            this.render();
        },
        beforeUnmount() {
            if (this.widthObserver) this.widthObserver.disconnect();
            if (this.contentObserver) this.contentObserver.disconnect();
            if (this.timers) this.timers.forEach((timer) => root.clearTimeout(timer));
        },
        methods: {
            t(text) { return moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text; },
            render() {
                const count = ++this.renderCount;
                const hv = moqui.htmlView;
                const host = this.$el && root.getComputedStyle ? root.getComputedStyle(this.$el) : null;
                const look = { dark: this.isDark, allowRemote: this.remoteAllowed, font: host ? host.fontFamily : null, fontSize: host ? host.fontSize : null };
                const finish = (built) => {
                    if (count !== this.renderCount) return;
                    this.designed = !!built.designed;
                    this.blocked = built.blocked || 0;
                    this.srcdoc = hv.srcdoc(Object.assign({}, look, built));
                    this.$emit("remote-images", this.remoteAllowed ? 0 : this.blocked);
                };
                const plain = (text) => ({ content: hv.textToHtml(text, this.collapseQuotes), css: "", blocked: 0, designed: false });
                if (!this.html) { this.state = "ready"; finish(plain(this.text || "")); return; }
                Promise.all([moqui.loadScript("/libs/dompurify/purify.min.js", null, () => !!root.DOMPurify), hvCidData(this.cidMap)]).then(([, cids]) => {
                    if (count !== this.renderCount) return;
                    this.state = "ready";
                    finish(hv.sanitize(this.html, root.DOMPurify, { allowRemote: this.remoteAllowed, cidMap: cids, collapseQuotes: this.collapseQuotes }));
                }).catch((err) => {
                    if (count !== this.renderCount) return;
                    // without the cleaner the message is its text: the text body, or the HTML's text read without running it
                    console.info("m-html-view: showing the text of the message: " + err);
                    this.state = "failed";
                    let text = this.text;
                    if (!text && root.DOMParser) text = new root.DOMParser().parseFromString(String(this.html), "text/html").body.textContent;
                    finish(plain(text || ""));
                });
            },
            showImages() { this.showRemote = true; this.$emit("show-images"); },
            // the height of the content: #m-hv-root's box, measured from here (no script runs in the frame), again when
            // the frame's width or content changes, an image loads or a quote opens
            measure() {
                const frame = this.$refs.frame;
                let doc = null;
                try { doc = frame ? frame.contentDocument : null; } catch (err) { doc = null; }
                const box = doc ? doc.getElementById("m-hv-root") : null;
                if (!box) return;
                const height = Math.max(Math.ceil(box.getBoundingClientRect().height), 16);
                if (height !== this.height) this.height = height;
            },
            onLoad() {
                const frame = this.$refs.frame;
                let doc = null;
                try { doc = frame ? frame.contentDocument : null; } catch (err) { doc = null; }
                if (!doc || !doc.getElementById("m-hv-root")) return;
                this.measure();
                if (this.contentObserver) this.contentObserver.disconnect();
                if (root.ResizeObserver) {
                    this.contentObserver = new root.ResizeObserver(() => this.measure());
                    this.contentObserver.observe(doc.getElementById("m-hv-root"));
                }
                const again = () => this.measure();
                doc.addEventListener("toggle", again, true);
                doc.addEventListener("load", again, true);
                if (this.timers) this.timers.forEach((timer) => root.clearTimeout(timer));
                this.timers = [100, 400, 1500].map((delay) => root.setTimeout(again, delay));
                this.$emit("loaded", { height: this.height, blocked: this.blocked });
            }
        }
    });

    /* ========== the suite shell: palette, keys, tray and quick create ========== */

    // The pure part of the palette (IronStone design-drawers-commons §1.5), tested under Node: the prefix of the text, the
    // screens of the shell data with their ranking by title, and the groups in the order the palette shows them.
    const palette = {
        LIMIT: 5,
        RECENT_LIMIT: 25,
        // > is actions only, : screens only, and the prefix of a palette source (@, #, the gallery's ~) the provider groups of
        // the sources with it (the longest prefix that starts the text); otherwise everything is searched. The text after the
        // prefix is trimmed.
        parse(text, sources) {
            const raw = String(text === null || text === undefined ? "" : text).replace(/^\s+/, "");
            const head = raw.charAt(0);
            if (head === ">") return { text: raw.slice(1).trim(), mode: "actions", prefix: ">", groups: [] };
            if (head === ":") return { text: raw.slice(1).trim(), mode: "screens", prefix: ":", groups: [] };
            // ? asks Assist the rest of the text (the Ask row alone)
            if (head === "?") return { text: raw.slice(1).trim(), mode: "ask", prefix: "?", groups: [] };
            let prefix = "";
            (sources || []).forEach((source) => {
                const own = source && source.prefix ? String(source.prefix) : "";
                if (own && raw.startsWith(own) && own.length > prefix.length) prefix = own;
            });
            if (prefix) {
                const groups = [];
                sources.forEach((source) => { if (source && source.prefix === prefix && groups.indexOf(source.group) < 0) groups.push(source.group); });
                return { text: raw.slice(prefix.length).trim(), mode: "sources", prefix, groups };
            }
            return { text: raw.trim(), mode: "all", prefix: "", groups: [] };
        },
        // How well a title matches the text, lower is better and -1 is no match: the whole title, its start, every word of
        // the text starting a word of the title, every word anywhere in it. Case is ignored.
        score(title, text) {
            const shown = String(title || "").toLowerCase();
            const wanted = String(text || "").toLowerCase().trim();
            if (!wanted) return 0;
            if (shown === wanted) return 0;
            if (shown.startsWith(wanted)) return 1;
            const words = wanted.split(/\s+/);
            const titleWords = shown.split(/[\s\-_/.,:;()·›]+/).filter((word) => word.length);
            if (words.every((word) => titleWords.some((titleWord) => titleWord.startsWith(word)))) return 2;
            if (words.every((word) => shown.indexOf(word) >= 0)) return 3;
            return -1;
        },
        // The items of list whose title matches, best first; ties keep the order of the list (the menu order).
        rank(list, text, titleOf) {
            const title = titleOf || ((item) => item.title);
            const scored = [];
            (list || []).forEach((item, index) => {
                const score = palette.score(title(item), text);
                if (score >= 0) scored.push({ item, score, index });
            });
            scored.sort((left, right) => left.score - right.score || left.index - right.index);
            return scored.map((entry) => entry.item);
        },
        // The Screens group's rows: each app root, then its menu entries (depth one and two, as shellData lists them) with
        // the app and the parent entry as the subtitle.
        screens(apps) {
            const out = [];
            (apps || []).forEach((app) => {
                if (!app) return;
                out.push({ title: app.title, subtitle: "", url: app.path, image: app.image, imageType: app.imageType, app: app.name });
                const parents = [];
                (app.menu || []).forEach((entry) => {
                    const depth = Math.max(1, Number(entry.depth) || 1);
                    parents.length = depth - 1;
                    const trail = [app.title].concat(parents.filter((title) => title)).join(" › ");
                    out.push({ title: entry.title, subtitle: trail, url: entry.path, image: entry.image, imageType: entry.imageType, app: app.name });
                    parents[depth - 1] = entry.title;
                });
            });
            return out;
        },
        // The groups shown for a parsed text, in this order: Recent before any text; else Screens, Saved finds and Actions,
        // which the client has, then the provider groups the server answered (input.remote, shellSearch), each with up to
        // LIMIT rows and a last "more" row to its moreUrl. A > text shows only Actions and a : text only Screens, with more
        // rows each; a source prefix only the provider groups of its sources. Every row is a copy with its index in the
        // whole list (the arrow keys move over it).
        groups(query, input) {
            const given = input || {};
            const labels = given.labels || {};
            const out = [];
            const add = (key, label, icon, rows, limit, moreUrl) => {
                if (!rows || !rows.length) return;
                const group = { key, label, icon: icon || null, rows: rows.slice(0, limit).map((row) => Object.assign({}, row)) };
                if (moreUrl) group.rows.push({ more: true, title: (labels.more || "More") + ": " + label, url: moreUrl, icon: "sym_o_arrow_forward" });
                out.push(group);
            };
            const text = query.text;
            const recentLabel = labels.recent || "Recent";
            const screensLabel = labels.screens || "Screens";
            const findsLabel = labels.savedFinds || "Saved finds";
            const actionsLabel = labels.actions || "Actions";
            if (query.mode === "all" && !text) {
                add("recent", recentLabel, "sym_o_history", given.recent, palette.RECENT_LIMIT);
            } else if (query.mode === "screens") {
                add("screens", screensLabel, "sym_o_web", palette.rank(given.screens, text), palette.RECENT_LIMIT);
            } else if (query.mode === "actions") {
                add("actions", actionsLabel, "sym_o_bolt", palette.rank(given.actions, text), 50);
            } else if (query.mode === "ask") {
                // nothing else: the Ask row comes last, below
            } else {
                if (query.mode === "all") {
                    add("screens", screensLabel, "sym_o_web", palette.rank(given.screens, text), palette.LIMIT);
                    add("savedFinds", findsLabel, "sym_o_saved_search", palette.rank(given.savedFinds, text), palette.LIMIT);
                    add("actions", actionsLabel, "sym_o_bolt", palette.rank(given.actions, text), palette.LIMIT);
                }
                (given.remote || []).forEach((group) => {
                    if (!group || (query.mode === "sources" && query.groups.indexOf(group.group) < 0)) return;
                    // a row of a record keeps its entity and id (recordEntity, recordId), so Assist's @ inserts the record
                    const rows = (group.rows || []).filter((row) => row && row.url).map((row) => Object.assign({ title: row.title || row.url, subtitle: row.subtitle || "",
                        icon: row.icon || group.icon || null, url: row.url }, row.recordEntity && row.recordId ? { recordEntity: String(row.recordEntity), recordId: String(row.recordId) } : {}));
                    add("source:" + group.group, group.label || group.group, group.icon, rows, palette.LIMIT, group.moreUrl);
                });
            }
            // the last group: Ask Assist the text (given.ask: Assist is there for this user), alone after ?
            if (given.ask && text && (query.mode === "all" || query.mode === "ask")) {
                const asked = text.length > 80 ? text.slice(0, 79) + "…" : text;
                out.push({ key: "ask", label: labels.assist || "Assist", icon: "sym_o_auto_awesome",
                    rows: [{ ask: true, text, title: (labels.askAssist || "Ask Assist") + " ‘" + asked + "’", icon: "sym_o_auto_awesome" }] });
            }
            let index = 0;
            out.forEach((group) => group.rows.forEach((row) => { row.index = index; index += 1; }));
            return out;
        },
        rows(groups) { return [].concat(...(groups || []).map((group) => group.rows)); }
    };
    moqui.shellPalette = palette;

    // The key map of the suite shell (design-drawers-commons §1.7), tested under Node. Bindings are registered by scope: the
    // shell's under "global", a screen's or a .qvue component's with registerScope(scope, bindings), which returns the
    // function that takes them back. A binding is { keys, label, run(event), action, when(), notInEditor }: keys like "Mod+K"
    // (Ctrl, or Cmd on a Mac), "Ctrl+Shift+A", "Esc", "?" or the chord "g c" (g, then c within 1.5 s); run returns false
    // when it did not use the key; when() says whether it is on now; notInEditor leaves the key to a rich-text editor
    // (contenteditable). The latest scope is tried first and "global" last. A text input, select or editable element
    // keeps the single keys it types (a key with no Ctrl, Alt or Cmd). The KEYMAP user preference changes a binding's keys:
    // JSON {scope: {id: keys}}, where the id is the binding's action, else its keys as registered.
    const KEY_NAMES = { esc: "escape", escape: "escape", enter: "enter", return: "enter", space: " ", spacebar: " ", tab: "tab", up: "arrowup",
        down: "arrowdown", left: "arrowleft", right: "arrowright", del: "delete", delete: "delete", backspace: "backspace", plus: "+" };
    const MODIFIER_KEYS = ["shift", "control", "alt", "meta", "os", "altgraph"];
    const keymap = {
        scopes: [],
        overrides: {},
        pending: null,
        CHORD_MS: 1500,
        isMac: false,
        // A function that, when it answers true, keeps the single typed keys from running (the shell: while a dialog is open).
        typedBlocked: null,
        // The steps of a key text: each { key, ctrl, alt, shift, meta, mod }, key in lower case as KeyboardEvent.key has it.
        parse(text) {
            return String(text || "").trim().split(/\s+/).filter((part) => part.length).map((part) => {
                let key = part;
                let modifiers = [];
                if (part.length > 1 && part.endsWith("++")) { key = "+"; modifiers = part.slice(0, -2).split("+"); }
                else if (part !== "+" && part.indexOf("+") > 0) { const pieces = part.split("+"); key = pieces.pop(); modifiers = pieces; }
                const step = { key: "", ctrl: false, alt: false, shift: false, meta: false, mod: false };
                modifiers.forEach((name) => {
                    const lower = name.toLowerCase();
                    if (lower === "ctrl" || lower === "control") step.ctrl = true;
                    else if (lower === "alt" || lower === "option") step.alt = true;
                    else if (lower === "shift") step.shift = true;
                    else if (lower === "meta" || lower === "cmd" || lower === "command") step.meta = true;
                    else if (lower === "mod") step.mod = true;
                });
                const lower = key.toLowerCase();
                step.key = KEY_NAMES[lower] || lower;
                return step;
            });
        },
        // Whether a keydown event is the step. Mod is Ctrl or Cmd. Shift counts for a letter and a named key; other
        // printable characters (? !) are what the keyboard types with or without it.
        matches(step, event) {
            if (!step || !event || String(event.key || "").toLowerCase() !== step.key) return false;
            if (step.mod) { if (!(event.ctrlKey || event.metaKey)) return false; }
            else if (!!event.ctrlKey !== step.ctrl || !!event.metaKey !== step.meta) return false;
            if (!!event.altKey !== step.alt) return false;
            if ((/^[a-z]$/.test(step.key) || step.key.length > 1) && !!event.shiftKey !== step.shift) return false;
            return true;
        },
        sameStep(left, right) {
            return left.key === right.key && left.ctrl === right.ctrl && left.alt === right.alt && left.shift === right.shift && left.meta === right.meta && left.mod === right.mod;
        },
        // a key a text input types: one character with no Ctrl, Alt, Cmd or Mod
        isTyped(step) { return step.key.length === 1 && !step.ctrl && !step.alt && !step.meta && !step.mod; },
        isEditable(target) {
            if (!target) return false;
            if (target.isContentEditable) return true;
            return !!(target.closest && target.closest("[contenteditable]:not([contenteditable=false])"));
        },
        // a text input, a text area, a select or an editable element
        isText(target) {
            if (!target) return false;
            if (keymap.isEditable(target)) return true;
            const tag = String(target.tagName || "").toUpperCase();
            if (tag === "TEXTAREA" || tag === "SELECT") return true;
            if (tag !== "INPUT") return false;
            const type = String(target.type || "text").toLowerCase();
            return ["button", "checkbox", "radio", "submit", "reset", "image", "file", "range", "color"].indexOf(type) < 0;
        },
        registerScope(scope, bindings) {
            const entry = { scope: String(scope || "global"), bindings: (bindings || []).filter((binding) => binding && binding.keys) };
            keymap.scopes.push(entry);
            return () => {
                const index = keymap.scopes.indexOf(entry);
                if (index >= 0) keymap.scopes.splice(index, 1);
            };
        },
        // The KEYMAP preference: JSON text or a map of scope to {id: keys}. A value that is not one is ignored and answered false.
        setOverrides(value) {
            let map = value;
            if (typeof value === "string") {
                if (!value.trim()) { keymap.overrides = {}; return true; }
                try { map = JSON.parse(value); } catch (err) { keymap.overrides = {}; return false; }
            }
            if (!moqui.isPlainObject(map)) { keymap.overrides = {}; return value === null || value === undefined; }
            const out = {};
            Object.keys(map).forEach((scope) => {
                if (!moqui.isPlainObject(map[scope])) return;
                out[scope] = {};
                Object.keys(map[scope]).forEach((id) => { if (typeof map[scope][id] === "string" && map[scope][id].trim()) out[scope][id] = map[scope][id].trim(); });
            });
            keymap.overrides = out;
            return true;
        },
        idOf(binding) { return binding.action || binding.keys; },
        keysOf(scope, binding) {
            const own = keymap.overrides[scope];
            const id = keymap.idOf(binding);
            return own && own[id] ? own[id] : binding.keys;
        },
        // The bindings on now, in the order they are tried: the other scopes, latest first, then global.
        active() {
            const ordered = keymap.scopes.filter((entry) => entry.scope !== "global").reverse()
                .concat(keymap.scopes.filter((entry) => entry.scope === "global"));
            const out = [];
            ordered.forEach((entry) => entry.bindings.forEach((binding) => {
                if (binding.when && !binding.when()) return;
                const keys = keymap.keysOf(entry.scope, binding);
                out.push({ scope: entry.scope, binding, keys, steps: keymap.parse(keys) });
            }));
            return out;
        },
        // The keys as the help overlay shows them: Mod as Ctrl, or ⌘ on a Mac.
        format(keys) {
            return String(keys || "").trim().split(/\s+/).map((part) => part.replace(/\bMod\b/g, keymap.isMac ? "⌘" : "Ctrl")
                .replace(/\bEsc(ape)?\b/gi, "Esc")).join(" ");
        },
        // What the ? overlay lists: each active binding with a label, the keys formatted, once per scope and keys.
        describe() {
            const seen = {};
            return keymap.active().filter((entry) => entry.binding.label).filter((entry) => {
                const key = entry.scope + "\u0001" + entry.keys;
                if (seen[key]) return false;
                seen[key] = true;
                return true;
            }).map((entry) => ({ scope: entry.scope, keys: keymap.format(entry.keys), label: entry.binding.label }));
        },
        // Handle a keydown event: run the binding the key completes, or keep it as the start of a chord. True when the key
        // was used, and then its default is prevented.
        handle(event, now) {
            if (!event || event.isComposing || event.defaultPrevented) return false;
            const at = now === undefined ? Date.now() : now;
            if (MODIFIER_KEYS.indexOf(String(event.key || "").toLowerCase()) >= 0) return false;
            const inText = keymap.isText(event.target);
            const inEditor = keymap.isEditable(event.target);
            const typedBlocked = keymap.typedBlocked ? !!keymap.typedBlocked() : false;
            const pending = keymap.pending && at - keymap.pending.time <= keymap.CHORD_MS ? keymap.pending.steps : [];
            keymap.pending = null;
            const used = () => { if (event.preventDefault) event.preventDefault(); return true; };
            const attempt = (prefix) => {
                let started = null;
                const entries = keymap.active();
                for (let i = 0; i < entries.length; i++) {
                    const entry = entries[i];
                    const steps = entry.steps;
                    if (steps.length <= prefix.length || !prefix.every((step, index) => keymap.sameStep(step, steps[index]))) continue;
                    const step = steps[prefix.length];
                    if (!keymap.matches(step, event)) continue;
                    // an earlier key of a chord was typed already, so only the first key is kept by a text input
                    if (keymap.isTyped(step) && prefix.length === 0 && (inText || typedBlocked)) continue;
                    if (inEditor && entry.binding.notInEditor) continue;
                    if (steps.length === prefix.length + 1) {
                        if (entry.binding.run && entry.binding.run(event) !== false) return "ran";
                    } else if (!started) started = prefix.concat([step]);
                }
                if (started) {
                    keymap.pending = { steps: started, time: at };
                    return "started";
                }
                return null;
            };
            if (pending.length && attempt(pending)) return used();
            return attempt([]) ? used() : false;
        }
    };
    moqui.keymap = keymap;

    // The palette (design §1.5): a dialog at the top with one input and grouped results. The groups are palette.groups over
    // the root's shell data and the shellSearch answer, which is asked 250 ms after the text stops changing; the arrow keys
    // move, Enter opens the selected row (the first by default) and Ctrl+Enter or Cmd+Enter opens it in a new tab.
    component("m-shell-palette", {
        name: "mShellPalette",
        data() { return { text: "", remote: [], searching: false, selected: 0, seq: 0, timer: null, remoteKey: "" }; },
        template:
            '<q-dialog :model-value="$root.paletteOpen" @update:model-value="$root.paletteOpen = $event" position="top" @show="onShow" @hide="onHide">' +
                '<q-card id="m-palette" flat bordered class="m-palette column no-wrap">' +
                    '<div class="q-pa-sm"><m-text-line v-model="text" id="m-palette-input" name="paletteText" :placeholder="$root.labels.paletteHint" @keydown="onKey">' +
                        '<template v-slot:append><q-spinner v-if="searching" size="xs" color="grey-6"></q-spinner><q-icon v-else name="sym_o_search" size="xs"></q-icon></template>' +
                    '</m-text-line></div>' +
                    '<q-separator></q-separator>' +
                    '<div ref="results" class="col scroll m-palette-results"><q-list dense class="q-pb-sm">' +
                        '<template v-for="group in groups" :key="group.key">' +
                            '<q-item-label header class="q-pt-sm q-pb-xs text-weight-medium">{{group.label}}</q-item-label>' +
                            '<q-item v-for="row in group.rows" :key="group.key + \'_\' + row.index" clickable :active="row.index === selected" active-class="m-palette-active"' +
                                    ' :data-index="row.index" @click="open(row, $event)" @mousemove="selected = row.index">' +
                                '<q-item-section avatar class="m-palette-icon">' +
                                    '<m-menu-icon v-if="row.icon || (row.image && row.imageType === \'icon\')" :name="row.icon || row.image" size="xs"></m-menu-icon>' +
                                    '<img v-else-if="row.image" :src="row.image" alt="" width="18" class="invertible">' +
                                    '<q-icon v-else name="sym_o_chevron_right" size="xs"></q-icon>' +
                                '</q-item-section>' +
                                '<q-item-section><q-item-label lines="1" :class="{\'text-primary\': row.more}">{{row.title}}</q-item-label>' +
                                    '<q-item-label v-if="row.subtitle" caption lines="1">{{row.subtitle}}</q-item-label></q-item-section>' +
                            '</q-item>' +
                        '</template>' +
                        '<q-item v-if="!rows.length && !searching"><q-item-section class="m-muted">{{query.text ? $root.labels.noResults : $root.labels.paletteEmpty}}</q-item-section></q-item>' +
                    '</q-list></div>' +
                    '<q-separator class="gt-xs"></q-separator>' +
                    '<div class="row items-center q-px-sm q-py-xs text-caption m-muted gt-xs">{{hint}}</div>' +
                '</q-card>' +
            '</q-dialog>',
        computed: {
            query() { return palette.parse(this.text, this.$root.paletteSources); },
            groups() {
                const root_ = this.$root;
                return palette.groups(this.query, { recent: root_.paletteRecent, screens: root_.paletteScreens, savedFinds: root_.paletteSavedFinds,
                    actions: root_.paletteActions, remote: this.remote, labels: root_.labels, ask: !!root_.assistOn });
            },
            rows() { return palette.rows(this.groups); },
            // the keys and the prefixes, under the results
            hint() {
                const labels = this.$root.labels;
                const prefixes = [];
                (this.$root.paletteSources || []).forEach((source) => { if (source.prefix && prefixes.indexOf(source.prefix + " " + source.label) < 0) prefixes.push(source.prefix + " " + source.label); });
                return [labels.paletteKeys, "> " + labels.actions, ": " + labels.screens].concat(prefixes).join(" · ");
            }
        },
        watch: {
            query(next) { this.selected = 0; this.schedule(next); },
            rows(list) { if (this.selected >= list.length) this.selected = 0; }
        },
        methods: {
            onShow() {
                const input = root.document.getElementById("m-palette-input");
                if (input && input.focus) input.focus();
            },
            onHide() {
                clearTimeout(this.timer);
                this.seq += 1;
                this.text = "";
                this.remote = [];
                this.remoteKey = "";
                this.searching = false;
                this.selected = 0;
            },
            // the provider groups of the text, 250 ms after it stops changing; an answer to an older text is dropped
            schedule(query) {
                clearTimeout(this.timer);
                const wanted = query.text && (query.mode === "all" || query.mode === "sources") && (this.$root.paletteSources || []).length;
                if (!wanted) {
                    this.seq += 1;
                    this.remote = [];
                    this.remoteKey = "";
                    this.searching = false;
                    return;
                }
                const key = query.mode + "|" + query.groups.join(",") + "|" + query.text;
                if (key === this.remoteKey) return;
                this.searching = true;
                this.timer = setTimeout(() => this.search(query, key), 250);
            },
            search(query, key) {
                this.seq += 1;
                const seq = this.seq;
                const params = { text: query.text, limit: String(palette.LIMIT) };
                if (query.groups.length) params.groups = query.groups.join(",");
                moqui.request(this.$root.appRootPath + "/shellSearch", { params }).then((result) => {
                    if (seq !== this.seq) return;
                    this.searching = false;
                    this.remoteKey = key;
                    this.remote = result.json && Array.isArray(result.json.groups) ? result.json.groups : [];
                }, (err) => {
                    if (seq !== this.seq) return;
                    this.searching = false;
                    this.remote = [];
                    moqui.handleAjaxError(err);
                });
            },
            onKey(event) {
                const key = event.key;
                if (key === "ArrowDown" || key === "ArrowUp") {
                    event.preventDefault();
                    const count = this.rows.length;
                    if (!count) return;
                    this.selected = (this.selected + (key === "ArrowDown" ? 1 : count - 1)) % count;
                    this.$nextTick(() => {
                        const el = this.$refs.results && this.$refs.results.querySelector('[data-index="' + this.selected + '"]');
                        if (el && el.scrollIntoView) el.scrollIntoView({ block: "nearest" });
                    });
                } else if (key === "Enter" && event.shiftKey && this.$root.assistOn) {
                    // Shift+Enter asks Assist the text
                    event.preventDefault();
                    this.open(this.rows.find((row) => row.ask), event);
                } else if (key === "Enter") {
                    event.preventDefault();
                    this.open(this.rows.find((row) => row.index === this.selected) || this.rows[0], event);
                }
            },
            open(row, event) {
                if (row && row.ask) {
                    this.$root.paletteOpen = false;
                    if (moqui.assist) moqui.assist.ask(row.text);
                    return;
                }
                if (!row || !row.url) return;
                const newTab = !!(event && (event.ctrlKey || event.metaKey));
                this.$root.paletteOpen = false;
                if (newTab) root.open(this.$root.getLinkPath(row.url), "_blank");
                else this.$root.setUrl(row.url);
            }
        }
    });

    // The tray panel (design §1.4): one panel at the right of the canvas, 400 px from 600 px and the full width below, with a tab
    // for each tray screen the theme names. A tab is an m-dynamic-container loaded on its first opening and kept (v-show), so
    // its state lasts across navigation; the root keeps the open state, the tab and each tab's scroll position.
    component("m-shell-tray", {
        name: "mShellTray",
        // the containers and scroll elements of the tabs, by key, from the function refs: not reactive
        data() { return { containers: markRaw({}), bodies: markRaw({}) }; },
        template:
            '<q-page-sticky v-if="$root.trayMounted" v-show="$root.trayOpen" position="right" expand class="m-tray-sticky">' +
                '<q-card id="m-tray" flat bordered square class="m-tray column no-wrap">' +
                    '<div class="row items-center no-wrap q-pl-xs">' +
                        '<q-tabs :model-value="$root.trayTab" @update:model-value="$root.openTray($event)" dense no-caps inline-label narrow-indicator align="left"' +
                                ' class="col text-grey-7" active-color="primary" indicator-color="primary">' +
                            '<q-tab v-for="tab in $root.trayTabs" :key="tab.key" :name="tab.key" :icon="tab.icon" :label="tab.label">' +
                                '<q-badge v-if="$root.countOf(tab.key)" color="negative" rounded floating>{{$root.countText(tab.key)}}</q-badge></q-tab>' +
                        '</q-tabs>' +
                        '<q-btn flat round dense icon="sym_o_close" @click="$root.closeTray()"><q-tooltip>{{$root.labels.close}}</q-tooltip></q-btn>' +
                    '</div>' +
                    '<q-separator></q-separator>' +
                    '<div v-for="tab in $root.trayTabs" :key="tab.key" v-show="$root.trayTab === tab.key" :ref="(el) => { bodies[tab.key] = el; }"' +
                            ' class="col scroll q-pa-sm m-tray-body" @scroll="onScroll(tab.key, $event)">' +
                        '<m-dynamic-container v-if="$root.trayLoaded[tab.key]" :ref="(vm) => { containers[tab.key] = vm; }" :id="\'m-tray-\' + tab.key" :url="tab.loadUrl"></m-dynamic-container>' +
                    '</div>' +
                '</q-card>' +
            '</q-page-sticky>',
        methods: {
            onScroll(key, event) { this.$root.keepTrayScroll(key, event.target.scrollTop); },
            // the tab's last scroll position, once it shows again (a hidden element loses it)
            restoreScroll(key) {
                this.$nextTick(() => {
                    const body = this.bodies[key];
                    const top = this.$root.trayScroll[key];
                    if (body && top) body.scrollTop = top;
                });
            },
            // a tab's screen refreshes itself when it can (m-dynamic-container.shellRefresh), else it is loaded again
            reload(key) {
                const container = this.containers[key];
                if (container && container.shellRefresh) container.shellRefresh();
                else if (container && container.reload) container.reload();
            }
        },
        watch: {
            "$root.trayTab"(key) { if (key) this.restoreScroll(key); },
            "$root.trayOpen"(open) { if (open && this.$root.trayTab) this.restoreScroll(this.$root.trayTab); }
        },
        mounted() { this.$root.shellTray = this; },
        unmounted() { if (this.$root.shellTray === this) this.$root.shellTray = null; }
    });

    // The quick-create frame (design §1.5): the screen of a + menu kind in a dialog, with the screen context as parameters
    // (recordEntity, recordId, recordTitle). A form in it that submits without errors closes it (mSubmitHide, which m-form
    // takes when it has no submit-hide-id of its own); it is also the container m-quick-create while open.
    component("m-shell-quick-create", {
        name: "mShellQuickCreate",
        provide() { return { mSubmitHide: this }; },
        template:
            '<m-dialog :model-value="$root.quickCreateShow" @update:model-value="$root.quickCreateShow = $event" id="m-quick-create" :title="$root.quickCreateTitle" width="560" @on-show="onShow">' +
                '<m-dynamic-container v-if="$root.quickCreateShow && $root.quickCreateUrl" id="m-quick-create-body" :url="$root.quickCreateUrl"></m-dynamic-container>' +
            '</m-dialog>',
        methods: {
            onShow() { this.$root.addContainer("m-quick-create", this); },
            hide() { this.$root.quickCreateShow = false; },
            reload() { const url = this.$root.quickCreateUrl; this.$root.quickCreateUrl = ""; this.$nextTick(() => { this.$root.quickCreateUrl = url; }); },
            load(url) { this.$root.quickCreateUrl = url; }
        }
    });

    // The ? overlay: the keys on now, by scope.
    component("m-shell-keys", {
        name: "mShellKeys",
        data() { return { entries: [] }; },
        template:
            '<m-dialog :model-value="$root.keysHelpShow" @update:model-value="$root.keysHelpShow = $event" id="m-keys-help" :title="$root.labels.keyboardShortcuts" width="520">' +
                '<div v-for="group in groups" :key="group.scope" class="q-mb-md">' +
                    '<div class="text-subtitle2 q-mb-xs">{{group.title}}</div>' +
                    '<q-list dense bordered separator class="rounded-borders">' +
                        '<q-item v-for="entry in group.entries" :key="entry.keys"><q-item-section>{{entry.label}}</q-item-section>' +
                            '<q-item-section side><span class="m-kbd">{{entry.keys}}</span></q-item-section></q-item>' +
                    '</q-list>' +
                '</div>' +
            '</m-dialog>',
        computed: {
            groups() {
                const out = [];
                this.entries.forEach((entry) => {
                    let group = out.find((item) => item.scope === entry.scope);
                    if (!group) {
                        group = { scope: entry.scope, title: entry.scope === "global" ? this.$root.labels.keysEverywhere : entry.scope, entries: [] };
                        out.push(group);
                    }
                    group.entries.push(entry);
                });
                return out;
            }
        },
        watch: { "$root.keysHelpShow"(shown) { if (shown) this.entries = keymap.describe(); } }
    });

    /* ========== the application ========== */

    // The application component. Its template is the markup of the page shell (WebrootVue.qvt.html.fancy), which Vue takes from
    // the element it mounts on.
    // The shell of /qapps: the layout, the navigation and the re-login dialog. It is the root's template, kept as a component so it is precompiled
    // with the others; the text the server knows (the theme's logo, title and footer, the labels in the user's language, the logout URL) comes in
    // config.shell and config.labels. The footer items of the theme are HTML that is not compiled by Vue.
    component("m-qapps-root", {
        template:
        "<q-layout view=\"hHh LpR fFf\">\n" +
        "    <q-header bordered id=\"top\" :class=\"$q.dark.isActive ? 'bg-dark text-white' : 'bg-white text-dark'\"><q-toolbar dense>\n" +
        "        <q-btn flat round dense icon=\"menu\" @click=\"toggleLeftOpen()\"></q-btn>\n" +
        // the launcher: the theme's logo as a button with a grid of the apps the user may open (shellData), the current one marked
        "        <q-btn flat dense no-caps id=\"m-launcher\" class=\"q-mx-xs\" :aria-label=\"labels.applications\">\n" +
        "            <img v-if=\"shell.logoUrl\" :src=\"shell.logoUrl\" alt=\"\" height=\"28\"><q-icon v-else name=\"sym_o_apps\"></q-icon>\n" +
        "            <q-tooltip>{{labels.applications}}</q-tooltip>\n" +
        "            <q-menu anchor=\"bottom left\" self=\"top left\" class=\"m-launcher\"><div class=\"q-pa-sm\">\n" +
        "                <div class=\"m-launcher-grid\">\n" +
        "                    <q-btn v-for=\"app in shellApps\" :key=\"app.name\" unelevated no-caps v-close-popup class=\"m-btn-tonal m-launcher-app\"\n" +
        "                            :class=\"{'m-launcher-current': currentApp && app.name === currentApp.name}\" @click=\"setUrl(app.path)\">\n" +
        "                        <div class=\"column items-center no-wrap full-width\">\n" +
        "                            <m-menu-icon v-if=\"app.image && app.imageType === 'icon'\" :name=\"app.image\" size=\"md\"></m-menu-icon>\n" +
        "                            <img v-else-if=\"app.image\" :src=\"app.image\" :alt=\"app.title\" height=\"28\" class=\"invertible\">\n" +
        "                            <q-icon v-else name=\"sym_o_apps\" size=\"md\"></q-icon>\n" +
        "                            <div class=\"m-launcher-title\">{{app.title}}</div>\n" +
        "                        </div>\n" +
        "                    </q-btn>\n" +
        "                </div>\n" +
        "                <q-separator class=\"q-my-sm\"></q-separator>\n" +
        "                <q-btn flat no-caps dense icon=\"sym_o_apps\" :label=\"labels.allApplications\" v-close-popup @click=\"setUrl(linkBasePath + '/AppList')\"></q-btn>\n" +
        "            </div></q-menu>\n" +
        "        </q-btn>\n" +
        // the current app's title; the theme's title outside an app (the AppList). In an app the title opens the menu of the
        // app root's subscreens (the top-level screens): the menu the app-root breadcrumb had before the trail started below it.
        // (below 600 px the launcher stands for it: the title is hidden, style-guide §2.2 and §7)
        "        <q-btn v-if=\"appScreenMenu\" flat dense no-caps id=\"m-app-screens\" class=\"gt-xs m-app-screens text-subtitle1 text-weight-medium\">\n" +
        "            {{headerTitle}}<q-icon name=\"arrow_drop_down\" size=\"xs\"></q-icon>\n" +
        "            <q-menu anchor=\"bottom left\" self=\"top left\"><q-list dense style=\"min-width:200px;\">\n" +
        "                <q-item v-for=\"subscreen in appScreenMenu.subscreens\" :key=\"subscreen.name\" clickable v-close-popup :active=\"subscreen.active\" active-class=\"text-primary text-weight-medium\" :to=\"subscreen.pathWithParams\">\n" +
        "                    <q-item-section avatar>\n" +
        "                        <m-menu-icon v-if=\"subscreen.image && subscreen.imageType === 'icon'\" :name=\"subscreen.image\" size=\"xs\"></m-menu-icon>\n" +
        "                        <img v-else-if=\"subscreen.image\" :src=\"subscreen.image\" :alt=\"subscreen.title\" width=\"18\" class=\"invertible\">\n" +
        "                        <q-icon v-else name=\"chevron_right\" size=\"xs\"></q-icon>\n" +
        "                    </q-item-section>\n" +
        "                    <q-item-section>{{subscreen.title}}</q-item-section>\n" +
        "                </q-item>\n" +
        "            </q-list></q-menu>\n" +
        "        </q-btn>\n" +
        "        <q-toolbar-title v-else-if=\"headerTitle\" shrink class=\"gt-xs text-subtitle1 text-weight-medium q-pr-sm\">{{headerTitle}}</q-toolbar-title>\n" +
        // the screen path from the app root: not the apps root, nor the app root whose title the header shows (style-guide §2.2)
        "        <q-breadcrumbs id=\"m-breadcrumbs\" class=\"gt-xs\" gutter=\"xs\" separator-color=\"grey-5\" active-color=\"grey-8\">\n" +
        "            <template v-for=\"(navMenuItem, menuIndex) in navMenuList\" :key=\"navMenuItem.path || menuIndex\"><template v-if=\"menuIndex >= breadcrumbStart\">\n" +
        "                <q-breadcrumbs-el v-if=\"menuIndex === navMenuList.length - 1\" :label=\"navMenuItem.title\" class=\"text-weight-medium\"></q-breadcrumbs-el>\n" +
        "                <q-breadcrumbs-el v-else-if=\"navMenuItem.subscreens && navMenuItem.subscreens.length && !navMenuItem.hasTabMenu\" class=\"cursor-pointer\">\n" +
        "                    {{navMenuItem.title}}<q-icon name=\"arrow_drop_down\" size=\"xs\"></q-icon>\n" +
        "                    <q-menu anchor=\"bottom left\" self=\"top left\"><q-list dense style=\"min-width:200px;\">\n" +
        "                        <q-item v-for=\"subscreen in navMenuItem.subscreens\" :key=\"subscreen.name\" clickable v-close-popup :active=\"subscreen.active\" active-class=\"text-primary text-weight-medium\" :to=\"subscreen.pathWithParams\">\n" +
        "                            <q-item-section avatar>\n" +
        "                                <m-menu-icon v-if=\"subscreen.image && subscreen.imageType === 'icon'\" :name=\"subscreen.image\" size=\"xs\"></m-menu-icon>\n" +
        "                                <img v-else-if=\"subscreen.image\" :src=\"subscreen.image\" :alt=\"subscreen.title\" width=\"18\" class=\"invertible\">\n" +
        "                                <q-icon v-else name=\"chevron_right\" size=\"xs\"></q-icon>\n" +
        "                            </q-item-section>\n" +
        "                            <q-item-section>{{subscreen.title}}</q-item-section>\n" +
        "                        </q-item>\n" +
        "                    </q-list></q-menu>\n" +
        "                </q-breadcrumbs-el>\n" +
        "                <q-breadcrumbs-el v-else :label=\"navMenuItem.title\" :to=\"getNavHref(menuIndex)\"></q-breadcrumbs-el>\n" +
        "            </template></template>\n" +
        "        </q-breadcrumbs>\n" +
        "\n" +
        "        <q-space></q-space>\n" +
        "\n" +
        // Search opens the palette (Ctrl+K or Cmd+K anywhere); a label and the key from 1024 px, the icon alone below
        "        <q-btn flat dense no-caps id=\"m-search\" icon=\"sym_o_search\" :round=\"!$q.screen.gt.sm\" :label=\"$q.screen.gt.sm ? labels.search : undefined\" @click=\"openPalette()\">\n" +
        "            <span v-if=\"$q.screen.gt.sm\" class=\"m-kbd q-ml-sm\">{{paletteKeyHint}}</span>\n" +
        "            <q-tooltip>{{labels.search}} ({{paletteKeyHint}})</q-tooltip>\n" +
        "        </q-btn>\n" +
        // quick create: the kinds of the apps the user may open, the current app's first; a kind that needs a record only with one in context
        "        <q-btn v-if=\"quickCreateItems.length\" flat round dense id=\"m-quick-create-menu\" icon=\"sym_o_add\">\n" +
        "            <q-tooltip>{{labels.quickCreate}}</q-tooltip>\n" +
        "            <q-menu anchor=\"bottom right\" self=\"top right\"><q-list dense style=\"min-width:220px;\">\n" +
        "                <q-item v-for=\"item in quickCreateItems\" :key=\"item.key\" clickable v-close-popup @click=\"openQuickCreate(item)\">\n" +
        "                    <q-item-section avatar><m-menu-icon :name=\"item.icon || 'sym_o_add'\" size=\"xs\"></m-menu-icon></q-item-section>\n" +
        "                    <q-item-section>{{item.label}}</q-item-section>\n" +
        "                </q-item>\n" +
        "            </q-list></q-menu>\n" +
        "        </q-btn>\n" +
        // Today, Chat, Mail, Messages and Notifications, each only when the theme names its screen; the tray ones open the tray on their tab
        "        <q-btn v-for=\"counter in headerCounters\" :key=\"counter.key\" flat round dense :id=\"'m-counter-' + counter.key\" :icon=\"counter.icon\"\n" +
        "                :class=\"{'text-primary': counter.tray && trayOpen && (counter.any || trayTab === counter.key)}\" @click=\"counter.tray ? toggleTray(counter.any ? null : counter.key) : setUrl(counter.url)\">\n" +
        "            <q-badge v-if=\"counterCount(counter)\" floating rounded color=\"negative\">{{counterText(counter)}}</q-badge>\n" +
        "            <span v-if=\"counter.key === 'chat' && myPresence\" class=\"m-avatar-presence m-shell-presence\" :class=\"'m-presence-' + myPresence\"></span>\n" +
        "            <q-tooltip>{{counter.label}}</q-tooltip>\n" +
        "        </q-btn>\n" +
        "\n" +
        "        <component :is=\"qzVue\" ref=\"qzVue\"></component>\n" +
        "\n" +
        "        <q-btn v-if=\"documentMenuList.length\" flat round dense icon=\"help_outline\">\n" +
        "            <q-tooltip>{{labels.documentation}}</q-tooltip>\n" +
        "            <q-menu><q-list dense style=\"min-width:200px;\">\n" +
        "                <q-item v-for=\"screenDoc in documentMenuList\" :key=\"screenDoc.index\"><q-item-section>\n" +
        "                    <m-dynamic-dialog :url=\"currentPath + '/screenDoc?docIndex=' + screenDoc.index\" :button-text=\"screenDoc.title\" :title=\"screenDoc.title\"></m-dynamic-dialog>\n" +
        "                </q-item-section></q-item>\n" +
        "            </q-list></q-menu>\n" +
        "        </q-btn>\n" +
        "\n" +
        "        <template v-for=\"navPlugin in navPlugins\"><component :is=\"navPlugin\"></component></template>\n" +
        "\n" +
        // the notifications of this page, when no Notifications tab is in the tray (a deployment without BOSS)
        "        <q-btn v-if=\"!shellValues.trayNotifications\" flat round dense icon=\"notifications\">\n" +
        "            <q-tooltip>{{labels.notifyHistory}}</q-tooltip>\n" +
        "            <q-menu><q-list dense separator style=\"min-width:320px;max-width:440px;\">\n" +
        "                <q-item v-if=\"!notifyHistoryList.length\"><q-item-section class=\"text-grey-6\">{{labels.notifyHistory}}</q-item-section></q-item>\n" +
        "                <q-item v-for=\"(histItem, histIndex) in notifyHistoryList\" :key=\"histIndex\" :clickable=\"!!histItem.link\" @click=\"histItem.link && moqui.openLink(histItem.link)\">\n" +
        "                    <q-item-section avatar><q-icon :name=\"notifyIcon(histItem.type)\" :color=\"getQuasarColor(histItem.type) || 'info'\"></q-icon></q-item-section>\n" +
        "                    <q-item-section><q-item-label>{{histItem.message}}</q-item-label><q-item-label caption>{{histItem.time}}</q-item-label></q-item-section>\n" +
        "                </q-item>\n" +
        "            </q-list></q-menu>\n" +
        "        </q-btn>\n" +
        "\n" +
        // (the screen history is the palette's Recent group)
        "        <q-btn flat round dense id=\"m-account\">\n" +
        // the user's avatar with their presence dot (the chat store's, design §1.2)
        "            <m-avatar :name=\"userFullName || username\" :user-id=\"userId\" size=\"28px\"></m-avatar>\n" +
        "            <q-tooltip>{{userFullName || labels.account}}</q-tooltip>\n" +
        "            <q-menu><q-list style=\"min-width:240px;\">\n" +
        "                <q-item v-if=\"userFullName || username\"><q-item-section>\n" +
        "                    <q-item-label class=\"text-weight-medium\">{{userFullName || username}}</q-item-label><q-item-label v-if=\"userFullName\" caption>{{username}}</q-item-label>\n" +
        "                </q-item-section></q-item>\n" +
        "                <q-separator v-if=\"userFullName || username\"></q-separator>\n" +
        // the status: online, away, do not disturb, invisible, a status message (WebrootChat.js)
        "                <m-user-status-menu v-if=\"chatStatus\"></m-user-status-menu>\n" +
        "                <q-separator v-if=\"chatStatus\"></q-separator>\n" +
        // the account-menu-item providers of the apps (IronStone: Preferences and Account)
        "                <q-item v-for=\"item in accountMenuItems\" :key=\"item.url\" clickable v-close-popup @click=\"setUrl(item.url)\">\n" +
        "                    <q-item-section avatar><m-menu-icon :name=\"item.icon || 'sym_o_chevron_right'\"></m-menu-icon></q-item-section>\n" +
        "                    <q-item-section>{{item.label}}</q-item-section>\n" +
        "                </q-item>\n" +
        "                <q-separator v-if=\"accountMenuItems.length\"></q-separator>\n" +
        "                <q-item v-for=\"(accountPlugin, accountIndex) in accountPlugins\" :key=\"accountIndex\"><q-item-section><component :is=\"accountPlugin\"></component></q-item-section></q-item>\n" +
        // the browser terminal (apps/Terminal.xml), when the server serves /termws
        "                <q-item v-if=\"terminalUrl\" id=\"m-account-terminal\" clickable v-close-popup @click=\"setUrl(terminalUrl)\">\n" +
        "                    <q-item-section avatar><q-icon name=\"terminal\"></q-icon></q-item-section>\n" +
        "                    <q-item-section>{{labels.terminal}}</q-item-section>\n" +
        "                </q-item>\n" +
        "                <q-item tag=\"label\" clickable>\n" +
        "                    <q-item-section avatar><q-icon name=\"dark_mode\"></q-icon></q-item-section>\n" +
        "                    <q-item-section>{{labels.switchDarkLight}}</q-item-section>\n" +
        "                    <q-item-section side><q-toggle dense :model-value=\"$q.dark.isActive\" @update:model-value=\"switchDarkLight()\"></q-toggle></q-item-section>\n" +
        "                </q-item>\n" +
        "                <q-item clickable v-close-popup @click=\"reLoginShowDialog\">\n" +
        "                    <q-item-section avatar><q-icon name=\"autorenew\"></q-icon></q-item-section>\n" +
        "                    <q-item-section>{{labels.reLogin}}</q-item-section>\n" +
        "                </q-item>\n" +
        "                <q-separator></q-separator>\n" +
        "                <q-item clickable tag=\"a\" :href=\"shell.logoutUrl\" :data-confirm=\"labels.logout + ' ' + userFullName + '?'\">\n" +
        "                    <q-item-section avatar><q-icon name=\"logout\" color=\"negative\"></q-icon></q-item-section>\n" +
        "                    <q-item-section class=\"text-negative\">{{labels.logout}}</q-item-section>\n" +
        "                </q-item>\n" +
        "            </q-list></q-menu>\n" +
        "        </q-btn>\n" +
        // the Assist toggle, rightmost (WebrootAssist.js, for a user who may use it); its right q-drawer (view LpR) below, the
        // screen context and assistReach its inputs (IronStone impl-drawers-commons R1)
        "        <m-assist-button v-if=\"assistOn\"></m-assist-button>\n" +
        "    </q-toolbar></q-header>\n" +
        "\n" +
        "    <q-drawer v-model=\"leftOpen\" side=\"left\" bordered :width=\"260\" :breakpoint=\"1023\">\n" +
        "        <q-scroll-area class=\"fit\"><q-list dense padding><m-menu-nav-item :menu-index=\"0\"></m-menu-nav-item></q-list></q-scroll-area>\n" +
        "    </q-drawer>\n" +
        "    <m-shell-assist v-if=\"assistOn\"></m-shell-assist>\n" +
        "\n" +
        "    <q-page-container id=\"m-canvas\">\n" +
        // an Assist canvas takes the page while the drawer stays docked; the page stays mounted under it (WebrootAssist.js)
        "        <q-page v-show=\"!assistCanvasShown\" class=\"q-pa-sm\"><m-subscreens-active></m-subscreens-active></q-page>\n" +
        "        <m-assist-canvas v-if=\"assistOn\"></m-assist-canvas>\n" +
        "        <m-shell-tray v-if=\"trayTabs.length\"></m-shell-tray>\n" +
        "    </q-page-container>\n" +
        "\n" +
        "    <q-footer bordered id=\"footer\" class=\"text-caption q-pa-xs\" :class=\"$q.dark.isActive ? 'bg-dark text-grey-5' : 'bg-grey-2 text-grey-8'\">\n" +
        "        <div v-html=\"shell.footerHtml\" class=\"row full-width\"></div>\n" +
        "    </q-footer>\n" +
        "</q-layout>\n" +
        "\n" +
        "<m-dialog v-model=\"reLoginShow\" width=\"400\" :title=\"labels.reLogin\">\n" +
        "    <div v-if=\"reLoginMfaData\">\n" +
        "        <div class=\"text-center q-pb-sm\">User <strong>{{username}}</strong> requires an authentication code, you have these options:</div>\n" +
        "        <div class=\"text-center q-pb-sm\">{{reLoginMfaData.factorTypeDescriptions.join(\", \")}}</div>\n" +
        "        <q-form @submit.prevent=\"reLoginVerifyOtp\" autocapitalize=\"off\" autocomplete=\"off\" class=\"q-gutter-y-sm\">\n" +
        "            <m-text-line v-model=\"reLoginOtp\" name=\"code\" type=\"password\" :label=\"labels.authenticationCode\"></m-text-line>\n" +
        "            <q-btn unelevated no-caps color=\"primary\" type=\"submit\" :label=\"labels.signIn\"></q-btn>\n" +
        "        </q-form>\n" +
        "        <div v-for=\"sendableFactor in reLoginMfaData.sendableFactors\" :key=\"sendableFactor.factorId\" class=\"q-pt-sm\">\n" +
        "            <q-btn unelevated no-caps dense class=\"m-btn-tonal\" :label=\"labels.sendCodeTo + ' ' + sendableFactor.factorOption\" @click.prevent=\"reLoginSendOtp(sendableFactor.factorId)\"></q-btn>\n" +
        "        </div>\n" +
        "    </div>\n" +
        "    <div v-else>\n" +
        "        <div class=\"text-center q-pb-sm\">Please sign in to continue as user <strong>{{username}}</strong></div>\n" +
        "        <q-form @submit.prevent=\"reLoginSubmit\" autocapitalize=\"off\" autocomplete=\"off\" class=\"q-gutter-y-sm\">\n" +
        "            <m-text-line v-model=\"reLoginPassword\" name=\"password\" type=\"password\" :label=\"labels.password\"></m-text-line>\n" +
        "            <div><q-btn unelevated no-caps color=\"primary\" type=\"submit\" :label=\"labels.signIn\"></q-btn>\n" +
        "            <q-btn flat no-caps color=\"negative\" @click.prevent=\"reLoginReload\" :label=\"labels.reloadPage\"></q-btn></div>\n" +
        "        </q-form>\n" +
        "    </div>\n" +
        "</m-dialog>\n" +
        "\n" +
        // the palette, the quick-create dialog and the ? overlay of the keys
        "<m-shell-palette></m-shell-palette>\n" +
        "<m-shell-quick-create></m-shell-quick-create>\n" +
        "<m-shell-keys></m-shell-keys>\n" +
        "<m-user-status-dialog v-if=\"chatStatus\"></m-user-status-dialog>\n"
    });

    // The text of the suite shell when the page's labels (qapps.xml, localized there) do not have it.
    qvt.shellLabels = { terminal: "Terminal UI", applications: "Applications", allApplications: "All applications", search: "Search", quickCreate: "Create",
        quickCreateFirst: "Create in this app", today: "Today", chat: "Chat", mail: "Mail", messages: "Messages", notifications: "Notifications", close: "Close",
        closeTopmost: "Close the dialog, menu or panel", keyboardShortcuts: "Keyboard shortcuts", keysEverywhere: "Everywhere", recent: "Recent",
        screens: "Screens", savedFinds: "Saved finds", actions: "Actions", more: "More", noResults: "Nothing found",
        paletteHint: "Search screens, records and actions", paletteEmpty: "Type to search screens, records, people and actions",
        paletteKeys: "↑ ↓ to move, Enter to open, Ctrl+Enter in a new tab", assist: "Assist", askAssist: "Ask Assist" };

    const rootOptions = (config) => ({
        data() {
            return {
                basePath: config.basePath || "", linkBasePath: config.linkBasePath || "", currentPathList: [], extraPathList: [], currentParameters: {}, bodyParameters: null,
                activeSubscreens: [], navMenuList: [], navHistoryList: [], navPlugins: [], accountPlugins: [], notifyHistoryList: [],
                lastNavTime: Date.now(), loading: 0, currentLoadRequest: null, activeContainers: {}, urlListeners: [],
                moquiSessionToken: config.sessionToken || "", appHost: config.appHost || "", appRootPath: config.appRootPath || "", userId: config.userId || "",
                username: config.username || "", locale: config.locale || "en", userFullName: config.userFullName || "",
                terminalUrl: config.terminalUrl || "",
                reLoginShow: false, reLoginPassword: null, reLoginMfaData: null, reLoginOtp: null,
                notificationClient: null, sessionTokenBc: null, qzVue: null, leftOpen: !!config.leftOpen,
                // the m-screen-context registrations, oldest first; formsVersion changes as the forms of the screens come and go
                screenContexts: [], formsVersion: 0,
                // the suite shell: shellData (fetched once per page load and after a re-login) and the latest counts; the palette;
                // the tray's open state, tab, the tabs loaded once and their scroll positions; the quick-create dialog; the ? overlay
                shellData: null, shellCounts: {}, paletteOpen: false,
                trayOpen: false, trayTab: "", trayMounted: false, trayLoaded: {}, trayScroll: {},
                quickCreateShow: false, quickCreateUrl: "", quickCreateTitle: "", keysHelpShow: false,
                shell: Object.assign({ logoUrl: "", title: "", logoutUrl: "/Login/logout", footerHtml: "" }, config.shell), labels: Object.assign({}, qvt.shellLabels, config.labels)
            };
        },
        // the page's screen for inject: ["mScreen"] (moqui.urlScreen; a container provides its own): the current path and parameters
        provide() {
            const vm = this;
            return { mScreen: { get url() { const search = vm.currentSearch; return vm.currentPath + (search ? "?" + search : ""); },
                get path() { return vm.currentPath; }, get parameters() { return vm.currentParameters; }, containerId: null } };
        },
        methods: {
            setUrl(url, bodyParameters, onComplete, pushState = true) {
                // cancel the load that is going on
                if (this.currentLoadRequest) {
                    this.currentLoadRequest.abort();
                    this.currentLoadRequest = null;
                    this.loading = 0;
                }
                // always set bodyParameters, null when there are none, to clear the last ones
                this.bodyParameters = bodyParameters;
                url = this.getLinkPath(url);
                if (this.currentLinkUrl === url && url !== this.linkBasePath) {
                    // the same URL again is a reload: a cached (server-static) target screen makes a new use with fresh data too
                    this.reloadTarget = true;
                    this.reloadSubscreens();
                    if (onComplete) this.callOnComplete(onComplete, this.currentPath);
                    return;
                }
                const redirectedFrom = this.currentPath;
                const urlInfo = moqui.parseHref(url);
                // the extra path comes from the menu data when the screen has one
                this.extraPathList = [];
                // currentSearch before currentPath so that it is there when the path changes
                this.currentSearch = urlInfo.search;
                this.currentPath = urlInfo.path;
                const srch = this.currentSearch;
                const screenUrl = this.currentPath + (srch.length > 0 ? "?" + srch : "");
                if (!screenUrl || screenUrl.length === 0) return;
                this.lastNavTime = Date.now();
                this.activeContainers = {};
                // a link or a response that goes to another screen closes the quick-create dialog
                this.quickCreateShow = false;

                // the menu data changes, and that changes the screens and subscreens shown
                const menuDataUrl = this.appRootPath && this.appRootPath.length && screenUrl.indexOf(this.appRootPath) === 0 ?
                    this.appRootPath + "/menuData" + screenUrl.slice(this.appRootPath.length) : "/menuData" + screenUrl;
                moqui.request(menuDataUrl).then((result) => {
                    if (Array.isArray(result.json)) {
                        this.navMenuList = result.json;
                        if (onComplete) this.callOnComplete(onComplete, redirectedFrom);
                    }
                }, moqui.handleAjaxError);

                if (pushState) root.history.pushState(null, this.ScreenTitle, url);
                this.urlListeners.forEach((callback) => callback(url, this));
                root.document.documentElement.scrollTop = 0;
                root.document.body.scrollTop = 0;
            },
            callOnComplete(onComplete, redirectedFrom) {
                if (!onComplete) return;
                const route = this.getRoute();
                if (redirectedFrom) route.redirectedFrom = redirectedFrom;
                onComplete(route);
            },
            getRoute() {
                return { name: this.currentPathList[this.currentPathList.length - 1], meta: {}, path: this.currentPath, hash: "", query: this.currentParameters,
                    params: this.bodyParameters || {}, fullPath: this.currentLinkUrl, matched: [{ path: this.currentPath }] };
            },
            setParameters(parmObj) {
                if (parmObj) {
                    this.currentParameters = Object.assign({}, this.currentParameters, parmObj);
                    // no change of path, so the parameters of the latest history item change
                    const curUrl = this.currentLinkUrl;
                    const curHistoryItem = this.navHistoryList[0];
                    if (curHistoryItem) curHistoryItem.pathWithParams = curUrl;
                    root.history.pushState(null, (curHistoryItem && curHistoryItem.title) || "", curUrl);
                }
                this.reloadSubscreens();
            },
            addSubscreen(saComp) {
                saComp.pathIndex = this.activeSubscreens.length;
                // this also makes the first load of an m-subscreens-active
                saComp.loadActive();
                this.activeSubscreens.push(saComp);
            },
            reloadSubscreens() {
                const fullPathList = this.currentPathList;
                const activeSubscreens = this.activeSubscreens;
                if (fullPathList.length === 0 && activeSubscreens.length > 0) {
                    activeSubscreens.splice(1);
                    activeSubscreens[0].loadActive();
                    return;
                }
                for (let i = 0; i < activeSubscreens.length; i++) {
                    if (i >= fullPathList.length) break;
                    // the first one that loads clears the rest, whose placeholders will register and load
                    if (activeSubscreens[i].loadActive()) { activeSubscreens.splice(i + 1); break; }
                }
            },
            goPreviousScreen() {
                const currentPath = this.currentPath;
                const prev = this.navHistoryList.find((item) => item.pathWithParams.indexOf(currentPath) < 0);
                if (prev && prev.pathWithParams && prev.pathWithParams.length) this.setUrl(prev.pathWithParams);
            },
            // a container component has reload(), load(url) and hide()
            addContainer(contId, comp) { this.activeContainers[contId] = comp; },
            // an unmounted container takes its registration back, unless another has the id since
            removeContainer(contId, comp) { if (this.activeContainers[contId] === comp) delete this.activeContainers[contId]; },
            reloadContainer(contId) { const comp = this.activeContainers[contId]; if (comp) comp.reload(); else console.error("Container with ID " + contId + " not found, not reloading"); },
            loadContainer(contId, url) { const comp = this.activeContainers[contId]; if (comp) comp.load(url); else console.error("Container with ID " + contId + " not found, not loading url " + url); },
            hideContainer(contId) { const comp = this.activeContainers[contId]; if (comp && comp.hide) comp.hide(); else console.error("Container with ID " + contId + " not found, not hiding"); },

            // The context of a screen, from its m-screen-context: a new key is the latest registration, a known one keeps its place.
            setScreenContext(key, value) {
                const entry = { key, value: Object.assign({}, value) };
                const index = this.screenContexts.findIndex((item) => item.key === key);
                if (index >= 0) this.screenContexts.splice(index, 1, entry); else this.screenContexts.push(entry);
            },
            removeScreenContext(key) {
                const index = this.screenContexts.findIndex((item) => item.key === key);
                if (index >= 0) this.screenContexts.splice(index, 1);
            },

            // ---- the suite shell (js/README.md, "Shell data and providers") ----

            // The shell data of the page: the apps, their providers, the theme's shell values and the first counts. Fetched
            // once when the page starts and again after a re-login; the keys of the providers are registered from it.
            loadShellData() {
                return moqui.request(this.appRootPath + "/shellData").then((result) => {
                    const data = result.json && moqui.isPlainObject(result.json) ? result.json : null;
                    if (!data) return;
                    this.shellData = { apps: Array.isArray(data.apps) ? data.apps : [], shell: moqui.isPlainObject(data.shell) ? data.shell : {}, counts: data.counts };
                    this.shellCounts = moqui.isPlainObject(data.counts) ? data.counts : {};
                    this.registerProviderKeys();
                    this.restoreTray();
                }, moqui.handleAjaxError);
            },
            // the counts alone, for the poll while the notification socket is down; nothing without a counts service
            loadShellCounts() {
                if (!this.shellData || this.shellData.counts === null || this.shellData.counts === undefined) return null;
                return moqui.request(this.appRootPath + "/shellCounts").then((result) => {
                    const counts = result.json && moqui.isPlainObject(result.json.counts) ? result.json.counts : null;
                    if (counts) this.shellCounts = counts;
                }, (err) => console.info("shell counts not loaded", err && err.status));
            },
            pollShellCounts() {
                const socket = this.notificationClient && this.notificationClient.webSocket;
                if (socket && root.WebSocket && socket.readyState === root.WebSocket.OPEN) return;
                this.loadShellCounts();
                if (this.trayOpen) this.reloadTrayTab(this.trayTab);
            },
            // A message of the notification socket: shellCounts sets the badges and reloads the open tab; todayItems, a chatRoom:
            // topic, notifications and the apps' notification-topic providers reload their tab. A chat signal (typing, read) does
            // not, nor a chat event while a screen listens to the chat store (moqui.chat.trayReloads).
            onShellNotification(jsonObj) {
                const topic = jsonObj && jsonObj.topic ? String(jsonObj.topic) : "";
                if (!topic) return;
                if (topic === "shellCounts") {
                    let counts = jsonObj.message;
                    if (moqui.isString(counts)) { try { counts = JSON.parse(counts); } catch (err) { counts = null; } }
                    if (moqui.isPlainObject(counts)) this.shellCounts = counts;
                    if (this.trayOpen) this.reloadTrayTab(this.trayTab);
                } else if (topic === "todayItems") this.reloadTrayTab("today");
                else if (topic.indexOf("chatRoom:") === 0) { if (!moqui.chat || moqui.chat.trayReloads(jsonObj)) this.reloadTrayTab("chat"); }
                else if (topic === "notifications" || this.shellNotificationTopics.indexOf(topic) >= 0) this.reloadTrayTab("notifications");
            },
            // a count of shellCounts: a number, or a map's total or count
            countOf(key) {
                const value = this.shellCounts ? this.shellCounts[key] : null;
                if (moqui.isPlainObject(value)) return Number(value.total !== undefined ? value.total : value.count) || 0;
                return Number(value) || 0;
            },
            countText(key) { const count = this.countOf(key); return count > 99 ? "99+" : String(count); },
            // a header button's badge: the sum of its counts
            counterCount(counter) { return (counter.counts || [counter.key]).reduce((sum, key) => sum + this.countOf(key), 0); },
            counterText(counter) { const count = this.counterCount(counter); return count > 99 ? "99+" : String(count); },
            // a /qapps link of the shell data as a path to load a screen from (/apps)
            shellLoadPath(url) {
                const text = String(url || "");
                const link = this.linkBasePath;
                if (link && (text === link || text.indexOf(link + "/") === 0 || text.indexOf(link + "?") === 0)) return this.basePath + text.slice(link.length);
                return text;
            },

            openPalette() { this.paletteOpen = true; },
            openTray(key) {
                if (!this.trayTabs.some((tab) => tab.key === key)) return;
                this.trayTab = key;
                this.trayLoaded[key] = true;
                this.trayMounted = true;
                this.trayOpen = true;
                this.saveTray();
            },
            closeTray() { this.trayOpen = false; this.saveTray(); },
            // key null: the one tray button of a phone header, which closes the tray, or opens it on its last tab (else the first)
            toggleTray(key) {
                if (key === null || key === undefined) {
                    if (this.trayOpen) this.closeTray();
                    else this.openTray(this.trayTabs.some((tab) => tab.key === this.trayTab) ? this.trayTab : (this.trayTabs[0] || {}).key);
                    return;
                }
                if (this.trayOpen && this.trayTab === key) this.closeTray(); else this.openTray(key);
            },
            reloadTrayTab(key) { if (key && this.trayLoaded[key] && this.shellTray) this.shellTray.reload(key); },
            keepTrayScroll(key, top) {
                this.trayScroll[key] = top;
                clearTimeout(this.traySaveTimer);
                this.traySaveTimer = setTimeout(() => this.saveTray(), 250);
            },
            // the tray's state for a reload of the page, in sessionStorage (each tab of the browser its own); none when storage is off
            saveTray() {
                try { root.sessionStorage.setItem("moqui.shell.tray", JSON.stringify({ open: this.trayOpen, tab: this.trayTab, scroll: this.trayScroll })); } catch (err) { /* no storage */ }
            },
            restoreTray() {
                let saved = null;
                try { saved = JSON.parse(root.sessionStorage.getItem("moqui.shell.tray") || "null"); } catch (err) { saved = null; }
                if (!moqui.isPlainObject(saved) || !saved.tab || !this.trayTabs.some((tab) => tab.key === saved.tab)) return;
                if (moqui.isPlainObject(saved.scroll)) this.trayScroll = Object.assign({}, saved.scroll);
                if (saved.open) this.openTray(saved.tab);
                else this.trayTab = saved.tab;
            },

            // A + menu kind: its screen in the quick-create dialog, with the record of the screen context as parameters.
            openQuickCreate(item) {
                if (!item || !item.url) return;
                const record = this.screenRecord;
                const params = {};
                if (record) {
                    params.recordEntity = record.entity;
                    params.recordId = record.id;
                    if (record.title) params.recordTitle = record.title;
                }
                const search = moqui.objToSearch(params);
                const url = this.shellLoadPath(item.url);
                this.quickCreateTitle = item.label || "";
                this.quickCreateUrl = url + (search ? (url.indexOf("?") > 0 ? "&" : "?") + search : "");
                this.quickCreateShow = true;
            },

            // The shell's own keys: the palette (not inside a rich-text editor, whose Ctrl+K is its own), Esc, ? and c.
            shellKeyBindings() {
                const labels = this.labels;
                return [
                    { keys: "Mod+K", action: "palette", label: labels.search, notInEditor: true, run: () => { this.openPalette(); } },
                    { keys: "Esc", action: "close", label: labels.closeTopmost, run: () => this.closeTopmost() },
                    { keys: "?", action: "help", label: labels.keyboardShortcuts, run: () => { this.keysHelpShow = true; } },
                    { keys: "c", action: "quickCreate", label: labels.quickCreateFirst, run: () => {
                        const item = this.currentAppQuickCreate[0];
                        if (!item) return false;
                        this.openQuickCreate(item);
                        return true;
                    } }
                ];
            },
            // Esc: a Quasar dialog or menu closes itself; else the tray closes
            closeTopmost() {
                if (this.shellPopupOpen()) return false;
                if (this.trayOpen) { this.closeTray(); return true; }
                return false;
            },
            shellPopupOpen() { return !!(root.document && root.document.querySelector(".q-dialog, .q-menu")); },
            // The keymap providers of the apps: a "global" scope is on everywhere, another while its app is the current one.
            registerProviderKeys() {
                (this.providerKeysOff || []).forEach((off) => off());
                this.providerKeysOff = [];
                this.shellApps.forEach((app) => {
                    const keymaps = app.providers && Array.isArray(app.providers.keymaps) ? app.providers.keymaps : [];
                    keymaps.forEach((map) => {
                        const scope = map.scope || "global";
                        const when = scope === "global" ? null : () => this.currentAppName === app.name || this.currentAppName === scope;
                        const bindings = (map.keys || []).map((key) => ({
                            keys: key.keys, label: key.label, action: key.action || null, when,
                            run: () => {
                                if (key.url) { this.setUrl(key.url); return true; }
                                if (key.action === "palette") { this.openPalette(); return true; }
                                if (key.action === "quickCreate") { const item = this.currentAppQuickCreate[0]; if (item) { this.openQuickCreate(item); return true; } }
                                return false;
                            }
                        }));
                        this.providerKeysOff.push(keymap.registerScope(scope, bindings));
                    });
                });
            },
            onShellKeydown(event) { keymap.handle(event); },

            addPlugin(list, url) { moqui.loadComponent(this.appRootPath + url, (comp) => { list.push(comp); }); },
            addPluginsWait(list, urlList, urlIndex) {
                if (!urlList || urlList.length <= urlIndex) return;
                this.addPlugin(list, urlList[urlIndex]);
                if (urlList.length > urlIndex + 1) setTimeout(() => this.addPluginsWait(list, urlList, urlIndex + 1), 500);
            },
            addUrlListener(listener) { if (this.urlListeners.indexOf(listener) < 0) this.urlListeners.push(listener); },

            addNotify(message, type, link, icon) {
                const now = new Date();
                const time = String(now.getHours()).padStart(2, "0") + ":" + String(now.getMinutes()).padStart(2, "0");
                this.notifyHistoryList = [{ message, type, time, link, icon }].concat(this.notifyHistoryList).slice(0, 25);
            },
            setPreference(key, value) {
                moqui.request(this.appRootPath + "/apps/setPreference", { method: "POST", params: { moquiSessionToken: this.moquiSessionToken, preferenceKey: key, preferenceValue: value } })
                    .catch(moqui.handleAjaxError);
            },
            switchDarkLight() {
                this.$q.dark.toggle();
                this.setPreference("QUASAR_DARK", this.$q.dark.isActive ? "true" : "false");
            },
            toggleLeftOpen() {
                this.leftOpen = !this.leftOpen;
                this.setPreference("QUASAR_LEFT_OPEN", this.leftOpen ? "true" : "false");
            },
            getNavHref(navIndex) {
                if (navIndex === undefined || navIndex === null || navIndex < 0) navIndex = this.navMenuList.length - 1;
                const navMenu = this.navMenuList[navIndex];
                if (navMenu.extraPathList && navMenu.extraPathList.length) {
                    let href = navMenu.path + "/" + navMenu.extraPathList.join("/");
                    const questionIdx = navMenu.pathWithParams.indexOf("?");
                    if (questionIdx > 0) href += navMenu.pathWithParams.slice(questionIdx);
                    return href;
                }
                return navMenu.pathWithParams || navMenu.path;
            },
            // A path from the screens ("/apps/...") as the address of the page ("/qapps/...").
            getLinkPath(path) {
                if (moqui.isPlainObject(path)) path = moqui.makeHref(path);
                if (this.appRootPath && this.appRootPath.length && path.indexOf(this.appRootPath) !== 0) path = this.appRootPath + path;
                const pathList = path.split("/");
                // element 0 is the empty text before the leading slash
                const wrapperIdx = this.appRootPath.split("/").length;
                pathList[wrapperIdx] = this.linkBasePath.split("/").slice(-1)[0];
                return pathList.join("/");
            },
            getQuasarColor(bootstrapColor) { return moqui.getQuasarColor(bootstrapColor); },
            notifyIcon(type) {
                const color = moqui.getQuasarColor(type);
                return color === "positive" ? "check_circle" : color === "negative" ? "error" : color === "warning" ? "warning" : "info";
            },
            // the session token changes with a new session, after login
            getCsrfToken(response) {
                const token = response && response.headers && response.headers.get ? response.headers.get("X-CSRF-Token") : null;
                if (token && token.length && token !== this.moquiSessionToken) this.setSessionToken(token, true);
            },
            setSessionToken(token, share) {
                this.moquiSessionToken = token;
                moqui.sessionToken = token;
                if (share && this.sessionTokenBc) this.sessionTokenBc.postMessage(token);
            },
            receiveBcCsrfToken(event) {
                const token = event.data;
                if (token && token.length && this.moquiSessionToken !== token) this.setSessionToken(token, false);
            },
            reLoginCheckShow() { this.reLoginShowDialog(); },
            reLoginShowDialog() {
                // no MFA data, that would skip the login with the password
                this.reLoginMfaData = null;
                this.reLoginOtp = null;
                this.reLoginShow = true;
            },
            reLoginPostLogin() {
                this.reLoginShow = false;
                this.reLoginPassword = null;
                this.reLoginOtp = null;
                this.reLoginMfaData = null;
                const msg = "Background login successful";
                this.$q.notify({ timeout: 12000, type: "positive", message: msg });
                this.addNotify(msg, "positive");
                // the new session may be another user's view of the apps
                this.loadShellData();
            },
            reLoginSubmit() {
                moqui.request(this.appRootPath + "/rest/login", { method: "POST", params: { username: this.username, password: this.reLoginPassword } })
                    .then(this.reLoginHandleResponse, moqui.handleAjaxError);
            },
            reLoginHandleResponse(result) {
                this.getCsrfToken(result.response);
                const resp = result.json || {};
                if (resp.secondFactorRequired) this.reLoginMfaData = resp;
                else if (resp.loggedIn) this.reLoginPostLogin();
            },
            reLoginReload() { if (root.confirm("Reload page? All changes will be lost.")) root.location.href = this.currentLinkUrl; },
            reLoginSendOtp(factorId) {
                moqui.request(this.appRootPath + "/rest/sendOtp", { method: "POST", params: { moquiSessionToken: this.moquiSessionToken, factorId } })
                    .then((result) => { if (result.json) moqui.notifyMessages(result.json.messages, result.json.errors, result.json.validationErrors); }, moqui.handleAjaxError);
            },
            reLoginVerifyOtp() {
                moqui.request(this.appRootPath + "/rest/verifyOtp", { method: "POST", params: { moquiSessionToken: this.moquiSessionToken, code: this.reLoginOtp } })
                    .then((result) => { this.getCsrfToken(result.response); if (result.json && result.json.loggedIn) this.reLoginPostLogin(); }, moqui.handleAjaxError);
            },
            qLayoutMinHeight(offset) { return { minHeight: offset ? "calc(100vh - " + offset + "px)" : "100vh" }; }
        },
        watch: {
            // the bar at the top of the page while anything loads
            loading(count, before) {
                if (!this.$q || !this.$q.loadingBar) return;
                if (count > 0 && !(before > 0)) this.$q.loadingBar.start();
                else if (count <= 0 && before > 0) this.$q.loadingBar.stop();
            },
            navMenuList(newList) {
                if (newList.length === 0) return;
                const cur = newList[newList.length - 1];
                const par = newList.length > 1 ? newList[newList.length - 2] : null;
                if (cur.extraPathList) this.extraPathList = cur.extraPathList;
                // the full path, for a minimal path URL: the menu and link paths are in the screen tree, so the servlet context path is not in them
                this.currentPathList = cur.path.split("/").slice(this.basePathSize + 1);
                this.reloadSubscreens();

                // history and the title of the document
                let newTitle = (par ? par.title + " - " : "") + cur.title;
                let curUrl = cur.pathWithParams;
                const questIdx = curUrl.indexOf("?");
                if (questIdx > 0) {
                    const excludeKeys = ["pageIndex", "orderBySelect", "orderByField", "moquiSessionToken"];
                    const parmList = curUrl.substring(questIdx + 1).split("&");
                    curUrl = curUrl.substring(0, questIdx);
                    let dpCount = 0;
                    let titleParms = "";
                    parmList.forEach((parm) => {
                        curUrl += curUrl.indexOf("?") === -1 ? "?" : "&";
                        curUrl += parm;
                        if (dpCount > 3) return;
                        const eqIdx = parm.indexOf("=");
                        if (eqIdx <= 0) return;
                        const key = parm.substring(0, eqIdx);
                        const value = parm.substring(eqIdx + 1);
                        if (key.indexOf("_op") > 0 || key.indexOf("_not") > 0 || key.indexOf("_ic") > 0 || excludeKeys.indexOf(key) >= 0 || key === value) return;
                        if (titleParms.length > 0) titleParms += ", ";
                        try { titleParms += decodeURIComponent(value); } catch (err) { titleParms += value; }
                        dpCount++;
                    });
                    if (titleParms.length > 0) {
                        if (titleParms.length > 70) titleParms = titleParms.substring(0, 70) + "...";
                        newTitle += " (" + titleParms + ")";
                    }
                }
                this.navHistoryList = [{ title: newTitle, pathWithParams: curUrl, image: cur.image, imageType: cur.imageType }]
                    .concat(this.navHistoryList.filter((item) => item.pathWithParams !== curUrl)).slice(0, 25);
                root.document.title = newTitle;
            },
            currentPathList(newList) {
                // the class of the last screen on the container of the application, for styles that depend on the screen
                const lastPath = newList[newList.length - 1];
                const container = root.document.getElementById("apps-root");
                if (lastPath && container) container.className = lastPath;
            }
        },
        computed: {
            currentPath: {
                get() {
                    const curPath = this.currentPathList;
                    const extraPath = this.extraPathList;
                    return this.basePath + (curPath && curPath.length > 0 ? "/" + curPath.join("/") : "") + (extraPath && extraPath.length > 0 ? "/" + extraPath.join("/") : "");
                },
                set(newPath) {
                    if (!newPath || newPath.length === 0) { this.currentPathList = []; return; }
                    if (newPath.slice(newPath.length - 1) === "/") newPath = newPath.slice(0, newPath.length - 1);
                    if (newPath.indexOf(this.linkBasePath) === 0) newPath = newPath.slice(this.linkBasePath.length + 1);
                    else if (newPath.indexOf(this.basePath) === 0) newPath = newPath.slice(this.basePath.length + 1);
                    this.currentPathList = newPath.split("/");
                }
            },
            currentLinkPath() {
                const curPath = this.currentPathList;
                const extraPath = this.extraPathList;
                return this.linkBasePath + (curPath && curPath.length > 0 ? "/" + curPath.join("/") : "") + (extraPath && extraPath.length > 0 ? "/" + extraPath.join("/") : "");
            },
            currentSearch: {
                get() { return moqui.objToSearch(this.currentParameters); },
                set(newSearch) { this.currentParameters = moqui.searchToObj(newSearch); }
            },
            currentLinkUrl() { const search = this.currentSearch; return this.currentLinkPath + (search.length > 0 ? "?" + search : ""); },
            basePathSize() { return this.basePath.split("/").length - this.appRootPath.split("/").length; },
            // the first menu entry the breadcrumbs show: past the apps root, and past the current app's root, whose title is in the
            // header (outside an app, as on the AppList, the trail starts below the apps root)
            breadcrumbStart() { return this.basePathSize + (this.currentApp ? 1 : 0); },
            // the app root's menu entry, the one breadcrumbStart skips: its subscreens are the app's top-level screens, and the
            // header title opens that menu. Null outside an app, while the menu data is still loading, and when the app root is
            // its own tab, wizard or stack menu (hasTabMenu) or has no subscreens.
            appScreenMenu() {
                if (!this.currentApp) return null;
                const item = this.navMenuList[this.basePathSize];
                if (!item || item.hasTabMenu || !(item.subscreens && item.subscreens.length)) return null;
                return item;
            },
            ScreenTitle() { return this.navMenuList.length > 0 ? this.navMenuList[this.navMenuList.length - 1].title : ""; },
            // The record of the latest m-screen-context (a record needs an entity and an id), apart from the rest of the screen
            // context so the + menu does not follow every field of every form.
            screenRecord() {
                const list = this.screenContexts;
                const latest = list.length ? list[list.length - 1].value : null;
                return latest && latest.recordEntity && latest.recordId ? { entity: latest.recordEntity, id: latest.recordId, title: latest.title || "" } : null;
            },
            // What the shell and Assist know of the screen shown: its path and parameters, the record of the latest m-screen-context,
            // the selected rows of its selection-form, the active find parameters of the lists, and the forms with their field
            // names (moqui.screenFormsContext).
            screenContext() {
                const list = this.screenContexts;
                const latest = list.length ? list[list.length - 1].value : null;
                const selectionForm = latest && latest.selectionForm ? latest.selectionForm : "";
                // formsVersion: a form came or went
                const gathered = this.formsVersion >= 0 ? moqui.screenFormsContext(moqui.screenForms, moqui.screenFormsDom(), this.currentParameters, selectionForm) :
                    { forms: [], filters: null, selection: null };
                return { path: this.currentPath, parameters: Object.assign({}, this.currentParameters), record: this.screenRecord,
                    selection: gathered.selection, filters: gathered.filters, forms: gathered.forms };
            },
            // The ids of the elements Assist may reach (IronStone design §1.1): the canvas, the tray panel while it is there and the
            // quick-create dialog while it is open; not the header, the drawer or the palette.
            assistReach() {
                return ["m-canvas"].concat(this.trayMounted && this.trayOpen ? ["m-tray"] : [], this.quickCreateShow ? ["m-quick-create"] : []);
            },

            // ---- the suite shell ----
            shellApps() { return this.shellData ? this.shellData.apps : []; },
            shellValues() { return this.shellData && this.shellData.shell ? this.shellData.shell : {}; },
            currentAppName() { return this.currentPathList.length ? this.currentPathList[0] : ""; },
            currentApp() { return this.shellApps.find((app) => app.name === this.currentAppName) || null; },
            // the current app's title in the header; the theme's title outside an app (the AppList)
            headerTitle() { return this.currentApp ? this.currentApp.title : this.shell.title; },
            // the apps with the current one first, for the + menu and the account menu
            shellAppsCurrentFirst() {
                const current = this.currentApp;
                return current ? [current].concat(this.shellApps.filter((app) => app !== current)) : this.shellApps;
            },
            // the + menu: the quick-create kinds of the apps, the current app's first, once per screen; a kind that needs a record
            // only with one in the screen context
            quickCreateItems() {
                const record = this.screenRecord;
                const seen = {};
                const out = [];
                this.shellAppsCurrentFirst.forEach((app) => ((app.providers && app.providers.quickCreate) || []).forEach((item) => {
                    if (!item.url || seen[item.url] || (item.requiresRecord && !record)) return;
                    seen[item.url] = true;
                    out.push(Object.assign({ key: app.name + ":" + (item.kind || item.url), app: app.name }, item));
                }));
                return out;
            },
            currentAppQuickCreate() { return this.quickCreateItems.filter((item) => this.currentApp && item.app === this.currentApp.name); },
            accountMenuItems() {
                const seen = {};
                const out = [];
                this.shellAppsCurrentFirst.forEach((app) => ((app.providers && app.providers.accountMenuItems) || []).forEach((item) => {
                    if (!item.url || seen[item.url]) return;
                    seen[item.url] = true;
                    out.push(item);
                }));
                return out;
            },
            shellNotificationTopics() {
                const out = [];
                this.shellApps.forEach((app) => ((app.providers && app.providers.notificationTopics) || []).forEach((topic) => { if (out.indexOf(topic) < 0) out.push(topic); }));
                return out;
            },
            // the tabs of the tray: the screens the theme names (STRT_TRAY_TODAY, STRT_TRAY_CHAT, STRT_TRAY_NOTIFICATIONS)
            trayTabs() {
                const values = this.shellValues;
                const labels = this.labels;
                return [{ key: "today", icon: "sym_o_today", label: labels.today, url: values.trayToday },
                    { key: "chat", icon: "sym_o_chat", label: labels.chat, url: values.trayChat },
                    { key: "notifications", icon: "sym_o_notifications", label: labels.notifications, url: values.trayNotifications }]
                    .filter((tab) => tab.url).map((tab) => Object.assign(tab, { loadUrl: this.shellLoadPath(tab.url) }));
            },
            // the header's count buttons in their order: Today, Chat, Mail and Messages (links), Notifications, each when its theme
            // value is set; `counts` are the shellCounts keys of its badge. Below 600 px Mail and Messages are one button to the mail
            // screen with the two counts summed, and the tray tabs (Today, Chat, Notifications) one tray button (key tray, any: it
            // opens the tray on its last tab, whose tabs switch) with their counts summed, so a phone header does not overflow. The
            // updates count has no button: the Notifications tab shows it.
            headerCounters() {
                const tray = (key) => this.trayTabs.find((tab) => tab.key === key);
                const values = this.shellValues;
                const narrow = !!(this.$q && this.$q.screen && this.$q.screen.lt.sm);
                const out = [];
                const trayTabs = this.trayTabs.filter((tab) => ["today", "chat", "notifications"].indexOf(tab.key) >= 0);
                const foldTray = narrow && trayTabs.length > 1;
                if (foldTray) {
                    const icon = (tray("notifications") || trayTabs[0]).icon;
                    out.push({ key: "tray", icon, label: trayTabs.map((tab) => tab.label).join(", "), tray: true, any: true, counts: trayTabs.map((tab) => tab.key) });
                }
                if (!foldTray) ["today", "chat"].forEach((key) => { const tab = tray(key); if (tab) out.push({ key, icon: tab.icon, label: tab.label, tray: true, counts: [key] }); });
                const mail = values.mailScreen ? { key: "email", icon: "sym_o_mail", label: this.labels.mail, url: values.mailScreen, tray: false, counts: ["email"] } : null;
                const messages = values.messagesScreen ? { key: "messages", icon: "sym_o_forum", label: this.labels.messages, url: values.messagesScreen, tray: false, counts: ["messages"] } : null;
                if (mail && messages && narrow) {
                    out.push({ key: "mail", icon: "sym_o_mail", label: this.labels.mail, url: mail.url, tray: false, counts: ["email", "messages"] });
                } else {
                    if (mail) out.push(mail);
                    if (messages) out.push(messages);
                }
                const notifications = tray("notifications");
                if (notifications && !foldTray) out.push({ key: "notifications", icon: notifications.icon, label: notifications.label, tray: true, counts: ["notifications"] });
                return out;
            },
            paletteKeyHint() { return keymap.isMac ? "⌘K" : "Ctrl K"; },
            // the chat store (WebrootChat.js), for a signed in user: the status in the account menu and the user's own dot
            chatStatus() { return !!(moqui.chat && this.userId); },
            // Assist (WebrootAssist.js): for a signed in user whose bootstrap answered with an agent to use
            assistOn() { return !!(moqui.assist && this.userId && moqui.assist.state.available); },
            // an Assist canvas covers the page (the page under it stays mounted)
            assistCanvasShown() { return !!(this.assistOn && moqui.assist.canvasShown && moqui.assist.canvasShown()); },
            myPresence() { return moqui.chat && this.userId ? moqui.chat.myPresence() : ""; },
            paletteSources() {
                const out = [];
                this.shellApps.forEach((app) => ((app.providers && app.providers.paletteSources) || []).forEach((source) => {
                    if (!out.some((other) => other.group === source.group)) out.push(source);
                }));
                return out;
            },
            paletteScreens() { return palette.screens(this.shellApps); },
            paletteActions() {
                const seen = {};
                const out = [];
                this.shellAppsCurrentFirst.forEach((app) => ((app.providers && app.providers.paletteActions) || []).forEach((action) => {
                    if (!action.url || seen[action.url]) return;
                    seen[action.url] = true;
                    out.push({ title: action.label, subtitle: app.title, icon: action.icon || "sym_o_bolt", url: action.url });
                }));
                return out;
            },
            // the saved finds of the screens on the current path, from its menu data
            paletteSavedFinds() {
                const seen = {};
                const out = [];
                this.navMenuList.forEach((item) => (item.savedFinds || []).forEach((find) => {
                    if (!find.pathWithParams || seen[find.pathWithParams]) return;
                    seen[find.pathWithParams] = true;
                    out.push({ title: find.title, subtitle: item.title, icon: "sym_o_saved_search", url: find.pathWithParams });
                }));
                return out;
            },
            // the screens of this page before the current one, newest first (the palette's Recent)
            paletteRecent() {
                return this.navHistoryList.slice(1).map((item) => ({ title: item.title, url: item.pathWithParams, image: item.image, imageType: item.imageType }));
            },
            documentMenuList() {
                const docList = [];
                this.navMenuList.forEach((item) => { if (item.screenDocList && item.screenDocList.length) item.screenDocList.forEach((doc) => docList.push(doc)); });
                return docList;
            }
        },
        created() {
            moqui.webrootVue = this;
            moqui.sessionToken = this.moquiSessionToken;
            moqui.appRootPath = this.appRootPath;
            moqui.onSessionToken = (token) => { if (token !== this.moquiSessionToken) this.setSessionToken(token, true); };
            this.$q.dark.set(!!config.darkMode);
            moqui.applyDensity(config.density);
            // Open the socket only for a signed in user, and when the server names an endpoint (/notws by default).
            if (config.notificationEndpoint && config.userId) {
                const wsScheme = root.location.protocol === "https:" ? "wss://" : "ws://";
                this.notificationClient = new moqui.NotificationClient(wsScheme + this.appHost + this.appRootPath + config.notificationEndpoint);
            }
            // the session token is shared between the tabs and windows of the site
            if (root.BroadcastChannel) {
                this.sessionTokenBc = new root.BroadcastChannel("SessionToken");
                this.sessionTokenBc.onmessage = this.receiveBcCsrfToken;
            }
            this.addPluginsWait(this.navPlugins, config.navPlugins, 0);
            this.addPluginsWait(this.accountPlugins, config.accountPlugins, 0);
            // the keys: Mod is Cmd on a Mac; the user's KEYMAP preference changes bindings; single keys wait while a dialog is open
            const nav = root.navigator || {};
            keymap.isMac = /Mac|iPhone|iPad|iPod/.test(nav.platform || nav.userAgent || "");
            const keymapPref = config.preferences ? config.preferences.KEYMAP : null;
            if (!keymap.setOverrides(keymapPref)) console.info("The KEYMAP preference is not JSON of {scope: {action: keys}}, ignored");
            keymap.typedBlocked = () => !!(root.document && root.document.querySelector(".q-dialog"));
        },
        mounted() {
            // the shell is hidden until Vue has compiled it
            root.document.getElementById("apps-root").style.display = "";
            this.setUrl(moqui.startPath(root.location.pathname, this.linkBasePath, config.startApp) + root.location.search);
            if (this.notificationClient) {
                this.notificationClient.registerListener("ALL");
                this.notificationClient.registerListener("ALL", this.onShellNotification);
            }
            // the chat store: presence, status, heartbeat, the chat pushes (WebrootChat.js)
            if (moqui.chat) moqui.chat.start(this);
            // Assist: its bootstrap, the keys, follow and the run signals (WebrootAssist.js)
            if (moqui.assist) moqui.assist.start(this);
            // the suite shell: its data once, its keys, and the counts every 5 minutes while the socket is down
            this.loadShellData();
            this.shellKeysOff = keymap.registerScope("global", this.shellKeyBindings());
            root.document.addEventListener("keydown", this.onShellKeydown);
            this.shellPollTimer = setInterval(() => this.pollShellCounts(), 5 * 60 * 1000);
            // desktop notifications: ask once, if the browser has not been told yet
            if (root.Notification && root.Notification.permission === "default") {
                root.Notification.requestPermission().then((status) => {
                    if (status === "granted") moqui.notifyMessages("Browser notifications enabled, if you don't want them use browser notification settings to block");
                    else if (status === "denied") moqui.notifyMessages("Browser notifications disabled, if you want them use browser notification settings to allow");
                });
            }
            if (config.qz && config.qz.enabled) this.loadQz();
        },
        unmounted() {
            if (this.sessionTokenBc) this.sessionTokenBc.close();
            if (this.shellKeysOff) this.shellKeysOff();
            (this.providerKeysOff || []).forEach((off) => off());
            root.document.removeEventListener("keydown", this.onShellKeydown);
            clearInterval(this.shellPollTimer);
        }
    });

    // QZ Tray printing, when the user turned it on: qz-tray.js and the print component are loaded now and not before.
    qvt.rootMethods = {
        loadQz() {
            Promise.all([moqui.loadScript("/libs/qz-tray/qz-tray.js", null, () => !!root.qz), moqui.loadScript("/js/MoquiQzComponent.js")]).then(() => {
                if (moqui.qzComponent) this.qzVue = markRaw(moqui.qzComponent);
            }, (err) => console.error("QZ Tray scripts did not load: " + err));
        }
    };

    qvt.rootOptions = rootOptions;

    // Quasar link components call a vue-router. This is the part of one that they use, over setUrl.
    qvt.makeRouter = () => ({
        resolve(to) {
            const location = moqui.isString(to) ? moqui.parseHref(to) : Object.assign({ path: "/", query: {} }, to);
            let path = location.path;
            if (moqui.webrootVue) location.path = path = moqui.webrootVue.getLinkPath(path);
            const lslIdx = path.lastIndexOf("/");
            const name = lslIdx === -1 ? path : path.slice(lslIdx + 1);
            const route = { name, meta: {}, path, hash: location.hash || "", query: location.query || {}, params: {}, fullPath: path, matched: [{ path }] };
            return { location, route, href: moqui.makeHref(location), normalizedTo: location, resolved: route, matched: route.matched, params: {}, path, fullPath: path, name };
        },
        replace(location) { moqui.webrootVue.setUrl(location, null); return Promise.resolve(location); },
        push(location) { moqui.webrootVue.setUrl(location, null); return Promise.resolve(location); }
    });

    // The Quasar plugins of the qapps shell and of the public shell: Notify (moqui.notifyMessages, $q.notify), LoadingBar (the root's
    // loading count) and Dialog, so a screen can confirm with this.$q.dialog({title, message, cancel: true}).onOk(...).
    qvt.quasarPlugins = (quasar) => {
        const plugins = {};
        ["Notify", "LoadingBar", "Dialog"].forEach((name) => { if (quasar && quasar[name]) plugins[name] = quasar[name]; });
        return plugins;
    };

    // Start the application: the language pack, the formatter, the components, and the mount on the shell.
    // The render functions of the components, precompiled from their templates into WebrootVue.qvt.render.js (moqui.qvtRenders), which loads
    // first. One is used only while its hash matches the template here, so a template edited without making the file again is compiled in the
    // browser as before; the build without the compiler needs them all. The hash is FNV-1a over the UTF-16 code units, as the generator reads it.
    qvt.templateHash = (text) => {
        let hash = 0x811c9dc5;
        for (let i = 0; i < text.length; i++) {
            hash ^= text.charCodeAt(i);
            hash = Math.imul(hash, 0x01000193) >>> 0;
        }
        return hash.toString(16);
    };
    qvt.applyRenders = () => {
        const renders = moqui.qvtRenders || {};
        Object.keys(renders).forEach((name) => {
            const comp = qvt.components[name];
            const made = renders[name];
            if (!comp || typeof comp.template !== "string" || made.hash !== qvt.templateHash(comp.template)) return;
            comp.render = made.make(root.Vue);
            delete comp.template;
        });
    };
    // A component of a script that loads after this one (a client part loaded on first use, such as the Assist canvas): its
    // precompiled render function is applied as applyRenders does (moqui.qvtRenders, while the hash matches), so a page of the
    // build without the compiler (webapp_qvt_static_only) runs it too, and it is registered on the running application, or kept
    // for start when the application is not running yet. Returns the definition.
    qvt.registerLate = (name, def) => {
        component(name, def);
        const made = (moqui.qvtRenders || {})[name];
        if (made && root.Vue && typeof def.template === "string" && made.hash === qvt.templateHash(def.template)) {
            def.render = made.make(root.Vue);
            delete def.template;
        }
        if (qvt.app) qvt.app.component(name, def);
        return def;
    };

    qvt.start = () => {
        qvt.applyRenders();
        const doc = root.document;
        const configEl = doc.getElementById("moqui-config");
        const config = JSON.parse(configEl.textContent);
        qvt.config = config;
        moqui.config = config;
        moqui.setNumberSeparators(config.locale);
        if (moqui.l10n) {
            moqui.l10n.setLocale(config.locale);
            moqui.l10n.setTimeZone(config.timeZone);
            if (config.currencies) moqui.l10n.setCurrencies(config.currencies);
            const catalogUrl = (config.appRootPath || "") + "/apps/l10nCatalog?locale=" + encodeURIComponent((config.locale || "en").replace("-", "_"));
            moqui.l10n.loadCatalog(config.locale, () => moqui.request(catalogUrl).then((result) => result.json)).catch((err) => console.warn("no locale catalog", err));
        }
        // A confirmation is a data-confirm attribute on the button or link. It is asked before any other handler runs.
        doc.addEventListener("click", (event) => {
            const el = event.target.closest && event.target.closest("[data-confirm]");
            if (el && !root.confirm(el.getAttribute("data-confirm"))) {
                event.preventDefault();
                event.stopImmediatePropagation();
            }
        }, true);
        const options = rootOptions(config);
        Object.assign(options.methods, qvt.rootMethods);
        // the shell's render function when it is precompiled, else its template for the compiler build
        const shell = qvt.components["m-qapps-root"];
        if (shell.render) options.render = shell.render;
        else options.template = shell.template;
        const app = root.Vue.createApp(options);
        const quasarOptions = { config: Object.assign({ brand: { info: "#1e7b8e" }, notify: { progress: true, closeBtn: "X", position: "top-right" }, loadingBar: { color: "primary", size: "3px", position: "top" } }, config.quasar),
            plugins: qvt.quasarPlugins(root.Quasar) };
        const langKey = String(config.quasarLang || "enUS").replace(/-([a-zA-Z])/g, (m, ch) => ch.toUpperCase()).replace("-", "");
        if (root.Quasar.Lang && root.Quasar.Lang[langKey]) quasarOptions.lang = root.Quasar.Lang[langKey];
        // Quasar's own icons (select arrows, pagination, expansion) as Material Symbols, outlined (the sym_o_* names; qapps.xml loads the set)
        if (root.Quasar.IconSet && root.Quasar.IconSet.materialSymbolsOutlined) quasarOptions.iconSet = root.Quasar.IconSet.materialSymbolsOutlined;
        app.use(root.Quasar, quasarOptions);
        Object.keys(qvt.components).forEach((name) => app.component(name, qvt.components[name]));
        qvt.app = app;
        app.config.globalProperties.moqui = moqui;
        // Field rules call $mq. The browser compiler reads this, not the page global.
        app.config.globalProperties.$mq = moqui;
        app.config.globalProperties.window = root;
        app.config.globalProperties.$router = qvt.makeRouter();
        Object.defineProperty(app.config.globalProperties, "$route", { get: () => (moqui.webrootVue ? moqui.webrootVue.getRoute() : { matched: [], params: {} }) });
        app.config.errorHandler = (err, instance, info) => { console.error("Vue error (" + info + ")", err); };
        moqui.webrootVue = app.mount("#apps-root");
        root.addEventListener("popstate", () => { moqui.webrootVue.setUrl(root.location.pathname + root.location.search, null, null, false); });
        return moqui.webrootVue;
    };

    // The application starts when the shell is there and the libraries are. A test or another page can load this file alone.
    if (root.document && root.document.getElementById && root.document.getElementById("moqui-config") && root.Vue && root.Quasar) qvt.start();
})(globalThis);
