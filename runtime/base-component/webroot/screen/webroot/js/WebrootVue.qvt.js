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

    moqui.urlExtensions = { js: "qjs", vue: "qvue", qvt: "qvt" };

    moqui.getQuasarColor = (bootstrapColor) => {
        // Quasar colors: primary, secondary, accent, dark, positive, negative, info, warning
        if (bootstrapColor === "success") return "positive";
        if (bootstrapColor === "danger") return "negative";
        return bootstrapColor;
    };

    const rootVm = () => moqui.webrootVue;
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
                    notify(Object.assign({}, moqui.notifyOptsInfo, { type, message: item.message }));
                    addNotify(item.message, type);
                    notified = true;
                } else show(moqui.notifyOptsInfo, item, "info");
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
        if (jsonObj.link) options.actions = [{ label: "View", color: "white", handler: () => rootVm().setUrl(jsonObj.link) }];
        notify(options);
        addNotify(jsonObj.title, jsonObj.type, jsonObj.link, jsonObj.icon);
    };

    /* ========== component loading ========== */

    moqui.componentCache = new moqui.LruMap(50);

    moqui.NotFound = markRaw({ template: '<div id="current-page-root"><h4>Screen not found at {{$root.currentPath}}</h4></div>' });
    moqui.EmptyComponent = markRaw({ template: '<div id="current-page-root"><div class="spinner"><div>&nbsp;</div></div></div>' });

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

    const addStyle = (css, scoped) => {
        if (root.document.head.querySelector("style[data-mq-style=\"" + css.length + css.slice(0, 40) + "\"]")) return;
        const style = root.document.createElement("style");
        style.setAttribute("data-mq-style", css.length + css.slice(0, 40));
        style.textContent = scoped ? css : css;
        root.document.head.appendChild(style);
    };

    // path: the screen path, extraPath, search and bodyParameters as the router has them. renderModes lists the render
    // modes the screen says it supports: qjs or qvue makes the request for that companion instead of qvt.
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
        if (url.slice(-vueExt.length) === vueExt) kind = vueExt;
        else if (url.slice(-jsExt.length) === jsExt) kind = jsExt;
        else if (modes.indexOf(vueExt) >= 0) { url += "." + vueExt; kind = vueExt; }
        else if (modes.indexOf(jsExt) >= 0) { url += "." + jsExt; kind = jsExt; }
        else url += "." + qvtExt;
        if (extraPath && extraPath.length > 0) url += "/" + extraPath;
        if (search && search.length > 0) url += "?" + search;

        const controller = new root.AbortController();
        const hasBody = bodyParameters && !moqui.isEmptyObject(bodyParameters);
        const options = { signal: controller.signal, accept: "text/html,application/javascript,*/*" };
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
                    const qvtUrl = (path.slice(-jsExt.length) === jsExt ? path.slice(0, -jsExt.length) : path) + "." + qvtExt;
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

    // The helpers a template calls as $mq: the few things an expression needs that are not in the language.
    const NUMBER_TEXT = /^[-+]?\d+(\.\d+)?$/;
    qvt.helpers = {
        str: (value) => (value === null || value === undefined ? "" : String(value)),
        dflt: (value, fallback) => (value === null || value === undefined || value === "" ? fallback : value),
        nul: (value) => (value === undefined || value === "" ? null : value),
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
        merge: (...maps) => {
            const out = {};
            maps.forEach((map) => { if (map && typeof map === "object") Object.keys(map).forEach((key) => { out[key] = map[key]; }); });
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
        if (!kind && pattern && typeof value === "string") {
            if (NUMBER_TEXT.test(value.trim()) && /[#0]/.test(pattern) && !/[yMdHhms]/.test(pattern.replace(/'[^']*'/g, ""))) kind = "number";
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
            rowInitial: Function, formLocation: String, findAction: String, defaultColumns: Array, columnFields: Array, columnsAction: String,
            headerInitial: Function, subFields: Array, specialRows: Object, selectionIdField: String, selectionParameter: String, selectionListMode: Boolean },
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
                return {
                    finds: Array.isArray(pkg.formListFinds) ? pkg.formListFinds : [], activeFindId: pkg.formListFindId || "", defaultFindId: pkg.defaultFormListFindId || "",
                    params, hasParams: Object.keys(params).length > 0, orderByField: this.searchObj.orderByField || pkg.orderByField || "",
                    findAction: linkPath, saveFindAction: this.findAction || "", screenPath: screen.path,
                    findUrl: (id) => linkPath + "?formListFindId=" + encodeURIComponent(id),
                    findFields: (id) => Object.assign({ formLocation: this.formLocation, formListFindId: id }, params),
                    newFindFields: () => Object.assign({ formLocation: this.formLocation, _findDescription: "" }, params),
                    exportUrl: (mode) => screen.path + "?" + query({ renderMode: mode, pageNoLimit: "true", lastStandalone: "true", saveFilename: this.name + "." + mode }),
                    printUrl: () => screen.path + "?" + query({ renderMode: "html-print", pageNoLimit: "true" }),
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
                const out = { moquiFormName: this.name, _isMulti: "true", pageIndex: String((this.pkg && this.pkg.pageIndex) || 0) };
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
            const selCell = (rowIndex) => {
                if (!sel) return null;
                const row = rowIndex === null ? null : this.rows[rowIndex];
                const id = row ? row[this.selectionIdField] : null;
                if (id === null || id === undefined || id === "") return h("div", { class: "td" }, "\u00a0");
                return h("div", { class: "td" }, [h("div", { class: "q-my-auto" }, [h("input", { type: "checkbox", style: "transform: scale(1.3)", checked: !!sel.checkboxStates[rowIndex],
                    onClick: (event) => sel.clickCheckbox(event, rowIndex), onChange: (event) => { sel.checkboxStates[rowIndex] = event.target.checked; } })])]);
            };
            const selHead = sel ? h("div", { class: "th" }, [h("span", { class: "q-my-auto" }, [
                h(V.resolveComponent("q-checkbox"), { size: "sm", modelValue: sel.checkboxAllState, "onUpdate:modelValue": sel.setCheckboxAllState }),
                h(V.resolveComponent("q-btn"), { dense: true, flat: true, icon: "build", color: sel.checkboxStates && sel.checkboxStates.includes(true) ? "positive" : "" }, { default: () => [
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
                return h("div", { class: "tr sub-rows" }, [sel ? h("div", { class: "td" }, "\u00a0") : null, h("div", { class: "td" }, [h("div", { class: "q-pl-lg" },
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
                    ? h("div", { class: "tr " + kind }, [sel ? h("div", { class: "td" }, "\u00a0") : null].concat(columns.map((column) => h("div", { class: "td" },
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
            const nav = h("div", { class: "form-list-nav-row row items-center" }, [
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
            return h("div", { class: "q-my-sm q-table__container q-table__card q-table--horizontal-separator q-table--dense q-table--flat" }, [nav, table]);
        }
    });

    /* ========== inline components ========== */

    component("m-link", {
        name: "mLink",
        props: { href: { type: String, required: true }, loadId: String, confirmation: String },
        template: '<a :href="linkHref" @click.prevent="go" class="q-link"><slot></slot></a>',
        methods: {
            go(event) {
                if (event.button !== 0) return;
                if (this.confirmation && this.confirmation.length && !root.confirm(this.confirmation)) return;
                if (this.loadId && this.loadId.length > 0) this.$root.loadContainer(this.loadId, this.href);
                else if (event.ctrlKey || event.metaKey) root.open(this.linkHref, "_blank");
                else this.$root.setUrl(this.linkHref);
            }
        },
        computed: { linkHref() { return this.$root.getLinkPath(this.href); } }
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
        template:
            '<q-card flat bordered class="q-ma-sm m-container-box">' +
                '<q-card-actions @click.self="toggleBody">' +
                    '<h5 v-if="title && title.length" @click="toggleBody" :class="\'text-\' + type">{{title}}</h5>' +
                    '<slot name="header"></slot>' +
                    '<q-space></q-space>' +
                    '<slot name="toolbar"></slot>' +
                    '<q-btn color="grey" round flat dense :icon="isBodyOpen ? \'keyboard_arrow_up\' : \'keyboard_arrow_down\'" @click="toggleBody"></q-btn>' +
                '</q-card-actions>' +
                '<div v-show="isBodyOpen"><q-card-section><slot></slot></q-card-section></div>' +
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
            '<q-dialog :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :id="id" @show="onShow" @hide="onHide" :maximized="$q.platform.is.mobile">' +
                '<q-card ref="dialogCard" flat bordered :style="{width:((width||760)+\'px\'),\'max-width\':($q.platform.is.mobile?\'100vw\':\'90vw\')}">' +
                    '<q-card-actions ref="dialogHeader" :style="{cursor:(draggable?\'move\':\'default\')}">' +
                        '<h5 class="q-pl-sm non-selectable">{{title}}</h5><q-space></q-space>' +
                        '<q-btn icon="close" flat round dense v-close-popup></q-btn>' +
                    '</q-card-actions><q-separator></q-separator>' +
                    '<q-card-section ref="dialogBody"><slot></slot></q-card-section>' +
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

    component("m-container-dialog", {
        name: "mContainerDialog",
        props: { id: String, color: String, buttonText: String, buttonClass: String, title: String, width: String, openDialog: { type: Boolean, default: false }, buttonIcon: { type: String, default: "open_in_new" } },
        data() { return { isShown: false }; },
        template:
            '<span>' +
                '<span @click="show()"><slot name="button"><q-btn dense outline no-caps :icon="buttonIcon" :label="buttonText" :color="color" :class="buttonClass"></q-btn></slot></span>' +
                '<m-dialog v-model="isShown" :id="id" :title="title" :color="color" :width="width"><slot></slot></m-dialog>' +
            '</span>',
        methods: { show() { this.isShown = true; }, hide() { this.isShown = false; } },
        mounted() { if (this.openDialog) this.isShown = true; }
    });

    component("m-dynamic-container", {
        name: "mDynamicContainer",
        props: { id: { type: String, required: true }, url: String },
        data() { return { curComponent: moqui.EmptyComponent, curUrl: "" }; },
        template: '<component :is="curComponent"></component>',
        methods: {
            reload() { const saveUrl = this.curUrl; this.curUrl = ""; setTimeout(() => { this.curUrl = saveUrl; }, 20); },
            load(url) { if (this.curUrl === url) this.reload(); else this.curUrl = url; }
        },
        watch: {
            curUrl(newUrl) {
                if (!newUrl || newUrl.length === 0) { this.curComponent = moqui.EmptyComponent; return; }
                moqui.loadComponent(newUrl, (comp) => { this.curComponent = comp; }, this.id);
            }
        },
        mounted() { this.$root.addContainer(this.id, this); this.curUrl = this.url; }
    });

    component("m-dynamic-dialog", {
        name: "mDynamicDialog",
        props: { id: String, url: { type: String, required: true }, color: String, buttonText: String, buttonClass: String, title: String, width: String,
            openDialog: { type: Boolean, default: false }, dynamicParams: { type: Object, default: null } },
        data() { return { curComponent: moqui.EmptyComponent, curUrl: "", isShown: false }; },
        template:
            '<span>' +
                '<q-btn dense outline no-caps icon="open_in_new" :label="buttonText" :color="color" :class="buttonClass" @click="isShown = true"></q-btn>' +
                '<m-dialog ref="dialog" v-model="isShown" :id="id" :title="title" :color="color" :width="width"><component :is="curComponent"></component></m-dialog>' +
            '</span>',
        methods: {
            reload() { if (this.isShown) { this.isShown = false; this.$nextTick(() => { this.isShown = true; }); } },
            load(url) { this.curUrl = url; },
            hide() { this.isShown = false; }
        },
        watch: {
            curUrl(newUrl) {
                if (!newUrl || newUrl.length === 0) { this.curComponent = moqui.EmptyComponent; return; }
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
        }
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
                '<ul v-show="open" v-if="hasChildren"><m-tree-item v-for="child in model.children" :key="child.id" :model="child" :top="top"></m-tree-item></ul>' +
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
                    moqui.request(url, { method: "POST", params }).then((result) => { this.loadedChildren = result.json; }, moqui.handleAjaxError);
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

    // Click to edit with the popup editor of Quasar. The text shows as the element named by label-type. The widget type
    // is textarea or text-line; with load-url the current value is fetched when the editor opens.
    component("m-editable", {
        name: "mEditable",
        props: { id: { type: String, required: true }, labelType: { type: String, default: "span" }, labelValue: { type: String, required: true },
            url: { type: String, required: true }, urlParameters: { type: Object, default: () => ({}) },
            parameterName: { type: String, default: "value" }, widgetType: { type: String, default: "textarea" },
            loadUrl: String, loadParameters: Object, indicator: { type: String, default: "Saving" }, tooltip: { type: String, default: "Click to edit" },
            cancel: { type: String, default: "Cancel" }, submit: { type: String, default: "Save" } },
        data() { return { shown: this.labelValue, editValue: "", saving: false }; },
        template:
            '<component :is="labelType" :id="id" class="editable-label cursor-pointer" :title="tooltip">' +
                '<span v-html="shown"></span>' +
                '<q-popup-edit v-model="editValue" buttons :label-set="submit" :label-cancel="cancel" @before-show="beforeShow" @save="save" v-slot="scope">' +
                    '<q-input v-model="scope.value" dense autofocus :type="widgetType === \'textarea\' ? \'textarea\' : \'text\'" :loading="saving" @keyup.enter="widgetType === \'textarea\' ? null : scope.set()"></q-input>' +
                '</q-popup-edit>' +
            '</component>',
        methods: {
            beforeShow() {
                // the label may hold markup, the editor starts from the text
                this.editValue = moqui.htmlDecode(String(this.shown).replace(/<br\s*\/?>/gi, "\n"));
                if (!this.loadUrl || !this.loadUrl.length) return;
                const params = Object.assign({ currentValue: this.editValue }, this.loadParameters);
                moqui.request(this.loadUrl, { method: "POST", params, accept: "application/json,text/plain" }).then((result) => {
                    const json = result.json;
                    const value = json !== undefined ? (Array.isArray(json) ? json[0] : json) : result.text;
                    if (value !== undefined && value !== null) this.editValue = moqui.isPlainObject(value) ? (value.value !== undefined ? value.value : value.label) : String(value);
                }, moqui.handleAjaxError);
            },
            save(value) {
                const params = Object.assign({ parameterName: this.parameterName }, this.urlParameters, { [this.parameterName]: value });
                this.saving = true;
                moqui.request(this.url, { method: "POST", params }).then((result) => {
                    this.saving = false;
                    this.shown = moqui.htmlEncode(value).replace(/\n/g, "<br>");
                    if (result.json) moqui.notifyMessages(result.json.messageInfos, result.json.errors);
                }, (err) => { this.saving = false; moqui.handleAjaxError(err); });
            }
        }
    });

    /* ========== form components ========== */

    moqui.screenForms = [];
    moqui.registerScreenForm = (vm) => { if (vm && moqui.screenForms.indexOf(vm) < 0) moqui.screenForms.push(vm); };
    moqui.unregisterScreenForm = (vm) => { const i = moqui.screenForms.indexOf(vm); if (i >= 0) moqui.screenForms.splice(i, 1); };

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
            ' :checkboxStates="checkboxStates" :clickCheckbox="clickCheckbox" :addCheckboxParameters="addCheckboxParameters"></slot></span>'
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

    // The fields and the buttons of a form. The value a field starts with is the fields-initial, and a changed field is
    // tinted by the widget through fieldChanged.
    component("m-form", {
        name: "mForm",
        mixins: [checkboxSetMixin],
        props: { fieldsInitial: Object, action: { type: String, required: true }, method: { type: String, default: "POST" }, readOnly: { type: Boolean, default: false },
            submitMessage: String, submitReloadId: String, submitHideId: String, focusField: String, noValidate: Boolean, excludeEmptyFields: Boolean,
            parentCheckboxSet: Object, checkboxOptional: Boolean, bodyParameterNames: Array, validationErrorsInitial: Array },
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
                else this.$refs.qForm.validate().then((success) => { if (success) this.submitGo(); });
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
                    if (err && err.json) err = Object.assign({}, err, { json: moqui.placeFieldErrors(this, err.json) });
                    moqui.handleLoadError(err && err.status !== undefined ? err : { status: 0, error: err, text: "" });
                });
            },
            handleResponse(resp) {
                let notified = false;
                if (resp && moqui.isPlainObject(resp)) {
                    const rest = moqui.placeFieldErrors(this, resp);
                    notified = moqui.notifyMessages(rest.messageInfos, rest.errors, rest.validationErrors);
                    if (resp.screenUrl && resp.screenUrl.length > 0) this.$root.setUrl(resp.screenUrl);
                    else if (resp.redirectUrl && resp.redirectUrl.length > 0) root.location.href = resp.redirectUrl;
                } else console.warn("m-form no response or non-JSON response");
                if (this.submitHideId && this.submitHideId.length > 0) this.$root.hideContainer(this.submitHideId);
                if (this.submitReloadId && this.submitReloadId.length > 0) this.$root.reloadContainer(this.submitReloadId);
                if (this.submitMessage && this.submitMessage.length) {
                    notify(Object.assign({}, moqui.notifyOpts, { message: this.submitMessage }));
                    addNotify(this.submitMessage, "success");
                } else if (!notified) notify(Object.assign({}, moqui.notifyOpts, { message: "Submit successful" }));
            }
        },
        mounted() {
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
        beforeUnmount() { moqui.unregisterScreenForm(this); }
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
            '<div v-if="paginate && paginate.count > 1" class="q-pagination row no-wrap items-center">' +
                '<template v-if="paginate.pageIndex > 0">' +
                    '<q-btn dense flat no-caps @click.prevent="setIndex(0)" icon="skip_previous"></q-btn>' +
                    '<q-btn dense flat no-caps @click.prevent="setIndex(paginate.pageIndex-1)" icon="fast_rewind"></q-btn></template>' +
                '<template v-else><q-btn dense flat no-caps disabled icon="skip_previous"></q-btn><q-btn dense flat no-caps disabled icon="fast_rewind"></q-btn></template>' +
                '<q-btn v-for="prevIndex in prevArray" :key="prevIndex" dense flat no-caps @click.prevent="setIndex(prevIndex)" :label="prevIndex+1" color="primary"></q-btn>' +
                '<q-btn dense flat no-caps disabled>{{paginate.pageIndex+1}} / {{paginate.pageMaxIndex+1}} ({{paginate.pageRangeLow}}-{{paginate.pageRangeHigh}} / {{paginate.count}})</q-btn>' +
                '<q-btn v-for="nextIndex in nextArray" :key="nextIndex" dense flat no-caps @click.prevent="setIndex(nextIndex)" :label="nextIndex+1" color="primary"></q-btn>' +
                '<template v-if="paginate.pageIndex < paginate.pageMaxIndex">' +
                    '<q-btn dense flat no-caps @click.prevent="setIndex(paginate.pageIndex+1)" icon="fast_forward"></q-btn>' +
                    '<q-btn dense flat no-caps @click.prevent="setIndex(paginate.pageMaxIndex)" icon="skip_next"></q-btn></template>' +
                '<template v-else><q-btn dense flat no-caps disabled icon="fast_forward"></q-btn><q-btn dense flat no-caps disabled icon="skip_next"></q-btn></template>' +
            '</div>',
        computed: {
            prevArray() {
                const pag = this.paginate;
                if (!pag || pag.pageIndex < 1) return [];
                const out = [];
                for (let i = Math.max(pag.pageIndex - 3, 0); i <= pag.pageIndex - 1; i++) out.push(i);
                return out;
            },
            nextArray() {
                const pag = this.paginate;
                if (!pag || pag.pageIndex >= pag.pageMaxIndex) return [];
                const out = [];
                for (let i = pag.pageIndex + 1; i <= Math.min(pag.pageIndex + 3, pag.pageMaxIndex); i++) out.push(i);
                return out;
            }
        },
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
            '<q-form v-if="!formList || (formList.paginate && formList.paginate.pageMaxIndex > 4)" @submit.prevent="goPage">' +
                '<q-input dense outlined v-model="pageIndex" type="text" size="4" name="pageIndex" :placeholder="$q.lang.label ? \'Page #\' : \'Page #\'"' +
                    ' :rules="[val => /^\\d*$/.test(val) || \'digits only\', val => !val || ((formList && +val <= formList.paginate.pageMaxIndex + 1) || (maxIndex && +val <= maxIndex + 1)) || \'higher than max\']">' +
                    '<template v-slot:append><q-btn dense flat no-caps type="submit" icon="redo"></q-btn></template>' +
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
                '<q-list v-for="(column, columnIdx) in columns" :key="column.id" bordered dense>' +
                    '<q-item-label header>{{column.label}}</q-item-label>' +
                    '<q-item v-for="(field, fieldIdx) in column.children" :key="field.id">' +
                        '<q-item-section side v-if="columnIdx !== 0"><q-btn dense flat icon="cancel" @click="hideField(columnIdx, fieldIdx)"><q-tooltip>Hide</q-tooltip></q-btn></q-item-section>' +
                        '<q-item-section><q-item-label>{{field.label}}</q-item-label></q-item-section>' +
                        '<q-item-section v-if="columnIdx === 0" side>' +
                            '<q-btn-dropdown dense outline no-caps label="Display"><q-list dense>' +
                                '<q-item v-for="(toColumn, toColumnIdx) in columns.slice(1)" :key="toColumn.id" clickable v-close-popup>' +
                                    '<q-item-section @click="moveToCol(columnIdx, fieldIdx, toColumnIdx+1)">{{toColumn.label}}</q-item-section></q-item>' +
                                '<q-item clickable v-close-popup><q-item-section @click="moveToCol(columnIdx, fieldIdx, columns.length+1)">New Column</q-item-section></q-item>' +
                            '</q-list></q-btn-dropdown>' +
                        '</q-item-section>' +
                        '<q-item-section v-else side><q-btn-group flat>' +
                            '<q-btn :disabled="columnIdx <= 1" dense flat icon="north" @click="moveToCol(columnIdx, fieldIdx, columnIdx-1)"></q-btn>' +
                            '<q-btn :disabled="fieldIdx === 0" dense flat icon="expand_less" @click="moveInCol(columnIdx, fieldIdx, fieldIdx-1)"></q-btn>' +
                            '<q-btn :disabled="(fieldIdx + 1) === column.children.length" dense flat icon="expand_more" @click="moveInCol(columnIdx, fieldIdx, fieldIdx+1)"></q-btn>' +
                            '<q-btn dense flat icon="south" @click="moveToCol(columnIdx, fieldIdx, columnIdx+1)"></q-btn>' +
                        '</q-btn-group></q-item-section>' +
                    '</q-item>' +
                '</q-list>' +
                '<div class="q-my-md">' +
                    '<q-btn dense outline no-caps @click.prevent="saveColumns()" label="Save Changes"></q-btn>' +
                    '<q-btn dense outline no-caps @click.prevent="resetColumns()" label="Undo Changes"></q-btn>' +
                    '<q-btn dense outline no-caps @click.prevent="resetToDefault()" label="Reset to Default"></q-btn>' +
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
                        '<q-btn v-if="csvButton" dense outline type="a" :href="csvUrl" label="CSV"></q-btn>' +
                        '<slot name="nav"></slot>' +
                    '</div>' +
                    '<div class="table q-table" :id="idVal+\'_table\'">' +
                        '<div v-if="!skipHeader" class="thead">' +
                            '<m-form-link v-if="headerForm && !headerDialog" :name="idVal+\'_header\'" :id="idVal+\'_header\'" :action="$root.currentLinkPath" :fields-initial="searchObj" v-slot:default="formProps">' +
                                '<slot name="headerForm" :search="formProps.fields"></slot>' +
                            '</m-form-link>' +
                            '<div class="tr">' +
                                '<div v-if="selection" class="th"><q-checkbox size="sm" :model-value="allSelected" @update:model-value="selectAll"></q-checkbox></div>' +
                                '<slot name="header" :search="searchObj" :sort-by="sortBy" :order-by="orderBy"></slot>' +
                            '</div>' +
                        '</div>' +
                        '<div class="tbody">' +
                            '<template v-for="(fields, rowIndex) in rowList" :key="rowIndex">' +
                                '<div class="tr" :class="{selected:selected[rowIndex]}">' +
                                    '<div v-if="selection" class="td"><q-checkbox size="sm" :model-value="!!selected[rowIndex]" @update:model-value="selected[rowIndex] = $event"></q-checkbox></div>' +
                                    '<slot name="row" :fields="fields" :row-index="rowIndex" :selected="!!selected[rowIndex]"></slot>' +
                                '</div>' +
                                '<template v-if="fields.aggregateSubList && fields.aggregateSubList.length">' +
                                    '<div v-for="(subRow, subIndex) in fields.aggregateSubList" :key="rowIndex+\'_\'+subIndex" class="tr form-list-sub-row"><slot name="subRow" :row="fields" :sub-row="subRow" :sub-index="subIndex"></slot></div>' +
                                '</template>' +
                            '</template>' +
                            '<div v-if="!rowList.length && !loading" class="tr"><div class="td">No results found</div></div>' +
                        '</div>' +
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
        props: { label: String, bgColor: String, disable: Boolean },
        template: '<q-field dense outlined stack-label :label="label" :bg-color="bgColor" :disable="disable" v-bind="$attrs" :model-value="1"><template v-slot:control><slot></slot></template></q-field>'
    });

    component("m-date-time", {
        name: "mDateTime",
        mixins: [fieldMixin, localValueMixin],
        props: { id: String, name: { type: String, required: true }, modelValue: String, type: { type: String, default: "date-time" }, label: String,
            size: String, format: String, tooltip: String, form: String, required: Boolean, rules: Array, disable: Boolean, autoYear: String,
            minuteStep: { type: Number, default: 5 } },
        emits: ["update:modelValue"],
        template:
            '<q-input dense outlined stack-label :label="label" :model-value="local" @update:model-value="onInput" :rules="allRules"' +
                    ' :mask="inputMask" fill-mask :for="id" :name="name" :form="form" :disable="disable" :size="sizeVal"' +
                    ' style="max-width:max-content;" :bg-color="bg" v-bind="errorAttrs">' +
                '<template v-slot:prepend v-if="hasDate">' +
                    '<q-icon name="event" class="cursor-pointer">' +
                        '<q-popup-proxy ref="qDateProxy" transition-show="scale" transition-hide="scale">' +
                            '<q-date :model-value="local" @update:model-value="onDate" :mask="formatVal"></q-date>' +
                        '</q-popup-proxy>' +
                    '</q-icon>' +
                '</template>' +
                '<template v-slot:append v-if="hasTime">' +
                    '<q-icon name="access_time" class="cursor-pointer">' +
                        '<q-popup-proxy ref="qTimeProxy" transition-show="scale" transition-hide="scale">' +
                            '<q-time :model-value="local" @update:model-value="onTime" :mask="formatVal" format24h></q-time>' +
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
                        '<q-btn dense flat icon="calendar_view_day" @click="toggleMode"><q-tooltip>Period Select Mode</q-tooltip></q-btn>' +
                        '<q-btn dense flat icon="clear" @click="clearAll"><q-tooltip>Clear</q-tooltip></q-btn>' +
                    '</template>' +
                '</m-date-time>' +
            '</div>' +
            '<div v-else class="row"><q-input dense outlined stack-label :label="label" v-model="fields[name+\'_pdate\']"' +
                    ' mask="####-##-##" fill-mask :for="id" :name="name+\'_pdate\'" :form="form" style="max-width:max-content;"' +
                    ' :bg-color="changed(name+\'_pdate\')">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<template v-slot:before>' +
                    '<q-select class="q-pr-xs" dense outlined options-dense emit-value map-options v-model="fields[name+\'_poffset\']"' +
                        ' :name="name+\'_poffset\'" :bg-color="changed(name+\'_poffset\')" stack-label label="Offset" :options="dateOffsets" :form="form" behavior="menu"></q-select>' +
                    '<q-select dense outlined options-dense emit-value map-options v-model="fields[name+\'_period\']"' +
                        ' :name="name+\'_period\'" :bg-color="changed(name+\'_period\')" stack-label label="Period" :options="datePeriods" :form="form" behavior="menu"></q-select>' +
                '</template>' +
                '<template v-slot:prepend>' +
                    '<q-icon name="event" class="cursor-pointer">' +
                        '<q-popup-proxy ref="qDateProxy" transition-show="scale" transition-hide="scale">' +
                            '<q-date v-model="fields[name+\'_pdate\']" mask="YYYY-MM-DD" @update:model-value="hideDate"></q-date>' +
                        '</q-popup-proxy>' +
                    '</q-icon>' +
                '</template>' +
                '<template v-slot:after>' +
                    '<q-btn dense flat icon="date_range" @click="toggleMode"><q-tooltip>Date Range Mode</q-tooltip></q-btn>' +
                    '<q-btn dense flat icon="clear" @click="clearAll"><q-tooltip>Clear</q-tooltip></q-btn>' +
                '</template>' +
            '</q-input></div>',
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
        template:
            '<q-select ref="qSelect" :model-value="modelValue" @update:model-value="handleInput"' +
                    ' dense outlined options-dense use-input :hide-selected="multiple" :name="name" :for="id" :form="form"' +
                    ' :input-debounce="serverSearch ? serverDelay : 100" @filter="filterFn" :clearable="allowEmpty||multiple" :disable="disable"' +
                    ' :multiple="multiple" :emit-value="!onSelectGoTo" map-options behavior="menu" :new-value-mode="combo ? \'add-unique\' : undefined"' +
                    ' :rules="[val => allowEmpty||multiple||combo||val===\'\'||(val&&val.length)||\'Please select an option\']"' +
                    ' stack-label :label="label" :loading="loading" :bg-color="bg" :options="curOptions" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<template v-slot:no-option><q-item><q-item-section class="text-grey">No results</q-item-section></q-item></template>' +
                '<template v-if="multiple" v-slot:prepend><div>' +
                    '<q-chip v-for="valueEntry in modelValue" :key="valueEntry" dense size="md" class="q-my-xs" removable @remove="removeValue(valueEntry)">{{optionLabel(valueEntry)}}</q-chip>' +
                '</div></template>' +
                '<template v-slot:after><slot name="after"></slot></template>' +
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
            optionLabel(value) {
                const option = (this.allOptions || []).find((item) => item.value === value);
                return option ? option.label : "";
            },
            removeValue(value) {
                const current = this.modelValue;
                if (!Array.isArray(current)) return;
                const next = current.filter((entry) => entry !== value);
                if (next.length !== current.length) this.$emit("update:modelValue", next);
                this.maybeSubmit();
            }
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
            requiredMessage: { type: String, default: "Required" }, maxlength: String, form: String,
            defaultUrl: String, defaultParameters: Object, dependsOn: Object, dependsOptional: Boolean, defaultLoadInit: Boolean },
        emits: ["update:modelValue"],
        data() { return { loading: false }; },
        template:
            '<q-input dense outlined stack-label :label="label" :prefix="prefix" :model-value="local" @update:model-value="onInput" :type="type"' +
                    ' :for="id" :name="name" :size="size" :maxlength="maxlength" :form="form" :loading="loading" :rules="allRules" :disable="disable" :bg-color="bg"' +
                    ' :mask="mask" :fill-mask="fillMask" :reverse-fill-mask="reverseFillMask" autocapitalize="off" autocomplete="off" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
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
    // sanitized by DOMPurify), both lazy. The rich editor simplifies markup it does not understand.
    component("m-text-area", {
        name: "mTextArea",
        mixins: [fieldMixin, localValueMixin],
        props: { modelValue: String, name: String, id: String, label: String, editorType: String, rows: { type: String, default: "3" }, cols: String,
            readonly: Boolean, autogrow: Boolean, maxlength: String, disable: Boolean, form: String, tooltip: String },
        emits: ["update:modelValue"],
        data() {
            return { preview: false, previewHtml: "", toolbar: [["bold", "italic", "strike", "underline"], ["unordered", "ordered", "outdent", "indent"],
                ["quote", "link"], ["left", "center", "right", "justify"], ["undo", "redo"], ["viewsource"]] };
        },
        template:
            '<div v-if="editorType === \'html\'" class="m-text-area-editor">' +
                '<div v-if="label" class="text-caption">{{label}}</div>' +
                '<q-editor :model-value="local" @update:model-value="onInput" :toolbar="toolbar" min-height="12rem" :readonly="readonly" :disable="disable"></q-editor>' +
                '<input type="hidden" :name="name" :value="local" :form="form">' +
            '</div>' +
            '<div v-else-if="editorType === \'md\'" class="m-text-area-editor">' +
                '<q-input type="textarea" dense outlined stack-label :label="label" :model-value="local" @update:model-value="onInput" :name="name" :for="id" :rows="rows"' +
                    ' :readonly="readonly" :disable="disable" :form="form" :bg-color="bg" autogrow v-bind="errorAttrs">' +
                    '<template v-slot:after><q-btn dense flat :icon="preview ? \'edit\' : \'visibility\'" @click="togglePreview"><q-tooltip>Preview</q-tooltip></q-btn></template>' +
                '</q-input>' +
                '<div v-if="preview" class="q-pa-sm markdown-preview" v-html="previewHtml"></div>' +
            '</div>' +
            '<q-input v-else type="textarea" dense outlined stack-label :label="label" :model-value="local" @update:model-value="onInput" :name="name" :for="id" :rows="rows"' +
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
            tooltip: String, noWrapper: Boolean, trueValue: String, falseValue: String, singleLabel: String },
        emits: ["update:modelValue"],
        template:
            '<q-checkbox v-if="!options.length" size="xs" :model-value="local" @update:model-value="onInput" :true-value="trueValue" :false-value="falseValue"' +
                ' :label="singleLabel" :name="name" :id="id" :form="form" :disable="disable"></q-checkbox>' +
            '<m-field v-else-if="!noWrapper" :label="label" :bg-color="bg" :disable="disable" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<q-checkbox v-for="(option, index) in options" :key="option.value" size="xs" :val="option.value" :label="option.label" :model-value="local"' +
                    ' @update:model-value="onInput" :name="name" :id="index > 0 ? id + \'_\' + index : id" :form="form" :disable="disable"></q-checkbox>' +
            '</m-field>' +
            '<template v-else><q-checkbox v-for="(option, index) in options" :key="option.value" size="xs" :val="option.value" :label="option.label" :model-value="local"' +
                ' @update:model-value="onInput" :name="name" :id="index > 0 ? id + \'_\' + index : id" :form="form" :disable="disable"></q-checkbox></template>'
    });

    component("m-radio", {
        name: "mRadio",
        mixins: [fieldMixin, localValueMixin],
        props: { modelValue: String, name: String, id: String, label: String, options: { type: Array, default: () => [] }, disable: Boolean, form: String, tooltip: String },
        emits: ["update:modelValue"],
        template:
            '<m-field :label="label" :bg-color="bg" :disable="disable" v-bind="errorAttrs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip>' +
                '<q-radio v-for="(option, index) in options" :key="option.value" size="xs" :val="option.value" :label="option.label" :model-value="local"' +
                    ' @update:model-value="onInput" :name="name" :id="index > 0 ? id + \'_\' + index : id" :form="form" :disable="disable"></q-radio>' +
            '</m-field>'
    });

    component("m-file", {
        name: "mFile",
        mixins: [fieldMixin],
        props: { modelValue: { type: null }, name: String, id: String, label: String, size: String, multiple: Boolean, accept: String, maxlength: String,
            disable: Boolean, form: String, tooltip: String },
        emits: ["update:modelValue"],
        template:
            '<q-file dense outlined stack-label :label="label" :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :name="name" :for="id"' +
                ' :size="size" :multiple="multiple" :accept="accept" :form="form" :disable="disable" :bg-color="bg" v-bind="errorAttrs">' +
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
        template:
            '<m-text-line :label="label" :name="name" :id="id" :size="size" :maxlength="maxlength" :form="form" :tooltip="tooltip" v-model="fields[name]"></m-text-line>' +
            '<div v-if="hideOptions !== \'true\'" class="row items-center text-find-options">' +
                '<q-checkbox v-if="hideOptions !== \'operator\'" size="xs" :name="name + \'_not\'" label="Not" true-value="Y" false-value="N" v-model="fields[name + \'_not\']" :form="form"></q-checkbox>' +
                '<q-select v-if="hideOptions !== \'operator\'" dense outlined options-dense emit-value map-options :name="name + \'_op\'" v-model="fields[name + \'_op\']" :options="operators" :form="form" behavior="menu"></q-select>' +
                '<q-checkbox v-if="hideOptions !== \'ignore-case\'" size="xs" :name="name + \'_ic\'" label="Ignore Case" true-value="Y" false-value="N" v-model="fields[name + \'_ic\']" :form="form"></q-checkbox>' +
            '</div>'
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
        props: { config: { type: Object, default: () => ({ startOnLoad: false, securityLevel: "strict" }) }, height: { type: String, default: "400px" }, width: { type: String, default: "100%" } },
        template: '<div class="mermaid-container" :style="{height:height,width:width}"><pre ref="source" style="display:none;"><slot></slot></pre><div ref="target"></div></div>',
        mounted() {
            moqui.loadScript("/libs/mermaid/mermaid.min.js", null, () => !!root.mermaid).then(() => {
                const diagram = root.document.createElement("pre");
                diagram.className = "mermaid";
                diagram.textContent = this.$refs.source.textContent.trim();
                this.$refs.target.appendChild(diagram);
                root.mermaid.initialize(Object.assign({ startOnLoad: false, securityLevel: "strict" }, this.config));
                return root.mermaid.run({ nodes: [diagram] });
            }).catch((err) => console.error("m-mermaid: " + err));
        }
    });

    /* ========== subscreens and the menu ========== */

    component("m-subscreens-tabs", {
        name: "mSubscreensTabs",
        data() { return { pathIndex: -1 }; },
        template:
            '<div v-if="subscreens.length > 1"><q-tabs dense no-caps align="left" active-color="primary" indicator-color="primary" :model-value="activeTab">' +
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

    component("m-menu-nav-item", {
        name: "mMenuNavItem",
        props: { menuIndex: Number },
        template:
            '<q-expansion-item v-if="navMenuItem && navMenuItem.subscreens && navMenuItem.subscreens.length" :model-value="true" :content-inset-level="0.3"' +
                    ' switch-toggle-side dense dense-toggle expanded-icon="arrow_drop_down" :to="navMenuItem.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="navMenuItem" active></m-menu-item-content></template>' +
                '<template v-slot:default><m-menu-subscreen-item v-for="(subscreen, ssIndex) in navMenuItem.subscreens" :key="subscreen.name" :menu-index="menuIndex" :subscreen-index="ssIndex"></m-menu-subscreen-item></template>' +
            '</q-expansion-item>' +
            '<q-expansion-item v-else-if="navMenuItem && navMenuItem.savedFinds && navMenuItem.savedFinds.length" :model-value="true" :content-inset-level="0.3"' +
                    ' switch-toggle-side dense dense-toggle expanded-icon="arrow_drop_down" :to="navMenuItem.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="navMenuItem" active></m-menu-item-content></template>' +
                '<template v-slot:default><q-expansion-item v-for="(savedFind, ssIndex) in navMenuItem.savedFinds" :key="savedFind.name"' +
                        ' :model-value="false" switch-toggle-side dense dense-toggle expand-icon="chevron_right" :to="savedFind.pathWithParams">' +
                    '<template v-slot:header><m-menu-item-content :menu-item="savedFind" :active="savedFind.active"></m-menu-item-content></template>' +
                '</q-expansion-item></template>' +
            '</q-expansion-item>' +
            '<q-expansion-item v-else-if="navMenuItem && menuIndex < (navMenuLength - 1)" :model-value="true" :content-inset-level="0.3"' +
                    ' switch-toggle-side dense dense-toggle expanded-icon="arrow_drop_down" :to="navMenuItem.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="navMenuItem" active></m-menu-item-content></template>' +
                '<template v-slot:default><m-menu-nav-item :menu-index="menuIndex + 1"></m-menu-nav-item></template>' +
            '</q-expansion-item>' +
            '<q-expansion-item v-else-if="navMenuItem" :model-value="false" switch-toggle-side dense dense-toggle expand-icon="arrow_right" :to="navMenuItem.pathWithParams">' +
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
            '<q-expansion-item v-else :model-value="false" switch-toggle-side dense dense-toggle expand-icon="arrow_right" :to="subscreen.pathWithParams">' +
                '<template v-slot:header><m-menu-item-content :menu-item="subscreen"></m-menu-item-content></template>' +
            '</q-expansion-item>',
        computed: { subscreen() { return this.$root.navMenuList[this.menuIndex].subscreens[this.subscreenIndex]; } }
    });

    component("m-menu-item-content", {
        name: "mMenuItemContent",
        props: { menuItem: Object, active: Boolean },
        template:
            '<div class="q-item__section column q-item__section--main justify-center"><div class="q-item__label">' +
                '<i v-if="menuItem.image && menuItem.imageType === \'icon\'" :class="menuItem.image" style="padding-right: 8px;"></i>' +
                '<span v-else-if="menuItem.image" style="padding-right:8px;"><img :src="menuItem.image" :alt="menuItem.title" height="14" class="invertible"></span>' +
                '<span :class="{\'text-primary\':active}">{{menuItem.title}}</span>' +
            '</div></div>'
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
        "    <q-header reveal bordered class=\"bg-black text-white\" id=\"top\"><q-toolbar style=\"font-size:15px;\">\n" +
        "        <q-btn dense flat icon=\"menu\" @click=\"toggleLeftOpen()\"></q-btn>\n" +
        "        <m-link v-if=\"shell.logoUrl\" href=\"/apps\"><div class=\"q-mx-md q-mt-sm\"><img :src=\"shell.logoUrl\" alt=\"Home\" height=\"32\"></div></m-link>\n" +
        "        <q-toolbar-title v-if=\"shell.title\">{{shell.title}}</q-toolbar-title>\n" +
        "        <template v-for=\"(navMenuItem, menuIndex) in navMenuList\"><template v-if=\"menuIndex < (navMenuList.length - 1)\">\n" +
        "            <m-link v-if=\"navMenuItem.hasTabMenu\" :href=\"getNavHref(menuIndex)\" class=\"gt-xs\">{{navMenuItem.title}}</m-link>\n" +
        "            <div v-else-if=\"navMenuItem.subscreens && navMenuItem.subscreens.length\" class=\"cursor-pointer gt-xs\">\n" +
        "                {{navMenuItem.title}}\n" +
        "                <q-menu anchor=\"bottom left\" self=\"top left\"><q-list dense style=\"min-width: 200px\">\n" +
        "                    <q-item v-for=\"subscreen in navMenuItem.subscreens\" :key=\"subscreen.name\" :class=\"{'bg-primary':subscreen.active, 'text-white':subscreen.active}\" clickable v-close-popup><q-item-section>\n" +
        "                        <m-link :href=\"subscreen.pathWithParams\">\n" +
        "                            <template v-if=\"subscreen.image\">\n" +
        "                                <i v-if=\"subscreen.imageType === 'icon'\" :class=\"subscreen.image\" style=\"padding-right: 4px;\"></i>\n" +
        "                                <img v-else :src=\"subscreen.image\" :alt=\"subscreen.title\" width=\"18\" class=\"invertible\" style=\"padding-right: 4px;\">\n" +
        "                            </template>\n" +
        "                            <i v-else class=\"fa fa-link\" style=\"padding-right: 8px;\"></i>\n" +
        "                            {{subscreen.title}}\n" +
        "                        </m-link>\n" +
        "                    </q-item-section></q-item>\n" +
        "                </q-list></q-menu>\n" +
        "            </div>\n" +
        "            <m-link v-else :href=\"getNavHref(menuIndex)\" class=\"gt-xs\">{{navMenuItem.title}}</m-link>\n" +
        "            <q-icon size=\"1.5em\" name=\"chevron_right\" color=\"grey\" class=\"gt-xs\"></q-icon>\n" +
        "        </template></template>\n" +
        "        <m-link v-if=\"navMenuList.length > 0\" :href=\"getNavHref(navMenuList.length - 1)\" class=\"gt-xs\">{{navMenuList[navMenuList.length - 1].title}}</m-link>\n" +
        "\n" +
        "        <q-space></q-space>\n" +
        "\n" +
        "        <q-circular-progress indeterminate size=\"20px\" color=\"light-blue\" class=\"q-ma-xs\" :class=\"{ hidden: loading < 1 }\"></q-circular-progress>\n" +
        "\n" +
        "        <component :is=\"qzVue\" ref=\"qzVue\"></component>\n" +
        "\n" +
        "        <q-btn dense flat icon=\"help_outline\" color=\"info\" :class=\"{hidden:!documentMenuList.length}\">\n" +
        "            <q-tooltip>{{labels.documentation}}</q-tooltip>\n" +
        "            <q-menu><q-list dense class=\"q-my-md\">\n" +
        "                <q-item v-for=\"screenDoc in documentMenuList\" :key=\"screenDoc.index\"><q-item-section>\n" +
        "                    <m-dynamic-dialog :url=\"currentPath + '/screenDoc?docIndex=' + screenDoc.index\" :button-text=\"screenDoc.title\" :title=\"screenDoc.title\"></m-dynamic-dialog>\n" +
        "                </q-item-section></q-item>\n" +
        "            </q-list></q-menu>\n" +
        "        </q-btn>\n" +
        "\n" +
        "        <template v-for=\"navPlugin in navPlugins\"><component :is=\"navPlugin\"></component></template>\n" +
        "\n" +
        "        <q-btn dense flat icon=\"notifications\">\n" +
        "            <q-tooltip>{{labels.notifyHistory}}</q-tooltip>\n" +
        "            <q-menu><q-list dense style=\"min-width: 300px\">\n" +
        "                <q-item v-for=\"(histItem, histIndex) in notifyHistoryList\" :key=\"histIndex\"><q-item-section>\n" +
        "                    <q-banner dense rounded class=\"text-white\" :class=\"'bg-' + getQuasarColor(histItem.type)\">\n" +
        "                        <strong>{{histItem.time}}</strong> <span>{{histItem.message}}</span>\n" +
        "                        <template v-if=\"histItem.link != null\" v-slot:action>\n" +
        "                            <q-btn :to=\"histItem.link\" flat color=\"white\" label=\"View\"></q-btn>\n" +
        "                        </template>\n" +
        "                    </q-banner>\n" +
        "                </q-item-section></q-item>\n" +
        "            </q-list></q-menu>\n" +
        "        </q-btn>\n" +
        "\n" +
        "        <q-btn dense flat icon=\"history\">\n" +
        "            <q-tooltip>{{labels.screenHistory}}</q-tooltip>\n" +
        "            <q-menu><q-list dense style=\"min-width: 300px\">\n" +
        "                <q-item v-for=\"histItem in navHistoryList\" :key=\"histItem.pathWithParams\" clickable v-close-popup><q-item-section>\n" +
        "                    <m-link :href=\"histItem.pathWithParams\">\n" +
        "                        <template v-if=\"histItem.image\">\n" +
        "                            <i v-if=\"histItem.imageType === 'icon'\" :class=\"histItem.image\" style=\"padding-right: 8px;\"></i>\n" +
        "                            <img v-else :src=\"histItem.image\" :alt=\"histItem.title\" width=\"18\" style=\"padding-right: 4px;\">\n" +
        "                        </template>\n" +
        "                        <i v-else class=\"fa fa-link\" style=\"padding-right: 8px;\"></i>\n" +
        "                        {{histItem.title}}\n" +
        "                    </m-link>\n" +
        "                </q-item-section></q-item>\n" +
        "            </q-list></q-menu>\n" +
        "        </q-btn>\n" +
        "\n" +
        "        <q-btn dense flat icon=\"account_circle\">\n" +
        "            <q-tooltip>{{userFullName || labels.account}}</q-tooltip>\n" +
        "            <q-menu><q-card flat bordered>\n" +
        "                <q-card-section horizontal class=\"q-pa-md\">\n" +
        "                    <q-card-section>\n" +
        "                        <div v-if=\"userFullName\" class=\"q-mb-sm text-strong\">{{labels.welcome}} {{userFullName}}</div>\n" +
        "                        <template v-for=\"(accountPlugin, accountIndex) in accountPlugins\" :key=\"accountIndex\"><component :is=\"accountPlugin\"></component></template>\n" +
        "                    </q-card-section>\n" +
        "                    <q-separator vertical></q-separator>\n" +
        "                    <q-card-actions vertical class=\"justify-around q-px-md\">\n" +
        "                        <q-btn flat dense icon=\"settings_power\" color=\"negative\" type=\"a\" :href=\"shell.logoutUrl\" :data-confirm=\"labels.logout + ' ' + userFullName + '?'\">\n" +
        "                            <q-tooltip>{{labels.logout}} {{userFullName}}</q-tooltip></q-btn>\n" +
        "                        <q-btn flat dense @click.prevent=\"switchDarkLight()\" icon=\"invert_colors\">\n" +
        "                            <q-tooltip>{{labels.switchDarkLight}}</q-tooltip></q-btn>\n" +
        "                        <q-btn flat dense icon=\"autorenew\" color=\"negative\" @click=\"reLoginShowDialog\"><q-tooltip>Re-Login</q-tooltip></q-btn>\n" +
        "                    </q-card-actions>\n" +
        "                </q-card-section>\n" +
        "            </q-card></q-menu>\n" +
        "        </q-btn>\n" +
        "    </q-toolbar></q-header>\n" +
        "\n" +
        "    <q-page-container>\n" +
        "        <div class=\"shell-row\">\n" +
        "            <div v-show=\"leftOpen\" class=\"app-menu-col\" :class=\"{ 'app-menu-overlay': $q.screen.lt.md }\">\n" +
        "                <q-btn dense flat icon=\"menu\" @click=\"toggleLeftOpen()\" class=\"lt-sm\"></q-btn>\n" +
        "                <q-list dense padding><m-menu-nav-item :menu-index=\"0\"></m-menu-nav-item></q-list>\n" +
        "            </div>\n" +
        "            <q-page class=\"shell-page-col\">\n" +
        "                <div class=\"q-ma-sm\"><m-subscreens-active></m-subscreens-active></div>\n" +
        "            </q-page>\n" +
        "        </div>\n" +
        "    </q-page-container>\n" +
        "\n" +
        "    <q-footer reveal bordered class=\"bg-grey-9 text-white row q-pa-xs\" id=\"footer\">\n" +
        "        <div v-html=\"shell.footerHtml\" class=\"row full-width\"></div>\n" +
        "    </q-footer>\n" +
        "</q-layout>\n" +
        "\n" +
        "<m-dialog v-model=\"reLoginShow\" width=\"400\" :title=\"labels.reLogin\">\n" +
        "    <div v-if=\"reLoginMfaData\">\n" +
        "        <div style=\"text-align:center;padding-bottom:10px\">User <strong>{{username}}</strong> requires an authentication code, you have these options:</div>\n" +
        "        <div style=\"text-align:center;padding-bottom:10px\">{{reLoginMfaData.factorTypeDescriptions.join(\", \")}}</div>\n" +
        "        <q-form @submit.prevent=\"reLoginVerifyOtp\" autocapitalize=\"off\" autocomplete=\"off\">\n" +
        "            <m-text-line v-model=\"reLoginOtp\" name=\"code\" type=\"password\" :label=\"labels.authenticationCode\"></m-text-line>\n" +
        "            <q-btn outline no-caps color=\"primary\" type=\"submit\" :label=\"labels.signIn\"></q-btn>\n" +
        "        </q-form>\n" +
        "        <div v-for=\"sendableFactor in reLoginMfaData.sendableFactors\" :key=\"sendableFactor.factorId\" style=\"padding:8px\">\n" +
        "            <q-btn outline no-caps dense :label=\"labels.sendCodeTo + ' ' + sendableFactor.factorOption\" @click.prevent=\"reLoginSendOtp(sendableFactor.factorId)\"></q-btn>\n" +
        "        </div>\n" +
        "    </div>\n" +
        "    <div v-else>\n" +
        "        <div style=\"text-align:center;padding-bottom:10px\">Please sign in to continue as user <strong>{{username}}</strong></div>\n" +
        "        <q-form @submit.prevent=\"reLoginSubmit\" autocapitalize=\"off\" autocomplete=\"off\">\n" +
        "            <m-text-line v-model=\"reLoginPassword\" name=\"password\" type=\"password\" :label=\"labels.password\"></m-text-line>\n" +
        "            <q-btn outline no-caps color=\"primary\" type=\"submit\" :label=\"labels.signIn\"></q-btn>\n" +
        "            <q-btn outline no-caps color=\"negative\" @click.prevent=\"reLoginReload\" :label=\"labels.reloadPage\"></q-btn>\n" +
        "        </q-form>\n" +
        "    </div>\n" +
        "</m-dialog>\n"
    });

    const rootOptions = (config) => ({
        data() {
            return {
                basePath: config.basePath || "", linkBasePath: config.linkBasePath || "", currentPathList: [], extraPathList: [], currentParameters: {}, bodyParameters: null,
                activeSubscreens: [], navMenuList: [], navHistoryList: [], navPlugins: [], accountPlugins: [], notifyHistoryList: [],
                lastNavTime: Date.now(), loading: 0, currentLoadRequest: null, activeContainers: {}, urlListeners: [],
                moquiSessionToken: config.sessionToken || "", appHost: config.appHost || "", appRootPath: config.appRootPath || "", userId: config.userId || "",
                username: config.username || "", locale: config.locale || "en", userFullName: config.userFullName || "",
                reLoginShow: false, reLoginPassword: null, reLoginMfaData: null, reLoginOtp: null,
                notificationClient: null, sessionTokenBc: null, qzVue: null, leftOpen: !!config.leftOpen,
                shell: Object.assign({ logoUrl: "", title: "", logoutUrl: "/Login/logout", footerHtml: "" }, config.shell), labels: Object.assign({}, config.labels)
            };
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
            reloadContainer(contId) { const comp = this.activeContainers[contId]; if (comp) comp.reload(); else console.error("Container with ID " + contId + " not found, not reloading"); },
            loadContainer(contId, url) { const comp = this.activeContainers[contId]; if (comp) comp.load(url); else console.error("Container with ID " + contId + " not found, not loading url " + url); },
            hideContainer(contId) { const comp = this.activeContainers[contId]; if (comp && comp.hide) comp.hide(); else console.error("Container with ID " + contId + " not found, not hiding"); },

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
                if (!navIndex) navIndex = this.navMenuList.length - 1;
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
            ScreenTitle() { return this.navMenuList.length > 0 ? this.navMenuList[this.navMenuList.length - 1].title : ""; },
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
        },
        mounted() {
            // the shell is hidden until Vue has compiled it
            root.document.getElementById("apps-root").style.display = "";
            this.setUrl(root.location.pathname + root.location.search);
            if (this.notificationClient) this.notificationClient.registerListener("ALL");
            // desktop notifications: ask once, if the browser has not been told yet
            if (root.Notification && root.Notification.permission === "default") {
                root.Notification.requestPermission().then((status) => {
                    if (status === "granted") moqui.notifyMessages("Browser notifications enabled, if you don't want them use browser notification settings to block");
                    else if (status === "denied") moqui.notifyMessages("Browser notifications disabled, if you want them use browser notification settings to allow");
                });
            }
            if (config.qz && config.qz.enabled) this.loadQz();
        },
        unmounted() { if (this.sessionTokenBc) this.sessionTokenBc.close(); }
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
        const quasarOptions = { config: Object.assign({ brand: { info: "#1e7b8e" }, notify: { progress: true, closeBtn: "X", position: "top-right" }, loadingBar: { color: "primary" } }, config.quasar),
            plugins: { Notify: root.Quasar.Notify, LoadingBar: root.Quasar.LoadingBar } };
        const langKey = String(config.quasarLang || "enUS").replace(/-([a-zA-Z])/g, (m, ch) => ch.toUpperCase()).replace("-", "");
        if (root.Quasar.Lang && root.Quasar.Lang[langKey]) quasarOptions.lang = root.Quasar.Lang[langKey];
        app.use(root.Quasar, quasarOptions);
        Object.keys(qvt.components).forEach((name) => app.component(name, qvt.components[name]));
        app.config.globalProperties.moqui = moqui;
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
