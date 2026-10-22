/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The helpers the qvt client and the screens' own scripts use through the moqui global. No library is needed: this is plain
 * ES2020 over fetch, and the functions that touch no document run under Node (MoquiLib.test.js). The Select2, jQuery Validate,
 * Bootstrap Notify and Inputmask setup of the Java file is gone with those libraries.
 */
(function (root, factory) {
    "use strict";
    const node = typeof module === "object" && module.exports;
    const moqui = node ? {} : (root.moqui = root.moqui || {});
    // $mq is the name the server's field rules use ($mq.isStringNumber and the rest). Scripts see this
    // page global. A merged qvt template does not: Vue resolves $mq on app.config.globalProperties,
    // which qvt.start sets. A server-static template resolves it on the scope host.
    root.$mq = moqui;
    factory(root, moqui);
    if (node) module.exports = moqui;
})(globalThis, function (root, moqui) {
    "use strict";

    const toString = Object.prototype.toString;

    Object.assign(moqui, {
        // Quasar language packs by language, for the locale of the user. The page loads the pack that matches.
        quasarLangs: ["en-US", "de", "es", "fr", "it", "pt-BR", "ja", "zh-CN", "ru", "nl"],

        isString: (obj) => typeof obj === "string",
        isBoolean: (obj) => typeof obj === "boolean",
        isNumber: (obj) => typeof obj === "number",
        isArray: (obj) => Array.isArray(obj),
        isFunction: (obj) => typeof obj === "function",
        isPlainObject: (obj) => obj !== null && typeof obj === "object" && toString.call(obj) === "[object Object]",
        isEmptyObject: (obj) => !obj || (typeof obj === "object" && Object.keys(obj).length === 0),

        deepCopy: (value) => {
            if (typeof value !== "object" || value === null) return value;
            if (Array.isArray(value)) return value.map(moqui.deepCopy);
            const out = {};
            Object.keys(value).forEach((key) => { out[key] = moqui.deepCopy(value[key]); });
            return out;
        },

        arraysEqual: (array1, array2, ignoreOrder) => {
            if (!array1 && !array2) return true;
            if (!array1 && array2 && !array2.length) return true;
            if (!array2 && array1 && !array1.length) return true;
            if (!Array.isArray(array1)) return array1 === array2;
            if (!Array.isArray(array2) || array1.length !== array2.length) return false;
            for (let i = 0; i < array1.length; i++) {
                if (ignoreOrder) {
                    if (array2.indexOf(array1[i]) === -1) return false;
                } else if (array2[i] !== array1[i]) return false;
            }
            return true;
        },

        // Two values are equal when both are empty, or when one is empty and the other is the placeholder of an unfilled mask.
        equalsOrPlaceholder: (obj1, obj2) => {
            if (!obj1 && (!obj2 || !obj2.length)) return true;
            if (!obj2 && (!obj1 || !obj1.length)) return true;
            if ((!obj1 || !obj1.length) && obj2 && obj2.length >= 2 && obj2.slice(0, 2) === "__") return true;
            if ((!obj2 || !obj2.length) && obj1 && obj1.length >= 2 && obj1.slice(0, 2) === "__") return true;
            return obj1 === obj2;
        },

        fieldChanged: (fields, original, name) => {
            const current = fields[name];
            const was = original[name];
            return Array.isArray(current) ? !moqui.arraysEqual(current, was, true) : !moqui.equalsOrPlaceholder(current, was);
        },

        fieldValuesDiff: (fields, fieldsOriginal) => Object.keys(fields).some((name) => moqui.fieldChanged(fields, fieldsOriginal, name)),

        // ---- URLs ----

        // A list becomes repeated names. Names and values are encoded, and null and undefined are left out.
        objToSearch: (obj) => {
            if (!moqui.isPlainObject(obj)) return "";
            const parts = [];
            Object.keys(obj).forEach((key) => {
                const value = obj[key];
                (Array.isArray(value) ? value : [value]).forEach((item) => {
                    if (item === null || item === undefined) return;
                    parts.push(encodeURIComponent(key) + "=" + encodeURIComponent(item));
                });
            });
            return parts.join("&");
        },

        searchToObj: (search) => {
            if (!search || search.length === 0) return {};
            const decode = (text) => {
                try { return decodeURIComponent(text.replace(/\+/g, " ")); } catch (err) { return text; }
            };
            const out = {};
            search.split("&").forEach((parm) => {
                const eq = parm.indexOf("=");
                if (eq < 1) return;
                const key = decode(parm.slice(0, eq));
                const value = decode(parm.slice(eq + 1));
                const existing = out[key];
                if (existing === undefined) out[key] = value;
                else if (Array.isArray(existing)) existing.push(value);
                else out[key] = [existing, value];
            });
            return out;
        },

        parseHref: (href) => {
            const result = { protocol: "", host: "", path: "/", query: {}, search: "", hash: "", name: "" };
            const ssIdx = href.indexOf("://");
            if (ssIdx >= 0) {
                result.protocol = href.slice(0, ssIdx);
                const slIdx = href.indexOf("/", ssIdx + 3);
                if (slIdx === -1) {
                    if (href.length > ssIdx + 3) result.host = href.slice(ssIdx + 3);
                    return result;
                }
                result.host = href.slice(ssIdx + 3, slIdx);
                href = href.slice(slIdx);
            }
            const splitHash = href.split("#");
            if (splitHash.length > 1 && splitHash[1].length) {
                result.hash = splitHash[1];
                href = splitHash[0];
            }
            const splitQuery = href.split("?");
            if (splitQuery.length > 1 && splitQuery[1].length) {
                result.search = splitQuery.slice(1).join("?");
                result.query = moqui.searchToObj(result.search);
            }
            result.path = splitQuery[0];
            if (result.path.length) {
                const lslIdx = result.path.lastIndexOf("/");
                result.name = lslIdx === -1 ? result.path : result.path.slice(lslIdx + 1);
            }
            return result;
        },

        makeHref: (urlInfo) => {
            let href = "";
            if (urlInfo.protocol && urlInfo.protocol.length) href += urlInfo.protocol + "://";
            if (urlInfo.host && urlInfo.host.length) href += urlInfo.host;
            href += urlInfo.path || "/";
            if (urlInfo.search && urlInfo.search.length) href += "?" + urlInfo.search;
            else if (urlInfo.query) {
                const queryStr = moqui.objToSearch(urlInfo.query);
                if (queryStr && queryStr.length) href += "?" + queryStr;
            }
            if (urlInfo.hash && urlInfo.hash.length) href += "#" + urlInfo.hash;
            return href;
        },

        // ---- text ----

        htmlEncode: (value) => String(value === null || value === undefined ? "" : value)
            .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;"),

        htmlDecode: (value) => {
            const text = String(value === null || value === undefined ? "" : value);
            if (typeof root.DOMParser === "function") {
                return new root.DOMParser().parseFromString("<!doctype html><body>" + text, "text/html").body.textContent;
            }
            return text.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, "&");
        },

        // ---- numbers ----

        decimalSeparator: ".",
        thousandSeparator: ",",
        isStringInteger: (value) => {
            if (!moqui.isString(value)) return false;
            for (let i = 0; i < value.length; i++) {
                const ch = value[i];
                if (ch >= "0" && ch <= "9") continue;
                if (i === 0 && ch === "-") continue;
                if (ch === moqui.thousandSeparator) continue;
                return false;
            }
            return true;
        },
        isStringNumber: (value) => {
            if (!moqui.isString(value)) return false;
            for (let i = 0; i < value.length; i++) {
                const ch = value[i];
                if (ch >= "0" && ch <= "9") continue;
                if (i === 0 && ch === "-") continue;
                if (ch === moqui.thousandSeparator || ch === moqui.decimalSeparator) continue;
                return false;
            }
            return true;
        },
        parseNumber: (value) => {
            const replValue = String(value).split(moqui.thousandSeparator).join("");
            return replValue.indexOf(moqui.decimalSeparator) === -1 ? parseInt(replValue, 10) : parseFloat(replValue.replace(moqui.decimalSeparator, "."));
        },
        setNumberSeparators: (locale) => {
            try {
                const parts = new Intl.NumberFormat(locale).formatToParts(1000.5);
                parts.forEach((part) => {
                    if (part.type === "group") moqui.thousandSeparator = part.value;
                    if (part.type === "decimal") moqui.decimalSeparator = part.value;
                });
            } catch (err) { /* the defaults stay */ }
        },

        // Format with the formatter of MoquiL10n.js, when it is loaded. The Java pattern subset, not moment tokens.
        format: (value, format, type) => (moqui.l10n ? moqui.l10n.format(value, format, type) : value === null || value === undefined ? "" : String(value)),

        debounce: (func, wait) => {
            let timeout;
            return function (...args) {
                clearTimeout(timeout);
                timeout = setTimeout(() => { timeout = null; func.apply(this, args); }, wait);
            };
        },

        // ---- scripts and styles ----

        // Load a script once. The callback gets (error, script) as in the Java client, and a promise is returned too. A validate
        // function says whether the library has set its global yet; it is retried with a growing wait, and the promise rejects
        // when it never does. One load runs per src (moqui.scriptLoads): a call while it runs waits for it, and when it fails
        // (the script does not load) every waiter is rejected and the failed element removed, so a later call tries again.
        loadScript: (src, callback, validate) => {
            const doc = root.document;
            let load = moqui.scriptLoads.get(src);
            if (!load) {
                load = new Promise((resolve, reject) => {
                    // a script the page wrote itself (not one of these loads) has run before the client starts
                    const present = Array.from(doc.querySelectorAll("script[src]")).find((item) =>
                        item.getAttribute("src").indexOf(src) !== -1 && !item.dataset.mqState);
                    if (present) {
                        resolve(present);
                        return;
                    }
                    const script = doc.createElement("script");
                    script.src = src;
                    script.async = false;
                    script.dataset.mqState = "loading";
                    script.onload = () => { script.dataset.mqState = "loaded"; script.onerror = script.onload = null; resolve(script); };
                    script.onerror = () => {
                        script.onerror = script.onload = null;
                        script.dataset.mqState = "failed";
                        if (script.parentNode) script.parentNode.removeChild(script);
                        reject(new Error("Error loading script " + src));
                    };
                    doc.head.appendChild(script);
                });
                moqui.scriptLoads.set(src, load);
                load.catch((error) => {
                    console.error(error);
                    if (moqui.scriptLoads.get(src) === load) moqui.scriptLoads.delete(src);
                });
            }
            const ready = validate ? load.then((script) => new Promise((resolve, reject) => moqui.retryValidateCallback(() => resolve(script), validate, 1,
                () => reject(new Error("Script " + src + " loaded but its library is not there"))))) : load;
            if (callback) ready.then((script) => callback(null, script), (error) => callback(error, null));
            return ready;
        },
        // src -> the promise of its load, while it runs or once it loaded
        scriptLoads: new Map(),

        // Call callback once validate() is true, trying again with a growing wait; after the last try (about 20 s) call onFail.
        retryValidateCallback: (callback, validate, count, onFail) => {
            if (!validate || validate()) callback();
            else {
                const attempt = count || 1;
                if (attempt <= 8) setTimeout(moqui.retryValidateCallback, attempt * attempt * 100, callback, validate, attempt + 1, onFail);
                else if (onFail) onFail();
            }
        },

        loadStylesheet: (href, rel, type) => {
            const doc = root.document;
            const loaded = Array.from(doc.querySelectorAll("link[href]")).some((item) => item.getAttribute("href").indexOf(href) !== -1);
            if (loaded) return;
            const link = doc.createElement("link");
            link.href = href;
            link.rel = rel || "stylesheet";
            link.type = type || "text/css";
            doc.head.appendChild(link);
        },

        // Run the text of an m-script. A script that needs a library that is still loading is tried again a few times.
        retryInlineScript: (src, count) => {
            try {
                (0, eval)(src);
            } catch (e) {
                const text = src.trim();
                const retryTime = count <= 5 ? count * count * 100 : -1;
                console.warn("inline script error " + count + " retry " + retryTime + " script: " + text.slice(0, 80) + "...");
                console.warn(e);
                if (count <= 5) setTimeout(moqui.retryInlineScript, retryTime, text, count + 1);
            }
        },

        // ---- requests ----

        sessionToken: "",
        appRootPath: "",

        // fetch with the session cookie and token. params is an object (a FormData, a string or a plain object). Resolves to
        // { ok, status, headers, text, json } and rejects with the same shape plus error on a network failure.
        request: (url, options) => {
            const opts = options || {};
            const method = (opts.method || "GET").toUpperCase();
            const headers = Object.assign({ Accept: opts.accept || "application/json" }, opts.headers);
            if (method !== "GET" && moqui.sessionToken && !headers["X-CSRF-Token"]) headers["X-CSRF-Token"] = moqui.sessionToken;
            const init = { method, headers, credentials: "same-origin", signal: opts.signal };
            let target = url;
            const data = opts.params;
            if (data instanceof root.FormData) init.body = data;
            else if (typeof data === "string") {
                init.body = data;
                headers["Content-Type"] = "application/x-www-form-urlencoded";
            } else if (moqui.isPlainObject(data)) {
                const search = moqui.objToSearch(data);
                if (method === "GET") target = url + (search ? (url.indexOf("?") > 0 ? "&" : "?") + search : "");
                else {
                    init.body = search;
                    headers["Content-Type"] = "application/x-www-form-urlencoded";
                }
            }
            return root.fetch(target, init).then((response) => response.text().then((text) => {
                let json;
                if (text && /^\s*[[{]/.test(text)) {
                    try { json = JSON.parse(text); } catch (err) { json = undefined; }
                }
                const result = { ok: response.ok, status: response.status, statusText: response.statusText, headers: response.headers, text, json, response };
                const token = response.headers.get("X-CSRF-Token");
                if (token && moqui.onSessionToken) moqui.onSessionToken(token);
                // the user preferences the request set (percent-encoded JSON), which the page applies (moqui.applyPreference)
                const preferences = response.headers.get("X-Moqui-Preferences");
                if (preferences && moqui.onPreferences) {
                    try { moqui.onPreferences(JSON.parse(decodeURIComponent(preferences))); } catch (err) { console.info("X-Moqui-Preferences not read", err); }
                }
                if (!response.ok) throw result;
                return result;
            }), (error) => {
                if (error && error.name === "AbortError") throw { aborted: true, status: 0, error };
                throw { ok: false, status: 0, error, text: "" };
            });
        },

        // ---- misc DOM ----

        // Set the columns of the tables under an element to the same widths, as a share of the widest cells.
        makeColumnsConsistent: (outerId) => {
            const doc = root.document;
            const tables = Array.from(doc.querySelectorAll("#" + outerId + " table"));
            const firstRow = (table) => {
                if (!table.rows || table.rows.length === 0) return null;
                let index = 0;
                let row = table.rows[0];
                while (index < 5 && index < table.rows.length) {
                    if ((!row.cells || row.cells.length <= 1) && table.rows.length > index + 1) {
                        index++;
                        row = table.rows[index];
                    } else break;
                }
                return row.cells && row.cells.length ? row : null;
            };
            const maxWidths = [];
            tables.forEach((table) => {
                const row = firstRow(table);
                if (!row) return;
                Array.from(row.cells).forEach((cell, j) => {
                    const width = cell.getBoundingClientRect().width;
                    if (!maxWidths[j] || maxWidths[j] < width) maxWidths[j] = width;
                });
            });
            const total = maxWidths.reduce((sum, width) => sum + width, 0);
            if (!total) return;
            tables.forEach((table) => {
                const row = firstRow(table);
                if (!row) return;
                Array.from(row.cells).forEach((cell, j) => { cell.style.width = (maxWidths[j] * 100 / total) + "%"; });
            });
        },

        downloadData: (data, filename, type) => {
            const doc = root.document;
            const url = root.URL.createObjectURL(new root.Blob([data], { type }));
            const a = doc.createElement("a");
            a.href = url;
            a.download = filename;
            doc.body.appendChild(a);
            a.click();
            setTimeout(() => { doc.body.removeChild(a); root.URL.revokeObjectURL(url); }, 0);
        },

        // A stand-in for the define of a module loader, as the Java client had: the last function or value is the module.
        define: (name, deps, callback) => {
            let cb = callback;
            if (!moqui.isString(name)) cb = deps === undefined ? name : deps;
            if (!moqui.isArray(cb) && !moqui.isFunction(cb)) return cb;
            return moqui.isFunction(cb) ? cb() : undefined;
        },

        // The component object a qjs response holds: an object expression, define(...), or statements that return one.
        evalComponentJs: (text) => {
            const source = String(text).trim().replace(/;\s*$/, "");
            try {
                return new Function("define", "return (" + source + "\n);")(moqui.define);
            } catch (err) {
                if (!(err instanceof SyntaxError)) throw err;
                return new Function("define", source)(moqui.define);
            }
        },

        // The parts of a component file: template, script and style. The script is the body of a function that assigns
        // module.exports, or says export default, or returns the object; define(...) is also understood. Style is added to the page once.
        parseVue: (text, styleSink) => {
            const part = (tag) => {
                const open = new RegExp("<" + tag + "(\\s[^>]*)?>", "i").exec(text);
                if (!open) return null;
                const start = open.index + open[0].length;
                const close = tag === "template" ? text.lastIndexOf("</template>") : text.toLowerCase().indexOf("</" + tag + ">", start);
                if (close < start) return null;
                return { attrs: open[1] || "", body: text.slice(start, close) };
            };
            const out = {};
            const template = part("template");
            if (template) out.template = template.body;
            const script = part("script");
            if (script && script.body.trim()) {
                let source = script.body.trim();
                source = source.replace(/^export\s+default\s+/, "return ").replace(/^define\s*\(/, "return define(");
                const define = moqui.define;
                const module = { exports: {} };
                const returned = new Function("module", "exports", "define", source)(module, module.exports, define);
                const exported = returned !== undefined ? returned : module.exports;
                if (moqui.isPlainObject(exported)) Object.assign(out, exported);
            }
            const style = part("style");
            if (style && style.body.trim() && styleSink) styleSink(style.body, /\bscoped\b/.test(style.attrs));
            return out;
        },

        // ---- notifications ----

        // A desktop notification when the browser allows it, else the fallback (the page toast).
        notifyNotification: (jsonObj, fallback) => {
            if (!jsonObj) return;
            const Notification = root.Notification;
            const options = {};
            if (jsonObj.topic && jsonObj.topic.length) options.tag = jsonObj.topic;
            const show = () => {
                const notif = new Notification(jsonObj.title, options);
                if (jsonObj.link && jsonObj.link.length) notif.onclick = () => root.open(jsonObj.link);
                if (moqui.webrootVue) moqui.webrootVue.addNotify(jsonObj.title, jsonObj.type);
            };
            if (Notification && Notification.permission === "granted") show();
            else if (Notification && Notification.permission !== "denied") {
                Notification.requestPermission().then((status) => { if (status === "granted") show(); else fallback(jsonObj); });
            } else fallback(jsonObj);
        },

        // Overridden by WebrootVue.qvt.js with a Quasar notify.
        notifyGrowl: (jsonObj) => {
            if (!jsonObj) return;
            console.info((jsonObj.type || "info") + ": " + jsonObj.title);
        },

        // A WebSocket for the topics a page listens to. It connects when the first listener is registered, and
        // only when an endpoint is configured: the object is not made without one. onState(fn) calls fn("open") or
        // fn("closed") as the socket opens and closes (a chat store sends what waited, and reads what it missed); the
        // browser coming back online reopens a closed socket at once, with its tries counted again. send(message)
        // writes an object as JSON (the frames of m-coedit); while the socket is not open it waits, the last 200 of
        // them, and goes after the subscribe of the next open.
        NotificationClient: function (webSocketUrl) {
            this.displayEnable = true;
            this.webSocketUrl = webSocketUrl;
            this.topicListeners = {};
            this.stateListeners = [];
            this.sendQueue = [];
            this.tryReopenCount = 0;
            this.disableDisplay = () => { this.displayEnable = false; };
            this.enableDisplay = () => { this.displayEnable = true; };
            this.isOpen = () => !!this.webSocket && this.webSocket.readyState === root.WebSocket.OPEN;
            // Calls fn(state) for each change, "open" or "closed"; returns a function that removes it.
            this.onState = (fn) => {
                this.stateListeners.push(fn);
                return () => { const index = this.stateListeners.indexOf(fn); if (index >= 0) this.stateListeners.splice(index, 1); };
            };
            const tellState = (state) => this.stateListeners.slice().forEach((fn) => {
                try { fn(state, this); } catch (err) { console.error("notification state listener failed", err); }
            });
            this.initWebSocket = () => {
                const socket = new root.WebSocket(this.webSocketUrl);
                this.webSocket = socket;
                socket.onopen = () => {
                    this.tryReopenCount = 0;
                    socket.send("subscribe:" + Object.keys(this.topicListeners).join(","));
                    this.sendQueue.splice(0).forEach((text) => socket.send(text));
                    tellState("open");
                };
                socket.onmessage = (event) => {
                    let jsonObj;
                    try { jsonObj = JSON.parse(event.data); } catch (err) { return; }
                    // copies, so a listener that unregisters itself (or another) while called skips none
                    (this.topicListeners[jsonObj.topic] || []).slice().forEach((callback) => callback(jsonObj, this));
                    (this.topicListeners.ALL || []).slice().forEach((callback) => callback(jsonObj, this));
                };
                socket.onclose = () => {
                    // a socket replaced by a reopen is not the client's state
                    if (this.webSocket === socket) tellState("closed");
                    setTimeout(() => this.tryReopen(), 30 * 1000);
                };
                socket.onerror = (event) => { console.log(event); };
            };
            this.tryReopen = () => {
                const socket = this.webSocket;
                if ((!socket || socket.readyState === root.WebSocket.CLOSED || socket.readyState === root.WebSocket.CLOSING) && this.tryReopenCount < 6) {
                    this.tryReopenCount++;
                    this.initWebSocket();
                }
            };
            // back online: a socket that was made and closed opens now, with its six tries again
            if (root.addEventListener) root.addEventListener("online", () => {
                if (!this.webSocket) return;
                this.tryReopenCount = 0;
                this.tryReopen();
            });
            this.displayNotify = (jsonObj) => {
                if (!this.displayEnable) return;
                if (jsonObj.title && jsonObj.showAlert === true) moqui.notifyNotification(jsonObj, moqui.notifyGrowl);
            };
            // Sends message as JSON text; true when it went now, false when it waits for the socket to open (the oldest
            // of more than 200 waiting is dropped). The first send without a socket opens one.
            this.send = (message) => {
                const text = JSON.stringify(message);
                if (this.isOpen()) { this.webSocket.send(text); return true; }
                this.sendQueue.push(text);
                if (this.sendQueue.length > 200) this.sendQueue.shift();
                if (!this.webSocket) this.initWebSocket();
                return false;
            };
            // Calls callback(message, client) for each message of the topic (ALL: every message); no callback is the
            // alert display. Returns a function that unregisters it, as unregisterListener(topic, callback) does.
            this.registerListener = (topic, callback) => {
                if (!this.webSocket) this.initWebSocket();
                const listener = callback || this.displayNotify;
                let list = this.topicListeners[topic];
                if (!list) {
                    list = [];
                    this.topicListeners[topic] = list;
                    if (this.webSocket.readyState === root.WebSocket.OPEN) this.webSocket.send("subscribe:" + topic);
                }
                if (list.indexOf(listener) < 0) list.push(listener);
                return () => this.unregisterListener(topic, listener);
            };
            // Removes a listener registerListener added (the same function; none is the alert display). With the topic's
            // last listener gone the socket stops asking for the topic. True when the listener was there.
            this.unregisterListener = (topic, callback) => {
                const listener = callback || this.displayNotify;
                const list = this.topicListeners[topic];
                const index = list ? list.indexOf(listener) : -1;
                if (index < 0) return false;
                list.splice(index, 1);
                if (!list.length) {
                    delete this.topicListeners[topic];
                    const socket = this.webSocket;
                    if (socket && socket.readyState === root.WebSocket.OPEN) socket.send("unsubscribe:" + topic);
                }
                return true;
            };
        },

        BasicWebSocketClient: function (webSocketUrl, onDataFunction) {
            this.webSocketUrl = webSocketUrl;
            this.onDataFunction = onDataFunction;
            this.tryReopenCount = 0;
            this.initWebSocket = () => {
                const socket = new root.WebSocket(this.webSocketUrl);
                this.webSocket = socket;
                socket.onopen = () => { this.tryReopenCount = 0; };
                socket.onmessage = (event) => { this.onDataFunction(event.data); };
                socket.onclose = () => { setTimeout(() => this.tryReopen(), 30 * 1000); };
                socket.onerror = (event) => { console.log(event); };
            };
            this.tryReopen = () => {
                const socket = this.webSocket;
                if ((!socket || socket.readyState === root.WebSocket.CLOSED || socket.readyState === root.WebSocket.CLOSING) && this.tryReopenCount < 6) {
                    this.tryReopenCount++;
                    this.initWebSocket();
                }
            };
            this.initWebSocket();
        },

        // ---- OAuth sign-in popup ----

        // A mail account's sign-in with Google or Microsoft in a popup (org.moqui.impl.EmailClientServices
        // start#OAuthSignIn, docs/Mail-OAuth.md). Call it in the click handler: the window opens at once, so a popup
        // blocker lets it through, then start() (the app's request to start#OAuthSignIn, a promise of {authUrl,
        // signInId}) points it at the provider. The outcome is the one the callback page sends: a message from this
        // page's own origin and from that window, or the same on the BroadcastChannel (a provider page can cut
        // window.opener), else what status(signInId) (the app's request to get#OAuthStatus) says, asked every
        // statusPollMs while the sign-in runs and at once when the window closes: the first answer that is not
        // pending ends the wait. A window cut off from this page (Google's page sets Cross-Origin-Opener-Policy, so it
        // reads as closed at once) is waited for cutOffWaitMs at most (3 minutes; maxWaitMs is the older name), then
        // the result is cancelled with errorCode timedOut and the window is closed.
        // Resolves with {status: "success" | "error" | "cancelled", errorCode, message, emailAccountId, emailAddress,
        // expectedAddress, signInId}; no token ever reaches the page. The promise has cancel(), which closes the
        // window and resolves it cancelled (as an aborted signal does). options: width, height, pollMs (the check of
        // the window), statusPollMs, closedWaitMs, cutOffMs, cutOffWaitMs, signal (an AbortSignal that cancels).
        oauthPopup: (start, status, options) => {
            const opts = options || {};
            const width = opts.width || 520, height = opts.height || 680;
            const left = Math.max(0, Math.round((root.screenX || 0) + ((root.outerWidth || width) - width) / 2));
            const top = Math.max(0, Math.round((root.screenY || 0) + ((root.outerHeight || height) - height) / 2));
            const popup = root.open("", "moquiOAuth", "popup=yes,width=" + width + ",height=" + height + ",left=" + left + ",top=" + top);
            const origin = root.location && root.location.origin;
            const FIELDS = ["status", "errorCode", "message", "emailAccountId", "emailAddress", "expectedAddress", "signInId"];
            const pick = (data) => {
                const out = {};
                FIELDS.forEach((name) => { out[name] = data && data[name] != null && data[name] !== "" ? String(data[name]) : null; });
                if (!["success", "error", "cancelled"].includes(out.status)) out.status = "error";
                return out;
            };
            let cancel = () => {};
            const promise = new Promise((resolve) => {
                let signInId = null, done = false, timer = null, channel = null, openedAt = 0, closedAt = 0, asking = false, askedAt = 0;
                let cutOff = false;
                const statusPollMs = opts.statusPollMs ?? 2000;
                const cutOffWaitMs = opts.cutOffWaitMs ?? opts.maxWaitMs ?? 180000;
                const finish = (result, closeWindow) => {
                    if (done) return;
                    done = true;
                    if (timer) root.clearInterval(timer);
                    root.removeEventListener("message", onMessage);
                    if (channel) { try { channel.close(); } catch (err) { /* closed already */ } }
                    if (closeWindow && popup) { try { popup.close(); } catch (err) { /* gone */ } }
                    resolve(result);
                };
                const accept = (data) => {
                    if (done || !signInId || !data || data.type !== "moqui-email-oauth" || data.signInId !== signInId) return;
                    finish(pick(data), false);
                };
                const onMessage = (event) => {
                    if (!origin || event.origin !== origin) return;
                    if (popup && event.source !== popup) return;
                    accept(event.data);
                };
                root.addEventListener("message", onMessage);
                if (typeof root.BroadcastChannel === "function") {
                    try {
                        channel = new root.BroadcastChannel("moqui-email-oauth");
                        channel.onmessage = (event) => accept(event.data);
                    } catch (err) { channel = null; }
                }
                cancel = () => finish({ status: "cancelled", errorCode: "cancelled", message: null, emailAccountId: null,
                    emailAddress: null, expectedAddress: null, signInId }, true);
                if (opts.signal) opts.signal.addEventListener("abort", cancel);
                if (!popup) {
                    finish(pick({ status: "error", errorCode: "popupBlocked", message: "The browser blocked the sign-in window. Allow pop-ups for this site and try again." }), false);
                    return;
                }
                const check = () => {
                    if (done || asking) return;
                    let closed;
                    try { closed = popup.closed; } catch (err) { closed = true; }
                    const now = Date.now();
                    // a window cut off from this page reads as closed at once; then only the status says when it ends
                    if (closed && !closedAt) { closedAt = now; cutOff = now - openedAt < (opts.cutOffMs ?? 2000); }
                    const ended = closed && !cutOff;
                    // the status every statusPollMs while the sign-in runs, at each check once the window closed
                    if (!ended && askedAt && now - askedAt < statusPollMs) return;
                    if (!ended && !askedAt && now - openedAt < statusPollMs) return;
                    asking = true;
                    askedAt = now;
                    // no answer that ends it (pending, or the request failed): the waits
                    const waited = () => {
                        if (done) return;
                        if (ended && Date.now() - closedAt > (opts.closedWaitMs ?? 3000)) {
                            finish(pick({ status: "cancelled", errorCode: "closed", message: "The sign-in window was closed.", signInId }), false);
                        } else if (cutOff && Date.now() - openedAt > cutOffWaitMs) {
                            finish(pick({ status: "cancelled", errorCode: "timedOut", signInId,
                                message: "The sign-in window did not answer in time. If you finished signing in, check the account; else try again." }), true);
                        }
                    };
                    Promise.resolve().then(() => status(signInId)).then((answer) => {
                        asking = false;
                        if (done) return;
                        if (answer && answer.status && answer.status !== "pending") finish(pick(answer), false);
                        else waited();
                    }, () => { asking = false; waited(); });
                };
                Promise.resolve().then(start).then((started) => {
                    if (done) return;
                    const url = started && started.authUrl ? String(started.authUrl) : "";
                    if (!started || !started.signInId || !/^https?:\/\//i.test(url)) {
                        finish(pick({ status: "error", errorCode: "startFailed", message: (started && (started.errors || started.message)) || "The sign-in could not start." }), true);
                        return;
                    }
                    signInId = String(started.signInId);
                    openedAt = Date.now();
                    popup.location.href = url;
                    timer = root.setInterval(check, opts.pollMs || 1000);
                }, (err) => {
                    finish(pick({ status: "error", errorCode: "startFailed", message: (err && err.message) || String(err) }), true);
                });
            });
            promise.cancel = () => cancel();
            return promise;
        },

        // A map that keeps the most recently used entries. The end of the list is the least recently used.
        LruMap: function (limit) {
            this.limit = limit;
            this.valueMap = new Map();
            this.lruList = [];
            const used = (key) => {
                const index = this.lruList.indexOf(key);
                if (index >= 0) this.lruList.splice(index, 1);
                this.lruList.unshift(key);
            };
            this.put = (key, value) => {
                this.valueMap.set(key, value);
                used(key);
                while (this.lruList.length > this.limit) this.valueMap.delete(this.lruList.pop());
            };
            this.get = (key) => {
                const value = this.valueMap.get(key);
                if (value) used(key);
                return value;
            };
            this.remove = (key) => {
                this.valueMap.delete(key);
                const index = this.lruList.indexOf(key);
                if (index >= 0) this.lruList.splice(index, 1);
            };
            this.containsKey = (key) => !!this.valueMap.get(key);
        }
    });
});
