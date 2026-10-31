/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Declared actions and panels (plans/Comms-Framework.md §3.3; crates/moqui-core/src/screen/ext.rs): what another app adds to a
 * .qvue screen it does not own, through ext-action and ext-panel in a screen-extend.
 *
 * - moqui.ext.items(path): {actions, panels} of the screen at path (/extItems), fetched once per path and page. The server
 *   has left out the targets the user may not view.
 * - <m-ext-actions slot="message-actions" :item="message" @done="reload">: the actions of a slot whose condition the item
 *   meets, as menu items (as="items", for a q-list in a q-menu) or buttons (as="buttons"). A screen action opens in the shell
 *   dialog or as the page; a transition action posts the mapped parameters and emits done.
 * - <m-ext-panel slot="room-panel" :item="room">: each panel of the slot, its screen loaded with the mapped parameters in a
 *   container of its own; one that fails shows the not-found state, the others still render.
 *
 * Only the mapped fields of the item go to a target, never the whole item. A condition is a field test, not code.
 *
 * Loaded after MoquiLib.js and before WebrootVue.qvt.js (qapps.xml): its components are in moqui.qvtComponents.
 */
;(function (root) {
    "use strict"

    const moqui = root.moqui
    const cache = new Map()

    function vm() {
        return moqui.webrootVue || {}
    }

    // the request path of a screen path: /extItems before it, after the app root path when there is one, as /menuData
    function itemsUrl(path) {
        const appRoot = vm().appRootPath || ""
        if (appRoot && path.indexOf(appRoot) === 0) return appRoot + "/extItems" + path.slice(appRoot.length)
        return "/extItems" + path
    }

    function fieldOf(item, field) {
        if (!item) return undefined
        return field.split(".").reduce((value, key) => (value == null ? undefined : value[key]), item)
    }

    const ext = {
        items(path) {
            const clean = String(path || "").split("?")[0]
            if (!clean) return Promise.resolve({actions: [], panels: []})
            if (!cache.has(clean)) {
                cache.set(
                    clean,
                    moqui
                        .request(itemsUrl(clean))
                        .then(result => result.json || {actions: [], panels: []})
                        .catch(() => ({actions: [], panels: []}))
                )
            }
            return cache.get(clean)
        },

        // the item meets the declared condition
        matches(declared, item) {
            const test = declared.condition
            if (!test) return true
            const value = fieldOf(item, test.field)
            const text = value == null ? "" : String(value)
            if (test.op === "truthy") return !!value && text !== "false" && text !== "N"
            if (test.op === "falsy") return !value || text === "false" || text === "N"
            if (test.op === "equals") return text === test.value
            if (test.op === "not-equals") return text !== test.value
            return false
        },

        // the target's parameters: only the mapped fields of the item
        params(declared, item) {
            const out = {}
            for (const parameter of declared.parameters || []) {
                if (parameter.value != null) {
                    out[parameter.name] = parameter.value
                    continue
                }
                const value = fieldOf(item, parameter.from)
                if (value != null && value !== "") out[parameter.name] = value
            }
            return out
        },

        withQuery(url, params) {
            const search = moqui.objToSearch(params)
            return search ? url + (url.indexOf("?") > 0 ? "&" : "?") + search : url
        },

        // run an action for an item; resolves true when it did something
        run(declared, item) {
            const params = ext.params(declared, item)
            if (declared.confirm && !root.confirm(declared.confirm)) return Promise.resolve(false)
            const shell = vm()
            if (declared.kind === "transition") {
                return moqui
                    .request(declared.url, {method: "POST", params})
                    .then(result => {
                        if (result.json && moqui.notifyMessages)
                            moqui.notifyMessages(result.json.messageInfos || result.json.messages, result.json.errors)
                        return true
                    })
                    .catch(result => {
                        if (moqui.handleAjaxError) moqui.handleAjaxError(result)
                        return false
                    })
            }
            const url = ext.withQuery(declared.url, params)
            if (declared.target === "page" && shell.setUrl) {
                const base = shell.basePath || "/apps"
                const link = shell.linkBasePath || "/qapps"
                shell.setUrl(url.indexOf(base + "/") === 0 ? link + url.slice(base.length) : url)
                return Promise.resolve(true)
            }
            shell.quickCreateTitle = declared.label || ""
            shell.quickCreateUrl = url
            shell.quickCreateShow = true
            return Promise.resolve(true)
        }
    }
    moqui.ext = ext

    const components = (moqui.qvtComponents = moqui.qvtComponents || {})

    // the declared items of the host screen: its path from mScreen unless one is given
    const itemsMixin = {
        inject: {mScreen: {default: null}},
        props: {slot: {type: String, required: true}, item: {type: Object, default: null}, path: String},
        data() {
            return {declared: {actions: [], panels: []}}
        },
        computed: {
            screenPath() {
                return this.path || (this.mScreen && this.mScreen.path) || ""
            }
        },
        watch: {
            screenPath: {
                immediate: true,
                handler(path) {
                    ext.items(path).then(found => {
                        this.declared = found
                    })
                }
            }
        }
    }

    components["m-ext-actions"] = {
        name: "mExtActions",
        mixins: [itemsMixin],
        props: {as: {type: String, default: "items"}},
        emits: ["done"],
        computed: {
            visible() {
                return (this.declared.actions || []).filter(action => action.slot === this.slot && ext.matches(action, this.item))
            }
        },
        methods: {
            run(action) {
                ext.run(action, this.item).then(done => {
                    if (done) this.$emit("done", action)
                })
            }
        },
        template:
            "<template v-if=\"as === 'buttons'\">" +
            '<q-btn v-for="action in visible" :key="action.name" :data-ext-action="action.name" dense flat no-caps' +
            ' :icon="action.icon || undefined" :label="action.label" @click="run(action)"></q-btn>' +
            "</template>" +
            '<template v-else><q-item v-for="action in visible" :key="action.name" :data-ext-action="action.name"' +
            ' clickable v-close-popup @click="run(action)">' +
            '<q-item-section v-if="action.icon" avatar><q-icon :name="action.icon"></q-icon></q-item-section>' +
            "<q-item-section>{{action.label}}</q-item-section></q-item></template>"
    }

    // Each panel is its own m-dynamic-container, which gives the panel's screen its own mScreen (path and parameters). A panel
    // screen that fails to load shows the shell's not-found state in its place; the others still render.
    components["m-ext-panel"] = {
        name: "mExtPanel",
        mixins: [itemsMixin],
        computed: {
            visible() {
                return (this.declared.panels || [])
                    .filter(panel => panel.slot === this.slot && ext.matches(panel, this.item))
                    .map(panel => ({panel, url: ext.withQuery(panel.url, ext.params(panel, this.item))}))
            }
        },
        template:
            '<div v-for="entry in visible" :key="entry.panel.name + entry.url" class="m-ext-panel" :data-ext-panel="entry.panel.name">' +
            '<m-dynamic-container :id="\'m-ext-panel-\' + entry.panel.name" :url="entry.url"></m-dynamic-container>' +
            "</div>"
    }
})(globalThis)
