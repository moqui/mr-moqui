/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Joining a chat room (plans/Comms-Messaging.md §18.3 phases 17 and 18): a room's invite links and their QR codes, the
 * moderators' queue of join requests, and a room's text members (outside numbers in the room by text).
 *
 *   - moqui.chat.invites: the pure parts of the links (their expiry and uses as words, a link's state, the membership
 *     lines of a join by link or by text);
 *   - moqui.chat.texts: the pure parts of text members (the cost line, a member's state, the limit of one room per number);
 *   - m-chat-invites: the Invite links dialog (Copy, QR, Share, Revoke, New link, List in Browse);
 *   - m-chat-join-requests: the Join requests dialog (Approve, Deny, Approve all);
 *   - m-chat-text-members: the People pane's text members (Rename, Take out) and Add by phone number.
 *
 * The components take the Chat screen's transition base (base) and the room (roomId); their transitions are in
 * basic-comms template/ChatInviteTransitions.xml and ChatTextTransitions.xml. Loaded after WebrootChat.js and before
 * WebrootVue.qvt.js (qapps.xml, the desktop shell's index.html), which registers moqui.qvtComponents; their render
 * functions are in WebrootVue.qvt.render.js. Nothing here touches the page until a component mounts.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui;
    const tr = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
    const fill = (text, values) => String(text).replace(/\{(\w+)\}/g, (match, key) => (values[key] == null ? "" : String(values[key])));
    const HOUR = 3600000;
    const DAY = 24 * HOUR;

    // ---- invite links (phase 17) ----

    const invites = {
        /** The choices of a new link's expiry (hours; 0 is no end) and uses (null is no limit). */
        EXPIRY_CHOICES: [{ value: 1, label: "1 hour" }, { value: 24, label: "1 day" }, { value: 168, label: "7 days" },
            { value: 720, label: "30 days" }, { value: 0, label: "No end" }],
        USE_CHOICES: [{ value: null, label: "No limit" }, { value: 1, label: "1 person" }, { value: 5, label: "5 people" },
            { value: 10, label: "10 people" }, { value: 25, label: "25 people" }, { value: 100, label: "100 people" }],
        /** A link's expiry as words: no end, ends in minutes, hours or days, or expired. */
        expiryText(expiryDate, now) {
            if (expiryDate == null || expiryDate === "") return tr("No end");
            const left = Number(expiryDate) - Number(now == null ? Date.now() : now);
            if (!(left > 0)) return tr("Expired");
            if (left < HOUR) return fill(tr("Ends in {count} min"), { count: Math.max(1, Math.round(left / 60000)) });
            if (left < 2 * DAY) return fill(tr("Ends in {count} h"), { count: Math.round(left / HOUR) });
            return fill(tr("Ends in {count} days"), { count: Math.round(left / DAY) });
        },
        /** A link's uses as words: "3 used", or "3 of 10 used". */
        usesText(useCount, maxUses) {
            const used = Number(useCount || 0);
            return maxUses == null || maxUses === "" ? fill(tr("{count} used"), { count: used })
                : fill(tr("{count} of {max} used"), { count: used, max: Number(maxUses) });
        },
        /** A link's state: expired, usedUp or ok. */
        linkState(link, now) {
            if (!link) return "expired";
            if (link.expiryDate != null && Number(link.expiryDate) <= Number(now == null ? Date.now() : now)) return "expired";
            if (link.maxUses != null && Number(link.useCount || 0) >= Number(link.maxUses)) return "usedUp";
            return "ok";
        },
        /** The full URL of a link's path on this origin. */
        urlOf(path, origin) {
            return String(origin == null ? (root.location ? root.location.origin : "") : origin).replace(/\/+$/, "") + String(path || "");
        },
        /**
         * The timeline's line of a membership event that joined by a link or by text, or one that left by STOP (who: the
         * person it is about, by: who added them); null for any other.
         */
        membershipLine(content, who, by) {
            const about = content || {};
            if (about.action === "join" && about.via === "link") return fill(tr("{user} joined by an invite link"), { user: who });
            if (about.action === "add" && about.via === "text") return fill(tr("{by} added {user} by text"), { by: by, user: who });
            if (about.action === "leave" && about.via === "stop") return fill(tr("{user} left by replying STOP"), { user: who });
            return null;
        },
    };

    // ---- text members (phase 18) ----

    const texts = {
        /** The composer's line in a room with text members. */
        costLine(count) {
            const n = Number(count || 0);
            if (!n) return "";
            return n === 1 ? tr("Also sent as a text to 1 number") : fill(tr("Also sent as a text to {count} numbers"), { count: n });
        },
        /** The add dialog's note of the cost, with the members the room will have. */
        costNote(count) {
            return fill(tr("Each message in this room is sent as a text to each text member. With {count} text members, one message is {count} texts, and a long message or a photo costs more."),
                { count: Math.max(1, Number(count || 0)) });
        },
        /** Why a number of ours cannot take this person: the room they are already a text member of on it. */
        takenText(roomName) {
            return fill(tr("This person is already a text member of {room} on this number; another number is needed."), { room: roomName || tr("a room") });
        },
        /** A text member's state as words: opted out, texts off, the welcome or the last text not sent; empty when fine. */
        stateText(member) {
            const m = member || {};
            if (m.optedOut) return tr("Replied STOP: gets no texts");
            if (m.roomTextsOff) return tr("Texts off: the number no longer carries room texts");
            if (m.lastFailure) return fill(tr("The last text was not sent ({reason})"), { reason: m.lastFailure });
            if (m.welcomeReasonCode && !m.welcomeDate) return fill(tr("The welcome text was not sent ({reason})"), { reason: m.welcomeReasonCode });
            return "";
        },
    };

    moqui.chat = moqui.chat || {};
    moqui.chat.invites = invites;
    moqui.chat.texts = texts;

    // the transitions: a GET, or a POST that shows its messages; a refusal's errors are shown by the request's handler
    const get = (base, name, params) => moqui.request(base + "/" + name, { params: params || {} }).then((result) => result.json || {});
    const post = (base, name, params) => moqui.request(base + "/" + name, { method: "POST", params: params || {} }).then((result) => {
        const json = result.json || {};
        if (json.messageInfos && json.messageInfos.length) moqui.notifyMessages(json.messageInfos, json.errors);
        return json;
    }, (err) => { moqui.handleAjaxError(err); throw err; });
    const svgUrl = (svg) => "data:image/svg+xml;charset=utf-8," + encodeURIComponent(String(svg || ""));

    const components = moqui.qvtComponents = moqui.qvtComponents || {};

    components["m-chat-invites"] = {
        name: "mChatInvites",
        props: { modelValue: Boolean, base: { type: String, required: true }, roomId: String, roomName: String },
        emits: ["update:modelValue"],
        data() {
            return { loading: false, links: [], canMake: false, roomKind: null, canList: false, listed: false, making: false,
                form: { label: "", expiryHours: 168, maxUses: null, approvalRequired: false }, qr: null, now: Date.now() };
        },
        computed: {
            expiryOptions() { return invites.EXPIRY_CHOICES.map((choice) => ({ value: choice.value, label: tr(choice.label) })); },
            useOptions() { return invites.USE_CHOICES.map((choice) => ({ value: choice.value, label: tr(choice.label) })); },
            canShare() { return !!(root.navigator && root.navigator.share); },
        },
        watch: { modelValue(open) { if (open) this.load(); else this.qr = null; } },
        mounted() { if (this.modelValue) this.load(); },
        methods: {
            t: tr,
            expiryText(link) { return invites.expiryText(link.expiryDate, this.now); },
            usesText(link) { return invites.usesText(link.useCount, link.maxUses); },
            urlOf(link) { return invites.urlOf(link.path); },
            load() {
                if (!this.roomId) return;
                this.loading = true;
                this.now = Date.now();
                get(this.base, "inviteLinks", { chatRoomId: this.roomId }).then((json) => {
                    this.links = json.linkList || []; this.canMake = !!json.canMake; this.roomKind = json.roomKind || null;
                    this.canList = !!json.canList; this.listed = !!json.listed;
                }, (err) => moqui.handleAjaxError(err)).finally(() => { this.loading = false; });
            },
            make() {
                this.making = true;
                post(this.base, "createInviteLink", { chatRoomId: this.roomId, label: this.form.label || "", expiryHours: this.form.expiryHours,
                    maxUses: this.form.maxUses == null ? "" : this.form.maxUses, approvalRequired: this.roomKind === "private" && this.form.approvalRequired })
                    .then((json) => { if (json.path) { this.form.label = ""; this.load(); } }, () => {}).finally(() => { this.making = false; });
            },
            copy(link) {
                const url = this.urlOf(link);
                const done = () => moqui.notifyMessages([{ type: "success", message: tr("The link is copied") }]);
                if (root.navigator && root.navigator.clipboard) root.navigator.clipboard.writeText(url).then(done, () => {});
            },
            share(link) {
                root.navigator.share({ title: this.roomName || tr("A chat room"), text: fill(tr("Join {room}"), { room: this.roomName || "" }),
                    url: this.urlOf(link) }).catch(() => {});
            },
            showQr(link) {
                this.qr = { link, url: this.urlOf(link), svg: null, loading: true };
                get(this.base, "inviteQr", { chatInviteLinkId: link.chatInviteLinkId }).then((json) => {
                    if (this.qr && this.qr.link === link) this.qr = Object.assign({}, this.qr, { url: json.url || this.qr.url, svg: json.qrSvg || null, loading: false });
                }, (err) => { moqui.handleAjaxError(err); if (this.qr) this.qr.loading = false; });
            },
            qrSrc() { return this.qr && this.qr.svg ? svgUrl(this.qr.svg) : ""; },
            revoke(link) {
                post(this.base, "revokeInviteLink", { chatInviteLinkId: link.chatInviteLinkId }).then(() => { this.qr = null; this.load(); }, () => {});
            },
            setListed(value) {
                post(this.base, "setRoomListed", { chatRoomId: this.roomId, listed: !!value }).then((json) => { this.listed = !!json.listed; }, () => {});
            },
        },
        template:
            '<m-dialog :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :title="t(\'Invite links\')" width="560" class="m-chat-invites">' +
                '<q-linear-progress v-if="loading" indeterminate color="primary"></q-linear-progress>' +
                '<template v-if="qr">' +
                    '<div class="column items-center q-gutter-y-sm m-chat-invite-qr">' +
                        '<img v-if="qr.svg" :src="qrSrc()" :alt="t(\'QR code of the invite link\')" style="width:240px;height:240px;background:#fff;padding:8px">' +
                        '<div v-else-if="!qr.loading" class="m-muted">{{t(\'No QR code\')}}</div>' +
                        '<div class="text-caption" style="word-break:break-all">{{qr.url}}</div>' +
                        '<div class="row q-gutter-x-sm">' +
                            '<q-btn flat no-caps icon="sym_o_content_copy" :label="t(\'Copy\')" @click="copy(qr.link)"></q-btn>' +
                            '<q-btn v-if="qr.svg" flat no-caps icon="sym_o_download" :label="t(\'Download\')" :href="qrSrc()" download="invite-qr.svg"></q-btn>' +
                            '<q-btn flat no-caps icon="sym_o_arrow_back" :label="t(\'Back\')" @click="qr = null"></q-btn>' +
                        '</div>' +
                    '</div>' +
                '</template>' +
                '<template v-else>' +
                    '<q-list dense separator>' +
                        '<q-item v-for="link in links" :key="link.chatInviteLinkId" class="q-px-xs m-chat-invite-link" :data-link="link.chatInviteLinkId">' +
                            '<q-item-section>' +
                                '<q-item-label class="text-weight-medium">{{link.label || t(\'Invite link\')}}' +
                                    '<q-chip v-if="link.approvalRequired" dense square class="m-tonal q-ml-xs">{{t(\'Approval\')}}</q-chip></q-item-label>' +
                                '<q-item-label caption>{{expiryText(link)}} · {{usesText(link)}}<span v-if="link.createdByName"> · {{link.createdByName}}</span></q-item-label>' +
                            '</q-item-section>' +
                            '<q-item-section side><div class="row no-wrap">' +
                                '<q-btn flat round dense icon="sym_o_content_copy" :aria-label="t(\'Copy\')" @click="copy(link)"></q-btn>' +
                                '<q-btn flat round dense icon="sym_o_qr_code_2" :aria-label="t(\'QR code\')" @click="showQr(link)"></q-btn>' +
                                '<q-btn v-if="canShare" flat round dense icon="sym_o_share" :aria-label="t(\'Share\')" @click="share(link)"></q-btn>' +
                                '<q-btn flat round dense icon="sym_o_link_off" color="negative" :aria-label="t(\'Revoke\')" @click="revoke(link)"></q-btn>' +
                            '</div></q-item-section>' +
                        '</q-item>' +
                        '<q-item v-if="!loading && !links.length"><q-item-section class="m-muted">{{t(\'No invite links\')}}</q-item-section></q-item>' +
                    '</q-list>' +
                    '<template v-if="canMake">' +
                        '<div class="text-subtitle2 q-mt-md">{{t(\'New link\')}}</div>' +
                        '<div class="row q-col-gutter-sm items-end">' +
                            '<div class="col-12 col-sm-6"><q-input v-model="form.label" dense outlined stack-label maxlength="100" :label="t(\'Label (optional)\')" name="inviteLabel"></q-input></div>' +
                            '<div class="col-6 col-sm-3"><q-select v-model="form.expiryHours" :options="expiryOptions" emit-value map-options dense outlined stack-label :label="t(\'Ends\')" name="inviteExpiry"></q-select></div>' +
                            '<div class="col-6 col-sm-3"><q-select v-model="form.maxUses" :options="useOptions" emit-value map-options dense outlined stack-label :label="t(\'Takes\')" name="inviteUses"></q-select></div>' +
                        '</div>' +
                        '<q-toggle v-if="roomKind === \'private\'" v-model="form.approvalRequired" :label="t(\'A moderator approves each person\')" class="q-mt-xs"></q-toggle>' +
                        '<div class="text-caption m-muted q-mt-xs">{{roomKind === \'public\' ? t(\'Anyone with an account here may join a public room: the link is a shortcut.\') : t(\'People need an account on this server to join.\')}}</div>' +
                        '<div class="row justify-end q-mt-sm"><q-btn unelevated no-caps color="primary" icon="sym_o_add_link" :label="t(\'Make a link\')" :loading="making" class="m-chat-invite-make" @click="make"></q-btn></div>' +
                    '</template>' +
                    '<template v-if="canList">' +
                        '<q-separator class="q-my-md"></q-separator>' +
                        '<q-toggle :model-value="listed" @update:model-value="setListed" :label="t(\'List in Browse: people can ask to join\')" class="m-chat-listed"></q-toggle>' +
                    '</template>' +
                '</template>' +
            '</m-dialog>'
    };

    components["m-chat-join-requests"] = {
        name: "mChatJoinRequests",
        props: { modelValue: Boolean, base: { type: String, required: true }, roomId: String },
        emits: ["update:modelValue", "changed"],
        data() { return { loading: false, requests: [], busy: false }; },
        watch: { modelValue(open) { if (open) this.load(); } },
        mounted() { if (this.modelValue) this.load(); },
        methods: {
            t: tr,
            when(millis) { return moqui.chat.shortWhen ? moqui.chat.shortWhen(millis) : new Date(Number(millis)).toLocaleString(); },
            source(request) {
                if (request.fromBrowse) return tr("From Browse");
                return request.linkLabel ? fill(tr("By the link {label}"), { label: request.linkLabel }) : tr("By an invite link");
            },
            load() {
                if (!this.roomId) return;
                this.loading = true;
                get(this.base, "joinRequests", { chatRoomId: this.roomId }).then((json) => { this.requests = json.requestList || []; },
                    (err) => moqui.handleAjaxError(err)).finally(() => { this.loading = false; });
            },
            decide(requests, decision) {
                if (!requests.length) return;
                this.busy = true;
                const params = requests.length === 1 ? { chatJoinRequestId: requests[0].chatJoinRequestId, decision }
                    : { chatJoinRequestIds: requests.map((request) => request.chatJoinRequestId).join(","), decision };
                post(this.base, "decideJoinRequest", params).then((json) => {
                    if ((json.failedList || []).length) moqui.notifyMessages([], fill(tr("{count} requests were not decided"), { count: json.failedList.length }));
                    this.load();
                    this.$emit("changed");
                }, () => {}).finally(() => { this.busy = false; });
            },
        },
        template:
            '<m-dialog :model-value="modelValue" @update:model-value="$emit(\'update:modelValue\', $event)" :title="t(\'Join requests\')" width="560" class="m-chat-join-requests">' +
                '<q-linear-progress v-if="loading" indeterminate color="primary"></q-linear-progress>' +
                '<q-list dense separator>' +
                    '<q-item v-for="request in requests" :key="request.chatJoinRequestId" class="q-px-xs m-chat-join-request" :data-user="request.userId">' +
                        '<q-item-section avatar><m-avatar :name="request.name" :user-id="request.userId" size="32px"></m-avatar></q-item-section>' +
                        '<q-item-section><q-item-label class="text-weight-medium">{{request.name}}</q-item-label>' +
                            '<q-item-label v-if="request.note" class="ellipsis-2-lines">{{request.note}}</q-item-label>' +
                            '<q-item-label caption>{{when(request.requestedDate)}} · {{source(request)}}<span v-if="request.wasRemoved" class="text-warning"> · {{t(\'Was taken out of this room\')}}</span></q-item-label></q-item-section>' +
                        '<q-item-section side><div class="row no-wrap q-gutter-x-xs">' +
                            '<q-btn flat dense no-caps color="negative" :label="t(\'Deny\')" :disable="busy" @click="decide([request], \'deny\')"></q-btn>' +
                            '<q-btn unelevated dense no-caps color="primary" :label="t(\'Approve\')" :disable="busy" class="m-chat-join-approve" @click="decide([request], \'approve\')"></q-btn>' +
                        '</div></q-item-section>' +
                    '</q-item>' +
                    '<q-item v-if="!loading && !requests.length"><q-item-section class="m-muted">{{t(\'Nobody is waiting to join\')}}</q-item-section></q-item>' +
                '</q-list>' +
                '<div v-if="requests.length > 1" class="row justify-end q-mt-sm"><q-btn flat no-caps color="primary" :label="t(\'Approve all\')" :disable="busy" @click="decide(requests, \'approve\')"></q-btn></div>' +
            '</m-dialog>'
    };

    components["m-chat-text-members"] = {
        name: "mChatTextMembers",
        props: { base: { type: String, required: true }, roomId: String, canModerate: Boolean },
        emits: ["changed"],
        data() {
            return { members: [], canAdd: false, maxMembers: null, adding: false, busy: false,
                form: { phoneNumber: "", memberName: "", ourNumber: null, consent: false }, choices: [], checked: null };
        },
        computed: {
            costNote() { return texts.costNote(this.members.length + 1); },
            freeChoices() { return this.choices.filter((choice) => choice.free); },
            takenChoices() { return this.choices.filter((choice) => !choice.free); },
            numberOptions() { return this.freeChoices.map((choice) => ({ value: choice.phoneNumber, label: choice.description ? choice.phoneNumber + ' · ' + choice.description : choice.phoneNumber })); },
            pairWarning() {
                const pick = this.choices.find((choice) => choice.phoneNumber === this.form.ourNumber) || this.freeChoices[0];
                return !!pick && pick.pairRoomActive;
            },
            canSubmit() { return !!this.form.phoneNumber && this.form.consent && (!this.choices.length || this.freeChoices.length > 0); },
        },
        watch: { roomId() { this.load(); } },
        mounted() { this.load(); },
        methods: {
            t: tr,
            stateText(member) { return texts.stateText(member); },
            takenText(choice) { return texts.takenText(choice.takenRoomName); },
            load() {
                if (!this.roomId) return;
                get(this.base, "textMembers", { chatRoomId: this.roomId }).then((json) => {
                    this.members = json.memberList || []; this.canAdd = !!json.canAdd; this.maxMembers = json.maxMembers ?? null;
                }, () => {});
            },
            openAdd() {
                this.form = { phoneNumber: "", memberName: "", ourNumber: null, consent: false };
                this.choices = []; this.checked = null; this.adding = true;
            },
            // the numbers of ours this person can be added on, asked when the number is typed
            check() {
                const number = String(this.form.phoneNumber || "").trim();
                if (!number || number === this.checked) return;
                this.checked = number;
                get(this.base, "textMemberChoices", { chatRoomId: this.roomId, phoneNumber: number }).then((json) => {
                    this.choices = json.numberList || [];
                    if (!this.freeChoices.some((choice) => choice.phoneNumber === this.form.ourNumber)) this.form.ourNumber = this.freeChoices.length ? this.freeChoices[0].phoneNumber : null;
                }, () => {});
            },
            add() {
                this.busy = true;
                post(this.base, "addTextMember", { chatRoomId: this.roomId, phoneNumber: this.form.phoneNumber, memberName: this.form.memberName || "",
                    ourNumber: this.form.ourNumber || "", consent: !!this.form.consent }).then((json) => {
                    if (!json.chatActorId) return;
                    this.adding = false;
                    if (!json.welcomeSent) moqui.notifyMessages([{ type: "warning", message: tr("Added; the welcome text was not sent") }]);
                    this.load();
                    this.$emit("changed");
                }, () => {}).finally(() => { this.busy = false; });
            },
            rename(member) {
                this.$q.dialog({ title: tr("Rename"), message: tr("Their name in this room"), cancel: true,
                    prompt: { model: member.memberName || "", type: "text", attrs: { maxlength: 60 } } }).onOk((name) => {
                    post(this.base, "renameTextMember", { chatRoomId: this.roomId, chatActorId: member.chatActorId, memberName: name || "" }).then(() => {
                        this.load(); this.$emit("changed");
                    }, () => {});
                });
            },
            remove(member) {
                post(this.base, "removeMember", { chatRoomId: this.roomId, chatActorId: member.chatActorId }).then(() => {
                    this.load(); this.$emit("changed");
                }, () => {});
            },
        },
        template:
            '<div class="m-chat-text-members">' +
                '<q-list v-if="members.length" dense>' +
                    '<q-item v-for="member in members" :key="member.chatActorId" class="m-chat-text-member" :data-actor="member.chatActorId">' +
                        '<q-item-section avatar><q-avatar size="32px" class="m-tonal text-primary" icon="sym_o_sms"></q-avatar></q-item-section>' +
                        '<q-item-section><q-item-label>{{member.memberName || member.phoneNumber}}</q-item-label>' +
                            '<q-item-label caption>{{member.memberName ? member.phoneNumber + \' · \' : \'\'}}{{t(\'by text from\')}} {{member.ourNumber}}</q-item-label>' +
                            '<q-item-label v-if="stateText(member)" caption class="text-warning">{{stateText(member)}}</q-item-label></q-item-section>' +
                        '<q-item-section side class="row no-wrap items-center">' +
                            '<q-chip dense square icon="sym_o_sms" class="m-tonal text-info">{{t(\'Text\')}}</q-chip>' +
                            '<q-btn v-if="canModerate" flat round dense size="sm" icon="sym_o_more_vert" :aria-label="t(\'Member\')">' +
                                '<q-menu anchor="bottom right" self="top right"><q-list dense style="min-width:180px">' +
                                    '<q-item clickable v-close-popup @click="rename(member)"><q-item-section>{{t(\'Rename\')}}</q-item-section></q-item>' +
                                    '<q-item clickable v-close-popup @click="remove(member)"><q-item-section class="text-negative">{{t(\'Take out of the room\')}}</q-item-section></q-item>' +
                                '</q-list></q-menu>' +
                            '</q-btn>' +
                        '</q-item-section>' +
                    '</q-item>' +
                '</q-list>' +
                '<div v-if="canAdd" class="q-px-sm q-pb-sm"><q-btn flat dense no-caps color="primary" icon="sym_o_add_call" :label="t(\'Add by phone number\')" class="m-chat-text-add" @click="openAdd"></q-btn></div>' +
                '<m-dialog v-model="adding" :title="t(\'Add by phone number\')" width="480">' +
                    '<q-input v-model="form.phoneNumber" type="tel" dense outlined stack-label autofocus :label="t(\'Phone number\')" name="textMemberNumber" @blur="check" @keyup.enter="check"></q-input>' +
                    '<q-input v-model="form.memberName" dense outlined stack-label maxlength="60" class="q-mt-sm" :label="t(\'Their name in this room\')" name="textMemberName"></q-input>' +
                    '<q-select v-if="numberOptions.length > 1" v-model="form.ourNumber" :options="numberOptions" emit-value map-options dense outlined stack-label class="q-mt-sm" :label="t(\'Text from\')" name="textMemberFrom"></q-select>' +
                    '<div v-for="choice in takenChoices" :key="choice.phoneNumber" class="text-caption text-warning q-mt-xs m-chat-text-taken">{{choice.phoneNumber}}: {{takenText(choice)}}</div>' +
                    '<div v-if="pairWarning" class="text-caption text-warning q-mt-xs">{{t(\'You text with this person from this number: while they are in this room by text, their replies come here, and texts from your conversation with them are not sent.\')}}</div>' +
                    '<q-checkbox v-model="form.consent" class="q-mt-sm" :label="t(\'This person agreed to get the room\\\'s texts\')" name="textMemberConsent"></q-checkbox>' +
                    '<div class="text-caption m-muted q-mt-sm">{{costNote}}</div>' +
                    '<div class="row justify-end q-mt-md"><q-btn unelevated no-caps color="primary" :label="t(\'Add\')" :disable="!canSubmit" :loading="busy" class="m-chat-text-submit" @click="add"></q-btn></div>' +
                '</m-dialog>' +
            '</div>'
    };

    if (typeof module !== "undefined" && module.exports) module.exports = { invites, texts };
})(globalThis);
