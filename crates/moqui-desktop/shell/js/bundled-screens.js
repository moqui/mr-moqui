/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// Generated from the bundled .qvue screens of the desktop client's shell; do not edit. Make it again with
//   UPDATE_GOLDEN=1 cargo test -p moqui-core --lib bundled_screens_are_precompiled
// Each is moqui.qvtBundled[name] = { make(Vue) }, its templates compiled to render functions
// (moqui_core::screen::vue_check::precompiled_qvue); moqui.bundledComponent(name) makes it when it is first used.
(function (moqui) {
    moqui.qvtBundled = moqui.qvtBundled || {};
    moqui.qvtBundled["BasicComms/Panel/Chat"] = { make: function (Vue) {
        const module = { exports: {} };
        const define = moqui.define;
        // the file's script as it is (not indented: a template string in it keeps its text)
        const returned = (function (exports) {
const t = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
const DAY = 86400000;
const ROOM_ICONS = { CrtDirect: 'sym_o_group', CrtPrivate: 'sym_o_lock', CrtPublic: 'sym_o_tag', CrtRecord: 'sym_o_description' };
const fill = (text, values) => String(text).replace(/\{(\w+)\}/g, (match, key) => (values[key] == null ? '' : String(values[key])));
const timeText = (millis) => (millis ? new Date(Number(millis)).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }) : '');
const shortWhen = (millis) => {
    if (!millis) return '';
    const d = new Date(Number(millis));
    const now = new Date();
    if (d.toDateString() === now.toDateString()) return timeText(millis);
    if (now.getTime() - d.getTime() < 6 * DAY) return d.toLocaleDateString(undefined, { weekday: 'short' });
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
};
// kept for the page: the tray mounts this component again when it reloads the tab, and a reopened tab shows what it had
const memo = { roomId: null, rooms: null, me: null, prefs: null, roomComponent: null, loading: null };
const MAX_SHOWN = 30;

module.exports = {
    name: 'ChatPanel',
    inject: { mScreen: { default: null } },
    data() {
        return { loaded: !!memo.rooms, rooms: memo.rooms || [], me: memo.me || {}, prefs: memo.prefs || {}, roomId: null, componentVersion: 0,
            height: 520, newOpen: false, newUserIds: [], busy: false, inDialog: false };
    },
    computed: {
        // the Chat screen beside this panel's (.../Panel/Chat), under whichever app mounts it
        base() { const path = (this.mScreen && this.mScreen.path) || (this.$root.basePath + '/BasicComms/Panel/Chat'); return path.replace(/\/Panel\/Chat$/, '/Chat'); },
        linkBase() { const base = this.base; const root = this.$root; return base.indexOf(root.basePath + '/') === 0 ? root.linkBasePath + base.slice(root.basePath.length) : base; },
        roomComponent() { void this.componentVersion; return memo.roomComponent; },
        // mentions and unread first, then the latest
        shownRooms() {
            const rank = (room) => (this.countsOf(room).mentions ? 2 : (this.countsOf(room).unread ? 1 : 0));
            return this.rooms.filter((room) => room.isMember).slice().sort((left, right) => {
                const byRank = rank(right) - rank(left);
                if (byRank !== 0) return byRank;
                return (right.lastEventDate || 0) - (left.lastEventDate || 0);
            }).slice(0, MAX_SHOWN);
        },
        unreadTotal() { return this.rooms.reduce((sum, room) => sum + (this.countsOf(room).unread || 0), 0); }
    },
    methods: {
        t,
        fill,
        shortWhen,
        url(name) { return this.base + '/' + name; },
        go(target) { if (target) this.$root.setUrl(target); },
        post(name, params) {
            return moqui.request(this.url(name), { method: 'POST', params: params || {} }).then((result) => {
                const json = result.json || {};
                if (json.messageInfos && json.messageInfos.length) moqui.notifyMessages(json.messageInfos, json.errors);
                return json;
            }, (err) => { moqui.handleAjaxError(err); throw err; });
        },
        roomIcon(room) { return (room.record && room.record.icon) || ROOM_ICONS[room.roomTypeEnumId] || 'sym_o_chat'; },
        countsOf(room) {
            if (!room.isMember) return { unread: 0, mentions: 0 };
            if (moqui.chat && moqui.chat.state.unread[room.chatRoomId]) return moqui.chat.unreadOf(moqui.chat.state, room.chatRoomId);
            return { unread: room.unreadCount || 0, mentions: room.mentionCount || 0 };
        },
        snippetOf(room) {
            const last = room.lastMessage;
            if (!last) return room.topic || t('No messages yet');
            const text = last.encrypted ? t('Encrypted message') : last.redacted || !last.snippet ? t('Message removed') : last.snippet;
            if (last.senderUserId === this.me.userId) return t('You') + ': ' + text;
            if (room.roomTypeEnumId === 'CrtDirect' && room.otherUserId) return text;
            return String(last.senderName || '').split(' ')[0] + ': ' + text;
        },
        // the rooms, few and light: the panel is a glance (Open in Comms has them all)
        refresh() {
            return moqui.request(this.url('chatData'), { params: { limit: 40 } }).then((result) => {
                const json = result.json || {};
                this.me = memo.me = json.me || {};
                this.prefs = memo.prefs = json.prefs || {};
                this.rooms = memo.rooms = json.roomList || [];
                if (moqui.chat) this.rooms.forEach((room) => {
                    if (room.isMember && room.chatRoomId !== this.roomId) moqui.chat.state.unread[room.chatRoomId] = { unread: room.unreadCount || 0, mentions: room.mentionCount || 0 };
                });
                this.loaded = true;
            }, (err) => { this.loaded = true; console.info('Chat panel: the rooms could not be read', err && err.status); });
        },
        scheduleRooms() { clearTimeout(this.roomsTimer); this.roomsTimer = setTimeout(() => this.refresh(), 400); },
        // the tray's refresh of this tab (m-dynamic-container.shellRefresh): the rooms in place, the open room stays
        shellRefresh() { this.refresh(); },
        // the room view is Chat.qvue's chat-room, loaded once for the page the first time a room opens here
        roomView() {
            if (memo.roomComponent) return Promise.resolve(memo.roomComponent);
            if (!memo.loading) memo.loading = new Promise((resolve) => {
                moqui.loadComponent(this.base + '.qvue', (comp) => {
                    const room = comp && comp.components ? comp.components['chat-room'] : null;
                    if (room) memo.roomComponent = window.Vue && window.Vue.markRaw ? window.Vue.markRaw(room) : room;
                    else console.info('Chat panel: the room view did not load; the room opens in Chat');
                    memo.loading = null;
                    resolve(memo.roomComponent);
                });
            });
            return memo.loading;
        },
        openRoom(id) {
            if (!id) return;
            if (this.inDialog) { this.go(this.linkBase + '?chatRoomId=' + encodeURIComponent(id)); return; }
            this.roomId = memo.roomId = id;
            this.roomView().then((comp) => {
                if (!comp) { this.closeRoom(); this.go(this.linkBase + '?chatRoomId=' + encodeURIComponent(id)); return; }
                this.componentVersion++;
            });
        },
        closeRoom() { this.roomId = memo.roomId = null; this.refresh(); },
        onLeft(id) { if (this.roomId === id) this.closeRoom(); else this.scheduleRooms(); },
        onRoomRead(id) {
            if (moqui.chat) moqui.chat.state.unread[id] = { unread: 0, mentions: 0 };
            const room = this.rooms.find((item) => item.chatRoomId === id);
            if (room) { room.unreadCount = 0; room.mentionCount = 0; }
        },
        toggleNew() { this.newOpen = !this.newOpen; this.newUserIds = []; },
        startDirect() {
            if (!this.newUserIds.length) return;
            this.busy = true;
            this.post('createDirect', { userIds: this.newUserIds.join(',') }).then((json) => {
                this.busy = false;
                this.newOpen = false;
                this.newUserIds = [];
                this.scheduleRooms();
                this.openRoom(json.chatRoomId);
            }, () => { this.busy = false; });
        },
        markAllRead() {
            this.post('markAllRead', {}).then(() => {
                if (moqui.chat) Object.keys(moqui.chat.state.unread).forEach((id) => { moqui.chat.state.unread[id] = { unread: 0, mentions: 0 }; });
                this.rooms.forEach((room) => { room.unreadCount = 0; room.mentionCount = 0; });
            }, () => {});
        },
        onEvent(payload) {
            const event = payload && payload.event;
            if (!event) return;
            const room = this.rooms.find((item) => item.chatRoomId === payload.chatRoomId);
            if (!room) { this.scheduleRooms(); return; }
            room.lastEventSeq = Math.max(Number(room.lastEventSeq || 0), Number(event.eventSeq));
            if (event.type === 'message' && event.threadRootEventSeq == null) {
                const text = String(event.bodyText || '').replace(/\s+/g, ' ').trim();
                room.lastMessage = { eventSeq: event.eventSeq, senderUserId: event.senderUserId, senderName: event.senderName, sentDate: event.sentDate,
                    snippet: event.encrypted ? null : (text.length > 140 ? text.slice(0, 139) + '…' : text) || (event.attachments || []).map((file) => '📎 ' + file.fileName).join(', '),
                    redacted: false, encrypted: !!event.encrypted };
                room.lastEventDate = event.sentDate;
            } else if (event.type === 'redaction' || event.type === 'membership' || event.type === 'system') this.scheduleRooms();
        },
        measure() {
            const parent = this.$el && this.$el.parentElement;
            if (!parent) return;
            this.inDialog = !!(this.$el.closest && this.$el.closest('#m-quick-create-body'));
            if (this.inDialog) { this.height = Math.min(560, Math.max(360, window.innerHeight - 200)); return; }
            const style = window.getComputedStyle ? window.getComputedStyle(parent) : null;
            const padding = style ? (parseFloat(style.paddingTop) || 0) + (parseFloat(style.paddingBottom) || 0) : 16;
            const available = parent.clientHeight - padding;
            if (available > 200) this.height = available;
        }
    },
    mounted() {
        this.measure();
        if (this.inDialog) this.newOpen = true;
        else if (memo.roomId) this.openRoom(memo.roomId);
        if (window.ResizeObserver && this.$el.parentElement) {
            this.resizer = new window.ResizeObserver(() => this.measure());
            this.resizer.observe(this.$el.parentElement);
        }
        this.unsubscribe = [];
        if (moqui.chat) {
            this.unsubscribe.push(moqui.chat.on('event', (payload) => this.onEvent(payload)));
            this.unsubscribe.push(moqui.chat.on('room', () => this.scheduleRooms()));
        }
        this.refresh();
    },
    beforeUnmount() {
        if (this.resizer) this.resizer.disconnect();
        (this.unsubscribe || []).forEach((off) => off());
        clearTimeout(this.roomsTimer);
    }
};
        })(module.exports);
        const component = Object.assign({}, returned !== undefined ? returned : module.exports);
        {
            const target = component;
            target.render = (function () {
                const { toDisplayString: _toDisplayString, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, createTextVNode: _createTextVNode, createElementVNode: _createElementVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, renderList: _renderList, Fragment: _Fragment, createBlock: _createBlock, normalizeClass: _normalizeClass, resolveDynamicComponent: _resolveDynamicComponent, normalizeStyle: _normalizeStyle } = Vue

                return function render(_ctx, _cache) {
                  const _component_q_tooltip = _resolveComponent("q-tooltip")
                  const _component_q_btn = _resolveComponent("q-btn")
                  const _component_m_drop_down = _resolveComponent("m-drop-down")
                  const _component_q_separator = _resolveComponent("q-separator")
                  const _component_q_skeleton = _resolveComponent("q-skeleton")
                  const _component_m_avatar = _resolveComponent("m-avatar")
                  const _component_q_avatar = _resolveComponent("q-avatar")
                  const _component_q_item_section = _resolveComponent("q-item-section")
                  const _component_q_space = _resolveComponent("q-space")
                  const _component_q_item_label = _resolveComponent("q-item-label")
                  const _component_q_badge = _resolveComponent("q-badge")
                  const _component_q_item = _resolveComponent("q-item")
                  const _component_q_list = _resolveComponent("q-list")
                  const _component_q_icon = _resolveComponent("q-icon")

                  return (_openBlock(), _createElementBlock("div", {
                    class: "m-chat-panel column no-wrap",
                    style: _normalizeStyle({height: _ctx.height + 'px'})
                  }, [
                    (!_ctx.roomId)
                      ? (_openBlock(), _createElementBlock(_Fragment, { key: 0 }, [
                          _createElementVNode("div", { class: "row items-center no-wrap q-pb-xs" }, [
                            _createElementVNode("div", { class: "text-subtitle2 col ellipsis" }, [
                              _createTextVNode(_toDisplayString(_ctx.t('Chat')), 1),
                              (_ctx.unreadTotal)
                                ? (_openBlock(), _createElementBlock("span", {
                                    key: 0,
                                    class: "text-caption m-muted"
                                  }, " · " + _toDisplayString(_ctx.fill(_ctx.t('{count} unread'), {count: _ctx.unreadTotal})), 1))
                                : _createCommentVNode("", true)
                            ]),
                            _createVNode(_component_q_btn, {
                              flat: "",
                              round: "",
                              dense: "",
                              icon: "sym_o_edit_square",
                              color: "primary",
                              "aria-label": _ctx.t('New message'),
                              onClick: _ctx.toggleNew
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.t('New message')), 1)
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }, 8, ["aria-label", "onClick"]),
                            _createVNode(_component_q_btn, {
                              flat: "",
                              round: "",
                              dense: "",
                              icon: "sym_o_done_all",
                              disable: !_ctx.unreadTotal,
                              "aria-label": _ctx.t('Mark all read'),
                              onClick: _ctx.markAllRead
                            }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_tooltip, null, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.t('Mark all read')), 1)
                                  ]),
                                  _: 1
                                })
                              ]),
                              _: 1
                            }, 8, ["disable", "aria-label", "onClick"]),
                            _createVNode(_component_q_btn, {
                              flat: "",
                              dense: "",
                              "no-caps": "",
                              size: "sm",
                              color: "primary",
                              icon: "sym_o_open_in_new",
                              label: _ctx.t('Open in Comms'),
                              onClick: $event => (_ctx.go(_ctx.linkBase))
                            }, null, 8, ["label", "onClick"])
                          ]),
                          (_ctx.newOpen)
                            ? (_openBlock(), _createElementBlock("div", {
                                key: 0,
                                class: "q-pb-sm"
                              }, [
                                _createVNode(_component_m_drop_down, {
                                  modelValue: _ctx.newUserIds,
                                  "onUpdate:modelValue": $event => ((_ctx.newUserIds) = $event),
                                  name: "panelDirectUserIds",
                                  multiple: "",
                                  "server-search": "",
                                  "options-url": _ctx.url('searchUsers'),
                                  label: _ctx.t('To: one person or several')
                                }, null, 8, ["modelValue", "onUpdate:modelValue", "options-url", "label"]),
                                _createElementVNode("div", { class: "row q-gutter-x-sm q-mt-xs" }, [
                                  _createVNode(_component_q_btn, {
                                    dense: "",
                                    unelevated: "",
                                    "no-caps": "",
                                    color: "primary",
                                    icon: "sym_o_chat",
                                    label: _ctx.t('Start'),
                                    disable: !_ctx.newUserIds.length,
                                    loading: _ctx.busy,
                                    onClick: _ctx.startDirect
                                  }, null, 8, ["label", "disable", "loading", "onClick"]),
                                  _createVNode(_component_q_btn, {
                                    dense: "",
                                    flat: "",
                                    "no-caps": "",
                                    label: _ctx.t('Cancel'),
                                    onClick: $event => (_ctx.newOpen = false)
                                  }, null, 8, ["label", "onClick"])
                                ])
                              ]))
                            : _createCommentVNode("", true),
                          _createVNode(_component_q_separator),
                          _createElementVNode("div", {
                            class: "col scroll",
                            style: {"min-height":"0"}
                          }, [
                            (!_ctx.loaded)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 0,
                                  class: "q-pa-xs"
                                }, [
                                  (_openBlock(), _createElementBlock(_Fragment, null, _renderList(6, (n) => {
                                    return _createElementVNode("div", {
                                      key: n,
                                      class: "row items-center no-wrap q-py-sm q-gutter-x-sm"
                                    }, [
                                      _createVNode(_component_q_skeleton, {
                                        type: "QAvatar",
                                        size: "32px"
                                      }),
                                      _createElementVNode("div", { class: "col" }, [
                                        _createVNode(_component_q_skeleton, {
                                          type: "text",
                                          width: "45%"
                                        }),
                                        _createVNode(_component_q_skeleton, { type: "text" })
                                      ])
                                    ])
                                  }), 64))
                                ]))
                              : (_openBlock(), _createBlock(_component_q_list, {
                                  key: 1,
                                  dense: ""
                                }, {
                                  default: _withCtx(() => [
                                    (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.shownRooms, (room) => {
                                      return (_openBlock(), _createBlock(_component_q_item, {
                                        key: room.chatRoomId,
                                        clickable: "",
                                        class: "q-px-xs q-py-xs",
                                        "data-room": room.chatRoomId,
                                        onClick: $event => (_ctx.openRoom(room.chatRoomId))
                                      }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_item_section, {
                                            avatar: "",
                                            style: {"min-width":"40px"}
                                          }, {
                                            default: _withCtx(() => [
                                              (room.roomTypeEnumId === 'CrtDirect' && room.otherUserId)
                                                ? (_openBlock(), _createBlock(_component_m_avatar, {
                                                    key: 0,
                                                    name: room.displayName,
                                                    "user-id": room.otherUserId,
                                                    size: "32px"
                                                  }, null, 8, ["name", "user-id"]))
                                                : (_openBlock(), _createBlock(_component_q_avatar, {
                                                    key: 1,
                                                    size: "32px",
                                                    rounded: "",
                                                    class: "m-tonal text-primary",
                                                    icon: _ctx.roomIcon(room)
                                                  }, null, 8, ["icon"]))
                                            ]),
                                            _: 2
                                          }, 1024),
                                          _createVNode(_component_q_item_section, { style: {"min-width":"0"} }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_label, { class: "row no-wrap items-center" }, {
                                                default: _withCtx(() => [
                                                  _createElementVNode("span", {
                                                    class: _normalizeClass(["ellipsis", _ctx.countsOf(room).unread || _ctx.countsOf(room).mentions ? 'text-weight-bold' : ''])
                                                  }, _toDisplayString(room.displayName), 3),
                                                  _createVNode(_component_q_space),
                                                  _createElementVNode("span", { class: "text-caption m-muted text-no-wrap q-ml-xs" }, _toDisplayString(_ctx.shortWhen(room.lastMessage ? room.lastMessage.sentDate : room.lastEventDate)), 1)
                                                ]),
                                                _: 2
                                              }, 1024),
                                              _createVNode(_component_q_item_label, {
                                                caption: "",
                                                class: "row no-wrap items-center"
                                              }, {
                                                default: _withCtx(() => [
                                                  _createElementVNode("span", { class: "ellipsis col" }, _toDisplayString(_ctx.snippetOf(room)), 1),
                                                  (_ctx.countsOf(room).mentions)
                                                    ? (_openBlock(), _createBlock(_component_q_badge, {
                                                        key: 0,
                                                        color: "negative",
                                                        rounded: "",
                                                        class: "q-ml-xs"
                                                      }, {
                                                        default: _withCtx(() => [
                                                          _createTextVNode("@" + _toDisplayString(_ctx.countsOf(room).mentions), 1)
                                                        ]),
                                                        _: 2
                                                      }, 1024))
                                                    : (_ctx.countsOf(room).unread)
                                                      ? (_openBlock(), _createBlock(_component_q_badge, {
                                                          key: 1,
                                                          color: room.isMuted || room.notificationLevelEnumId !== 'CnlAll' ? 'grey-6' : 'primary',
                                                          rounded: "",
                                                          class: "q-ml-xs"
                                                        }, {
                                                          default: _withCtx(() => [
                                                            _createTextVNode(_toDisplayString(_ctx.countsOf(room).unread), 1)
                                                          ]),
                                                          _: 2
                                                        }, 1032, ["color"]))
                                                      : _createCommentVNode("", true)
                                                ]),
                                                _: 2
                                              }, 1024)
                                            ]),
                                            _: 2
                                          }, 1024)
                                        ]),
                                        _: 2
                                      }, 1032, ["data-room", "onClick"]))
                                    }), 128))
                                  ]),
                                  _: 1
                                })),
                            (_ctx.loaded && !_ctx.rooms.length)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 2,
                                  class: "column items-center q-pa-lg text-center"
                                }, [
                                  _createVNode(_component_q_icon, {
                                    name: "sym_o_forum",
                                    size: "40px",
                                    class: "m-muted"
                                  }),
                                  _createElementVNode("div", { class: "text-subtitle2 q-mt-sm" }, _toDisplayString(_ctx.t('No conversations yet')), 1),
                                  _createElementVNode("div", { class: "text-caption m-muted q-mb-sm" }, _toDisplayString(_ctx.t('Message a colleague: one person, or several for a group.')), 1),
                                  _createVNode(_component_q_btn, {
                                    unelevated: "",
                                    "no-caps": "",
                                    color: "primary",
                                    icon: "sym_o_person_add",
                                    label: _ctx.t('New message'),
                                    onClick: $event => (_ctx.newOpen = true)
                                  }, null, 8, ["label", "onClick"])
                                ]))
                              : _createCommentVNode("", true),
                            (_ctx.loaded && _ctx.rooms.length > _ctx.shownRooms.length)
                              ? (_openBlock(), _createElementBlock("div", {
                                  key: 3,
                                  class: "text-center q-py-sm"
                                }, [
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    dense: "",
                                    "no-caps": "",
                                    size: "sm",
                                    color: "primary",
                                    label: _ctx.t('All conversations'),
                                    onClick: $event => (_ctx.go(_ctx.linkBase))
                                  }, null, 8, ["label", "onClick"])
                                ]))
                              : _createCommentVNode("", true)
                          ])
                        ], 64))
                      : (_openBlock(), _createElementBlock(_Fragment, { key: 1 }, [
                          (_ctx.roomComponent)
                            ? (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.roomComponent), {
                                key: _ctx.roomId,
                                base: _ctx.base,
                                "room-id": _ctx.roomId,
                                me: _ctx.me,
                                prefs: _ctx.prefs,
                                compact: "",
                                "can-go-back": "",
                                "link-base": _ctx.linkBase,
                                onBack: _ctx.closeRoom,
                                onLeft: _ctx.onLeft,
                                onChanged: _ctx.scheduleRooms,
                                onRead: _ctx.onRoomRead
                              }, null, 40, ["base", "room-id", "me", "prefs", "link-base", "onBack", "onLeft", "onChanged", "onRead"]))
                            : (_openBlock(), _createElementBlock("div", {
                                key: 1,
                                class: "q-pa-sm"
                              }, [
                                _createElementVNode("div", { class: "row items-center no-wrap q-mb-md" }, [
                                  _createVNode(_component_q_btn, {
                                    flat: "",
                                    round: "",
                                    dense: "",
                                    icon: "sym_o_arrow_back",
                                    "aria-label": _ctx.t('Back'),
                                    onClick: _ctx.closeRoom
                                  }, null, 8, ["aria-label", "onClick"]),
                                  _createVNode(_component_q_skeleton, {
                                    type: "text",
                                    width: "50%",
                                    class: "q-ml-sm"
                                  })
                                ]),
                                (_openBlock(), _createElementBlock(_Fragment, null, _renderList(5, (n) => {
                                  return _createElementVNode("div", {
                                    key: n,
                                    class: "row no-wrap q-gutter-x-sm q-mb-md"
                                  }, [
                                    _createVNode(_component_q_skeleton, {
                                      type: "QAvatar",
                                      size: "28px"
                                    }),
                                    _createElementVNode("div", { class: "col" }, [
                                      _createVNode(_component_q_skeleton, {
                                        type: "text",
                                        width: "35%"
                                      }),
                                      _createVNode(_component_q_skeleton, { type: "text" })
                                    ])
                                  ])
                                }), 64))
                              ]))
                        ], 64))
                  ], 4))
                }
            })();
            delete target.template;
        }
        return { component: component, style: null, scoped: false };
    } };
    moqui.qvtBundled["Shell/Root"] = { make: function (Vue) {
        const module = { exports: {} };
        const define = moqui.define;
        // the file's script as it is (not indented: a template string in it keeps its text)
        const returned = (function (exports) {
const t = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
const host = () => window.moquiDesktop;
const fill = (text, values) => String(text).replace(/\{(\w+)\}/g, (match, key) => (values[key] == null ? '' : String(values[key])));
// a count of shellCounts: a number, or a map's total or count, as m-qapps-root reads it
const countOf = (counts, key) => {
    const value = counts ? counts[key] : null;
    if (moqui.isPlainObject(value)) return Number(value.total !== undefined ? value.total : value.count) || 0;
    return Number(value) || 0;
};

module.exports = {
    name: 'DesktopShell',
    props: { state: Object, config: Object, shellData: Object, offline: Boolean, chatPath: String },
    provide() {
        // the bundled ChatPanel's screen: its path is the server's, so its requests go to the comms app's Chat transitions
        return { mScreen: { path: this.chatPath } };
    },
    data() {
        const data = this.shellData || {};
        return {
            drawer: false,
            // as m-qapps-root: what the client's components read of their $root
            appRootPath: '', basePath: '/apps', linkBasePath: '/qapps',
            userId: this.config.userId, username: this.config.username,
            notificationClient: null, socketState: null,
            apps: Array.isArray(data.apps) ? data.apps : [],
            shellCounts: moqui.isPlainObject(data.counts) ? data.counts : {},
            chatPanel: moqui.bundledComponent(this.chatPath),
            devicesOpen: false, devices: [], devicesLoaded: false
        };
    },
    computed: {
        accounts() { return (this.state && this.state.accounts) || []; },
        currentId() { return this.state && this.state.current; },
        keyStore() { return this.state && this.state.keyStore; },
        serverName() { return (this.config.server && this.config.server.name) || (this.config.shell && this.config.shell.title) || 'MR Moqui'; },
        userName() { return this.config.userFullName || this.config.username || ''; },
        // the shell's counts as m-qapps-root shows them (its keys chat, email, messages, notifications), each opening its
        // screen in an app window
        countItems() {
            const shell = (this.shellData && this.shellData.shell) || {};
            const commsRoot = String((this.config.calls && this.config.calls.appRoot) || '/qapps/BasicComms');
            const items = [
                { key: 'chat', icon: 'sym_o_chat', label: t('Chat'), link: commsRoot + '/Chat' },
                { key: 'email', icon: 'sym_o_mail', label: t('Mail'), link: shell.mailScreen || commsRoot + '/Inbox' },
                shell.messagesScreen ? { key: 'messages', icon: 'sym_o_forum', label: t('Messages'), link: shell.messagesScreen } : null,
                { key: 'notifications', icon: 'sym_o_notifications', label: t('Notifications'), link: commsRoot }
            ];
            return items.filter((item) => item).map((item) => Object.assign(item, { count: countOf(this.shellCounts, item.key) }));
        }
    },
    watch: {
        // each count as a number for the tray and the badge, as m-qapps-root hands them
        shellCounts: {
            handler(counts) {
                const desktop = host();
                if (!desktop || !desktop.setCounts) return;
                const out = {};
                Object.keys(counts || {}).forEach((key) => { out[key] = countOf(counts, key); });
                desktop.setCounts(out);
            },
            immediate: true
        },
        // the socket back after an offline start: the shell opens again, with what the server has now
        socketState(state) { if (state === 'open' && this.offline) location.reload(); }
    },
    methods: {
        t,
        // what a host command that failed says, in a toast
        failed(err) { this.$q.notify({ type: 'negative', message: String((err && err.message) || err) }); },
        // a link of the client's components (ChatPanel's Open in Comms, a room it cannot open here, a toast's link): the
        // server's page in an app window
        setUrl(url) { if (url) host().openRemote(String(url)).catch((err) => this.failed(err)); },
        switchAccount(id) { if (id !== this.currentId) host().switchAccount(id).catch((err) => this.failed(err)); },
        addAccount() { host().addAccount().catch((err) => this.failed(err)); },
        signOut() {
            this.$q.dialog({ title: t('Sign out'), message: t('Sign out of this account on this computer? Its device key is revoked.'), cancel: true })
                .onOk(() => host().signOut().catch((err) => this.failed(err)));
        },
        // the user's apps and devices (/qapps/clientDevices), each to sign out; this app's own signs out here too
        openDevices() {
            this.devicesOpen = true;
            this.devicesLoaded = false;
            moqui.request('/qapps/clientDevices').then((result) => {
                this.devices = (result.json && result.json.deviceList) || [];
                this.devicesLoaded = true;
            }, (err) => { this.devicesLoaded = true; this.failed(err && err.status ? t('The devices could not be read') : err); });
        },
        signOutDevice(device) {
            if (device.current) { this.devicesOpen = false; this.signOut(); return; }
            this.$q.dialog({ title: t('Sign out'), message: fill(t('Sign {name} out? It needs to sign in again.'), { name: device.deviceName }), cancel: true })
                .onOk(() => moqui.request('/qapps/revokeClientDevice', { method: 'POST', params: { loginKey: device.loginKey } })
                    .then(() => { this.devices = this.devices.filter((other) => other.loginKey !== device.loginKey); }, (err) => this.failed(err && err.status ? t('Not signed out') : err)));
        },
        when(millis) { return millis ? new Date(Number(millis)).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : ''; },
        // as m-qapps-root, for the client's helpers (moqui.notifyMessages, handleAjaxError): the shell keeps no notify
        // history, and a 401 has the host check the key (it shows the sign-in when the key is no longer taken)
        addNotify() {},
        reLoginCheckShow() {
            // the host checks the key on a 401 of its own; the sign-in shows once it has found the key no longer taken
            setTimeout(() => host().state().then((state) => { if (!state || !state.signedIn) location.reload(); }, () => {}), 2000);
        },
        onNotification(message) {
            if (!message || message.topic !== 'shellCounts') return;
            let counts = message.message;
            if (moqui.isString(counts)) { try { counts = JSON.parse(counts); } catch (err) { counts = null; } }
            if (moqui.isPlainObject(counts)) this.shellCounts = counts;
        }
    },
    created() {
        // the root the client's helpers talk to (toasts, links, a 401), as m-qapps-root is on /qapps
        moqui.webrootVue = this;
        moqui.appRootPath = '';
        // the account's /notws, held by the host (moqui.openSocket): counts, notifications, the chat store
        if (this.config.notificationEndpoint) {
            const client = new moqui.NotificationClient('ws://' + location.host + this.config.notificationEndpoint);
            client.onState((state) => { this.socketState = state; });
            client.registerListener('ALL');
            client.registerListener('shellCounts', (message) => this.onNotification(message));
            this.notificationClient = client;
        }
    },
    mounted() {
        if (moqui.chat) moqui.chat.start(this);
        // my presence on the tray, as m-qapps-root hands it
        const desktop = host();
        if (desktop && desktop.setPresence && moqui.chat) this.$watch(() => moqui.chat.myPresence(), (status) => { if (status) desktop.setPresence(status); },
            { immediate: true });
        // the counts while the socket is down, every five minutes, as m-qapps-root polls them
        this.countsTimer = setInterval(() => {
            if (this.notificationClient && this.notificationClient.isOpen()) return;
            moqui.request('/shellCounts').then((result) => {
                const counts = result.json && moqui.isPlainObject(result.json.counts) ? result.json.counts : null;
                if (counts) this.shellCounts = counts;
            }, () => {});
        }, 5 * 60 * 1000);
    },
    unmounted() { clearInterval(this.countsTimer); }
};
        })(module.exports);
        const component = Object.assign({}, returned !== undefined ? returned : module.exports);
        {
            const target = component;
            target.render = (function () {
                const { resolveComponent: _resolveComponent, createVNode: _createVNode, toDisplayString: _toDisplayString, createTextVNode: _createTextVNode, withCtx: _withCtx, openBlock: _openBlock, createBlock: _createBlock, createCommentVNode: _createCommentVNode, renderList: _renderList, Fragment: _Fragment, createElementBlock: _createElementBlock, resolveDirective: _resolveDirective, withDirectives: _withDirectives, resolveDynamicComponent: _resolveDynamicComponent } = Vue

                return function render(_ctx, _cache) {
                  const _component_q_btn = _resolveComponent("q-btn")
                  const _component_q_toolbar_title = _resolveComponent("q-toolbar-title")
                  const _component_q_chip = _resolveComponent("q-chip")
                  const _component_q_badge = _resolveComponent("q-badge")
                  const _component_q_tooltip = _resolveComponent("q-tooltip")
                  const _component_q_item_label = _resolveComponent("q-item-label")
                  const _component_q_icon = _resolveComponent("q-icon")
                  const _component_q_item_section = _resolveComponent("q-item-section")
                  const _component_q_item = _resolveComponent("q-item")
                  const _component_q_separator = _resolveComponent("q-separator")
                  const _component_q_list = _resolveComponent("q-list")
                  const _component_q_menu = _resolveComponent("q-menu")
                  const _component_q_toolbar = _resolveComponent("q-toolbar")
                  const _component_q_header = _resolveComponent("q-header")
                  const _component_m_menu_icon = _resolveComponent("m-menu-icon")
                  const _component_q_drawer = _resolveComponent("q-drawer")
                  const _component_q_card_section = _resolveComponent("q-card-section")
                  const _component_q_card_actions = _resolveComponent("q-card-actions")
                  const _component_q_card = _resolveComponent("q-card")
                  const _component_q_dialog = _resolveComponent("q-dialog")
                  const _component_q_page = _resolveComponent("q-page")
                  const _component_q_page_container = _resolveComponent("q-page-container")
                  const _component_q_layout = _resolveComponent("q-layout")
                  const _directive_close_popup = _resolveDirective("close-popup")

                  return (_openBlock(), _createBlock(_component_q_layout, { view: "hHh lpR fFf" }, {
                    default: _withCtx(() => [
                      _createVNode(_component_q_header, {
                        elevated: "",
                        class: "bg-primary text-white"
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_toolbar, null, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_btn, {
                                flat: "",
                                round: "",
                                dense: "",
                                icon: "sym_o_menu",
                                "aria-label": _ctx.t('Menu'),
                                onClick: $event => (_ctx.drawer = !_ctx.drawer)
                              }, null, 8, ["aria-label", "onClick"]),
                              _createVNode(_component_q_toolbar_title, { class: "ellipsis" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.serverName), 1)
                                ]),
                                _: 1
                              }),
                              (_ctx.offline)
                                ? (_openBlock(), _createBlock(_component_q_chip, {
                                    key: 0,
                                    dense: "",
                                    square: "",
                                    color: "warning",
                                    "text-color": "black",
                                    icon: "sym_o_cloud_off"
                                  }, {
                                    default: _withCtx(() => [
                                      _createTextVNode(_toDisplayString(_ctx.t('Offline: showing what was last seen')), 1)
                                    ]),
                                    _: 1
                                  }))
                                : (_ctx.socketState === 'closed')
                                  ? (_openBlock(), _createBlock(_component_q_chip, {
                                      key: 1,
                                      dense: "",
                                      square: "",
                                      color: "grey-7",
                                      "text-color": "white",
                                      icon: "sym_o_sync_problem"
                                    }, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(_ctx.t('Reconnecting')), 1)
                                      ]),
                                      _: 1
                                    }))
                                  : _createCommentVNode("", true),
                              (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.countItems, (item) => {
                                return (_openBlock(), _createBlock(_component_q_btn, {
                                  key: item.key,
                                  flat: "",
                                  dense: "",
                                  "no-caps": "",
                                  icon: item.icon,
                                  "aria-label": item.count ? item.label + ': ' + item.count : item.label,
                                  onClick: $event => (_ctx.setUrl(item.link))
                                }, {
                                  default: _withCtx(() => [
                                    (item.count)
                                      ? (_openBlock(), _createBlock(_component_q_badge, {
                                          key: 0,
                                          floating: "",
                                          rounded: "",
                                          color: "red"
                                        }, {
                                          default: _withCtx(() => [
                                            _createTextVNode(_toDisplayString(item.count > 99 ? '99+' : item.count), 1)
                                          ]),
                                          _: 2
                                        }, 1024))
                                      : _createCommentVNode("", true),
                                    _createVNode(_component_q_tooltip, null, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(item.label), 1)
                                      ]),
                                      _: 2
                                    }, 1024)
                                  ]),
                                  _: 2
                                }, 1032, ["icon", "aria-label", "onClick"]))
                              }), 128)),
                              _createVNode(_component_q_btn, {
                                flat: "",
                                "no-caps": "",
                                class: "q-ml-sm",
                                icon: "sym_o_account_circle",
                                label: _ctx.userName
                              }, {
                                default: _withCtx(() => [
                                  _createVNode(_component_q_menu, {
                                    anchor: "bottom right",
                                    self: "top right"
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_list, { class: "m-shell-account-menu" }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_item_label, { header: "" }, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(_ctx.t('Accounts')), 1)
                                            ]),
                                            _: 1
                                          }),
                                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.accounts, (account) => {
                                            return _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                              key: account.id,
                                              clickable: "",
                                              active: account.id === _ctx.currentId,
                                              onClick: $event => (_ctx.switchAccount(account.id))
                                            }, {
                                              default: _withCtx(() => [
                                                _createVNode(_component_q_item_section, { avatar: "" }, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_icon, {
                                                      name: account.id === _ctx.currentId ? 'sym_o_check' : 'sym_o_person'
                                                    }, null, 8, ["name"])
                                                  ]),
                                                  _: 2
                                                }, 1024),
                                                _createVNode(_component_q_item_section, null, {
                                                  default: _withCtx(() => [
                                                    _createVNode(_component_q_item_label, null, {
                                                      default: _withCtx(() => [
                                                        _createTextVNode(_toDisplayString(account.display || account.username), 1)
                                                      ]),
                                                      _: 2
                                                    }, 1024),
                                                    _createVNode(_component_q_item_label, { caption: "" }, {
                                                      default: _withCtx(() => [
                                                        _createTextVNode(_toDisplayString(account.serverName || account.server), 1)
                                                      ]),
                                                      _: 2
                                                    }, 1024)
                                                  ]),
                                                  _: 2
                                                }, 1024)
                                              ]),
                                              _: 2
                                            }, 1032, ["active", "onClick"])), [
                                              [_directive_close_popup]
                                            ])
                                          }), 128)),
                                          _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                            clickable: "",
                                            onClick: _ctx.addAccount
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, { avatar: "" }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_icon, { name: "sym_o_person_add" })
                                                ]),
                                                _: 1
                                              }),
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.t('Add an account')), 1)
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }, 8, ["onClick"])), [
                                            [_directive_close_popup]
                                          ]),
                                          _createVNode(_component_q_separator),
                                          (_ctx.keyStore === 'file')
                                            ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_item_section, { avatar: "" }, {
                                                    default: _withCtx(() => [
                                                      _createVNode(_component_q_icon, {
                                                        name: "sym_o_warning",
                                                        color: "warning"
                                                      })
                                                    ]),
                                                    _: 1
                                                  }),
                                                  _createVNode(_component_q_item_section, null, {
                                                    default: _withCtx(() => [
                                                      _createVNode(_component_q_item_label, { caption: "" }, {
                                                        default: _withCtx(() => [
                                                          _createTextVNode(_toDisplayString(_ctx.t('No key store on this computer: the device key is kept in a file only you can read.')), 1)
                                                        ]),
                                                        _: 1
                                                      })
                                                    ]),
                                                    _: 1
                                                  })
                                                ]),
                                                _: 1
                                              }))
                                            : _createCommentVNode("", true),
                                          _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                            clickable: "",
                                            onClick: _ctx.openDevices
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, { avatar: "" }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_icon, { name: "sym_o_devices" })
                                                ]),
                                                _: 1
                                              }),
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.t('Apps and devices')), 1)
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }, 8, ["onClick"])), [
                                            [_directive_close_popup]
                                          ]),
                                          _withDirectives((_openBlock(), _createBlock(_component_q_item, {
                                            clickable: "",
                                            onClick: _ctx.signOut
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_item_section, { avatar: "" }, {
                                                default: _withCtx(() => [
                                                  _createVNode(_component_q_icon, { name: "sym_o_logout" })
                                                ]),
                                                _: 1
                                              }),
                                              _createVNode(_component_q_item_section, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.t('Sign out of this account')), 1)
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }, 8, ["onClick"])), [
                                            [_directive_close_popup]
                                          ])
                                        ]),
                                        _: 1
                                      })
                                    ]),
                                    _: 1
                                  })
                                ]),
                                _: 1
                              }, 8, ["label"])
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }),
                      _createVNode(_component_q_drawer, {
                        modelValue: _ctx.drawer,
                        "onUpdate:modelValue": $event => ((_ctx.drawer) = $event),
                        "show-if-above": "",
                        bordered: "",
                        width: 240
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_list, null, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_item_label, { header: "" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.t('Applications')), 1)
                                ]),
                                _: 1
                              }),
                              (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.apps, (app) => {
                                return (_openBlock(), _createBlock(_component_q_item, {
                                  key: app.name,
                                  clickable: "",
                                  onClick: $event => (_ctx.setUrl(app.path))
                                }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_item_section, { avatar: "" }, {
                                      default: _withCtx(() => [
                                        (app.imageType === 'icon' && app.image)
                                          ? (_openBlock(), _createBlock(_component_m_menu_icon, {
                                              key: 0,
                                              name: app.image
                                            }, null, 8, ["name"]))
                                          : (_openBlock(), _createBlock(_component_q_icon, {
                                              key: 1,
                                              name: "sym_o_apps"
                                            }))
                                      ]),
                                      _: 2
                                    }, 1024),
                                    _createVNode(_component_q_item_section, null, {
                                      default: _withCtx(() => [
                                        _createTextVNode(_toDisplayString(app.title || app.name), 1)
                                      ]),
                                      _: 2
                                    }, 1024),
                                    _createVNode(_component_q_item_section, { side: "" }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_icon, {
                                          name: "sym_o_open_in_new",
                                          size: "xs"
                                        })
                                      ]),
                                      _: 1
                                    })
                                  ]),
                                  _: 2
                                }, 1032, ["onClick"]))
                              }), 128)),
                              (!_ctx.apps.length)
                                ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_item_section, null, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_item_label, { caption: "" }, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(_ctx.offline ? _ctx.t('The apps show once the server is reached.') : _ctx.t('No applications')), 1)
                                            ]),
                                            _: 1
                                          })
                                        ]),
                                        _: 1
                                      })
                                    ]),
                                    _: 1
                                  }))
                                : _createCommentVNode("", true)
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["modelValue", "onUpdate:modelValue"]),
                      _createVNode(_component_q_dialog, {
                        modelValue: _ctx.devicesOpen,
                        "onUpdate:modelValue": $event => ((_ctx.devicesOpen) = $event)
                      }, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_card, { class: "m-shell-devices" }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_card_section, { class: "text-h6" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.t('Apps and devices')), 1)
                                ]),
                                _: 1
                              }),
                              _createVNode(_component_q_card_section, { class: "q-pt-none text-body2 m-muted" }, {
                                default: _withCtx(() => [
                                  _createTextVNode(_toDisplayString(_ctx.t('The apps signed in to your account with a key of their own. Sign one out if you no longer use it, or lost it.')), 1)
                                ]),
                                _: 1
                              }),
                              _createVNode(_component_q_list, { separator: "" }, {
                                default: _withCtx(() => [
                                  (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.devices, (device) => {
                                    return (_openBlock(), _createBlock(_component_q_item, {
                                      key: device.loginKey
                                    }, {
                                      default: _withCtx(() => [
                                        _createVNode(_component_q_item_section, { avatar: "" }, {
                                          default: _withCtx(() => [
                                            _createVNode(_component_q_icon, {
                                              name: device.current ? 'sym_o_computer' : 'sym_o_devices'
                                            }, null, 8, ["name"])
                                          ]),
                                          _: 2
                                        }, 1024),
                                        _createVNode(_component_q_item_section, null, {
                                          default: _withCtx(() => [
                                            _createVNode(_component_q_item_label, null, {
                                              default: _withCtx(() => [
                                                _createTextVNode(_toDisplayString(device.deviceName), 1),
                                                (device.current)
                                                  ? (_openBlock(), _createElementBlock("span", {
                                                      key: 0,
                                                      class: "text-positive"
                                                    }, " · " + _toDisplayString(_ctx.t('this app')), 1))
                                                  : _createCommentVNode("", true)
                                              ]),
                                              _: 2
                                            }, 1024),
                                            _createVNode(_component_q_item_label, { caption: "" }, {
                                              default: _withCtx(() => [
                                                _createTextVNode(_toDisplayString(_ctx.t('Last used')) + " " + _toDisplayString(_ctx.when(device.lastUsedDate || device.fromDate)), 1)
                                              ]),
                                              _: 2
                                            }, 1024)
                                          ]),
                                          _: 2
                                        }, 1024),
                                        _createVNode(_component_q_item_section, { side: "" }, {
                                          default: _withCtx(() => [
                                            _createVNode(_component_q_btn, {
                                              flat: "",
                                              dense: "",
                                              "no-caps": "",
                                              color: "negative",
                                              label: _ctx.t('Sign out'),
                                              onClick: $event => (_ctx.signOutDevice(device))
                                            }, null, 8, ["label", "onClick"])
                                          ]),
                                          _: 2
                                        }, 1024)
                                      ]),
                                      _: 2
                                    }, 1024))
                                  }), 128)),
                                  (_ctx.devicesLoaded && !_ctx.devices.length)
                                    ? (_openBlock(), _createBlock(_component_q_item, { key: 0 }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_item_section, { class: "m-muted" }, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(_ctx.t('No apps or devices')), 1)
                                            ]),
                                            _: 1
                                          })
                                        ]),
                                        _: 1
                                      }))
                                    : _createCommentVNode("", true)
                                ]),
                                _: 1
                              }),
                              _createVNode(_component_q_card_actions, { align: "right" }, {
                                default: _withCtx(() => [
                                  _withDirectives(_createVNode(_component_q_btn, {
                                    flat: "",
                                    "no-caps": "",
                                    label: _ctx.t('Close')
                                  }, null, 8, ["label"]), [
                                    [_directive_close_popup]
                                  ])
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      }, 8, ["modelValue", "onUpdate:modelValue"]),
                      _createVNode(_component_q_page_container, null, {
                        default: _withCtx(() => [
                          _createVNode(_component_q_page, { padding: "" }, {
                            default: _withCtx(() => [
                              _createVNode(_component_q_card, {
                                flat: "",
                                bordered: "",
                                class: "m-shell-chat q-pa-sm"
                              }, {
                                default: _withCtx(() => [
                                  (_ctx.chatPanel)
                                    ? (_openBlock(), _createBlock(_resolveDynamicComponent(_ctx.chatPanel), { key: 0 }))
                                    : (_openBlock(), _createElementBlock("div", {
                                        key: 1,
                                        class: "q-pa-md m-muted"
                                      }, _toDisplayString(_ctx.t('Chat is not available on this server.')), 1))
                                ]),
                                _: 1
                              })
                            ]),
                            _: 1
                          })
                        ]),
                        _: 1
                      })
                    ]),
                    _: 1
                  }))
                }
            })();
            delete target.template;
        }
        return { component: component, style: null, scoped: false };
    } };
    moqui.qvtBundled["Shell/SignIn"] = { make: function (Vue) {
        const module = { exports: {} };
        const define = moqui.define;
        // the file's script as it is (not indented: a template string in it keeps its text)
        const returned = (function (exports) {
const t = (text) => (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize(text) : text);
const host = () => window.moquiDesktop;
const fill = (text, values) => String(text).replace(/\{(\w+)\}/g, (match, key) => (values[key] == null ? '' : String(values[key])));
const message = (err) => String((err && (err.message || err.error)) || err || t('Sign-in failed'));

module.exports = {
    name: 'DesktopSignIn',
    // asked: Add an account, from the shell of a signed-in one (which Cancel goes back to)
    // retryServer: the server a sign-in that did not finish was to, to try again
    props: { state: Object, failed: String, asked: Boolean, retryServer: String },
    data() {
        const current = this.asked ? null
            : ((this.state && this.state.accounts) || []).find((account) => account.id === (this.state && this.state.current));
        return { server: this.retryServer || (current ? current.server : ''), username: current && !this.retryServer ? current.username : '', password: '', code: '',
            codeAsk: null, sent: false, busy: false, error: '', providers: [], waiting: false, accounts: (this.state && this.state.accounts) || [] };
    },
    // another server: its providers are not the last one's
    watch: { serverText() { this.providers = []; this.waiting = false; } },
    computed: {
        serverText() { return this.server.trim(); },
        ready() { return this.codeAsk ? !!this.code.trim() : !!(this.server.trim() && this.username.trim() && this.password); },
        canCancel() { return this.asked && this.accounts.some((account) => account.id === (this.state && this.state.current) && account.signedIn); }
    },
    methods: {
        t,
        submit() {
            if (!this.ready || this.busy) return;
            this.busy = true;
            this.error = '';
            host().signIn({ server: this.server.trim(), username: this.username.trim(), password: this.password, code: this.code.trim() }).then((answer) => {
                this.busy = false;
                // signed in: the host shows the account's shell
                if (answer && answer.status === 'code') {
                    this.codeAsk = { text: answer.text, sendable: answer.sendable || [] };
                    this.code = '';
                }
            }, (err) => { this.busy = false; this.error = message(err); });
        },
        // the server's identity providers, for its SSO sign-in in the system browser
        findProviders() {
            this.busy = true;
            this.error = '';
            host().ssoProviders(this.server.trim()).then((list) => {
                this.busy = false;
                this.providers = list || [];
                if (!this.providers.length) this.error = t('This server offers no single sign-on.');
            }, (err) => { this.busy = false; this.error = message(err); });
        },
        sso(provider) {
            this.error = '';
            this.busy = true;
            host().ssoStart(this.server.trim(), provider.id).then(() => { this.busy = false; this.waiting = true; },
                (err) => { this.busy = false; this.error = message(err); });
        },
        cancelSso() { host().ssoCancel().then(() => { this.waiting = false; }, (err) => { this.error = message(err); }); },
        back() { this.codeAsk = null; this.code = ''; this.sent = false; this.error = ''; },
        cancel() { host().switchAccount(this.state.current).catch((err) => { this.error = message(err); }); },
        sendCode(factor) {
            this.busy = true;
            this.sent = false;
            this.error = '';
            host().sendCode({ server: this.server.trim(), username: this.username.trim(), password: this.password, factor }).then(() => {
                this.busy = false;
                this.sent = true;
            }, (err) => { this.busy = false; this.error = message(err); });
        },
        choose(account) {
            if (account.signedIn) { host().switchAccount(account.id); return; }
            this.server = account.server;
            this.username = account.username;
            this.password = '';
            this.back();
        },
        remove(account) {
            this.$q.dialog({ title: t('Remove from this computer'), cancel: true,
                message: fill(t('Remove {name} on {server} from this computer? Its device key is revoked, and what was kept for it is deleted.'),
                    { name: account.display || account.username, server: account.serverName || account.server }) })
                .onOk(() => host().removeAccount(account.id).then((state) => { if (state) this.accounts = state.accounts || []; }, (err) => { this.error = message(err); }));
        }
    }
};
        })(module.exports);
        const component = Object.assign({}, returned !== undefined ? returned : module.exports);
        {
            const target = component;
            target.render = (function () {
                const { toDisplayString: _toDisplayString, createElementVNode: _createElementVNode, openBlock: _openBlock, createElementBlock: _createElementBlock, createCommentVNode: _createCommentVNode, resolveComponent: _resolveComponent, withCtx: _withCtx, createVNode: _createVNode, createBlock: _createBlock, renderList: _renderList, Fragment: _Fragment, createTextVNode: _createTextVNode, withModifiers: _withModifiers } = Vue

                return function render(_ctx, _cache) {
                  const _component_q_card_section = _resolveComponent("q-card-section")
                  const _component_q_input = _resolveComponent("q-input")
                  const _component_q_btn = _resolveComponent("q-btn")
                  const _component_q_card_actions = _resolveComponent("q-card-actions")
                  const _component_q_card = _resolveComponent("q-card")
                  const _component_q_form = _resolveComponent("q-form")
                  const _component_q_item_label = _resolveComponent("q-item-label")
                  const _component_q_icon = _resolveComponent("q-icon")
                  const _component_q_item_section = _resolveComponent("q-item-section")
                  const _component_q_tooltip = _resolveComponent("q-tooltip")
                  const _component_q_item = _resolveComponent("q-item")
                  const _component_q_list = _resolveComponent("q-list")

                  return (_openBlock(), _createElementBlock("div", { class: "m-shell-sign-in q-pa-md" }, [
                    _createVNode(_component_q_form, { onSubmit: _ctx.submit }, {
                      default: _withCtx(() => [
                        _createVNode(_component_q_card, {
                          flat: "",
                          bordered: ""
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_card_section, null, {
                              default: _withCtx(() => [
                                _createElementVNode("div", { class: "text-h6" }, _toDisplayString(_ctx.codeAsk ? _ctx.t('Authentication code') : _ctx.t('Sign in to a server')), 1),
                                (_ctx.failed || _ctx.error)
                                  ? (_openBlock(), _createElementBlock("div", {
                                      key: 0,
                                      role: "alert",
                                      class: "text-negative q-mt-sm"
                                    }, _toDisplayString(_ctx.error || _ctx.failed), 1))
                                  : _createCommentVNode("", true)
                              ]),
                              _: 1
                            }),
                            (!_ctx.codeAsk)
                              ? (_openBlock(), _createBlock(_component_q_card_section, {
                                  key: 0,
                                  class: "q-gutter-y-sm"
                                }, {
                                  default: _withCtx(() => [
                                    _createVNode(_component_q_input, {
                                      modelValue: _ctx.server,
                                      "onUpdate:modelValue": $event => ((_ctx.server) = $event),
                                      outlined: "",
                                      dense: "",
                                      label: _ctx.t('Server'),
                                      placeholder: "erp.example.com",
                                      autocomplete: "url",
                                      disable: _ctx.busy,
                                      autofocus: !_ctx.server
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "disable", "autofocus"]),
                                    _createVNode(_component_q_input, {
                                      modelValue: _ctx.username,
                                      "onUpdate:modelValue": $event => ((_ctx.username) = $event),
                                      outlined: "",
                                      dense: "",
                                      label: _ctx.t('Username'),
                                      autocomplete: "username",
                                      disable: _ctx.busy
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "disable"]),
                                    _createVNode(_component_q_input, {
                                      modelValue: _ctx.password,
                                      "onUpdate:modelValue": $event => ((_ctx.password) = $event),
                                      outlined: "",
                                      dense: "",
                                      type: "password",
                                      label: _ctx.t('Password'),
                                      autocomplete: "current-password",
                                      disable: _ctx.busy,
                                      autofocus: !!_ctx.server
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "disable", "autofocus"])
                                  ]),
                                  _: 1
                                }))
                              : (_openBlock(), _createBlock(_component_q_card_section, {
                                  key: 1,
                                  class: "q-gutter-y-sm"
                                }, {
                                  default: _withCtx(() => [
                                    _createElementVNode("div", null, _toDisplayString(_ctx.codeAsk.text), 1),
                                    _createVNode(_component_q_input, {
                                      modelValue: _ctx.code,
                                      "onUpdate:modelValue": $event => ((_ctx.code) = $event),
                                      outlined: "",
                                      dense: "",
                                      label: _ctx.t('Code'),
                                      autocomplete: "one-time-code",
                                      disable: _ctx.busy,
                                      autofocus: ""
                                    }, null, 8, ["modelValue", "onUpdate:modelValue", "label", "disable"]),
                                    (_ctx.codeAsk.sendable && _ctx.codeAsk.sendable.length)
                                      ? (_openBlock(), _createElementBlock("div", {
                                          key: 0,
                                          class: "row q-gutter-sm"
                                        }, [
                                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.codeAsk.sendable, (factor) => {
                                            return (_openBlock(), _createBlock(_component_q_btn, {
                                              key: factor.id,
                                              flat: "",
                                              dense: "",
                                              "no-caps": "",
                                              color: "primary",
                                              icon: "sym_o_send",
                                              label: _ctx.t('Send code to') + ' ' + factor.label,
                                              disable: _ctx.busy,
                                              onClick: $event => (_ctx.sendCode(factor.id))
                                            }, null, 8, ["label", "disable", "onClick"]))
                                          }), 128))
                                        ]))
                                      : _createCommentVNode("", true),
                                    (_ctx.sent)
                                      ? (_openBlock(), _createElementBlock("div", {
                                          key: 1,
                                          class: "text-positive"
                                        }, _toDisplayString(_ctx.t('Code sent')), 1))
                                      : _createCommentVNode("", true)
                                  ]),
                                  _: 1
                                })),
                            (!_ctx.codeAsk && (_ctx.providers.length || _ctx.waiting))
                              ? (_openBlock(), _createBlock(_component_q_card_section, {
                                  key: 2,
                                  class: "q-pt-none"
                                }, {
                                  default: _withCtx(() => [
                                    (_ctx.waiting)
                                      ? (_openBlock(), _createElementBlock("div", {
                                          key: 0,
                                          role: "status",
                                          class: "row items-center no-wrap q-gutter-x-sm"
                                        }, [
                                          _createElementVNode("span", { class: "col text-body2" }, _toDisplayString(_ctx.t('Continue in your browser; this app signs in when it is done.')), 1),
                                          _createVNode(_component_q_btn, {
                                            flat: "",
                                            dense: "",
                                            "no-caps": "",
                                            label: _ctx.t('Cancel'),
                                            onClick: _ctx.cancelSso
                                          }, null, 8, ["label", "onClick"])
                                        ]))
                                      : (_openBlock(), _createElementBlock("div", {
                                          key: 1,
                                          class: "column q-gutter-y-xs"
                                        }, [
                                          _createElementVNode("div", { class: "text-caption m-muted" }, _toDisplayString(_ctx.t('Or sign in with')), 1),
                                          (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.providers, (provider) => {
                                            return (_openBlock(), _createBlock(_component_q_btn, {
                                              key: provider.id,
                                              outline: "",
                                              "no-caps": "",
                                              color: "primary",
                                              icon: provider.icon || 'sym_o_login',
                                              label: provider.description || provider.id,
                                              disable: _ctx.busy,
                                              onClick: $event => (_ctx.sso(provider))
                                            }, null, 8, ["icon", "label", "disable", "onClick"]))
                                          }), 128))
                                        ]))
                                  ]),
                                  _: 1
                                }))
                              : _createCommentVNode("", true),
                            _createVNode(_component_q_card_actions, { align: "right" }, {
                              default: _withCtx(() => [
                                (!_ctx.codeAsk && !_ctx.providers.length && !_ctx.waiting)
                                  ? (_openBlock(), _createBlock(_component_q_btn, {
                                      key: 0,
                                      flat: "",
                                      "no-caps": "",
                                      label: _ctx.t('Single sign-on'),
                                      disable: _ctx.busy || !_ctx.server.trim(),
                                      onClick: _ctx.findProviders
                                    }, null, 8, ["label", "disable", "onClick"]))
                                  : _createCommentVNode("", true),
                                (_ctx.codeAsk)
                                  ? (_openBlock(), _createBlock(_component_q_btn, {
                                      key: 1,
                                      flat: "",
                                      "no-caps": "",
                                      label: _ctx.t('Back'),
                                      disable: _ctx.busy,
                                      onClick: _ctx.back
                                    }, null, 8, ["label", "disable", "onClick"]))
                                  : (_ctx.canCancel)
                                    ? (_openBlock(), _createBlock(_component_q_btn, {
                                        key: 2,
                                        flat: "",
                                        "no-caps": "",
                                        label: _ctx.t('Cancel'),
                                        disable: _ctx.busy,
                                        onClick: _ctx.cancel
                                      }, null, 8, ["label", "disable", "onClick"]))
                                    : _createCommentVNode("", true),
                                _createVNode(_component_q_btn, {
                                  type: "submit",
                                  unelevated: "",
                                  "no-caps": "",
                                  color: "primary",
                                  label: _ctx.t('Sign in'),
                                  loading: _ctx.busy,
                                  disable: !_ctx.ready
                                }, null, 8, ["label", "loading", "disable"])
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        })
                      ]),
                      _: 1
                    }, 8, ["onSubmit"]),
                    (_ctx.accounts.length)
                      ? (_openBlock(), _createBlock(_component_q_card, {
                          key: 0,
                          flat: "",
                          bordered: "",
                          class: "q-mt-md"
                        }, {
                          default: _withCtx(() => [
                            _createVNode(_component_q_list, { separator: "" }, {
                              default: _withCtx(() => [
                                _createVNode(_component_q_item_label, { header: "" }, {
                                  default: _withCtx(() => [
                                    _createTextVNode(_toDisplayString(_ctx.t('Accounts on this computer')), 1)
                                  ]),
                                  _: 1
                                }),
                                (_openBlock(true), _createElementBlock(_Fragment, null, _renderList(_ctx.accounts, (account) => {
                                  return (_openBlock(), _createBlock(_component_q_item, {
                                    key: account.id,
                                    clickable: "",
                                    onClick: $event => (_ctx.choose(account))
                                  }, {
                                    default: _withCtx(() => [
                                      _createVNode(_component_q_item_section, { avatar: "" }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_icon, {
                                            name: account.signedIn ? 'sym_o_check_circle' : 'sym_o_person'
                                          }, null, 8, ["name"])
                                        ]),
                                        _: 2
                                      }, 1024),
                                      _createVNode(_component_q_item_section, null, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_item_label, null, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(account.display || account.username), 1)
                                            ]),
                                            _: 2
                                          }, 1024),
                                          _createVNode(_component_q_item_label, { caption: "" }, {
                                            default: _withCtx(() => [
                                              _createTextVNode(_toDisplayString(account.serverName || account.server), 1),
                                              (!account.signedIn)
                                                ? (_openBlock(), _createElementBlock("span", { key: 0 }, " · " + _toDisplayString(_ctx.t('signed out')), 1))
                                                : _createCommentVNode("", true)
                                            ]),
                                            _: 2
                                          }, 1024)
                                        ]),
                                        _: 2
                                      }, 1024),
                                      _createVNode(_component_q_item_section, { side: "" }, {
                                        default: _withCtx(() => [
                                          _createVNode(_component_q_btn, {
                                            flat: "",
                                            round: "",
                                            dense: "",
                                            icon: "sym_o_delete",
                                            "aria-label": _ctx.t('Remove from this computer'),
                                            onClick: _withModifiers($event => (_ctx.remove(account)), ["stop"])
                                          }, {
                                            default: _withCtx(() => [
                                              _createVNode(_component_q_tooltip, null, {
                                                default: _withCtx(() => [
                                                  _createTextVNode(_toDisplayString(_ctx.t('Remove from this computer')), 1)
                                                ]),
                                                _: 1
                                              })
                                            ]),
                                            _: 1
                                          }, 8, ["aria-label", "onClick"])
                                        ]),
                                        _: 2
                                      }, 1024)
                                    ]),
                                    _: 2
                                  }, 1032, ["onClick"]))
                                }), 128))
                              ]),
                              _: 1
                            })
                          ]),
                          _: 1
                        }))
                      : _createCommentVNode("", true)
                  ]))
                }
            })();
            delete target.template;
        }
        return { component: component, style: null, scoped: false };
    } };
})(globalThis.moqui);
