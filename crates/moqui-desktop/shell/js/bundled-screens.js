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
})(globalThis.moqui);
