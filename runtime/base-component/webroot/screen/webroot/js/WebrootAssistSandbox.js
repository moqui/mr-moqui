/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The frame runtime of a Vue SFC canvas (plans/LLM-A2A-Assist.md 4.10; docs/Assist.md "Vue SFC canvases"). It runs INSIDE
 * the frame document GET /assist/sandbox serves, in <iframe sandbox="allow-scripts">: an opaque origin, so nothing here
 * reaches the page's DOM, cookies, session token or storage, and the document's CSP leaves it no network (connect-src
 * 'none'), no form, no frame and no worker. Everything here is as untrusted as the component it runs: the parent
 * (moqui.assist.canvas.sfc in WebrootAssistCanvas.js) validates every message, and nothing a message says is believed
 * beyond its shape.
 *
 * The one way out is a MessageChannel port the parent transfers after the frame loads, with the nonce of the frame's URL
 * fragment (#n=…), accepted once and only from window.parent. Messages (small JSON):
 *   parent -> frame:  init {template, script, style, values, dark, external}   values {values}   theme {dark}
 *                     result {id, ok, json, status, error}
 *   frame -> parent:  ready   rendered   ping (every 2 s)   input {values}   get {id, path, query, list}
 *                     submit {id, action, values}   link {href}   resize {height}   error {phase, code, message}
 *                     leaving (the document is going away: a navigation)
 *
 * The component is compiled here with Vue 3's full build (its template compiler needs 'unsafe-eval', which only this
 * frame's policy has) and drawn with Quasar 2 and six widget shims over the bridge: this.$bridge.get(path, query, list)
 * reads through the parent and the read proxy; this.$bridge.submit(name, values) asks the parent to confirm an action of
 * the program (the parent shows what it saves outside the frame and the user's click there runs it on the server). A click
 * on a link never navigates the frame: the parent opens an /apps screen beside the canvas, another site only when the user
 * allows it. The component's own code cannot be stopped from navigating the frame in every way; when it does, "leaving"
 * goes out and the parent's watchdog closes the frame on its next load and records it.
 */
(function () {
    "use strict";

    const Vue = window.Vue;
    const Quasar = window.Quasar;
    const found = /(?:^#|&)n=([0-9a-f]{32})(?:&|$)/.exec(window.location.hash || "");
    const nonce = found ? found[1] : null;
    let port = null;
    let app = null;
    let mounting = false;
    let nextId = 1;
    const pending = {};
    // the values as the parent last sent them, so a change the component makes is sent once and an echo is not
    let snapshot = {};
    const state = Vue ? Vue.reactive({ values: {} }) : { values: {} };

    const send = (message) => { if (port) { try { port.postMessage(message); } catch (err) { /* closed */ } } };
    const plain = (value) => { try { return value === undefined ? null : JSON.parse(JSON.stringify(value)); } catch (err) { return null; } };
    const text = (err) => { const out = String((err && err.message) || err || ""); return out.length > 2000 ? out.slice(0, 2000) : out; };
    const report = (phase, err, code) => send({ t: "error", phase, code: code || phase, message: text(err) });

    window.addEventListener("error", (event) => report(mounting ? "compile" : "runtime", event.error || event.message));
    window.addEventListener("unhandledrejection", (event) => report("runtime", event.reason || "a promise failed"));
    window.addEventListener("pagehide", () => send({ t: "leaving" }));
    // a link is the parent's to open: never a navigation of this frame
    const onLink = (event) => {
        const anchor = event.target && event.target.closest ? event.target.closest("a[href]") : null;
        if (!anchor) return;
        event.preventDefault();
        // only a real click of the user's opens a link; a script-generated one is ignored (the parent also gates it)
        if (!event.isTrusted) return;
        const href = anchor.getAttribute("href") || "";
        if (href.charAt(0) !== "#") send({ t: "link", href: anchor.href });
    };
    document.addEventListener("click", onLink, true);
    document.addEventListener("auxclick", onLink, true);

    window.addEventListener("message", function onPort(event) {
        if (port || event.source !== window.parent) return;
        const data = event.data;
        if (!nonce || !data || data.t !== "port" || data.nonce !== nonce || !event.ports || !event.ports[0]) return;
        port = event.ports[0];
        port.onmessage = onPortMessage;
        window.removeEventListener("message", onPort);
        send({ t: "ready" });
        setInterval(() => send({ t: "ping" }), 2000);
    });

    function onPortMessage(event) {
        const message = event.data;
        if (!message || typeof message.t !== "string") return;
        if (message.t === "init") mount(message);
        else if (message.t === "values" && message.values && typeof message.values === "object") {
            snapshot = plain(message.values) || {};
            state.values = message.values;
        } else if (message.t === "theme") {
            if (Quasar && Quasar.Dark) Quasar.Dark.set(!!message.dark);
        } else if (message.t === "result") settle(message);
    }

    // ---- the bridge ----

    function call(message, timeout) {
        return new Promise((resolve, reject) => {
            const id = "r" + (nextId++);
            const timer = setTimeout(() => { delete pending[id]; reject(new Error("the page did not answer")); }, timeout);
            pending[id] = { resolve, reject, timer, kind: message.t };
            send(Object.assign({ id }, message));
        });
    }
    function settle(message) {
        const entry = pending[message.id];
        if (!entry) return;
        clearTimeout(entry.timer);
        delete pending[message.id];
        if (entry.kind === "submit") entry.resolve(message.json || { status: message.ok ? "saved" : "failed", error: message.error || null });
        else if (message.ok) entry.resolve(message.json);
        else entry.reject(new Error(message.error || ("the read failed" + (message.status ? " (" + message.status + ")" : ""))));
    }
    const bridge = {
        get(path, query, list) {
            return call({ t: "get", path: String(path || ""), query: plain(query) || {}, list: list ? String(list) : null }, 30000);
        },
        submit(name, values) {
            return call({ t: "submit", action: String(name || ""), values: plain(values) || {} }, 30 * 60 * 1000);
        },
    };

    // ---- the widgets: the six m- widgets a component may use, on Quasar 2, with their lookups over the bridge ----

    const optionList = (json, valueField, labelField) => {
        const list = Array.isArray(json) ? json : (json && (json.options || json.rows)) || [];
        return list.filter((item) => item && typeof item === "object").map((item) => {
            const value = item[valueField] !== undefined ? item[valueField] : item[labelField];
            const label = item[labelField] !== undefined ? item[labelField] : item[valueField];
            return { value, label: label === null || label === undefined ? "" : String(label) };
        });
    };
    function registerWidgets(app) {
        app.component("m-container-box", {
            props: { title: String, initialOpen: { type: Boolean, default: true } },
            data() { return { open: this.initialOpen }; },
            template: '<q-card flat bordered class="q-ma-xs"><q-card-section class="row items-center no-wrap q-py-sm">' +
                '<div v-if="title" class="text-subtitle1">{{title}}</div><q-space></q-space><slot name="toolbar"></slot>' +
                '<q-btn flat round dense size="sm" :icon="open ? \'expand_less\' : \'expand_more\'" @click="open = !open"></q-btn></q-card-section>' +
                '<q-card-section v-show="open" class="q-pt-none"><slot></slot></q-card-section></q-card>',
        });
        app.component("m-text-line", {
            props: { modelValue: [String, Number], label: String, type: { type: String, default: "text" }, name: String, id: String, tooltip: String, disable: Boolean },
            emits: ["update:modelValue"],
            template: '<q-input :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :label="label" :type="type" ' +
                ':name="name" :id="id" :disable="disable" dense outlined stack-label autocomplete="off" class="q-my-xs">' +
                '<q-tooltip v-if="tooltip">{{tooltip}}</q-tooltip></q-input>',
        });
        app.component("m-display", {
            props: { value: [String, Number], modelValue: [String, Number], label: String, name: String, id: String },
            computed: { shown() { const value = this.modelValue !== undefined ? this.modelValue : this.value; return value === null || value === undefined ? "" : String(value); } },
            template: '<q-input v-if="label" :model-value="shown" :label="label" :name="name" :id="id" dense outlined readonly stack-label class="q-my-xs"></q-input>' +
                '<span v-else :id="id">{{shown}}</span>',
        });
        app.component("m-drop-down", {
            props: { modelValue: [String, Number, Array], options: { type: Array, default: () => [] }, optionsUrl: String, optionsParameters: Object,
                valueField: { type: String, default: "value" }, labelField: { type: String, default: "label" }, serverSearch: Boolean,
                label: String, name: String, id: String, multiple: Boolean, allowEmpty: Boolean, disable: Boolean },
            emits: ["update:modelValue"],
            data() { return { shown: this.options.slice(), all: this.options.slice(), loading: false }; },
            mounted() { if (this.optionsUrl && !this.serverSearch) this.load(null).then((list) => { this.all = list; this.shown = list; }, () => {}); },
            methods: {
                load(term) {
                    const query = Object.assign({}, this.optionsParameters || {});
                    if (term !== null && term !== undefined) query.term = term;
                    this.loading = true;
                    return bridge.get(this.optionsUrl, query).then((json) => { this.loading = false; return optionList(json, this.valueField, this.labelField); },
                        (err) => { this.loading = false; throw err; });
                },
                filter(search, done, abort) {
                    if (this.optionsUrl && this.serverSearch) {
                        this.load(search || "").then((list) => done(() => { this.shown = list; this.all = list; }), () => abort());
                        return;
                    }
                    const needle = String(search || "").toLowerCase();
                    done(() => { this.shown = needle ? this.all.filter((option) => option.label.toLowerCase().indexOf(needle) >= 0) : this.all; });
                },
            },
            template: '<q-select :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :options="shown" :label="label" ' +
                ':name="name" :id="id" :multiple="multiple" :clearable="allowEmpty" :disable="disable" :loading="loading" dense outlined options-dense ' +
                'stack-label emit-value map-options use-input input-debounce="300" behavior="menu" @filter="filter" class="q-my-xs">' +
                '<template v-slot:no-option><q-item><q-item-section class="text-grey">No results</q-item-section></q-item></template></q-select>',
        });
        app.component("m-date-time", {
            props: { modelValue: String, type: { type: String, default: "date" }, label: String, name: String, id: String },
            emits: ["update:modelValue"],
            computed: {
                format() { return this.type === "time" ? "HH:mm" : this.type === "date-time" ? "YYYY-MM-DD HH:mm" : "YYYY-MM-DD"; },
                mask() { return this.format.replace(/\w/g, "#"); },
            },
            methods: { pick(value) { this.$emit("update:modelValue", value); if (this.$refs.proxy) this.$refs.proxy.hide(); } },
            template: '<q-input :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :label="label" :name="name" :id="id" ' +
                ':mask="mask" fill-mask dense outlined stack-label style="max-width:max-content" class="q-my-xs"><template v-slot:append>' +
                '<q-icon :name="type === \'time\' ? \'access_time\' : \'event\'" class="cursor-pointer"><q-popup-proxy ref="proxy">' +
                '<q-time v-if="type === \'time\'" :model-value="modelValue" @update:model-value="pick" :mask="format" format24h></q-time>' +
                '<q-date v-else :model-value="modelValue" @update:model-value="pick" :mask="format"></q-date>' +
                '</q-popup-proxy></q-icon></template></q-input>',
        });
        app.component("m-date-period", {
            props: { fields: { type: Object, required: true }, name: { type: String, required: true }, label: String },
            data() {
                return {
                    offsets: [["0", "This"], ["-1", "Last"], ["1", "Next"], ["-2", "-2"], ["2", "+2"], ["-3", "-3"]].map(([value, label]) => ({ value, label })),
                    periods: [["day", "Day"], ["7d", "7 Days"], ["30d", "30 Days"], ["week", "Week"], ["month", "Month"], ["quarter", "Quarter"], ["year", "Year"]]
                        .map(([value, label]) => ({ value, label })),
                };
            },
            methods: { set(suffix, value) { this.fields[this.name + suffix] = value; } },
            template: '<div class="row items-center q-gutter-xs q-my-xs">' +
                '<q-select :model-value="fields[name + \'_poffset\']" @update:model-value="set(\'_poffset\', $event)" :options="offsets" label="Offset" ' +
                'dense outlined options-dense emit-value map-options stack-label behavior="menu" style="min-width:90px"></q-select>' +
                '<q-select :model-value="fields[name + \'_period\']" @update:model-value="set(\'_period\', $event)" :options="periods" label="Period" ' +
                'dense outlined options-dense emit-value map-options stack-label behavior="menu" style="min-width:110px"></q-select>' +
                '<q-input :model-value="fields[name + \'_pdate\']" @update:model-value="set(\'_pdate\', $event)" :label="label" :name="name + \'_pdate\'" ' +
                'mask="####-##-##" fill-mask dense outlined stack-label style="max-width:150px"></q-input></div>',
        });
    }

    // ---- the component ----

    function changed(now) {
        const out = {};
        Object.keys(now).forEach((key) => { if (JSON.stringify(now[key]) !== JSON.stringify(snapshot[key])) out[key] = now[key]; });
        return out;
    }

    // WebRTC is not governed by connect-src, so it could leak data over STUN/TURN; remove it before the component runs
    try {
        ["RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel", "RTCSessionDescription", "RTCIceCandidate"].forEach((name) => {
            try { delete window[name]; } catch (err) { /* non-configurable */ }
            try { window[name] = undefined; } catch (err) { /* read-only */ }
        });
    } catch (err) { /* ignore */ }

    function mount(message) {
        if (app || !Vue) return;
        mounting = true;
        try {
            if (!Quasar) throw new Error("the sandbox libraries did not load");
            const module = { exports: {} };
            const script = String(message.script || "");
            try {
                if (script.trim()) new Function("module", "exports", script)(module, module.exports);
            } catch (err) {
                report("compile", err, "script");
                return;
            }
            const component = Object.assign({}, module.exports && typeof module.exports === "object" ? module.exports : {});
            component.template = String(message.template || "");
            if (message.style) {
                const style = document.createElement("style");
                style.textContent = String(message.style);
                document.head.appendChild(style);
            }
            snapshot = plain(message.values) || {};
            state.values = message.values && typeof message.values === "object" ? message.values : {};
            const onInput = (payload) => {
                if (!payload || typeof payload !== "object") return;
                if (typeof payload.name === "string" && Object.prototype.hasOwnProperty.call(payload, "value")) state.values[payload.name] = payload.value;
                else Object.keys(payload).forEach((key) => { state.values[key] = payload[key]; });
            };
            app = Vue.createApp({ render: () => Vue.h("div", { class: "q-pa-xs" }, [Vue.h(component, { values: state.values, onInput })]) });
            app.use(Quasar, {});
            if (Quasar.Dark) Quasar.Dark.set(!!message.dark);
            app.config.globalProperties.$bridge = bridge;
            app.config.errorHandler = (err) => report(mounting ? "compile" : "runtime", err, mounting && err && typeof err.code === "number" && err.loc ? "template" : null);
            registerWidgets(app);
            app.mount("#m-sfc-app");
            // a change the component made to the values (an emitted input, a widget that writes into them) goes out once
            Vue.watch(() => state.values, (now) => {
                const out = changed(plain(now) || {});
                if (Object.keys(out).length) { snapshot = plain(now) || {}; send({ t: "input", values: out }); }
            }, { deep: true });
            send({ t: "rendered" });
        } catch (err) {
            report("compile", err, err && typeof err.code === "number" && err.loc ? "template" : "script");
        } finally {
            mounting = false;
        }
        let last = 0;
        const measure = () => {
            const height = Math.ceil(document.documentElement.scrollHeight);
            if (height !== last) { last = height; send({ t: "resize", height }); }
        };
        if (window.ResizeObserver) new ResizeObserver(measure).observe(document.body);
        setInterval(measure, 500);
        measure();
    }
})();
