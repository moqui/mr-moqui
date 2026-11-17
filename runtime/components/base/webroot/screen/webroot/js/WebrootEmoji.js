/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The emoji picker (plans/Comms-Messaging.md §4): moqui.emoji and <m-emoji-picker @pick="react">.
 *
 * - moqui.emoji.load(): the emoji by group, fetched once from /libs/emoji/emoji.json (unicode-emoji-json, made smaller by
 *   libs/refresh.mjs: {version, groups: [{slug, name, emoji: [[emoji, name]]}]}).
 * - moqui.emoji.search(groups, text, limit): the emoji whose name has a word that starts with each word typed (a word is
 *   letters and digits: ":thumbs_up" and "blood" for "A button (blood type)" too), or that is the emoji typed, in the
 *   file's order.
 * - moqui.emoji.recent(userId), remember(userId, emoji): the user's recent emoji, newest first, at most RECENT_MAX, in this
 *   browser's storage under the account's scope (moqui.scopedKey); storage that throws or is missing keeps nothing.
 * - <m-emoji-picker :user-id="me" @pick="(emoji) => ...">: a search line, the groups as tabs (Recent first when there are
 *   any), and the emoji of the group or of the search; a pick is remembered and emitted. Enter in the search picks its first
 *   emoji. From the keyboard the tabs are one stop (the arrows, Home and End move and choose) and so is the grid (the
 *   arrows by cell and row, Home and End; Enter or Space picks). autofocus false leaves the search unfocused (a phone,
 *   where its keyboard would cover a sheet). The names are Unicode's (English); the group names and the picker's words go through moqui.l10n.
 *
 * Loaded after MoquiLib.js and before WebrootVue.qvt.js (qapps.xml, the desktop shell's index.html): its component is in
 * moqui.qvtComponents, precompiled in WebrootVue.qvt.render.js for the bundled shell.
 */
;(function (root) {
    "use strict"

    const moqui = root.moqui
    const t = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text)
    const RECENT_MAX = 24
    const SEARCH_MAX = 160
    const ICONS = {
        recent: "sym_o_schedule", smileys_emotion: "sym_o_mood", people_body: "sym_o_waving_hand", animals_nature: "sym_o_pets",
        food_drink: "sym_o_restaurant", travel_places: "sym_o_flight", activities: "sym_o_sports_soccer", objects: "sym_o_lightbulb",
        symbols: "sym_o_emoji_symbols", flags: "sym_o_flag"
    }
    let loading = null

    const emoji = {
        RECENT_MAX,
        ICONS,
        url: "/libs/emoji/emoji.json",
        load() {
            if (!loading) {
                loading = root.fetch(emoji.url, { credentials: "same-origin" })
                    .then((response) => { if (!response.ok) throw new Error("emoji data " + response.status); return response.json() })
                    .then((data) => (data && Array.isArray(data.groups) ? data.groups : []))
                    .catch((err) => { loading = null; throw err })
            }
            return loading
        },
        words(text) {
            return String(text || "").toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((word) => word)
        },
        search(groups, text, limit) {
            const typed = String(text || "").trim()
            if (!typed) return []
            const words = emoji.words(typed)
            const max = limit || SEARCH_MAX
            const out = []
            for (const group of groups || []) {
                for (const one of group.emoji || []) {
                    if (one[0] === typed) { out.unshift(one); continue }
                    if (!words.length) continue
                    const name = emoji.words(one[1])
                    if (words.every((word) => name.some((part) => part.indexOf(word) === 0))) out.push(one)
                    if (out.length >= max) return out
                }
            }
            return out
        },
        key(userId) { return moqui.scopedKey("moqui.emoji.recent." + (userId || "")) },
        recent(userId) {
            try {
                const text = root.localStorage ? root.localStorage.getItem(emoji.key(userId)) : null
                const list = text ? JSON.parse(text) : []
                return Array.isArray(list) ? list.filter((one) => typeof one === "string" && one).slice(0, RECENT_MAX) : []
            } catch (err) { return [] }
        },
        remember(userId, picked) {
            if (!picked) return emoji.recent(userId)
            const list = [picked].concat(emoji.recent(userId).filter((one) => one !== picked)).slice(0, RECENT_MAX)
            try { if (root.localStorage) root.localStorage.setItem(emoji.key(userId), JSON.stringify(list)) } catch (err) { /* kept nowhere */ }
            return list
        },
        // the name of an emoji, from the groups (for a recent one)
        nameOf(groups, picked) {
            for (const group of groups || []) for (const one of group.emoji || []) if (one[0] === picked) return one[1]
            return ""
        }
    }
    moqui.emoji = emoji

    const components = (moqui.qvtComponents = moqui.qvtComponents || {})

    components["m-emoji-picker"] = {
        name: "mEmojiPicker",
        props: { userId: { type: String, default: "" }, autofocus: { type: Boolean, default: true } },
        emits: ["pick"],
        data() { return { groups: [], loaded: false, failed: false, query: "", group: "", recentList: [], active: 0 } },
        computed: {
            user() { return this.userId || (moqui.config && moqui.config.userId) || (moqui.chat && moqui.chat.me) || "" },
            tabs() {
                const tabs = this.recentList.length ? [{ slug: "recent", label: t("Recent"), icon: ICONS.recent }] : []
                return tabs.concat(this.groups.map((group) => ({ slug: group.slug, label: t(group.name), icon: ICONS[group.slug] || "sym_o_category" })))
            },
            current() { return this.group || (this.tabs[0] && this.tabs[0].slug) || "" },
            searching() { return !!String(this.query || "").trim() },
            shown() {
                if (this.searching) return emoji.search(this.groups, this.query)
                if (this.current === "recent") return this.recentList.map((one) => [one, emoji.nameOf(this.groups, one) || one])
                const group = this.groups.find((one) => one.slug === this.current)
                return group ? group.emoji : []
            },
            heading() {
                if (this.searching) return t("Search results")
                const tab = this.tabs.find((one) => one.slug === this.current)
                return tab ? tab.label : ""
            },
            uid() { return "m-emoji-" + this.$.uid },
            // the grid's one Tab stop
            activeIndex() { return Math.min(this.active, Math.max(0, this.shown.length - 1)) }
        },
        watch: {
            shown() { this.active = 0 }
        },
        mounted() {
            this.recentList = emoji.recent(this.user)
            emoji.load().then((groups) => { this.groups = groups; this.loaded = true },
                (err) => { console.warn("The emoji did not load", err); this.failed = true; this.loaded = true })
            if (this.autofocus) this.$nextTick(() => { const input = this.$el.querySelector && this.$el.querySelector("input"); if (input) input.focus() })
        },
        methods: {
            t,
            show(slug) {
                this.group = slug
                this.query = ""
                const grid = this.$refs.grid
                if (grid) grid.scrollTop = 0
            },
            // the tabs: Left and Right (around), Home and End choose a group and keep the focus on its tab
            tabKey(event) {
                const at = this.tabs.findIndex((tab) => tab.slug === this.current)
                const last = this.tabs.length - 1
                const next = { ArrowRight: at >= last ? 0 : at + 1, ArrowLeft: at <= 0 ? last : at - 1, Home: 0, End: last }[event.key]
                if (next == null || last < 0) return
                event.preventDefault()
                this.show(this.tabs[next].slug)
                this.$nextTick(() => { const el = this.$el.querySelector('[data-group="' + this.tabs[next].slug + '"]'); if (el) el.focus() })
            },
            // the grid: the arrows by cell and by row (as many cells as fit across), Home and End
            gridKey(event) {
                const cells = Array.from(this.$el.querySelectorAll(".m-emoji-cell"))
                if (!cells.length) return
                const top = cells[0].offsetTop
                const across = Math.max(1, cells.filter((cell) => cell.offsetTop === top).length)
                const at = this.activeIndex
                const next = { ArrowRight: at + 1, ArrowLeft: at - 1, ArrowDown: at + across, ArrowUp: at - across, Home: 0, End: cells.length - 1 }[event.key]
                if (next == null) return
                event.preventDefault()
                this.active = Math.max(0, Math.min(cells.length - 1, next))
                this.$nextTick(() => { const el = this.$el.querySelectorAll(".m-emoji-cell")[this.active]; if (el) { el.focus(); if (el.scrollIntoView) el.scrollIntoView({ block: "nearest" }) } })
            },
            pick(picked) {
                this.recentList = emoji.remember(this.user, picked)
                this.$emit("pick", picked)
            },
            first() { if (this.shown.length) this.pick(this.shown[0][0]) }
        },
        template:
            '<div class="m-emoji-picker column no-wrap">' +
                '<q-input v-model="query" dense outlined clearable type="search" class="q-ma-sm m-emoji-search" :placeholder="t(\'Search emoji\')"' +
                    ' :aria-label="t(\'Search emoji\')" @keydown.enter.prevent="first">' +
                    '<template #prepend><q-icon name="sym_o_search" size="18px"></q-icon></template></q-input>' +
                '<div v-if="!searching" class="row no-wrap q-px-xs m-emoji-tabs" role="tablist" :aria-label="t(\'Emoji groups\')" @keydown="tabKey">' +
                    '<q-btn v-for="tab in tabs" :key="tab.slug" flat dense round size="sm" role="tab" :icon="tab.icon" :aria-label="tab.label"' +
                        ' :id="uid + \'-tab-\' + tab.slug" :aria-controls="uid + \'-panel\'" :tabindex="tab.slug === current ? 0 : -1"' +
                        ' :aria-selected="tab.slug === current ? \'true\' : \'false\'" :class="tab.slug === current ? \'text-primary m-emoji-tab-on\' : \'m-muted\'"' +
                        ' :data-group="tab.slug" @click="show(tab.slug)"><q-tooltip>{{tab.label}}</q-tooltip></q-btn>' +
                '</div>' +
                '<div ref="grid" class="scroll m-emoji-grid" :id="uid + \'-panel\'" :role="searching ? \'region\' : \'tabpanel\'"' +
                    ' :aria-labelledby="searching ? undefined : uid + \'-tab-\' + current" :aria-label="searching ? heading : undefined">' +
                    '<div v-if="!loaded" class="row justify-center q-pa-md"><q-spinner size="24px"></q-spinner></div>' +
                    '<div v-else-if="failed && !shown.length" class="text-caption m-muted q-pa-sm">{{t(\'The emoji did not load\')}}</div>' +
                    '<template v-else>' +
                        '<div class="text-caption m-muted q-px-sm m-emoji-heading">{{heading}}</div>' +
                        '<div class="row q-px-xs" role="group" :aria-label="heading" @keydown="gridKey">' +
                            '<button v-for="(one, index) in shown" :key="one[0]" type="button" class="m-emoji-cell" :title="one[1]" :aria-label="one[1]"' +
                                ' :tabindex="index === activeIndex ? 0 : -1" :data-emoji="one[0]" @focus="active = index" @click="pick(one[0])">{{one[0]}}</button>' +
                        '</div>' +
                        '<div v-if="searching && !shown.length" class="text-caption m-muted q-pa-sm m-emoji-none">{{t(\'No emoji match\')}}</div>' +
                    '</template>' +
                '</div>' +
            '</div>'
    }
})(globalThis)
