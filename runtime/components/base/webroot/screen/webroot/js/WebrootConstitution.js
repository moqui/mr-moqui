/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
/*
 * moqui.constitution: reading the Constitution in the browser (docs/ai/Constitution.md).
 *
 *   - m-constitution-view: a document for reading (title, provenance, where it comes from, each rule's id, title and
 *     description, its references), from the reading view of org.moqui.admin.ConstitutionServices (get#Constitution,
 *     get#ConstitutionSnapshot). System › Constitution and the Assist Rules view draw it.
 *   - m-reference-dialog: the trusted reference viewer. A reference opens only on a person's click, and is resolved again
 *     by the server at every open (/assist/constitutionReference: still in the document, still readable, its origin still
 *     trusted); the page is framed sandboxed (no top navigation, popups, forms or downloads), without a referrer, and
 *     unloaded on close, and focus returns to the link that opened it. A framed page that does not load cannot be told
 *     apart from one that does, so the dialog always offers to open it in a new tab.
 *   - m-assist-constitution: the Assist drawer's Constitution / Rules pane (moqui.assist), the rules of the active run
 *     while one runs, else the rules that apply now, with a reading language and Back to conversation.
 *
 * Render functions only (no templates), so the runtime build of Vue draws them. Loaded after WebrootAssist.js and
 * before WebrootVue.qvt.js (qapps.xml): the components go in moqui.qvtComponents, or through qvt.registerLate later.
 */
(function (root) {
    "use strict";
    const moqui = root.moqui = root.moqui || {};
    const constitution = moqui.constitution = {};
    const h = function () { return root.Vue.h.apply(null, arguments); };
    const R = (name) => root.Vue.resolveComponent(name);
    const T = (text) => (moqui.l10n && typeof moqui.l10n.localize === "function" ? moqui.l10n.localize(String(text)) : String(text));

    constitution.base = () => moqui.appRootPath || "";
    // a GET of /assist (signed-in readers; no Assist permission needed): resolves to its JSON
    constitution.get = (name, params) => {
        const clean = {};
        Object.keys(params || {}).forEach((key) => { if (params[key] !== null && params[key] !== undefined && params[key] !== "") clean[key] = params[key]; });
        return moqui.request(constitution.base() + "/assist/" + name, { method: "GET", params: clean }).then((result) => result.json || {});
    };
    constitution.load = (params) => constitution.get("constitution", params);

    // the state of one dialog (each view keeps its own)
    constitution.dialogState = () => {
        const state = { open: false, loading: false, reference: null, refused: null, title: "", opener: null };
        return root.Vue && root.Vue.reactive ? root.Vue.reactive(state) : state;
    };

    // open a reference: resolved by the server now, never from what the page holds
    constitution.openReference = (dialog, params, opener) => {
        dialog.opener = opener || (root.document ? root.document.activeElement : null);
        dialog.open = true;
        dialog.loading = true;
        dialog.reference = null;
        dialog.refused = null;
        dialog.title = params.title || "";
        return constitution.get("constitutionReference", params).then((json) => {
            if (!dialog.open) return;
            dialog.loading = false;
            if (json.allowed === "Y" && json.url) {
                dialog.reference = json;
                dialog.title = json.title || dialog.title;
            } else dialog.refused = json.message || T("This reference cannot be opened in the app.");
        }, () => {
            dialog.loading = false;
            dialog.refused = T("This reference could not be checked. Try again later.");
        });
    };
    constitution.closeReference = (dialog) => {
        dialog.open = false;
        dialog.reference = null;
        dialog.loading = false;
    };
    // after the dialog has hidden: focus back on the link that opened it
    constitution.restoreFocus = (dialog) => {
        const opener = dialog.opener;
        dialog.opener = null;
        if (opener && typeof opener.focus === "function" && root.document && root.document.contains(opener)) opener.focus();
    };
    // the references of a run (its snapshot), read once per run, for the links of its messages
    const runLoads = {};
    const runLists = {};
    constitution.runReferences = (runId) => {
        if (!runId) return Promise.resolve([]);
        if (!runLoads[runId]) {
            runLoads[runId] = constitution.load({ runId }).then((json) => {
                const list = [];
                (json.ruleList || []).forEach((rule) => (rule.referenceList || []).forEach((ref) => list.push(ref)));
                runLists[runId] = list;
                return list;
            }, () => { delete runLoads[runId]; return []; });
        }
        return runLoads[runId];
    };
    // a trusted reference of the run whose URL is exactly href, if its references are loaded
    constitution.knownReference = (runId, href) => (runLists[runId] || [])
        .find((ref) => ref.trusted === "Y" && (ref.url === href || constitution.href(ref.url) === href)) || null;
    // the one dialog of the chat's reference links (m-assist-body draws it, in the drawer or a float)
    let chat = null;
    constitution.chatDialog = () => chat || (chat = constitution.dialogState());
    // a reference URL for the browser: a path on this server under the app's root, an https URL as it is
    constitution.href = (url) => (String(url || "").startsWith("/") ? constitution.base() + url : String(url || ""));

    const components = {};

    components["m-reference-dialog"] = {
        name: "mReferenceDialog",
        props: { dialog: { type: Object, required: true } },
        render() {
            const dialog = this.dialog;
            const reference = dialog.reference;
            const body = [];
            if (dialog.loading) body.push(h("div", { class: "q-pa-md text-grey-7" }, T("Opening…")));
            else if (dialog.refused) body.push(h("div", { class: "q-pa-md text-warning", role: "alert" }, dialog.refused));
            else if (reference) {
                body.push(h("iframe", {
                    class: "m-reference-frame", src: constitution.href(reference.url), title: reference.title || reference.url,
                    sandbox: "allow-scripts allow-same-origin", referrerpolicy: "no-referrer", allow: "",
                    style: "width: 100%; height: 100%; border: 0; display: block;",
                }));
            }
            const foot = [];
            if (reference) {
                foot.push(h("span", { class: "text-caption text-grey-7 col" },
                    T("Not showing? The site may not allow being shown here.") + " "));
                foot.push(h("a", { href: constitution.href(reference.url), target: "_blank", rel: "noopener noreferrer", class: "text-caption q-mr-md" },
                    T("Open in a new tab")));
            } else foot.push(h("span", { class: "col" }));
            foot.push(h(R("q-btn"), { flat: true, dense: true, label: T("Close"), onClick: () => constitution.closeReference(dialog) }));
            return h(R("q-dialog"), {
                modelValue: dialog.open,
                "onUpdate:modelValue": (open) => { if (!open) constitution.closeReference(dialog); },
                onHide: () => constitution.restoreFocus(dialog),
                noRefocus: true,
                // the whole screen on a phone; else 90% of the height, 1000 px wide at most (css/WebrootVue.qvt.css)
                maximized: !!(this.$q && this.$q.screen && this.$q.screen.lt.sm),
            }, () => h(R("q-card"), { class: "m-reference-dialog column no-wrap" }, () => [
                h(R("q-card-section"), { class: "row items-center no-wrap q-pb-sm" }, () => [
                    h("div", { class: "col" }, [
                        h("div", { class: "text-subtitle1", id: "m-reference-dialog-title" }, dialog.title || T("Reference")),
                        reference && reference.origin ? h("div", { class: "text-caption text-grey-7" }, reference.origin) : null,
                    ]),
                    h(R("q-btn"), { flat: true, round: true, dense: true, icon: "sym_o_close", "aria-label": T("Close"),
                        onClick: () => constitution.closeReference(dialog) }),
                ]),
                h(R("q-card-section"), { class: "q-pa-none col m-reference-body" }, () => body),
                h(R("q-card-section"), { class: "row items-center q-pt-sm" }, () => foot),
            ]));
        },
    };

    components["m-constitution-view"] = {
        name: "mConstitutionView",
        props: {
            doc: { type: [Object, String], default: null },
            runId: { type: String, default: null },
            organizationPartyId: { type: String, default: null },
        },
        data() { return { dialog: constitution.dialogState() }; },
        computed: {
            document() {
                if (!this.doc) return null;
                if (typeof this.doc === "string") { try { return JSON.parse(this.doc); } catch (_) { return null; } }
                return this.doc;
            },
        },
        methods: {
            open(event, rule, ref) {
                event.preventDefault();
                constitution.openReference(this.dialog, { referenceId: ref.referenceId, runId: this.runId,
                    organizationPartyId: this.organizationPartyId, locale: this.document && this.document.locale, title: ref.title }, event.currentTarget);
            },
        },
        render() {
            const doc = this.document;
            if (!doc) return h("div", { class: "text-grey-7" }, T("No Constitution is available."));
            const locale = doc.locale;
            // a text from another language than the one read in says which (es for es_MX is not another)
            const from = (sourceLocale) => (sourceLocale && locale && sourceLocale !== locale && sourceLocale !== String(locale).split("_")[0]
                ? h("span", { class: "text-caption text-grey-7 q-ml-xs" }, "(" + sourceLocale + ")") : null);
            const rules = (doc.ruleList || []).map((rule) => h("section", { class: "m-constitution-rule q-mb-md", id: "rule-" + rule.ruleId,
                "aria-labelledby": "rule-title-" + rule.ruleId }, [
                h("h6", { class: "q-my-xs text-subtitle1", id: "rule-title-" + rule.ruleId }, [
                    h("span", { class: "text-weight-bold q-mr-sm" }, rule.ruleId), rule.title || "", from(rule.titleLocale)]),
                h("p", { class: "q-mb-xs" }, [rule.description || "", from(rule.descriptionLocale)]),
                (rule.referenceList || []).length ? h("ul", { class: "q-my-xs q-pl-md", "aria-label": T("References") },
                    rule.referenceList.map((ref) => h("li", { key: ref.referenceId }, ref.trusted === "Y"
                        ? [h("a", { href: constitution.href(ref.url), class: "m-constitution-reference", onClick: (event) => this.open(event, rule, ref) }, ref.title || ref.url)]
                        : [h("a", { href: ref.url, target: "_blank", rel: "noopener noreferrer" }, ref.title || ref.url),
                            h("span", { class: "text-caption text-grey-7 q-ml-xs" }, T("(opens outside the app)"))]))) : null,
            ]));
            return h("div", { class: "m-constitution-view" }, [
                doc.scopeLabel ? h("div", { class: "text-caption text-grey-7" }, doc.scopeLabel) : null,
                doc.provenance ? h("p", { class: "text-body2 q-mt-sm" }, doc.provenance) : null,
                h("div", { role: "list" }, rules),
                h(R("m-reference-dialog"), { dialog: this.dialog }),
            ]);
        },
    };

    components["m-assist-constitution"] = {
        name: "mAssistConstitution",
        data() { return { doc: null, loading: false, error: "", locale: null, current: false }; },
        computed: {
            store() { return moqui.assist && moqui.assist.state; },
            runId() { const convo = this.store && this.store.convo; return !this.current && convo ? convo.activeRunId || null : null; },
        },
        watch: { runId() { this.load(); } },
        mounted() { this.load(); },
        methods: {
            load() {
                this.loading = true;
                this.error = "";
                constitution.load({ runId: this.runId, locale: this.locale }).then((json) => {
                    this.loading = false;
                    if (json.errors) { this.error = String(json.errors); this.doc = null; return; }
                    this.doc = Object.assign({}, json, { scopeLabel: this.label(json) });
                }, () => { this.loading = false; this.error = T("The rules could not be loaded."); });
            },
            label(json) {
                if (json.runId) return T("Rules used for this run");
                if (json.source === "organization") return T("Rules of") + " " + (json.organizationName || json.organizationPartyId || "");
                return json.source === "instance" ? T("Rules of this site") : T("Rules");
            },
            back() { if (moqui.assist && moqui.assist.state) moqui.assist.state.pane = "chat"; },
        },
        render() {
            const doc = this.doc;
            const head = h("div", { class: "row items-center q-gutter-sm q-mb-sm" }, [
                h(R("q-btn"), { flat: true, dense: true, icon: "sym_o_arrow_back", label: T("Back to conversation"), id: "m-assist-constitution-back", onClick: this.back }),
                h("span", { class: "col" }),
                doc && (doc.localeList || []).length > 1 ? h("select", { class: "m-assist-constitution-locale", "aria-label": T("Reading language"), value: doc.locale,
                    onChange: (event) => { this.locale = event.target.value; this.load(); } },
                (doc.localeList || []).map((code) => h("option", { value: code, selected: code === doc.locale }, code))) : null,
            ]);
            const body = [];
            if (this.loading && !doc) body.push(h("div", { class: "text-grey-7" }, T("Loading the rules…")));
            if (this.error) body.push(h("div", { class: "text-warning", role: "alert" }, this.error));
            if (doc) {
                body.push(h("div", { class: "text-h6" }, doc.title || T("Constitution")));
                if (doc.runId) body.push(h(R("q-btn"), { flat: true, dense: true, noCaps: true, class: "q-mb-sm", label: T("Show the rules that apply now"),
                    onClick: () => { this.current = true; this.load(); } }));
                (doc.diagnosticList || []).forEach((text) => body.push(h("div", { class: "text-caption text-warning" }, text)));
                body.push(h(R("m-constitution-view"), { doc, runId: doc.runId || null }));
            }
            return h("div", { class: "m-assist-constitution q-pa-sm col scroll", id: "m-assist-constitution" }, [head].concat(body));
        },
    };

    const qvt = moqui.qvt;
    if (qvt && qvt.registerLate) Object.keys(components).forEach((name) => qvt.registerLate(name, components[name]));
    else {
        const registry = moqui.qvtComponents = moqui.qvtComponents || {};
        Object.keys(components).forEach((name) => { registry[name] = components[name]; });
    }
    constitution.components = components;
})(globalThis);
