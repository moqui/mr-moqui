/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
"use strict";
(function () {
// This isolated fixture checks the controls, focus and unmount on either side of a real asynchronous
// transition. The duration of the visual animation is not part of those checks.
Quasar.QDialog.props.transitionDuration.default = 40;
Quasar.QMenu.props.transitionDuration.default = 40;
const until = async (check, what, ms) => {
    const end = Date.now() + (ms || 5000);
    await Vue.nextTick();
    for (;;) {
        const value = check();
        if (value) return value;
        if (Date.now() > end) throw new Error("waited for " + what);
        await new Promise((resolve) => setTimeout(resolve, 10));
    }
};
// The mock host and HTTP answers resolve locally. Flush their promises and the render they schedule.
const settle = async () => { await new Promise((resolve) => setTimeout(resolve, 0)); await Vue.nextTick(); };
const text = () => document.body.textContent;
const clone = (value) => JSON.parse(JSON.stringify(value));
// the rows of the room's timeline that show a text
const rows = (needle) => Array.from(document.querySelectorAll(".m-chat-msg")).filter((row) => row.textContent.includes(needle)).length;
const button = (scope, label) => Array.from((scope || document).querySelectorAll("button")).find((el) => el.textContent.trim() === label);
const answerDialog = async label => {
    const control = await until(() => {
        const dialog = document.querySelector(".q-dialog");
        return dialog && button(dialog, label);
    }, "the dialog's " + label);
    const dialog = control.closest(".q-dialog");
    control.click();
    // Quasar keeps a closing dialog in the DOM for its leave transition. Do not answer that old dialog again.
    await until(() => !dialog.isConnected, "the answered " + label + " dialog to unmount");
};

// ---- the server, as the host passes it on: online its answers, offline the store's (X-Moqui-Offline) and no POST ----
const plainHeaders = { get: () => null };
const offlineHeaders = { get: (name) => (name === "X-Moqui-Offline" ? "1" : null) };
const unreachableHeaders = { get: (name) => (name === "X-Moqui-Offline" ? "unreachable" : null) };
const message = (seq, body, extra) => Object.assign({ eventSeq: seq, type: "message", senderUserId: "bob", senderName: "Bob", sentDate: Date.now() - 600000 + seq * 1000,
    body: "<p>" + body + "</p>", bodyText: body, reactions: [], attachments: [] }, extra || {});
const room = { chatRoomId: "R1", roomTypeEnumId: "CrtPrivate", roomName: "Ops", displayName: "Ops", lastEventSeq: 6, isMember: true, unreadCount: 0 };
// what the account's store holds of the room (the changes feeds keep it)
const server = {
    online: true,
    events: [message(4, "an older one"), message(5, "kept here"), message(6, "my own words", { senderUserId: "ann", senderName: "Ann" })],
    compose: null, saveError: null
};
// an encrypted room, its messages as the store keeps them (decrypted in the page: moqui.chat.openEvents, stood in for)
const sealedRoom = Object.assign({}, room, { chatRoomId: "R3", roomName: "Sealed", displayName: "Sealed", encrypted: true, lastEventSeq: 3 });
const sealedEvents = [message(2, "sealed one", { encrypted: true }), message(3, "sealed two", { encrypted: true })];
const roomOf = (params) => (params.chatRoomId === "R3" ? sealedRoom : room);
const timelineOf = (params) => {
    let list = params.chatRoomId === "R3" ? sealedEvents : server.events;
    if (params.eventSeq != null && params.eventSeq !== "") list = list.filter((event) => Number(event.eventSeq) === Number(params.eventSeq));
    else if (params.afterSeq != null && params.afterSeq !== "") list = list.filter((event) => Number(event.eventSeq) > Number(params.afterSeq));
    return { eventList: clone(list), hasMoreBefore: false, hasMoreAfter: false };
};
const answers = {
    "/qapps/clientConfig": () => ({ userId: "ann", username: "ann", userFullName: "Ann", locale: "en", server: { name: "Test Server", clientApi: 4 },
        calls: { appRoot: "/qapps/BasicComms" }, notificationEndpoint: "/notws" }),
    "/shellData": () => ({ apps: [], counts: {} }),
    "/apps/l10nCatalog": () => ({}),
    "/apps/BasicComms/Chat/chatData": () => ({ me: { userId: "ann" }, prefs: {}, roomList: [room, sealedRoom] }),
    "/apps/BasicComms/Chat/roomView": (params) => ({ room: roomOf(params), member: { lastReadEventSeq: 6 }, access: { isMember: true, canPost: true },
        memberList: [{ userId: "ann", name: "Ann" }, { userId: "bob", name: "Bob" }], timeline: timelineOf({ chatRoomId: params.chatRoomId }) }),
    "/apps/BasicComms/Chat/timeline": timelineOf,
    "/apps/BasicComms/Chat/edit": (params) => ({ event: { type: "edit", eventSeq: 20, editsEventSeq: Number(params.eventSeq), body: params.body,
        bodyText: String(params.body).replace(/<[^>]*>/g, ""), senderUserId: "ann", sentDate: Date.now() } }),
    "/apps/BasicComms/Inbox/Mail/inboxData": () => ({ accountList: [], mailboxList: [], searchList: [], totals: {}, me: { userId: "ann" }, prefs: {} }),
    "/apps/BasicComms/Inbox/Mail/threads": () => ({ threadList: [], hasMore: false, nextCursor: null, storeList: ["email"], failedStores: [], termList: [] }),
    "/apps/BasicComms/Inbox/Compose/composeData": () => ({ compose: clone(server.compose || { identityList: [{ value: "acct1", label: "ann@example.com" }],
        fromValue: "acct1", to: [], kind: "new" }) }),
    "/apps/BasicComms/Inbox/Compose/saveDraft": () => ({ emailMessageId: "E9", draftStamp: 77 })
};
const requests = [];
const asked = (path, method) => requests.filter((request) => request.path === path && request.method === (method || "GET"));
moqui.request = (url, options) => {
    const path = String(url).split("?")[0];
    const method = (options && options.method) || "GET";
    const params = options && moqui.isPlainObject(options.params) ? options.params : {};
    const request = { path, method, params, events: 0 };
    requests.push(request);
    const answer = answers[path];
    if (!server.online && method !== "GET") {
        moqui.noteAnswer(502, unreachableHeaders);
        return Promise.reject({ ok: false, status: 502, headers: unreachableHeaders, text: "The server could not be reached", json: undefined });
    }
    if (path === "/apps/BasicComms/Inbox/Compose/saveDraft" && server.saveError) {
        return Promise.reject({ ok: false, status: 503, headers: plainHeaders, json: { errors: server.saveError } });
    }
    const headers = server.online ? plainHeaders : offlineHeaders;
    moqui.noteAnswer(200, headers);
    const json = answer ? answer(params) : {};
    // how many events the answer has, for a check of how often a page decrypts them
    request.events = ((json.eventList || (json.timeline && json.timeline.eventList)) || []).length;
    return Promise.resolve({ ok: true, status: 200, headers, json, text: "" });
};

// Chat makes this browser's crypto device when it opens (Comms-Messaging §6.5): stood in for, so no module loads here
const madeDevices = [];
moqui.crypto.ensureDevice = () => { madeDevices.push(Date.now()); return Promise.reject({ reasonCode: "standIn", errors: "No crypto device in this test" }); };

// ---- the host: its outbox (the test moves an operation through its states, as the flusher does), drafts and /notws ----
const MD4_KINDS = ["chat-send", "chat-read", "mail-send", "mail-state"];
const MD5_KINDS = MD4_KINDS.concat(["chat-edit", "chat-redact", "chat-react", "poll-vote", "internal-send"]);
const ops = [];
const listeners = [];
const host = { kinds: { clientApi: 3, kinds: MD4_KINDS, internalSend: false }, added: [], retried: [], discarded: [], pendingOpen: null };
const tell = (event) => listeners.slice().forEach((fn) => fn(event || { kind: "changed" }));
const opOf = (id) => ops.find((op) => op.id === id);
// an operation moved on by the host (its state, error, refusal or conflict), and the pages told
const move = (id, fields) => { Object.assign(opOf(id), fields); tell(); };
const drop = (id) => { const at = ops.findIndex((op) => op.id === id); if (at >= 0) ops.splice(at, 1); };
// a message's change answered, told as the flusher tells it: changed first (the pages' list no longer has it), then sent,
// which names its room and message
const answered = async (op, json) => {
    tell();
    await until(() => !moqui.chat.state.changes.some((change) => change.clientRef === op.id), "the answered operation to leave the outbox");
    tell({ kind: "sent", op: op.kind, clientRef: op.id, chatRoomId: op.payload.chatRoomId, eventSeq: op.payload.eventSeq, threadRootEventSeq: null, json });
};
let added = 0;
const outbox = {
    add: (kind, payload, files) => {
        let taken = kind;
        if (kind === "mail-send" && payload.fields && payload.fields.fromValue === "internal") {
            if (!host.kinds.internalSend) return Promise.reject(new Error("An internal message needs a connection to send"));
            taken = "internal-send";
        }
        if (!host.kinds.kinds.includes(taken)) return Promise.reject(new Error(taken + " needs a connection to this server"));
        added += 1;
        const op = { id: payload.clientRef || "q" + added, kind: taken, state: "queued", attempts: 0, error: null, status: null, rejected: false, current: null,
            createdMs: Date.now() + added, payload: clone(payload), files: files || [] };
        ops.push(op);
        host.added.push(op);
        setTimeout(() => tell(), 0);
        return Promise.resolve({});
    },
    list: () => Promise.resolve(clone(ops)),
    kinds: () => Promise.resolve(clone(host.kinds)),
    retry: (id) => { host.retried.push(id); move(id, { state: "retry", error: null, status: null, rejected: false, attempts: 0 }); return Promise.resolve(true); },
    discard: (id) => { host.discarded.push(id); drop(id); tell(); return Promise.resolve(true); },
    on: (fn) => { listeners.push(fn); return () => {}; },
    unsent: () => Promise.resolve({ operations: clone(ops), drafts: [] })
};
const kept = new Map();
const draftOf = (key) => ({ key, value: kept.get(key), files: [], updatedMs: 1 });
const drafts = {
    get: (key) => Promise.resolve(kept.has(key) ? draftOf(key) : null),
    list: (prefix) => Promise.resolve(Array.from(kept.keys()).filter((key) => key.indexOf(prefix) === 0).map(draftOf)),
    put: (key, value) => { kept.set(key, String(value)); return Promise.resolve(); },
    remove: (key) => { kept.delete(key); return Promise.resolve(true); },
    saveFile: () => Promise.resolve(null)
};
// the account's /notws, held by the host: the test opens and closes it and brings its pushes
let socket = null;
const openSocket = () => { socket.readyState = 1; if (socket.onopen) socket.onopen(); };
const closeSocket = () => { socket.readyState = 3; if (socket.onclose) socket.onclose(); };
const push = (event) => socket.onmessage({ data: JSON.stringify({ topic: "chatRoom:R1", message: { kind: "event", chatRoomId: "R1", event } }) });
const shellState = window.Vue.reactive({ accounts: [{ id: "a1", username: "ann", server: "https://s.example.com", deviceMode: "personal" }], current: "a1",
    signedIn: true, keyStore: "os", clientApi: 4, deviceMode: "choose", loadToken: null });
window.moquiDesktop = {
    state: () => Promise.resolve(shellState),
    takeOpen: () => { const path = host.pendingOpen; host.pendingOpen = null; return Promise.resolve(path); },
    clearScopes: () => Promise.resolve(),
    socket: () => { socket = { readyState: 0, send() {}, close() {} }; return socket; },
    outbox, drafts,
    signOut: () => Promise.resolve(),
    openRemote: () => Promise.resolve(),
    setCounts() {}, setPresence() {}, storageScope: "a1"
};

// the shell, mounted once; open(path) shows a bundled screen as a link from the host does
let root = null;
const open = async (path, view) => {
    if (!root) {
        const shell = document.createElement("div");
        shell.id = "shell";
        document.body.appendChild(shell);
        host.pendingOpen = path;
        await moqui.loadScript("/files/crates/moqui-desktop/shell/js/shell.js");
        root = await until(() => moqui.webrootVue && moqui.webrootVue.commsItems && moqui.webrootVue, "the shell's root");
    } else {
        host.pendingOpen = path;
        window.moquiShell.poke();
    }
    await until(() => root.view === view && root.$refs.view, "the " + view + " screen");
    return root.$refs.view;
};
const openRoom = async () => {
    const chat = await open("/qapps/BasicComms/Chat?chatRoomId=R1", "chat");
    const vm = await until(() => chat.$refs.room && chat.$refs.room.roomId === "R1" && chat.$refs.room.state === "ready" && chat.$refs.room, "the room");
    await until(() => document.querySelector(".m-chat-msg"), "the room's messages");
    if (socket && socket.readyState !== 1) openSocket();
    return vm;
};
const eventOf = (vm, seq) => vm.events.find((event) => Number(event.eventSeq) === seq);
const change = (kind, pending) => document.querySelector(".m-chat-change[data-change='" + kind + "']" + (pending ? "[data-pending='" + pending + "']" : ""));

test("a message's edit, reaction and deletion wait in the app's outbox where the server takes them once; one refused says why, with Retry and Discard", async () => {
    const vm = await openRoom();
    // UI097: the real bundled Chat caller must give selected and unselected previews readable text.
    const chat=root.$refs.view, previousDark=Quasar.Dark.isActive, previousDraft=chat.drafts.R1;
    // The actual room and room-list helpers use the user's calendar day and clock, including DST boundaries.
    const priorZone=moqui.l10n.timeZone, priorLocale=moqui.l10n.locale, priorNow=Date.now;
    // Intl may put a narrow no-break space before AM and PM.
    const plain=text=>String(text).replace(/[\u202f\u00a0]/g,' ');
    try {
        moqui.l10n.setLocale('en_US');moqui.l10n.setTimeZone('America/Los_Angeles');
        Date.now=()=>Date.parse('2026-11-09T08:05:00Z');
        const beforeMidnight=Date.parse('2026-11-09T07:55:00Z'), afterMidnight=Date.parse('2026-11-09T08:01:00Z');
        const events=[message(1,'',{sentDate:beforeMidnight}),message(2,'',{type:'system',sentDate:afterMidnight,content:{action:'created'}})];
        const items=()=>vm.buildItems(events,{divider:false});
        assert.deepEqual(items().filter(item=>item.kind==='day').map(item=>item.label),['Yesterday','Today']);
        assert.equal(plain(items().find(item=>item.kind==='message').timeText),'11:55 PM');
        const fullTime=plain(items().find(item=>item.kind==='message').fullTime);
        assert.ok(fullTime.startsWith('Sunday, November 8, 2026') && fullTime.includes('11:55 PM'),fullTime);
        assert.equal(plain(items().find(item=>item.kind==='system').time),'12:01 AM');
        assert.equal(chat.shortWhen(beforeMidnight),'Sun');
        assert.equal(plain(chat.shortWhen(afterMidnight)),'12:01 AM');
        moqui.l10n.setTimeZone('Europe/Berlin');
        assert.deepEqual(items().filter(item=>item.kind==='day').map(item=>item.label),['Today']);
        assert.equal(plain(chat.shortWhen(afterMidnight)),'9:01 AM','the same instant follows the configured zone');
        // The user's language words the clock and the day: German has a 24-hour clock and German day names.
        moqui.l10n.setLocale('de_DE');
        assert.equal(chat.shortWhen(afterMidnight),'9:01');
        assert.equal(chat.shortWhen(beforeMidnight - 2*86400000),'Sa','Saturday in Berlin');
        // COM-UI-007: the phone home and tray list and the shell's own times follow the same zone and language
        const panelWhen=moqui.bundledComponent('BasicComms/Panel/Chat').methods.shortWhen, shellWhen=moqui.bundledComponent('Shell/Root').methods.when;
        assert.equal(panelWhen(afterMidnight),'9:01','the home list as the room shows it');
        assert.equal(panelWhen(Date.parse('2025-06-01T10:00:00Z')),moqui.chat.shortWhen(Date.parse('2025-06-01T10:00:00Z')),'with the year, by calendar day');
        assert.ok(plain(shellWhen(afterMidnight)).includes('09:01'),shellWhen(afterMidnight));
        moqui.l10n.setLocale('en_US');
        moqui.l10n.setTimeZone('America/Los_Angeles');
        Date.now=()=>Date.parse('2026-11-02T07:30:00Z');
        const priorDay=vm.buildItems([message(1,'',{sentDate:Date.parse('2026-10-31T20:00:00Z')})],{divider:false});
        assert.equal(priorDay[0].label,'Yesterday','subtracting 24 hours near the end of a 25-hour day would pick today');
        Date.now=()=>Date.parse('2026-03-09T07:30:00Z');
        const springDay=vm.buildItems([message(1,'',{sentDate:Date.parse('2026-03-08T20:00:00Z')})],{divider:false});
        assert.equal(springDay[0].label,'Yesterday','yesterday also survives a 23-hour day');
        moqui.l10n.setTimeZone('Not/AZone');
        const browserClock=new Date(Date.now()).toLocaleTimeString(undefined,{hour:'numeric',minute:'2-digit'});
        assert.equal(plain(chat.shortWhen(Date.now())),plain(browserClock),'an invalid zone retains the browser fallback');
    } finally {
        Date.now=priorNow;moqui.l10n.setLocale(priorLocale);moqui.l10n.setTimeZone(priorZone);
    }
    const previewContrast=element=>{
        const context=document.createElement('canvas').getContext('2d',{willReadFrequently:true}), ancestors=[];
        for (let node=element;node;node=node.parentElement) ancestors.unshift(node);
        context.fillStyle='#fff';context.fillRect(0,0,1,1);
        for (const node of ancestors) {context.fillStyle=getComputedStyle(node).backgroundColor;context.fillRect(0,0,1,1);}
        const luminance=()=>Array.from(context.getImageData(0,0,1,1).data).slice(0,3).map(value=>{
            const channel=value/255;return channel<=.04045?channel/12.92:Math.pow((channel+.055)/1.055,2.4);
        }).reduce((sum,value,index)=>sum+value*[.2126,.7152,.0722][index],0);
        const background=luminance(), style=getComputedStyle(element);
        context.globalAlpha=Number(style.opacity);context.fillStyle=style.color;context.fillRect(0,0,1,1);
        const foreground=luminance();return (Math.max(background,foreground)+.05)/(Math.min(background,foreground)+.05);
    };
    try {
        chat.drafts.R1='UI097 draft marker';
        for (const dark of [false,true,false]) {
            Quasar.Dark.set(dark);await Vue.nextTick();
            for (const [id,selected] of [['R1',true],['R3',false]]) {
                const row=chat.$el.querySelector('[data-room="'+id+'"]');
                assert.ok(row,'the existing room list is mounted');
                assert.equal(row.classList.contains('q-item--active'),selected);
                const preview=row.querySelector('.q-item__label--caption .ellipsis.col');
                assert.ok(preview && preview.textContent.trim(),'the actual room preview has text');
                const ratio=previewContrast(preview);
                assert.ok(ratio>=4.5,JSON.stringify({dark,id,ratio,color:getComputedStyle(preview).color}));
            }
            const draft=chat.$el.querySelector('[data-room="R1"] .q-item__label--caption .text-negative');
            assert.ok(draft,'the authored draft indicator remains beside the preview');
            assert.equal(getComputedStyle(draft).color,dark?'rgb(255, 138, 128)':'rgb(193, 0, 21)',
                'the preview does not replace its sibling draft status color');
        }
    } finally {
        Quasar.Dark.set(previousDark);
        if (previousDraft===undefined) delete chat.drafts.R1; else chat.drafts.R1=previousDraft;
        await Vue.nextTick();
    }
    // UI101: shared multiple controls may emit null while searching or clearing. Keep the actual Chat actions usable.
    const chatBefore={newDirect:chat.newDirect,post:chat.post,scheduleRooms:chat.scheduleRooms,openRoom:chat.openRoom};
    const roomBefore={side:vm.side,addPeople:vm.addPeople,post:vm.post,reloadMeta:vm.reloadMeta};
    const config=chat.$.appContext.config, errorHandler=config.errorHandler, pickerErrors=[], pickerPosts=[], openedRooms=[];
    config.errorHandler=error=>{pickerErrors.push(String(error));};
    try {
        chat.post=(name,params)=>{pickerPosts.push({name,params});return Promise.resolve({chatRoomId:'UI101-direct'});};
        chat.scheduleRooms=()=>{};chat.openRoom=id=>openedRooms.push(id);
        chat.newDirect={open:true,userIds:null,busy:false};await Vue.nextTick();
        const start=await until(()=>Array.from(document.querySelectorAll('.q-dialog button')).find(control=>
            control.querySelector('.q-btn__content > .block')?.textContent === 'Start'),'the direct-message Start control');
        assert.equal(start.disabled,true,'searching or clearing without a recipient keeps Start disabled');
        chat.startDirect();assert.equal(pickerPosts.length,0,'an empty direct selection never posts');
        chat.newDirect.userIds=['bob'];await Vue.nextTick();
        assert.equal(start.disabled,false);
        chat.newDirect.userIds=null;await Vue.nextTick();
        assert.equal(start.disabled,true,'clearing a previously selected recipient keeps the dialog usable');
        chat.newDirect.userIds=['bob'];await Vue.nextTick();start.click();
        await until(()=>pickerPosts.length===1,'the mounted Start button to post');await Vue.nextTick();
        assert.deepEqual(pickerPosts[0],{name:'createDirect',params:{userIds:'bob'}},'the click event is not a recipient list');
        assert.deepEqual(openedRooms,['UI101-direct']);
        await until(()=>!start.isConnected,'the started direct-message dialog to close');
        vm.post=(name,params)=>{pickerPosts.push({name,params});return Promise.resolve({});};vm.reloadMeta=()=>{};
        vm.side='members';vm.addPeople={userIds:null,busy:false};await Vue.nextTick();
        const add=await until(()=>button(vm.$el,'Add'),'the room Add control');
        assert.equal(add.disabled,true,'clearing members keeps Add disabled');
        vm.addMembers();assert.equal(pickerPosts.length,1,'an empty member selection never posts');
        vm.addPeople.userIds=['bob'];await Vue.nextTick();add.click();
        await until(()=>pickerPosts.length===2,'the mounted Add button to post');await Vue.nextTick();
        assert.deepEqual(pickerPosts[1],{name:'addMembers',params:{chatRoomId:'R1',userIds:'bob'}});
        assert.deepEqual(pickerErrors,[],'null picker states and action clicks cause no Vue errors');
    } finally {
        Object.assign(chat,chatBefore);Object.assign(vm,roomBefore);await Vue.nextTick();config.errorHandler=errorHandler;
    }
    // UI098: exercise the bundled panel's lifecycle with the real loader context, without rendering it again.
    const panel=moqui.bundledComponent('BasicComms/Panel/Chat'), panels=[], panelLinks=[];
    const panelOf=id=>{
        const context=moqui.qvt.components['m-dynamic-container'].provide.call({id,curUrl:'/apps/BasicComms/Panel/Chat'}).mScreen;
        const instance=Object.assign(panel.data(),{mScreen:context,$el:document.createElement('div'),$emit:()=>{},home:false,
            $root:{basePath:'/apps',linkBasePath:'/qapps',setUrl:target=>panelLinks.push(target)}});
        Object.entries(panel.methods).forEach(([name,method])=>{instance[name]=method.bind(instance);});
        for (const name of ['inDialog','base','linkBase']) Object.defineProperty(instance,name,{get:()=>panel.computed[name].call(instance)});
        instance.refresh=()=>Promise.resolve();instance.roomView=()=>Promise.resolve({});
        panel.mounted.call(instance);panels.push(instance);return instance;
    };
    try {
        const tray=panelOf('m-tray-chat');
        assert.equal(tray.inDialog,false);assert.equal(tray.newOpen,false);
        tray.openRoom('R1');await Promise.resolve();
        const quick=panelOf('m-quick-create-body');
        assert.equal(quick.inDialog,true);assert.equal(quick.newOpen,true);
        assert.equal(quick.roomId,null,'quick create does not restore the tray room');
        assert.equal(quick.$el.closest('#m-quick-create-body'),null,'no container ID DOM wrapper is needed');
        const quickPosts=[],quickRooms=[],quickOpen=quick.openRoom;
        quick.post=(name,params)=>{quickPosts.push({name,params});return Promise.resolve({chatRoomId:'UI101-panel'});};
        quick.scheduleRooms=()=>{};quick.openRoom=id=>quickRooms.push(id);
        quick.newUserIds=null;quick.startDirect();
        assert.deepEqual(quickPosts,[],'the cleared quick picker does not post or become busy');assert.equal(quick.busy,false);
        quick.newUserIds=['bob'];quick.startDirect();await Promise.resolve();
        assert.deepEqual(quickPosts,[{name:'createDirect',params:{userIds:'bob'}}]);assert.deepEqual(quickRooms,['UI101-panel']);
        quick.openRoom=quickOpen;quick.newUserIds=['bob'];
        panel.beforeUnmount.call(quick);panels.splice(panels.indexOf(quick),1);
        const reopened=panelOf('m-quick-create-body');
        assert.equal(reopened.newOpen,true);assert.deepEqual(reopened.newUserIds,[],'a reopened picker has no previous recipients');
        reopened.openRoom('R2');await Promise.resolve();
        assert.deepEqual(panelLinks,['/qapps/BasicComms/Chat?chatRoomId=R2'],'quick create opens the chosen room through the shell');
        const restoredTray=panelOf('m-tray-chat');await Promise.resolve();
        assert.equal(restoredTray.newOpen,false);assert.equal(restoredTray.roomId,'R1','the quick-create choice does not replace the kept tray room');
    } finally { panels.forEach(instance=>{if (!instance.inDialog) instance.closeRoom();panel.beforeUnmount.call(instance);}); }
    const namedEditor = label => Array.from(document.querySelectorAll('.m-chat-composer [role="textbox"]'))
        .find(element => element.getAttribute('aria-label') === label);
    assert.ok(await until(() => namedEditor('Message Ops'), "the named room editor"));
    // Metadata updates keep the mounted editor and unsent draft; its actual accessible name follows the caption.
    const composer=vm.composers.main, richEditor=composer.$refs.editor, editor=richEditor.editor;
    const editorNode=editor.view.dom, originalName=vm.room.displayName, originalBody=composer.body;
    editor.commands.setContent('<p>Draft stays <strong>here</strong></p>');await Vue.nextTick();
    editor.commands.setTextSelection({from:3,to:8});editor.view.focus();
    const draft=composer.body, html=editor.getHTML(), selection=editor.state.selection.toJSON(), editorProps=editor.options.editorProps;
    let labelUpdates=0;const countLabelUpdate=()=>{labelUpdates++;};editor.on('update',countLabelUpdate);
    try {
        vm.room.displayName='Renamed Ops';await Vue.nextTick();
        assert.equal(namedEditor('Message Renamed Ops'),editorNode,'the mounted textbox takes the current room name');
        assert.equal(editorNode.closest('.m-rich-text').querySelector('.m-rich-text-label').textContent,'Message Renamed Ops');
        assert.equal(richEditor.editor,editor,'renaming does not recreate the editor');
        assert.equal(composer.body,draft);assert.equal(editor.getHTML(),html);
        assert.deepEqual(editor.state.selection.toJSON(),selection);
        assert.equal(document.activeElement,editorNode,'renaming preserves focus');
        assert.equal(labelUpdates,0,'a label change is not a content edit');
        assert.equal(editor.options.editorProps.handlePaste,editorProps.handlePaste);
        assert.equal(editor.options.editorProps.handleDrop,editorProps.handleDrop);
        assert.equal(editorNode.getAttribute('role'),'textbox');assert.equal(editorNode.getAttribute('aria-multiline'),'true');
    } finally {
        editor.off('update',countLabelUpdate);vm.room.displayName=originalName;
        editor.commands.setContent(originalBody || '');await Vue.nextTick();
    }
    // Open a thread in the same mounted room; both actual rich editors need distinct names, not just placeholders.
    vm.openThread(6);
    assert.ok(await until(() => namedEditor('Reply in the thread'), "the named thread editor"));
    assert.ok(namedEditor('Message Ops'), "the room editor keeps its distinct name while the thread is open");
    vm.closeSide();await Vue.nextTick();
    vm.hoverKey=null;vm.tapKey=null;await Vue.nextTick();
    const actionRow=vm.$el.querySelector('.m-chat-msg[data-seq="6"]');
    assert.equal(actionRow.tabIndex,0);assert.equal(actionRow.getAttribute('role'),'article');
    assert.ok(actionRow.getAttribute('aria-label').includes('Ann'));
    assert.equal(actionRow.querySelector('button[aria-label="More"]'),null,'message actions start without pointer hover');
    actionRow.focus();await Vue.nextTick();
    const more=actionRow.querySelector('button[aria-label="More"]');
    assert.ok(more);more.focus();more.click();
    const editAction=await until(() => Array.from(document.querySelectorAll('.q-menu .q-item')).find(item => item.textContent.trim().endsWith('Edit')), "the focused message menu");
    editAction.focus();await Vue.nextTick();
    assert.ok(document.body.contains(more),'moving focus into the teleported menu retains its toolbar');
    editAction.click();
    assert.ok(await until(() => document.querySelector('.m-chat-msg [role="textbox"][aria-label="Edit message"]'), "the named edit editor"));
    vm.cancelEdit();await Vue.nextTick();
    await until(() => !actionRow.querySelector('button[aria-label="More"]'), "the toolbar hidden after focus leaves for the composer");
    // a server below clientApi 4: the edit is asked of the server now, as before
    await vm.saveEdit({ eventSeq: 6, event: eventOf(vm, 6) }, "<p>first fix</p>");
    assert.equal(asked("/apps/BasicComms/Chat/edit", "POST").length, 1);
    assert.equal(host.added.length, 0);
    await until(() => text().includes("first fix"), "the edit answered");

    // the server takes a message's changes once by a clientRef: they go through the outbox and show as waiting
    host.kinds = { clientApi: 4, kinds: MD5_KINDS, internalSend: true };
    await moqui.chat.readKinds(window.moquiDesktop.outbox);
    await vm.saveEdit({ eventSeq: 6, event: eventOf(vm, 6) }, "<p>second fix</p>");
    assert.equal(asked("/apps/BasicComms/Chat/edit", "POST").length, 1, "not asked of the server");
    const edit = host.added[host.added.length - 1];
    assert.equal(edit.kind, "chat-edit");
    assert.deepEqual([edit.payload.chatRoomId, edit.payload.eventSeq, edit.payload.body], ["R1", 6, "<p>second fix</p>"]);
    assert.ok(edit.payload.clientRef, "taken once by its clientRef");
    const waiting = await until(() => change("chat-edit", "waiting"), "the edit waiting");
    assert.ok(waiting.textContent.includes("Edit: waiting to send"));
    assert.ok(!text().includes("second fix"), "shown when the server has it");
    vm.react({ eventSeq: 5, event: eventOf(vm, 5) }, "👍");
    const react = await until(() => host.added.find((op) => op.kind === "chat-react"), "the reaction queued");
    assert.equal(react.payload.key, "👍");
    await until(() => change("chat-react", "waiting"), "the reaction waiting");

    // the edit answered, told as the host tells it: changed (the list without it) first, then sent, which names its message
    drop(edit.id);
    await answered(edit, { event: { type: "edit", eventSeq: 21, editsEventSeq: 6, body: "<p>second fix</p>",
        bodyText: "second fix", senderUserId: "ann", sentDate: Date.now() } });
    await until(() => text().includes("second fix") && !change("chat-edit"), "the edit made");

    // a reaction taken back while it still waits is dropped from the outbox, never sent
    vm.react({ eventSeq: 4, event: eventOf(vm, 4) }, "🎉");
    const party = await until(() => host.added.find((op) => op.kind === "chat-react" && op.payload.key === "🎉"), "the second reaction queued");
    await until(() => change("chat-react", "waiting") && opOf(party.id), "it waits");
    const reacts = host.added.filter((op) => op.kind === "chat-react").length;
    vm.react({ eventSeq: 4, event: eventOf(vm, 4) }, "🎉");
    await until(() => host.discarded.includes(party.id), "the waiting reaction dropped");
    assert.equal(host.added.filter((op) => op.kind === "chat-react").length, reacts, "no reaction queued to take it back");
    assert.ok(!(eventOf(vm, 4).reactions || []).some((reaction) => reaction.key === "🎉" && reaction.mine));

    // the reaction refused (a 4xx): not made, with the server's reason; Retry sends it again, Discard drops it
    move(react.id, { state: "failed", error: "Reactions are off in this room", status: 403, rejected: true });
    const refused = await until(() => change("chat-react", "failed"), "the reaction refused");
    assert.ok(refused.textContent.includes("Reaction not made: Reactions are off in this room"));
    button(refused, "Retry").click();
    await until(() => host.retried.includes(react.id) && change("chat-react", "waiting"), "the reaction tried again");
    move(react.id, { state: "failed", error: "Reactions are off in this room", status: 403, rejected: true });
    button(await until(() => change("chat-react", "failed"), "refused again"), "Discard").click();
    await until(() => host.discarded.includes(react.id) && !change("chat-react"), "the reaction discarded");
    // the reaction shown at once is read again as the store has it: none
    await until(() => asked("/apps/BasicComms/Chat/timeline").some((request) => Number(request.params.eventSeq) === 5), "the message read again");
    await until(() => !(eventOf(vm, 5).reactions || []).length, "no reaction left");

    // a deletion waits too, the message shown until the server has removed it
    vm.redact({ eventSeq: 6, event: eventOf(vm, 6) });
    await answerDialog("Delete");
    const redact = await until(() => host.added.find((op) => op.kind === "chat-redact"), "the deletion queued");
    await until(() => change("chat-redact", "waiting"), "the deletion waiting");
    assert.ok(text().includes("second fix"));
    drop(redact.id);
    await answered(redact, {});
    await until(() => !text().includes("second fix") && !change("chat-redact"), "the message removed");
});

test("a lost answer then a retry shows one message; a push that comes twice shows once; a reconnect after a feed reset reads the room as the store has it", async () => {
    const vm = await openRoom();
    // sent through the outbox; its answer is lost (sending, then waiting to try again)
    const sent = await moqui.chat.send({ chatRoomId: "R1", url: "/apps/BasicComms/Chat/send", params: { body: "<p>sent once</p>" }, text: "sent once", threadRootEventSeq: null });
    assert.equal(sent.queued, true);
    const op = opOf(sent.clientRef);
    move(op.id, { state: "sending" });
    await until(() => document.querySelector(".m-chat-pending[data-pending='sending']"), "sending");
    move(op.id, { state: "retry", attempts: 1 });
    await until(() => document.querySelector(".m-chat-pending[data-pending='waiting']"), "waiting to try again");
    // the server had taken it: its echo (the host drops the operation by its clientRef), then the retry's answer, a duplicate
    const echo = message(9, "sent once", { senderUserId: "ann", senderName: "Ann", clientRef: op.id });
    drop(op.id);
    push(echo);
    tell();
    tell({ kind: "sent", op: "chat-send", clientRef: op.id, chatRoomId: "R1", threadRootEventSeq: null, json: { event: echo, eventSeq: 9, duplicate: true } });
    await until(() => rows("sent once") === 1 && !document.querySelector(".m-chat-pending"), "the message, once");
    await settle();
    assert.equal(rows("sent once"), 1, "not twice");

    // the same push from two sockets
    const twice = message(10, "pushed twice");
    push(twice);
    push(twice);
    await until(() => rows("pushed twice") > 0, "the pushed message");
    await settle();
    assert.equal(rows("pushed twice"), 1);

    // offline for a while; the changes feeds reset and the store reads the room again: an edit and a deletion made meanwhile
    server.events = server.events.map((event) => (event.eventSeq === 5 ? Object.assign({}, event, { body: "<p>changed while away</p>", bodyText: "changed while away", edited: true })
        : event.eventSeq === 4 ? Object.assign({}, event, { redacted: true, body: null, bodyText: null }) : event));
    // a reconnect alone reads only what came after the last message shown
    const before = asked("/apps/BasicComms/Chat/timeline").length;
    closeSocket();
    openSocket();
    await until(() => asked("/apps/BasicComms/Chat/timeline").length > before, "the reconnect's read");
    await settle();
    assert.ok(asked("/apps/BasicComms/Chat/timeline").slice(before).every((request) => Number(request.params.afterSeq) >= 10), "only what came after");
    assert.ok(!text().includes("changed while away"));
    // the host says the feeds changed another room: nothing read here
    const again = asked("/apps/BasicComms/Chat/timeline").length;
    tell({ kind: "rooms", rooms: ["R7"], reset: false });
    await settle();
    assert.equal(asked("/apps/BasicComms/Chat/timeline").length, again);
    // the host says the chat feed was reset: the messages shown are read again as the store has them
    tell({ kind: "rooms", rooms: [], reset: true });
    await until(() => text().includes("changed while away"), "the room as the store has it");
    assert.ok(!text().includes("an older one"), "the deleted message is gone");
    const read = asked("/apps/BasicComms/Chat/timeline").slice(again);
    assert.ok(read.some((request) => Number(request.params.afterSeq) < 5), "the messages shown read again");
    assert.equal(vm.state, "ready");
});

test("the Inbox: a thread change that conflicted says it changed on the server and reads the list again; a send that conflicted keeps a copy, opened in Compose", async () => {
    // The real message preview is deliberately sandboxed; Chromium reports the blocked injected script as a page problem.
    // Keep the mounted message-body assertions below. This allowance is confined to this case, not the suite.
    harness.allowProblems();
    // a thread's archive waiting, then the server met the thread changed
    await outbox.add("mail-state", { targets: [{ store: "email", threadId: "T1", rowKey: "email:T1" }], action: "archive" });
    const state = host.added[host.added.length - 1];
    await open("/qapps/BasicComms/Inbox", "mail");
    await until(() => document.querySelector(".m-inbox-outbox") && document.querySelector(".m-inbox-outbox").textContent.includes("1 change waits to be made"), "the change waiting");
    const lists = asked("/apps/BasicComms/Inbox/Mail/threads").length;
    move(state.id, { state: "conflict", current: { conflicts: [{ key: "email:E1", current: { archived: false }, message: "A new message came" }] } });
    const conflict = await until(() => document.querySelector(".m-inbox-outbox-change[data-outbox='conflict']"), "the conflict");
    assert.ok(conflict.textContent.includes("Archive"));
    assert.ok(conflict.textContent.includes("Changed on the server: the thread shows its state there (A new message came)"));
    await until(() => asked("/apps/BasicComms/Inbox/Mail/threads").length > lists, "the list read again");
    assert.ok(!document.querySelector(".m-inbox-outbox").textContent.includes("change waits"), "not counted as waiting");
    // a later change of the same thread waits: Retry would land after it and undo it, so it asks first
    await outbox.add("mail-state", { targets: [{ store: "email", threadId: "T1", rowKey: "email:T1" }], action: "unarchive" });
    const later = host.added[host.added.length - 1];
    await until(() => document.querySelector(".m-inbox-outbox").textContent.includes("1 change waits to be made"), "the later change waiting");
    button(document.querySelector(".m-inbox-outbox-change[data-outbox='conflict']"), "Retry").click();
    await answerDialog("Keep the later change");
    assert.ok(!host.retried.includes(state.id), "not retried over the later change");
    drop(later.id);
    tell();
    await until(() => !document.querySelector(".m-inbox-outbox").textContent.includes("change waits"), "the later change made");
    // and one made on the server from here since (online, not queued) asks too
    const inbox = root.$refs.view;
    // The same instant follows the signed-in user's zone, including the local-day label, rather than the browser's.
    const priorZone = moqui.l10n.timeZone, priorLocale = moqui.l10n.locale, priorNow = Date.now;
    // Intl may put a narrow no-break space before AM and PM.
    const plain = (text) => String(text).replace(/[\u202f\u00a0]/g, " ");
    try {
        Date.now = () => Date.parse("2026-11-09T00:30:00Z");
        moqui.l10n.setLocale("en_US");
        const messageAt = Date.parse("2026-11-08T12:00:00Z");
        const groupsFor = (dates) => inbox.$options.computed.groups.call({ threads: dates.map(date => ({ sortDate: Date.parse(date) })) });
        moqui.l10n.setTimeZone("America/New_York");
        assert.equal(plain(inbox.shortDate(messageAt)), "7:00 AM", "the message is today in the user's zone");
        assert.ok(plain(inbox.fullDate(messageAt)).startsWith("Sun, Nov 8, 7:00 AM"), "the header agrees with the server's reply quote");
        assert.ok(plain(inbox.longDate(messageAt)).includes("7:00 AM"), "message details use the same zone");
        assert.equal(groupsFor(["2026-11-08T12:00:00Z"])[0].key, "today", "the list group uses the user's current day too");
        moqui.l10n.setTimeZone("Europe/Berlin");
        assert.equal(inbox.shortDate(messageAt), "Nov 8", "the same instant is yesterday in Berlin");
        assert.ok(plain(inbox.fullDate(messageAt)).startsWith("Sun, Nov 8, 1:00 PM"), "the browser zone cannot pin the header's time");
        assert.ok(plain(inbox.longDate(messageAt)).includes("1:00 PM"));
        moqui.l10n.setLocale("de_DE");
        assert.equal(inbox.longDate(messageAt).includes("13:00") && inbox.longDate(messageAt).startsWith("Sonntag"), true, inbox.longDate(messageAt));
        moqui.l10n.setLocale("en_US");
        const groups = groupsFor(["2026-11-09T00:15:00Z", "2026-11-08T12:00:00Z", "2026-11-03T12:00:00Z", "2026-11-02T12:00:00Z", "2026-10-31T12:00:00Z"]);
        assert.equal(groups.map(group => group.key).join(","), "today,yesterday,week,month,m2026-10", "local-day grouping preserves the six-day and month boundaries");
        assert.equal(groups[4].label, "October 2026");
        moqui.l10n.setTimeZone("America/New_York");
        Date.now = () => Date.parse("2026-11-02T05:30:00Z");
        assert.equal(groupsFor(["2026-11-01T04:15:00Z"])[0].key, "yesterday", "yesterday includes the full 25-hour autumn day");
    } finally {
        Date.now = priorNow;
        moqui.l10n.setLocale(priorLocale);
        moqui.l10n.setTimeZone(priorZone);
    }
    await inbox.post("state", { targetsJson: JSON.stringify([{ store: "email", threadId: "T1", rowKey: "email:T1" }]), action: "flag" });
    button(document.querySelector(".m-inbox-outbox-change[data-outbox='conflict']"), "Retry").click();
    await answerDialog("Retry anyway");
    await until(() => host.retried.includes(state.id), "the change sent anyway");
    // refused: not made, with the reason; Discard leaves the thread as the server has it
    move(state.id, { state: "failed", error: "This thread is not yours", status: 403, rejected: true });
    const refused = await until(() => document.querySelector(".m-inbox-outbox-change[data-outbox='failed']"), "the refusal");
    assert.ok(refused.textContent.includes("Not made: This thread is not yours"));
    button(refused, "Discard").click();
    await answerDialog("Discard");
    await until(() => host.discarded.includes(state.id) && !document.querySelector(".m-inbox-outbox-change"), "the change discarded");

    // The existing mounted Inbox also checks its message disclosures, using two local bodies and no send action.
    const threadPath = "/apps/BasicComms/Inbox/Mail/thread", listPath = "/apps/BasicComms/Inbox/Mail/threads";
    const oldThreadAnswer = answers[threadPath], oldListAnswer = answers[listPath];
    const pat = { name: "Pat Sender", address: "pat@example.invalid" }, alex = { name: "Alex Reviewer", address: "alex@example.invalid", isMe: true };
    const to = [{ name: "Ann", address: "ann@example.com", isMe: true }];
    const mail = (id, from, body, date) => ({ key: "email:" + id, emailMessageId: id, store: "email", from, toList: to,
        ccList: [], bccList: [], attachmentList: [], linkList: [], subject: "Keyboard review", text: body, date });
    const earlier = mail("KEY1", pat, "Older body available by keyboard", Date.now() - 60000);
    const laterMessage = mail("KEY2", alex, "Latest body", Date.now());
    const row = { store: "email", threadId: "TKEY", rowKey: "email:TKEY", subject: "Keyboard review", messageCount: 2,
        participantList: [pat, alex], fromName: alex.name, fromAddress: alex.address, sortDate: laterMessage.date,
        isUnread: true, unreadCount: 0, labelList: [], linkList: [], folderKindEnumId: "EfkInbox" };
    answers[threadPath] = () => ({ thread: Object.assign(clone(row), { messageList: [clone(earlier), clone(laterMessage)], canSend: false }) });
    answers[listPath] = () => ({ threadList: [clone(row)], hasMore: false, storeList: ["email"] });
    // Quasar's real button handlers turn keyup into the activation; the shell's Enter shortcut must not take it.
    const keys = event => moqui.keymap.handle(event);
    const press = (control, key, code) => {
        control.focus();
        control.dispatchEvent(new KeyboardEvent("keydown", { key, keyCode: code, bubbles: true, cancelable: true }));
        control.dispatchEvent(new KeyboardEvent("keyup", { key, keyCode: code, bubbles: true, cancelable: true }));
    };
    document.addEventListener("keydown", keys);
    try {
        await inbox.loadList(true);
        const threadRow = await until(() => document.getElementById("inbox-row-email_TKEY"), "the keyboard thread row");
        assert.equal(threadRow.getAttribute("aria-label"), "Keyboard review. Pat Sender, Alex Reviewer. 2 messages. Unread");
        assert.ok(threadRow.querySelector("button"), "the concise row name preserves its individual actions");
        threadRow.click();
        let disclosure = await until(() => document.getElementById("inbox-toggle-email_KEY1"), "the collapsed older message");
        // An accessible account's sender is not necessarily this viewer, even when routing marks its alias isMe.
        const sharedSender = await until(() => document.getElementById("inbox-msg-email_KEY2"), "the shared-account message");
        assert.ok(sharedSender.textContent.includes("Alex Reviewer"), "the header shows the actual email sender");
        assert.ok(document.getElementById("inbox-toggle-email_KEY2").getAttribute("aria-label").startsWith("Collapse message from Alex Reviewer"));
        assert.equal(inbox.$options.methods.avatarName.call({me: {name: "Ann"}, personName: inbox.personName}, alex), "Alex Reviewer", "the viewer does not replace the sender avatar");
        assert.equal(inbox.senderName({store: "email", from: {address: "alias@example.invalid", isMe: true}}), "alias@example.invalid", "an unnamed alias uses its own address");
        assert.equal(inbox.senderName({store: "internal", from: {partyId: "ann", name: "Ann", isMe: true}}), "me", "internal self presentation stays intact");
        assert.equal(inbox.personName(to[0]), "me", "recipient and reply identity semantics stay intact");
        assert.equal(inbox.thread.messageList[1].from.isMe, true, "presentation does not rewrite the routing identity");
        assert.equal(sharedSender.querySelector('button[aria-label="Reply"]'), null, "the shared reader still cannot reply");
        assert.equal(disclosure.tagName, "BUTTON");
        assert.equal(disclosure.tabIndex, 0);
        assert.equal(disclosure.getAttribute("aria-expanded"), "false");
        assert.ok(disclosure.getAttribute("aria-label").startsWith("Read message from Pat Sender"));
        const reads = asked(threadPath).length;
        press(disclosure, "Enter", 13);
        disclosure = await until(() => {
            const control = document.getElementById("inbox-toggle-email_KEY1");
            return control && control.getAttribute("aria-expanded") === "true" && document.activeElement === control && control;
        }, "the expanded message and retained disclosure focus");
        assert.ok(disclosure.getAttribute("aria-label").startsWith("Collapse message from Pat Sender"));
        await until(() => {
            const frame = document.querySelector("#inbox-body-email_KEY1 iframe");
            return frame && frame.contentDocument && frame.contentDocument.body && frame.contentDocument.body.textContent.includes("Older body available by keyboard");
        }, "the older message body");
        const recipients = document.getElementById("inbox-recipients-email_KEY1");
        assert.equal(recipients.tagName, "BUTTON");
        assert.equal(recipients.tabIndex, 0);
        assert.ok(recipients.getAttribute("aria-label").startsWith("Message details from Pat Sender"));
        press(recipients, "Enter", 13);
        await until(() => recipients.getAttribute("aria-expanded") === "true", "recipient details expanded");
        assert.ok(document.getElementById(recipients.getAttribute("aria-controls")).textContent.includes("pat@example.invalid"));
        assert.equal(document.activeElement, recipients);
        press(recipients, " ", 32);
        await until(() => recipients.getAttribute("aria-expanded") === "false", "recipient details collapsed");
        assert.equal(document.activeElement, recipients);
        press(disclosure, "Enter", 13);
        disclosure = await until(() => {
            const control = document.getElementById("inbox-toggle-email_KEY1");
            return control && control.getAttribute("aria-expanded") === "false" && document.activeElement === control && control;
        }, "collapsed message focus");
        press(disclosure, " ", 32);
        await until(() => document.getElementById("inbox-toggle-email_KEY1").getAttribute("aria-expanded") === "true", "Space expands the message");
        assert.equal(asked(threadPath).length, reads, "disclosure Enter did not reopen the whole thread");

        // A message's flag names the next action after each successful change, just like its conversation row.
        const statePath = "/apps/BasicComms/Inbox/Mail/state", changesBefore = asked(statePath, "POST").length;
        const flag = document.querySelector('#inbox-msg-email_KEY1 button[aria-label="Flag"]');
        assert.ok(flag, "the unflagged message offers Flag");
        flag.click();
        await until(() => flag.getAttribute("aria-label") === "Remove flag", "the flagged message offers Remove flag");
        assert.equal(inbox.thread.messageList.find(message => message.key === "email:KEY1").isFlagged, true);
        assert.ok(threadRow.querySelector('button[aria-label="Remove flag"]'), "the conversation reflects its flagged message");
        flag.click();
        await until(() => flag.getAttribute("aria-label") === "Flag", "the unflagged message offers Flag again");
        assert.equal(inbox.thread.messageList.find(message => message.key === "email:KEY1").isFlagged, false);
        assert.ok(threadRow.querySelector('button[aria-label="Flag"]'), "the conversation flag is restored");
        const changes = asked(statePath, "POST").slice(changesBefore);
        assert.deepEqual(changes.map(request => request.params.action), ["flag", "unflag"], "one request per toggle");
        assert.ok(changes.every(request => JSON.stringify(JSON.parse(request.params.targetsJson)) ===
            JSON.stringify([{ store: "email", emailMessageId: "KEY1" }])), "only the selected message is changed");
    } finally {
        document.removeEventListener("keydown", keys);
        answers[listPath] = oldListAnswer;
        if (oldThreadAnswer) answers[threadPath] = oldThreadAnswer; else delete answers[threadPath];
        inbox.closeThread();
        await until(() => !inbox.thread, "the keyboard thread closed");
        await inbox.loadList(true);
    }

    // a mail whose draft was changed elsewhere: the server's draft stays, the user's version is kept as a copy
    const copy = { at: 1, email: true, clientRef: "mCOPY", queued: [{ file: "f1", name: "plan.pdf", size: 3, contentType: "application/pdf" }], conflictOf: { clientRef: "mc1", emailMessageId: "E5" },
        draft: { to: [{ value: "cy@example.com", label: "Cy" }], cc: [], bcc: [], subject: "Quarterly report (mine)", body: "<p>my words</p>", quoteHtml: null,
            fromValue: "acct1", parentEmailMessageId: null, isForward: null, attachments: [], uploads: [] } };
    kept.set("ironstone.compose.email.mCOPY", JSON.stringify(copy));
    ops.push({ id: "mc1", kind: "mail-send", state: "conflict", attempts: 1, error: null, status: null, rejected: false, createdMs: Date.now(),
        payload: { clientRef: "mc1", fields: { subject: "Quarterly report", toJson: JSON.stringify([{ value: "cy@example.com", label: "Cy" }]), fromValue: "acct1" } },
        current: { compose: { subject: "Quarterly report (theirs)" }, copyKey: "ironstone.compose.email.mCOPY", copyRef: "mCOPY" } });
    tell();
    const send = await until(() => document.querySelector(".m-inbox-outbox-item[data-outbox='conflict']"), "the send's conflict");
    assert.ok(send.textContent.includes("Changed elsewhere: your version is kept as a copy"));
    assert.ok(send.querySelector(".m-inbox-open-copy"), "Open the copy");
    assert.ok(button(send, "Retry") && button(send, "Discard"));
    // Discard: not sent, and the copy stays, listed to open
    button(send, "Discard").click();
    await answerDialog("Discard");
    const listed = await until(() => document.querySelector(".m-inbox-outbox-copy"), "the copy listed");
    assert.ok(listed.textContent.includes("Quarterly report (mine)"));
    assert.ok(host.discarded.includes("mc1"));
    // opened: a new draft of the user's version, saved on the server by its own clientRef; then no longer kept here
    button(listed, "Open").click();
    await until(() => document.querySelector(".m-compose-copy"), "the copy in Compose");
    await until(() => Array.from(document.querySelectorAll(".m-compose input")).some((input) => input.value === "Quarterly report (mine)"), "its subject");
    const saved = await until(() => asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").find((request) => request.params.clientRef === "mCOPY"), "the copy saved");
    assert.equal(saved.params.subject, "Quarterly report (mine)");
    assert.ok(saved.params.body.includes("my words"));
    await until(() => !kept.has("ironstone.compose.email.mCOPY"), "the copy on the server now");
    // with its file still to send it stays kept here, by the server's draft, but as a copy no longer
    const now = await until(() => kept.get("ironstone.compose.email.E9"), "kept by its draft");
    assert.equal(JSON.parse(now).conflictOf, undefined);
    await until(() => !document.querySelector(".m-inbox-outbox-copy"), "no longer listed as a copy");

    // a copyRef that is not a kept copy (a server draft's id, edited offline) opens nothing and touches nothing
    const edited = JSON.stringify({ at: 2, email: true, clientRef: "mE123", queued: [], draft: { emailMessageId: "E123", to: [], cc: [], bcc: [],
        subject: "Edited offline", body: "<p>mine</p>", fromValue: "acct1", attachments: [], uploads: [] } });
    kept.set("ironstone.compose.email.E123", edited);
    for (const ref of ["E123", "../x"]) {
        await open("/qapps/BasicComms/Inbox/Compose?copyRef=" + encodeURIComponent(ref), "compose");
        await until(() => text().includes("This copy is no longer kept on this computer."), "not a copy: " + ref);
        await settle();
        assert.ok(!document.querySelector(".m-compose-copy"));
    }
    assert.equal(kept.get("ironstone.compose.email.E123"), edited, "the offline edits kept as they were");
});

test("Compose: offline, an internal message waits in the outbox where the server takes its send once, and a server draft's send carries its stamp", async () => {
    const readyCompose = async (matches, description) => {
        const current = await until(() => {
            const candidate = root.$refs.view;
            return root.view === 'compose' && candidate && candidate.state === 'ready' && matches(candidate) && candidate;
        }, description);
        // The same Compose instance handles another URL; ready is set before its next recipient-control render.
        await Vue.nextTick();
        await until(() => ['to','cc','bcc'].every(field => {
            const ref = current.$refs['add_' + field], control = Array.isArray(ref) ? ref[0] : ref;
            if (!control) return !current.recipientFields.some(item => item.name === field);
            return control.$el && control.$el.nodeType === 1 && control.$el.isConnected && control.$el.querySelector('input');
        }), description + ' recipient controls');
        return current;
    };
    server.online = false;
    host.kinds = { clientApi: 4, kinds: MD5_KINDS, internalSend: true };
    server.compose = { identityList: [{ value: "internal", label: "Internal", isInternal: true }], fromValue: "internal", to: [{ value: "party:bob", label: "Bob", kind: "party" }],
        subject: "Lunch", kind: "new" };
    let compose = await open("/qapps/BasicComms/Inbox/Compose?subject=Lunch", "compose");
    await until(() => document.querySelector(".m-compose-offline") && text().includes("this message waits in the outbox"), "the internal message's banner");
    compose = await readyCompose(current => current.query.subject === 'Lunch' && current.draft.subject === 'Lunch' &&
        current.draft.fromValue === 'internal' && current.draft.to.some(item => item.value === 'party:bob'), 'the new internal draft');
    const before = host.added.length;
    compose.send();
    const internal = await until(() => host.added.length > before && host.added[host.added.length - 1], "the internal send queued");
    assert.equal(internal.kind, "internal-send");
    assert.equal(internal.payload.fields.fromValue, "internal");
    assert.equal(internal.payload.fields.subject, "Lunch");

    // a server that does not take it once: it needs a connection, kept as a draft
    host.kinds = { clientApi: 4, kinds: MD5_KINDS.filter((kind) => kind !== "internal-send"), internalSend: false };
    compose = await open("/qapps/BasicComms/Inbox/Compose?subject=Lunch2", "compose");
    await until(() => text().includes("an internal message needs a connection to send"), "needs a connection");
    compose = await readyCompose(current => current.query.subject === 'Lunch2' && current.draft.fromValue === 'internal' &&
        current.draft.to.some(item => item.value === 'party:bob'), 'the next internal draft');
    const count = host.added.length;
    compose.send();
    await until(() => text().includes("An internal message needs a connection to send; it is kept as a draft"), "kept as a draft");
    assert.equal(host.added.length, count);

    // the server's draft edited here: its send waits with the stamp it was edited from, so one saved elsewhere is a conflict
    server.compose = { identityList: [{ value: "acct1", label: "ann@example.com" }], fromValue: "acct1", to: [{ value: "cy@example.com", label: "Cy" }], subject: "Plan",
        kind: "draft", emailMessageId: "E5", clientRef: "mE5", draftStamp: 1234 };
    compose = await open("/qapps/BasicComms/Inbox/Compose?emailMessageId=E5", "compose");
    compose = await readyCompose(current => current.query.emailMessageId === 'E5' && current.draft.emailMessageId === 'E5' &&
        current.draftStamp === 1234, 'the server draft');
    compose.send();
    const mail = await until(() => host.added.find((op) => op.kind === "mail-send" && op.payload.fields.emailMessageId === "E5"), "the send queued");
    assert.equal(mail.payload.fields.baseStamp, "1234");
    assert.equal(mail.id, "mE5", "by the draft's clientRef");
    server.online = true;
    server.compose = null;
});

test("Compose keeps unsaved changes open without an identity or after storage failures, and recovery preserves the message", async () => {
    server.online = true;
    server.compose = { identityList: [], fromValue: null, to: [],
        subject: "Retain this draft", kind: "new" };
    await open("/qapps/BasicComms/Inbox/Compose?subject=RetainThisDraft", "compose");
    const compose = await until(() => root.$refs.view && root.$refs.view.state === "ready"
        && root.$refs.view.draft.subject === "Retain this draft" && root.$refs.view, "the composer");
    const setUrl = root.setUrl;
    const desktop = window.moquiDesktop;
    const put = drafts.put;
    const local = moqui.drafts.local;
    const request = moqui.request;
    const closed = [];
    root.setUrl = (url) => closed.push(url);
    try {
        compose.draft.body = "<p>Keep these words until they are saved.</p>";
        delete window.moquiDesktop;
        await window.Vue.nextTick();
        const identityBanner = document.querySelector(".m-compose-no-from");
        assert.ok(identityBanner && identityBanner.textContent.includes("No sending identity is available"));
        const accounts = identityBanner.querySelector("a");
        assert.equal(accounts.getAttribute("target"), "_blank", "account setup leaves the unsaved message open");
        assert.ok(accounts.getAttribute("href").endsWith("/BasicComms/Settings/MailAccounts"));
        const beforeSave = asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").length;
        await compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 0);
        assert.equal(asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").length, beforeSave, "missing From is explained before calling the service");
        assert.ok(compose.saveError.includes("Add a mail account"));
        assert.equal(compose.draft.subject, "Retain this draft");
        assert.equal(compose.draft.body, "<p>Keep these words until they are saved.</p>");
        compose.draft.fromValue = "identity:removed";
        server.compose = { identityList: [{ value: "acct1", label: "ann@example.com", signatureHtml: "<p>Ann</p>" }], fromValue: "acct1", to: [], subject: "", body: "" };
        button(identityBanner, "Refresh sending identities").click();
        await until(() => !compose.refreshingIdentities && !compose.noFrom, "the sending identity is available");
        assert.equal(compose.draft.fromValue, "acct1");
        assert.equal(compose.draft.subject, "Retain this draft");
        assert.equal(compose.draft.body, "<p>Keep these words until they are saved.</p>");
        assert.deepEqual(asked("/apps/BasicComms/Inbox/Compose/composeData").slice(-1)[0].params, {}, "identity refresh cannot repeat draft/reply initialization");
        window.moquiDesktop = desktop;
        compose.savedAt = 1;
        await window.Vue.nextTick();
        server.saveError = "Draft storage is temporarily unavailable";
        await compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 0, "closing cannot discard unsaved changes");
        assert.equal(compose.dirty, true);
        assert.equal(compose.draft.body, "<p>Keep these words until they are saved.</p>");
        assert.equal(compose.savedText, "Not saved", "the previous Saved time is no longer presented as current");
        let banner = document.querySelector(".m-compose-save-error");
        assert.ok(banner && banner.textContent.includes(server.saveError), "the save failure is visible beside the draft");
        assert.equal(banner.getAttribute("role"), "alert");
        server.saveError = null;
        const beforeRetry = asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").length;
        button(banner, "Retry save").click();
        await until(() => !compose.saving && !compose.saveError && !compose.dirty, "the retry saved");
        assert.equal(asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").length, beforeRetry + 1,
            "the first server-assigned draft id does not trigger another save");
        assert.equal(compose.saveTimer, null, "the assigned id does not leave a redundant autosave timer");
        assert.ok(!document.querySelector(".m-compose-save-error"));
        assert.equal(asked("/apps/BasicComms/Inbox/Compose/saveDraft", "POST").slice(-1)[0].params.body,
            "<p>Keep these words until they are saved.</p>");
        const priorZone = moqui.l10n.timeZone, priorLocale = moqui.l10n.locale;
        try {
            moqui.l10n.setLocale("en_US");
            const saved = {savedAt: Date.parse("2026-11-09T05:26:00Z"), mode: "email"};
            moqui.l10n.setTimeZone("America/New_York");
            assert.equal(compose.$options.computed.savedText.call(saved).replace(/[\u202f\u00a0]/g, " "), "Saved 12:26 AM", "the saved status uses the signed-in user's zone");
            moqui.l10n.setTimeZone("Europe/Berlin");
            assert.equal(compose.$options.computed.savedText.call(saved).replace(/[\u202f\u00a0]/g, " "), "Saved 6:26 AM", "the browser zone cannot pin the saved status");
        } finally {
            moqui.l10n.setLocale(priorLocale);
            moqui.l10n.setTimeZone(priorZone);
        }
        await compose.close();
        assert.equal(closed.length, 1, "closing now keeps the saved draft");
        assert.ok(closed[0].endsWith("/Inbox"));

        // A slow save acknowledges its own content, not edits made while it was in flight. Close waits for those too.
        const pending = [];
        moqui.request = (url, options) => {
            const answer = request(url, options);
            if (!String(url).endsWith("/Compose/saveDraft")) return answer;
            return answer.then((result) => new Promise((resolve, reject) => {
                pending.push({ params: clone(options.params), resolve: () => resolve(result), reject });
            }));
        };
        compose.draft.body = "<p>The first server snapshot.</p>";
        await window.Vue.nextTick();
        const firstSave = compose.save();
        await until(() => pending.length === 1, "the first server save waiting");
        compose.draft.body = "<p>New words while the server saves.</p>";
        await window.Vue.nextTick();
        pending[0].resolve();
        await until(() => pending.length === 2 || !compose.saving, "the old server answer handled");
        assert.equal(compose.dirty, true, "an old server answer cannot mark newer text saved");
        assert.equal(pending.length, 2, "newer text is saved without waiting for the debounce timer");
        assert.equal(pending[1].params.body, "<p>New words while the server saves.</p>");
        const closingServer = compose.close();
        assert.equal(closed.length, 1, "Close waits for the current content");
        pending[1].reject({ ok: false, status: 503, headers: plainHeaders, json: { errors: "The follow-up save failed" } });
        await Promise.all([firstSave, closingServer]);
        assert.equal(closed.length, 1, "a failed follow-up retains the composer");
        assert.equal(compose.dirty, true);
        assert.equal(compose.draft.body, "<p>New words while the server saves.</p>");
        assert.ok(compose.saveError.includes("follow-up save failed"));
        moqui.request = request;
        await compose.save();
        assert.equal(compose.dirty, false);
        assert.equal(compose.saveTimer, null, "acknowledged content leaves no redundant autosave");

        // An offline email cannot close just because the server failed: the host must persist the fallback.
        server.online = false;
        drafts.put = () => Promise.reject(new Error("Device storage is full"));
        compose.draft.body = "<p>These offline changes must stay open.</p>";
        await window.Vue.nextTick();
        await compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 1);
        assert.equal(compose.dirty, true);
        assert.ok(compose.saveError.includes("This device could not keep the draft"));
        drafts.put = put;
        button(document.querySelector(".m-compose-save-error"), "Retry save").click();
        await until(() => !compose.saving && !compose.saveError && !compose.dirty, "the offline draft persisted");
        assert.equal(JSON.parse(kept.get(compose.emailLocalKey)).draft.body, "<p>These offline changes must stay open.</p>");
        await compose.close();
        assert.equal(closed.length, 2);

        // The host's asynchronous local write has the same race; the local fallback survives an older server answer.
        const localPending = [];
        drafts.put = (key, value) => new Promise((resolve, reject) => {
            localPending.push({ key, value, resolve: () => { kept.set(key, String(value)); resolve(); }, reject });
        });
        compose.draft.body = "<p>The first device snapshot.</p>";
        await window.Vue.nextTick();
        const localSave = compose.save();
        await until(() => localPending.length === 1, "the first device write waiting");
        compose.draft.body = "<p>New words while the device saves.</p>";
        await window.Vue.nextTick();
        localPending[0].resolve();
        await until(() => localPending.length === 2 || !compose.saving, "the old device answer handled");
        assert.equal(compose.dirty, true, "an old device answer cannot mark newer text saved");
        assert.equal(localPending.length, 2);
        assert.equal(JSON.parse(localPending[1].value).draft.body, "<p>New words while the device saves.</p>");
        const closingLocal = compose.close();
        assert.equal(closed.length, 2);
        localPending[1].resolve();
        await Promise.all([localSave, closingLocal]);
        assert.equal(closed.length, 3, "Close follows persistence of the newest device content");
        assert.equal(localPending.length, 2, "Close does not create an unchanged third write");
        drafts.put = put;

        server.online = true;
        moqui.request = (url, options) => {
            const answer = request(url, options);
            if (!String(url).endsWith("/Compose/saveDraft")) return answer;
            return answer.then((result) => new Promise((resolve, reject) => {
                pending.push({ params: clone(options.params), resolve: () => resolve(result), reject });
            }));
        };
        const fallbackKey = compose.localEmailKey;
        const reconnectSave = compose.save();
        await until(() => pending.length === 3, "the reconnect save waiting");
        compose.draft.subject = "New subject during reconnection";
        await window.Vue.nextTick();
        pending[2].resolve();
        await until(() => pending.length === 4, "the newer reconnect content waiting");
        assert.ok(kept.has(fallbackKey), "an older server snapshot does not delete the local fallback");
        pending[3].reject({ ok: false, status: 503, headers: plainHeaders, json: { errors: "Reconnection save failed" } });
        await reconnectSave;
        assert.ok(kept.has(fallbackKey), "a failed newer save still has its previous local recovery copy");
        assert.equal(compose.draft.subject, "New subject during reconnection");
        assert.equal(compose.dirty, true);
        moqui.request = request;
        await compose.save();
        await until(() => !kept.has(fallbackKey), "the current server content replaces its local fallback");

        // Clearing a new offline email during its first write must replace that snapshot even without a server id.
        server.online = false;
        compose.draft.emailMessageId = null;
        compose.draft.subject = "";
        compose.draft.body = "<p>Clear this before its first offline save finishes.</p>";
        localPending.length = 0;
        drafts.put = (key, value) => {
            if (key === compose.emailRefKey) return put(key, value);
            return new Promise((resolve) => {
                localPending.push({ key, value, resolve: () => { kept.set(key, String(value)); resolve(); } });
            });
        };
        await window.Vue.nextTick();
        const clearingSave = compose.save();
        await until(() => localPending.length === 1, "the new offline draft's first write waiting");
        compose.draft.body = "";
        await window.Vue.nextTick();
        assert.equal(compose.isEmpty, true);
        const closingEmpty = compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 3, "an empty draft still waits for its in-flight save before closing");
        localPending[0].resolve();
        await until(() => localPending.length === 2 || !compose.saving, "the cleared offline draft acknowledged");
        assert.equal(localPending.length, 2, "an empty draft replaces already-persisted offline content");
        assert.equal(JSON.parse(localPending[1].value).draft.body, "");
        assert.equal(closed.length, 3, "the empty shortcut cannot close while clearing is still being saved");
        localPending[1].resolve();
        await Promise.all([clearingSave, closingEmpty]);
        assert.equal(closed.length, 4);
        assert.equal(JSON.parse(kept.get(compose.emailLocalKey)).draft.body, "", "reopening cannot restore the cleared text");
        drafts.put = put;

        // Internal messages on the web use localStorage; a failed write must have the same recovery.
        server.online = true;
        delete window.moquiDesktop;
        moqui.drafts.local = () => ({setItem() { throw new Error("Storage quota exceeded"); }});
        compose.draft.fromValue = "internal";
        compose.draft.body = "<p>These browser changes must stay open.</p>";
        await window.Vue.nextTick();
        await compose.close();
        await window.Vue.nextTick();
        assert.equal(closed.length, 4);
        assert.equal(compose.dirty, true);
        assert.ok(document.querySelector(".m-compose-save-error"));
        moqui.drafts.local = local;
        button(document.querySelector(".m-compose-save-error"), "Retry save").click();
        await until(() => !compose.saving && !compose.saveError && !compose.dirty, "the browser draft persisted");
        assert.equal(JSON.parse(moqui.drafts.get(compose.draftStorageKey)).draft.body, "<p>These browser changes must stay open.</p>");
        await compose.close();
        assert.equal(closed.length, 5);
        await moqui.drafts.remove(compose.draftStorageKey);
    } finally {
        root.setUrl = setUrl;
        window.moquiDesktop = desktop;
        drafts.put = put;
        moqui.drafts.local = local;
        moqui.request = request;
        server.online = true;
        server.saveError = null;
        server.compose = null;
    }
});

test("a revoked key holds what waits, each saying to sign in again; a held item stops no other", async () => {
    server.online = true;
    ops.push({ id: "h1", kind: "chat-send", state: "held", attempts: 1, createdMs: Date.now(), payload: { chatRoomId: "R1", text: "held for the key", threadRootEventSeq: null } },
        { id: "h2", kind: "chat-edit", state: "held", attempts: 1, createdMs: Date.now(), payload: { chatRoomId: "R1", eventSeq: 5, body: "<p>x</p>" } },
        { id: "w1", kind: "chat-send", state: "queued", attempts: 0, createdMs: Date.now() + 1, payload: { chatRoomId: "R1", text: "after it", threadRootEventSeq: null } },
        { id: "h3", kind: "mail-send", state: "held", attempts: 1, createdMs: Date.now(), payload: { clientRef: "h3", fields: { subject: "Held mail", fromValue: "acct1" } } });
    tell();
    await openRoom();
    const held = await until(() => document.querySelector(".m-chat-pending[data-client-ref='h1'][data-pending='held']"), "the held message");
    assert.ok(held.textContent.includes("Waiting: sign in again to send it"));
    assert.ok((await until(() => change("chat-edit", "held"), "the held edit")).textContent.includes("Edit: waiting, sign in again to send it"));
    const after = await until(() => document.querySelector(".m-chat-pending[data-client-ref='w1']"), "the next message");
    assert.equal(after.getAttribute("data-pending"), "waiting");
    await open("/qapps/BasicComms/Inbox", "mail");
    const mail = await until(() => Array.from(document.querySelectorAll(".m-inbox-outbox-item")).find((item) => item.textContent.includes("Held mail")), "the held mail");
    assert.ok(mail.textContent.includes("Waiting: sign in again to send it"));
});

test("poll votes: one waiting counts for the next (several choices add up); a vote taken back waits for one still queued", async () => {
    const vm = await openRoom();
    const poll = (multiple) => ({ pollId: "P" + (multiple ? 1 : 2), question: "Lunch?", multiple, anonymous: false, closed: false, createdByUserId: "bob", voterCount: 0,
        options: [{ id: 1, text: "Soup", mine: false, voteCount: 0 }, { id: 2, text: "Salad", mine: false, voteCount: 0 }] });
    const votes = () => host.added.filter((op) => op.kind === "poll-vote");
    const many = { eventSeq: 11, poll: poll(true) };
    vm.vote(many, many.poll.options[0]);
    await until(() => votes().length === 1, "the first vote queued");
    assert.equal(votes()[0].payload.optionIds, "1");
    assert.equal(vm.optionIcon(many, many.poll.options[0]), "sym_o_check_box", "shown as chosen while it waits");
    vm.vote(many, many.poll.options[1]);
    await until(() => votes().length === 2, "the second vote queued");
    assert.equal(votes()[1].payload.optionIds, "1,2", "Soup is kept");
    // one choice: chosen, then taken back while the vote waits: asked of nobody, and said
    const one = { eventSeq: 12, poll: poll(false) };
    vm.vote(one, one.poll.options[0]);
    await until(() => votes().length === 3, "the single vote queued");
    const posts = asked("/apps/BasicComms/Chat/votePoll", "POST").length;
    const said = [];
    const notify = moqui.notifyMessages;
    moqui.notifyMessages = (...args) => { said.push(JSON.stringify(args)); return notify(...args); };
    // (the vote just queued counts, though the host's list may not have it yet)
    try { vm.vote(one, one.poll.options[0]); } finally { moqui.notifyMessages = notify; }
    assert.equal(said.length, 1);
    assert.ok(said[0].includes("Your vote is waiting to be counted"), said[0]);
    assert.equal(votes().length, 3, "not queued again");
    assert.equal(asked("/apps/BasicComms/Chat/votePoll", "POST").length, posts, "not asked ahead of the queued vote");
    // answered: the poll as the server counted it
    for (const op of votes()) drop(op.id);
    tell();
    await until(() => !moqui.chat.state.changes.some((change) => change.kind === "poll-vote"), "the votes to leave the outbox");
    tell({ kind: "sent", op: "poll-vote", clientRef: votes()[1].id, chatRoomId: "R1", eventSeq: 11, json: { poll: Object.assign(poll(true), { voterCount: 1 }) } });
    await settle();
    assert.equal(vm.myOptions(many).length, 0, "no vote waits; the poll's own answer counts");
});

test("an encrypted room: a reaction taken back while it waits is dropped, and a reconnect decrypts each page once", async () => {
    const chat = moqui.chat;
    const kept = { openEvents: chat.openEvents, openEvent: chat.openEvent, sealReaction: chat.sealReaction, myReaction: chat.myReaction };
    let opened = 0;
    chat.openEvents = (roomId, list) => { opened += 1; return Promise.resolve(list); };
    chat.openEvent = () => Promise.resolve();
    chat.sealReaction = () => Promise.resolve(JSON.stringify({ ciphertext: "x" }));
    chat.myReaction = () => null;
    try {
        const screen = await open("/qapps/BasicComms/Chat?chatRoomId=R3", "chat");
        const vm = await until(() => screen.$refs.room && screen.$refs.room.roomId === "R3" && screen.$refs.room.state === "ready" && screen.$refs.room, "the encrypted room");
        assert.ok(vm.sealed);
        vm.react({ eventSeq: 3, event: eventOf(vm, 3) }, "👍");
        const sealed = await until(() => host.added.find((op) => op.kind === "chat-react" && op.payload.chatRoomId === "R3"), "the encrypted reaction queued");
        assert.ok(sealed.payload.encryptedJson && !sealed.payload.key, "its key only as ciphertext");
        await until(() => change("chat-react", "waiting"), "it waits");
        // taken back before the server has it (no reaction event of its own to take back): dropped from the outbox
        vm.react({ eventSeq: 3, event: eventOf(vm, 3) }, "👍");
        await until(() => host.discarded.includes(sealed.id), "the waiting reaction dropped");
        assert.ok(!host.added.some((op) => op.kind === "chat-react" && op.payload.chatRoomId === "R3" && op.payload.reactionEventSeq != null));
        // a new message while away: the reconnect decrypts what each read brings once
        sealedEvents.push(message(4, "sealed three", { encrypted: true }));
        const from = requests.length;
        const before = opened;
        closeSocket();
        openSocket();
        await until(() => text().includes("sealed three"), "the new message");
        await settle();
        const reads = requests.slice(from).filter((request) => request.method === "GET" && request.params.chatRoomId === "R3" && request.events > 0).length;
        assert.ok(reads > 0);
        assert.equal(opened - before, reads, "once each");
    } finally {
        Object.assign(chat, kept);
    }
});

test("Send is an icon with bubbles; an encrypted room's search says it is not searched; an owner's succession reads as automatic and Leave names the heir", async () => {
    const vm = await openRoom();
    const kept = { width: vm.width, encrypted: vm.room.encrypted, access: vm.access, members: vm.members };
    try {
        // COM-UI-002: a room with bubbles (under 640 px, or the stacked Chat) shows Send as its icon, still named Send
        vm.width = 500;
        await Vue.nextTick();
        // with no text the composer's button records a voice note (Comms-Messaging §5.1), where the browser records sound
        const voice = vm.$el.querySelector(".m-chat-voice");
        assert.equal(!!voice, typeof window.MediaRecorder === "function" && !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia), "the microphone for no text");
        if (voice) assert.equal(voice.getAttribute("aria-label"), "Record a voice note");
        vm.composers.main.body = "<p>hi</p>";
        await Vue.nextTick();
        const send = vm.$el.querySelector(".m-chat-send");
        assert.equal(vm.$el.querySelector(".m-chat-voice"), null, "Send once there is text");
        assert.equal(vm.useBubbles, true, "the room has bubbles");
        assert.equal(send.getAttribute("aria-label"), "Send");
        assert.equal(send.querySelector(".block"), null, "no Send text beside the icon");
        vm.composers.main.body = "";
        await Vue.nextTick();
        // COM-UI-011: the search pane of an encrypted room says so before a query, and asks the server nothing
        vm.room.encrypted = true;
        vm.side = "search";
        await Vue.nextTick();
        assert.equal(vm.sealed, true, "an encrypted room");
        assert.notEqual(vm.$el.querySelector(".m-chat-search-sealed"), null, "the note in the search pane");
        const searches = asked("/apps/BasicComms/Chat/search").length;
        vm.searchPane.text = "anything";
        vm.runSearch();
        assert.equal(asked("/apps/BasicComms/Chat/search").length, searches, "no search asked");
        vm.closeSide();
        vm.room.encrypted = kept.encrypted;
        // COM-UI-009: the succession event names who left, not someone who made the heir an owner
        const item = vm.buildItems([{ eventSeq: 9, type: "membership", senderUserId: "ann", senderName: "Ann", sentDate: Date.now(),
            content: { action: "role", userId: "bob", roleEnumId: "CmrOwner", byUserId: null, automatic: true } }], { divider: false })
            .find((one) => one.kind === "system");
        assert.equal(item.text, "Bob became an owner when Ann left");
        // and the last owner's Leave names the heir by the server's rule: the longest-standing moderator, else member
        vm.access = Object.assign({}, vm.access, { isOwner: true });
        vm.members = [{ userId: "ann", name: "Ann", roleEnumId: "CmrOwner", joinedDate: 1 }, { userId: "bob", name: "Bob", roleEnumId: "CmrMember", joinedDate: 2 },
            { userId: "cy", name: "Cy", roleEnumId: "CmrModerator", joinedDate: 3 }, { userId: "bot", name: "Bot", roleEnumId: "CmrAgent", isAgent: true, joinedDate: 0 }];
        assert.equal(vm.ownerHeir().userId, "cy");
        vm.members = vm.members.filter((person) => person.userId !== "cy");
        assert.equal(vm.ownerHeir().userId, "bob");
    } finally {
        Object.assign(vm, { width: kept.width, access: kept.access, members: kept.members });
        vm.room.encrypted = kept.encrypted;
    }
});

test("Activity keeps distinct rows for one event, follows the refreshed cursor across a gap and ignores a late page", async () => {
    const methods = moqui.bundledComponent("BasicComms/Chat").methods;
    const feed = { view: null, list: [], next: null, loading: false, loaded: false, seq: 0 };
    const cursors = [];
    let response;
    const vm = { feed, filter: "activity", loadFeed: methods.loadFeed, refreshFeedTop: methods.refreshFeedTop,
        feedItems: methods.feedItems, openFeed: methods.openFeed, me: {}, $nextTick() {},
        feedRequest(view, cursor) { cursors.push(cursor); return Promise.resolve(response); } };
    const page = (ids, next) => ({ items: ids.map((id) => ({ chatUserActivityId: id, kind: "mention", chatRoomId: "R1", eventSeq: 7, date: 100 })), nextCursor: next });
    const keys = () => feed.list.map((item) => item.key);
    response = page(["D"], "after-D");
    await vm.loadFeed(false);
    response = page(["C", "B"], "after-B");
    await vm.loadFeed(true);
    assert.deepEqual(keys(), ["activity:D", "activity:C", "activity:B"]);
    response = page(["E", "D"], "after-D");
    await vm.refreshFeedTop();
    assert.deepEqual(keys(), ["activity:E", "activity:D", "activity:C", "activity:B"], "an overlap keeps the scrolled tail");
    assert.equal(feed.next, "after-B");
    let resolveLate;
    response = new Promise((resolve) => { resolveLate = resolve; });
    const late = vm.loadFeed(true);
    response = page(["I", "H"], "after-H");
    await vm.refreshFeedTop();
    resolveLate(page(["A"], null));
    await late;
    assert.deepEqual(keys(), ["activity:I", "activity:H"], "a disjoint first page uses its own boundary");
    response = page(["G", "F"], "after-F");
    await vm.loadFeed(true);
    assert.deepEqual(cursors, [null, "after-D", null, "after-B", null, "after-H"]);
    assert.deepEqual(keys(), ["activity:I", "activity:H", "activity:G", "activity:F"]);
    const old = { kind: "reply", chatRoomId: "R1", eventSeq: 8, date: 101 };
    const oldKey = vm.feedItems("activity", [clone(old)])[0].key;
    assert.ok(oldKey);
    assert.equal(vm.feedItems("activity", [clone(old)])[0].key, oldKey, "a cached response from an older server keeps a stable key");

    // An old thread falls below the first page after its last reply is removed. The first page can end on the same thread.
    const thread = (root, last) => ({ chatRoomId: "R1", threadRootEventSeq: root, lastEventSeq: last, lastDate: last });
    vm.filter = "threads";
    response = { items: [thread(1, 100), thread(2, 90)], nextCursor: "after-90" };
    await vm.loadFeed(false);
    response = { items: [thread(3, 70)], nextCursor: null };
    await vm.loadFeed(true);
    response = { items: [thread(4, 110), thread(2, 90)], nextCursor: "after-90" };
    await vm.refreshFeedTop();
    assert.equal(feed.next, "after-90");
    response = { items: [thread(1, 80), thread(3, 70)], nextCursor: null };
    await vm.loadFeed(true);
    assert.deepEqual(feed.list.map((item) => item.threadRootEventSeq), [4, 2, 1, 3]);

    // A room push refreshes Activity for the viewer who received reply activity; Threads follows any thread change.
    const changes = { feedShown: true, filter: "activity", me: { userId: "ann" }, refreshFeedTop() {} };
    const reply = { type: "message", senderUserId: "bob", threadRootEventSeq: 1 };
    methods.feedTouched.call(changes, Object.assign({ replyForViewer: false }, reply));
    assert.ok(!changes.feedTimer);
    methods.feedTouched.call(changes, reply);
    assert.ok(changes.feedTimer, "an older server's thread signal still refreshes Activity");
    clearTimeout(changes.feedTimer);
    changes.feedTimer = null;
    methods.feedTouched.call(changes, Object.assign({ replyForViewer: true }, reply));
    assert.ok(changes.feedTimer);
    clearTimeout(changes.feedTimer);
    changes.filter = "threads";
    changes.feedTimer = null;
    methods.feedTouched.call(changes, reply);
    assert.ok(changes.feedTimer);
    clearTimeout(changes.feedTimer);
});

test("Reactions follows a demoted message past the same page boundary and a delayed filter response cannot restore the old list", async () => {
    const methods = moqui.bundledComponent("BasicComms/Chat").components["chat-room"].methods;
    const tabs = { reactions: { list: [], next: null, loading: false, loaded: false } };
    const cursors = [];
    let page;
    const vm = { tabs, roomId: "R1", tabToken: methods.tabToken, tabKey: methods.tabKey, setReactionFilter: methods.setReactionFilter,
        loadTab: methods.loadTab, refreshTab: methods.refreshTab, $nextTick() {},
        fetchTab(name, cursor) { cursors.push(cursor); return Promise.resolve(page); } };
    const reaction = (message, latest) => ({ eventSeq: message, latestReactionSeq: latest });
    page = { list: [reaction(6, 20), reaction(5, 18)], next: 18 };
    await vm.loadTab("reactions", false);
    page = { list: [reaction(4, 16)], next: null };
    await vm.loadTab("reactions", true);
    // Message 6 lost its newest reaction and is now below 5; the first page still ends at the same reaction.
    page = { list: [reaction(8, 30), reaction(5, 18)], next: 18 };
    await vm.refreshTab("reactions");
    page = { list: [reaction(6, 17), reaction(4, 16)], next: null };
    await vm.loadTab("reactions", true);
    assert.deepEqual(cursors, ["", 18, "", 18]);
    assert.deepEqual(tabs.reactions.list.map((item) => item.eventSeq), [8, 5, 6, 4]);

    let resolveLate;
    page = new Promise((resolve) => { resolveLate = resolve; });
    const late = vm.refreshTab("reactions");
    let resolveFilter;
    page = new Promise((resolve) => { resolveFilter = resolve; });
    const filtering = vm.setReactionFilter(true);
    assert.deepEqual(tabs.reactions.list, []);
    page = { list: [reaction(5, 18)], next: null };
    await vm.refreshTab("reactions");
    resolveFilter({ list: [reaction(6, 17)], next: 17 });
    resolveLate({ list: [reaction(8, 30), reaction(5, 18)], next: 18 });
    await Promise.all([filtering, late]);
    assert.equal(tabs.reactions.onlyToMe, true);
    assert.deepEqual(tabs.reactions.list.map((item) => item.eventSeq), [5]);
    assert.equal(tabs.reactions.next, null);
});

// a touch the page makes itself (the test kit has no touch emulation): an event of the type with its touches
const touch = (el, type, x, y) => {
    const event = new Event(type, { bubbles: true, cancelable: true });
    const point = { identifier: 1, target: el, clientX: x, clientY: y };
    event.touches = type === "touchend" ? [] : [point];
    event.changedTouches = [point];
    el.dispatchEvent(event);
};
test("Back (moqui.back) closes the message sheet, then the pane, then goes back in the shell to the home, where it closes nothing; a swipe right on a bubble replies", async () => {
    const vm = await openRoom();
    const kept = vm.width;
    try {
        vm.width = 500;
        await Vue.nextTick();
        assert.equal(vm.useBubbles, true);
        // a swipe to the right past the threshold replies to the bubble, which follows the finger; the long press does not start
        const row = () => document.querySelector(".m-chat-msg[data-seq='5']");
        touch(row(), "touchstart", 20, 200);
        touch(row(), "touchmove", 60, 203);
        await Vue.nextTick();
        assert.ok(String(row().style.transform).includes("translateX(40px)"), "the bubble follows");
        touch(row(), "touchmove", 110, 205);
        touch(row(), "touchend", 110, 205);
        await Vue.nextTick();
        assert.equal(vm.replyTo && vm.replyTo.eventSeq, 5, "a reply to the swiped message");
        assert.equal(row().style.transform, "", "the bubble back in its place");
        assert.ok(!vm.holdTimer && !vm.sheet.open, "a swipe is not a long press");
        vm.cancelReply();
        // a short swipe, or a move up or down (a scroll), replies to nothing
        touch(row(), "touchstart", 20, 200);
        touch(row(), "touchmove", 60, 202);
        touch(row(), "touchend", 60, 202);
        touch(row(), "touchstart", 20, 200);
        touch(row(), "touchmove", 30, 260);
        touch(row(), "touchmove", 120, 262);
        touch(row(), "touchend", 120, 262);
        assert.equal(vm.replyTo, null);
        // a touch in a code block (it scrolls sideways) is that scroll, not a swipe
        const pre = document.createElement("pre");
        pre.textContent = "wide code";
        row().querySelector(".m-chat-body").appendChild(pre);
        touch(pre, "touchstart", 20, 200);
        touch(pre, "touchmove", 120, 202);
        touch(pre, "touchend", 120, 202);
        pre.remove();
        assert.equal(vm.replyTo, null, "no reply from a swipe in a code block");
        assert.equal(row().style.transform, "");
        // the layers: a pane, then the sheet over it
        vm.openSide("members");
        await Vue.nextTick();
        vm.openSheet({ eventSeq: 5, event: eventOf(vm, 5), senderName: "Bob" }, false);
        const sheet = await until(() => document.querySelector(".m-chat-sheet"), "the message sheet");
        assert.ok(moqui.back.popupOpen());
        assert.ok(moqui.back.pop(), "Back closes the sheet");
        await until(() => !sheet.isConnected, "the sheet to close");
        assert.equal(vm.sheet.open, false);
        assert.equal(vm.side, "members", "the pane stays");
        assert.equal(moqui.back.popupOpen(), false);
        assert.ok(moqui.back.pop(), "then the pane");
        assert.equal(vm.side, null);
        assert.equal(root.view, "chat", "the room stays");
        assert.equal(moqui.back.pop({ base: false }), false, "Esc has nothing more to close");
        // then the shell's history: the header's Back and Android's back go home, where back closes nothing (the app leaves)
        root.backStack = [];
        assert.ok(moqui.back.pop());
        assert.equal(root.view, null, "the home");
        assert.equal(moqui.back.pop(), false);
    } finally {
        vm.width = kept;
    }
});

test("one action list (COM-UI-035): the sheet, the More menu and a right click on a desktop row offer the same actions, the apps' and File on a record among them; Info reads the message again (COM-UI-008)", async () => {
    const vm = await openRoom();
    const priorItems = moqui.ext.items;
    const priorTimeline = answers["/apps/BasicComms/Chat/timeline"];
    const priorPrefs = { recordLink: vm.prefs.recordLink, readReceipts: vm.prefs.readReceipts };
    moqui.ext.items = () => Promise.resolve({ actions: [{ name: "makeTask", slot: "chat-message-actions", label: "Make a task", kind: "transition", url: "/x" }], panels: [] });
    // the message read again for Info: its delivery to an SMS bridge and the members' read markers as the server has them
    answers["/apps/BasicComms/Chat/timeline"] = (params) => {
        const page = priorTimeline(params);
        if (params.eventSeq == null) return page;
        page.eventList.forEach((event) => { event.delivery = { state: "accepted", destinations: [{ networkName: "SMS", state: "accepted", receipt: "delivered" }] }; });
        page.memberList = [{ userId: "ann", name: "Ann", lastReadEventSeq: 6 }, { userId: "bob", name: "Bob", lastReadEventSeq: 6 }];
        return page;
    };
    vm.prefs.recordLink = true;
    const keys = (scope) => Array.from(scope.querySelectorAll("[data-action], [data-ext-action]")).map((el) => el.getAttribute("data-action") || "app:" + el.getAttribute("data-ext-action"));
    const ALL = ["reply", "thread", "copy", "link", "forward", "edit", "pin", "unread", "app:makeTask", "file", "info", "delete"];
    const closed = async (el, what) => { assert.ok(moqui.back.pop(), "Back closes " + what); await until(() => !el.isConnected, what + " to close"); };
    try {
        assert.equal(vm.useBubbles, false, "a desktop's rows");
        const item = vm.items.find((one) => one.kind === "message" && one.eventSeq === 6);
        vm.openSheet(item, false);
        const sheet = await until(() => document.querySelector(".m-chat-sheet [data-ext-action]") && document.querySelector(".m-chat-sheet"), "the sheet with the apps' actions");
        assert.deepEqual(keys(sheet), ALL);
        assert.equal(sheet.querySelectorAll(".m-chat-sheet-emoji").length, 6, "the quick reactions, then +");
        sheet.querySelector(".m-chat-more-reactions").click();
        assert.ok(await until(() => sheet.querySelector(".m-emoji-picker"), "+ opens the picker in the sheet"));
        await closed(sheet, "the sheet");
        // the hover toolbar's More: the same list (its quick reactions are on the toolbar)
        const row = vm.$el.querySelector('.m-chat-msg[data-seq="6"]');
        vm.hoverKey = "m6";
        await Vue.nextTick();
        row.querySelector('button[aria-label="More"]').click();
        const more = await until(() => document.querySelector(".q-menu .m-chat-actions [data-ext-action]") && document.querySelector(".q-menu .m-chat-actions"), "the More menu");
        assert.deepEqual(keys(more), ALL);
        await closed(more, "the More menu");
        vm.hoverKey = null;
        // a right click on the row: the same menu at the pointer, with the quick reactions, and not the browser's
        const click = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 });
        row.querySelector(".m-chat-body").dispatchEvent(click);
        assert.ok(click.defaultPrevented, "the browser's menu is not shown");
        const menu = await until(() => document.querySelector(".m-chat-context-menu [data-ext-action]") && document.querySelector(".m-chat-context-menu"), "the right-click menu");
        assert.deepEqual(keys(menu), ALL);
        assert.equal(menu.querySelectorAll(".m-chat-sheet-emoji").length, 6);
        // Info: read again; Sent, the SMS delivery from its delivery row, Read by from the markers; never a device's delivery
        const reads = asked("/apps/BasicComms/Chat/timeline").length;
        menu.querySelector('[data-action="info"]').click();
        const info = await until(() => document.querySelector(".m-chat-info [data-info='delivered']") && document.querySelector(".m-chat-info"), "Info with the delivery");
        assert.equal(asked("/apps/BasicComms/Chat/timeline").length, reads + 1);
        assert.equal(asked("/apps/BasicComms/Chat/timeline").slice(-1)[0].params.eventSeq, 6);
        const line = (key) => info.querySelector("[data-info='" + key + "']").textContent.trim();
        assert.ok(line("sent").startsWith("Sent"));
        assert.ok(line("delivered").startsWith("SMS") && line("delivered").includes("network's receipt"), line("delivered"));
        assert.equal(line("read"), "Read by" + "Bob");
        assert.ok(line("note").includes("not known to the server"));
        // the viewer's receipts off: no names, said why
        vm.prefs.readReceipts = false;
        await Vue.nextTick();
        assert.ok(line("read").includes("Your read receipts are off") && !line("read").includes("Bob"));
        await closed(info, "Info");
        // a right click on a link keeps the browser's menu
        const link = document.createElement("a");
        link.href = "https://example.com/";
        link.textContent = "a link";
        row.querySelector(".m-chat-body").appendChild(link);
        const onLink = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
        link.dispatchEvent(onLink);
        link.remove();
        assert.equal(onLink.defaultPrevented, false);
        // a picture keeps the browser's menu too; text selected in another message is not this one's
        const picture = document.createElement("img");
        picture.alt = "a picture";
        row.querySelector(".m-chat-body").appendChild(picture);
        const onPicture = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
        picture.dispatchEvent(onPicture);
        picture.remove();
        assert.equal(onPicture.defaultPrevented, false);
        window.getSelection().selectAllChildren(vm.$el.querySelector('.m-chat-msg[data-seq="5"] .m-chat-body'));
        const elsewhere = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 300, clientY: 200 });
        row.querySelector(".m-chat-body").dispatchEvent(elsewhere);
        assert.ok(elsewhere.defaultPrevented, "a selection in another message leaves this one's menu");
        await closed(await until(() => document.querySelector(".m-chat-context-menu"), "the menu"), "the menu");
        window.getSelection().removeAllRanges();
        // a reaction chip's own context menu (Android's long press there) and Shift+Enter: who reacted, not the message's menu
        eventOf(vm, 5).reactions = [{ key: "👀", count: 1, userIds: ["bob"], mine: false }];
        const chip = await until(() => vm.$el.querySelector('.m-chat-msg[data-seq="5"] .m-chat-reaction'), "a reaction chip");
        assert.ok(chip.getAttribute("aria-label").includes("Bob") && chip.getAttribute("aria-keyshortcuts") === "Shift+Enter", chip.getAttribute("aria-label"));
        const onChip = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
        chip.dispatchEvent(onChip);
        assert.ok(onChip.defaultPrevented);
        const people = await until(() => document.querySelector(".m-chat-people [data-person='bob']") && document.querySelector(".m-chat-people"), "who reacted");
        assert.equal(document.querySelector(".m-chat-context-menu"), null, "not the message's menu");
        await closed(people, "who reacted");
        // the click that may follow it (a key's, a touch's) is not a tap on the chip
        chip.click();
        await Vue.nextTick();
        assert.ok(!eventOf(vm, 5).reactions.some((reaction) => reaction.mine), "no reaction from the click after it");
        chip.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true, cancelable: true }));
        await closed(await until(() => document.querySelector(".m-chat-people"), "who reacted by the keyboard"), "who reacted");
        chip.click(); // the key's click is consumed
        eventOf(vm, 5).reactions = [];
    } finally {
        moqui.ext.items = priorItems;
        answers["/apps/BasicComms/Chat/timeline"] = priorTimeline;
        vm.prefs.recordLink = priorPrefs.recordLink;
        vm.prefs.readReceipts = priorPrefs.readReceipts;
    }
});

test("a reader can inspect reactions and named votes without changing them, including in a closed poll and the Reactions pane", async () => {
    const vm = await openRoom();
    const event = eventOf(vm, 5);
    const access = vm.access.canPost, caps = vm.room.bridgeCapabilities;
    const poll = event.poll, reactions = event.reactions, notify = moqui.notifyMessages;
    const messages = [], changed = () => host.added.filter(op => op.kind === "poll-vote" || op.kind === "chat-react").length;
    const before = changed();
    const show = async (el) => {
        el.click();
        const dialog = await until(() => document.querySelector(".m-chat-people [data-person='bob']")?.closest(".m-chat-people"), "Bob in the names");
        assert.ok(moqui.back.pop());
        await until(() => !dialog.isConnected, "the names to close");
    };
    const option = () => vm.$el.querySelector('.m-chat-msg[data-seq="5"] [data-option="1"]');
    const chip = () => vm.$el.querySelector('.m-chat-msg[data-seq="5"] .m-chat-reaction');
    try {
        moqui.notifyMessages = (infos) => messages.push(...infos);
        event.poll = { question: "Lunch?", closed: true, options: [{ id: 1, text: "Soup", voteCount: 1, voterIds: ["bob"] }] };
        event.reactions = [{ key: "👍", mine: false, count: 1, userIds: ["bob"] }];
        await Vue.nextTick();
        await show(option()); // closed even for a writer
        vm.access.canPost = false;
        event.poll.closed = false;
        await Vue.nextTick();
        await show(option());
        await show(chip());
        event.poll.anonymous = true;
        await Vue.nextTick();
        option().click();
        assert.ok(messages.some(info => info.message === "This poll is anonymous"));
        assert.ok(!vm.peopleBox.open);
        vm.access.canPost = true;
        vm.room.bridgeCapabilities = { reaction: false };
        await Vue.nextTick();
        await show(chip());
        assert.equal(changed(), before);
        assert.ok(!event.reactions[0].mine && !event.poll.options[0].mine);
        vm.room.bridgeCapabilities = { reaction: true };
        event.poll.anonymous = false;
        await Vue.nextTick();
        const start = host.added.length;
        option().click(); chip().click();
        await until(() => changed() === before + 2, "a writer's taps queue one vote and one reaction");
        assert.deepEqual(host.added.slice(start).map(op => op.kind).sort(), ["chat-react", "poll-vote"]);
        for (const op of host.added.slice(start)) drop(op.id);
        tell();
        vm.side = "reactions";
        vm.tabs.reactions.list = [{ eventSeq: 5, senderUserId: "bob", snippet: "a reaction", reactions: event.reactions }];
        await Vue.nextTick();
        await show(vm.$el.querySelector(".m-chat-reacted [data-key]"));
        assert.equal(vm.side, "reactions", "a chip keeps the list open");
        vm.$el.querySelector(".m-chat-reacted .q-item__label").click();
        await until(() => vm.highlightSeq === 5, "the rest of the row opens the message");
    } finally {
        event.poll = poll; event.reactions = reactions;
        vm.access.canPost = access; vm.room.bridgeCapabilities = caps;
        moqui.notifyMessages = notify; vm.side = null;
    }
});

test("Voice and Video disclose whom they invite, and room settings reconcile per room and choice", async () => {
    await open("/qapps/BasicComms/Inbox/Mail", "mail");
    const oldCall = moqui.call, oldRequest = moqui.request, oldNotify = moqui.notifyMessages, oldError = moqui.handleAjaxError;
    const calls = [], writes = [], messages = [], refusals = [];
    const refusedSetting = { status: 409 };
    const phone = { chatRoomId: "R-phone", roomTypeEnumId: "CrtExternal", displayName: "+15550101000", isMember: true,
        lastEventSeq: 6, lastMessage: { eventSeq: 6 }, archivedEventSeq: null, isMuted: false, isPinned: false };
    const chatData = "/apps/BasicComms/Chat/chatData", roomView = "/apps/BasicComms/Chat/roomView";
    const oldRooms = answers[chatData], oldView = answers[roomView];
    const panelOptions = root.chatPanel, refresh = panelOptions.methods.refresh;
    let panel;
    const heldReads = new Map();
    const holdRead = path => {
        let reply;
        const response = new Promise(resolve => { reply = json => resolve({ json }); });
        heldReads.set(path, response);
        return reply;
    };
    try {
        moqui.call = Object.assign({}, oldCall, { startCall: (params) => calls.push(params) });
        moqui.notifyMessages = (infos) => messages.push(...infos);
        moqui.handleAjaxError = error => error === refusedSetting ? refusals.push(error) : oldError(error);
        moqui.request = (url, options) => {
            if (String(url).endsWith("/setNotification"))
                return new Promise((resolve, reject) => writes.push({ params: options.params, reply: json => resolve({ json }), refuse: () => reject(refusedSetting) }));
            const path = String(url).split("/").pop();
            if (!heldReads.has(path)) return oldRequest(url, options);
            const response = heldReads.get(path);
            heldReads.delete(path);
            return response;
        };
        const vm = await openRoom();
        const oldAccess = vm.access.canPost, oldRoom = vm.room;
        vm.room = Object.assign({}, oldRoom, { roomTypeEnumId: "CrtDirect", displayName: "Bob" });
        await Vue.nextTick();
        const voice = vm.$el.querySelector(".m-chat-voice-call");
        assert.ok(voice, JSON.stringify({ canCall: vm.canCall, compact: vm.compact, callees: vm.callees, calls: moqui.config.calls, access: vm.access }));
        assert.ok(voice.getAttribute("aria-description").includes("Bob"), voice.outerHTML);
        voice.click();
        vm.room.encrypted = true;
        await Vue.nextTick();
        vm.$el.querySelector(".m-chat-video-call").click();
        assert.deepEqual(calls, [{ chatRoomId: "R1", video: false }, { chatRoomId: "R1", video: true, requireE2ee: true }]);
        vm.room = oldRoom;
        await Vue.nextTick();
        assert.ok(vm.$el.querySelector(".m-chat-video-call").getAttribute("aria-description").includes("eligible members"), vm.callAudience);
        vm.access.canPost = false;
        await Vue.nextTick();
        assert.equal(vm.$el.querySelector(".m-chat-voice-call"), null);
        vm.access.canPost = oldAccess;
        const members = vm.members;
        vm.members = [{ userId: "ann" }, { userId: "agent", isAgent: true }, { chatActorId: "remote", external: true }];
        await Vue.nextTick();
        assert.equal(vm.$el.querySelector(".m-chat-video-call"), null, "a room with no other local person has no call button");
        vm.members = members;
        const metaReply = holdRead("room"), viewReply = holdRead("roomView");
        const metadata = vm.reloadMeta(), fullView = vm.load();
        const fullArchive = vm.toggleArchive();
        (await until(() => writes[0], "the full room archive")).reply({ archivedEventSeq: 5 });
        await fullArchive;
        assert.equal(vm.member.archivedEventSeq, 5);
        assert.equal(vm.archivedByMe, false, "message 6 is beyond the server watermark");
        const older = oldView({ chatRoomId: "R1" });
        older.room = { ...older.room, topic: "A changed topic" };
        older.member = { ...older.member, archivedEventSeq: null, isMuted: true };
        older.memberList.push({ userId: "cy", name: "Cy" });
        metaReply(older);
        await metadata;
        assert.equal(vm.member.archivedEventSeq, 5, "the older room read keeps the confirmed archive");
        assert.ok(vm.member.isMuted && vm.room.topic === "A changed topic" && vm.members.some(person => person.userId === "cy"));
        viewReply({ ...older, member: { ...older.member, isPinned: true } });
        await fullView;
        assert.equal(vm.member.archivedEventSeq, 5, "the older full view also keeps the confirmed archive");
        assert.ok(vm.member.isPinned);
        const freshReply = holdRead("room"), freshMetadata = vm.reloadMeta();
        freshReply(older);
        await freshMetadata;
        assert.equal(vm.member.archivedEventSeq, null, "a read started after confirmation can change the archive");
        vm.room = Object.assign({}, vm.room, { lastMessageSeq: 10 });
        vm.member.archivedEventSeq = 9;
        assert.equal(vm.archivedByMe, false, "a newer message outside the loaded page also brings the room back");
        const delayed = vm.toggleArchive();
        const pending = await until(() => writes[1], "the delayed room archive");
        const nextChat = await open("/qapps/BasicComms/Chat?chatRoomId=R3", "chat");
        const next = await until(() => nextChat.$refs.room?.roomId === "R3" && nextChat.$refs.room.state === "ready" && nextChat.$refs.room, "another open room");
        const nextWatermark = next.member.archivedEventSeq;
        pending.reply({ archivedEventSeq: 9 });
        await delayed;
        assert.equal(next.member.archivedEventSeq, nextWatermark, "an old room response cannot change the next room");
        writes.length = 0;
        answers[chatData] = params => Object.assign({}, oldRooms(params), { roomList: [clone(phone), clone(room)] });
        answers[roomView] = params => {
            const view = oldView(params);
            return Object.assign({}, view, { room: Object.assign({}, view.room, { bridged: true, encrypted: false }) });
        };
        panelOptions.methods.refresh = function () { panel = this; return refresh.call(this); };
        root.goHome();
        const phoneRow = await until(() => document.querySelector(".m-chat-home [data-room='R-phone']"), "the phone room on the home list");
        assert.equal(phoneRow.querySelector(".q-avatar .q-icon").textContent.trim(), "sms", "the home row uses the phone room icon");
        const row = () => panel.rooms.find(item => item.chatRoomId === phone.chatRoomId);
        const pin = panel.roomAction(row(), "pin"), archive = panel.roomAction(row(), "archive");
        await until(() => writes.length === 2, "independent room settings");
        assert.ok(panel.isArchived(row()) && row().isPinned, JSON.stringify(row()));
        assert.equal(messages.filter(info => info.message === "Conversation archived").length, 0);
        await panel.refresh();
        assert.ok(row().isPinned && panel.isArchived(row()), "the optimistic choices survive a replacement room row");
        const listReply = holdRead("chatData");
        const oldList = panel.refresh();
        const staleRooms = oldRooms({});
        phone.archivedEventSeq = 7;
        writes[1].reply({ archivedEventSeq: 7, isPinned: false });
        await archive;
        writes[0].refuse();
        await pin;
        listReply(staleRooms);
        await oldList;
        assert.equal(row().archivedEventSeq, 7, "an older refresh cannot replace the confirmed watermark");
        assert.equal(row().isPinned, false);
        assert.equal(messages.filter(info => info.message === "Conversation archived").length, 1);
        const beforeMessage = holdRead("chatData"), delayedList = panel.refresh();
        const oldRows = answers[chatData]({});
        panel.onEvent({ chatRoomId: phone.chatRoomId, event: message(8, "Later") });
        beforeMessage(oldRows);
        await delayedList;
        assert.ok(!panel.isArchived(row()), "a later message brings the room back");
        assert.equal(row().lastMessage.snippet, "Later", "an earlier list snapshot keeps the socket message");
        const mute = panel.roomAction(row(), "mute");
        const refused = panel.roomAction(row(), "archive");
        await until(() => writes.length === 4, "mute and another archive");
        writes[2].reply({ isMuted: true, archivedEventSeq: null });
        writes[3].refuse();
        await Promise.all([mute, refused]);
        assert.equal(row().archivedEventSeq, 7);
        assert.equal(row().isMuted, true);
        assert.equal(messages.filter(info => info.message === "Conversation archived").length, 1, "refusal has no success notice");
        const first = panel.roomAction(row(), "pin"), later = panel.roomAction(row(), "pin");
        const beforePin = holdRead("chatData"), pinRead = panel.refresh(), stalePin = answers[chatData]({});
        phone.isPinned = true;
        (await until(() => writes[4], "the first pin")).reply({ isPinned: true });
        await first;
        beforePin(stalePin);
        await pinRead;
        assert.equal(row().isPinned, false, "the later choice is still shown");
        (await until(() => writes[5], "the later unpin")).refuse();
        await later;
        assert.equal(row().isPinned, true, "refusal restores the last confirmed value");
        const pendingArchive = panel.roomAction(row(), "archive");
        const write = await until(() => writes[6], "another archive request");
        phone.archivedEventSeq = 12;
        await panel.refresh();
        assert.equal(row().archivedEventSeq, 8, "the pending archive stays visible during refresh");
        write.refuse();
        await pendingArchive;
        assert.equal(row().archivedEventSeq, 12, "a refusal restores the watermark refreshed from another device");
        assert.equal(refusals.length, 4, "each settings refusal reached the error handler");

        panel.openRoom("R1");
        const more = await until(() => document.querySelector(".m-chat-panel [aria-label='Room']"), "the compact room menu");
        for (const kind of ["voice", "video"]) {
            more.click();
            const item = await until(() => document.querySelector(".q-menu .m-chat-" + kind + "-call"), "the compact " + kind + " action");
            assert.ok(item.textContent.includes("members on this server only"), item.textContent);
            item.click();
            await until(() => !item.isConnected, "the compact menu to close");
        }
        assert.deepEqual(calls.slice(2), [{ chatRoomId: "R1", video: false }, { chatRoomId: "R1", video: true }]);
        panel.closeRoom();
    } finally {
        panelOptions.methods.refresh = refresh;
        answers[chatData] = oldRooms; answers[roomView] = oldView;
        moqui.call = oldCall; moqui.request = oldRequest; moqui.notifyMessages = oldNotify; moqui.handleAjaxError = oldError;
    }
});

test("the shell's history: a link forward to a place on the stack is a new entry; a thread closed by Back is a step back, not opened again", async () => {
    await openRoom();
    root.backStack = [];
    root.setUrl("/qapps/BasicComms/Chat?chatRoomId=R3");
    root.setUrl("/qapps/BasicComms/Chat?chatRoomId=R1");
    assert.deepEqual(root.backStack.map((url) => url.split("?")[1]), ["chatRoomId=R1", "chatRoomId=R3"]);
    root.historyBack();
    assert.equal(root.currentParameters.chatRoomId, "R3");
    root.historyBack();
    assert.equal(root.currentParameters.chatRoomId, "R1");
    const vm = await openRoom();
    root.backStack = [];
    vm.openThread(5);
    await until(() => Number(root.currentParameters.thread) === 5, "the thread in the shell's history");
    assert.equal(root.backStack.length, 1, "the thread is an entry");
    assert.ok(moqui.back.pop({ base: false }));
    assert.equal(vm.side, null);
    assert.equal(root.backStack.length, 0, "closing the thread by Back took its entry and put none on");
    assert.ok(!root.currentParameters.thread);
});

// plans/Comms-Messaging.md §7.6: the account view's notification check, as the host (the app) answers it
test("the notification check shows the device's state, a fix for each item that is off, and a test's time to arrive", async () => {
    await open("/qapps/BasicComms/Chat?chatRoomId=R1", "chat");
    let state = { permission: true, channels: { "chat-messages": true, "chat-mentions": false }, batteryOptimized: true, fullScreen: true,
        fcm: false, distributor: null, distributors: 1, registered: true, lastPushDate: null, lastAckId: null, recentAcks: {} };
    const fixes = [];
    const desktop = window.moquiDesktop;
    let tests = 0;
    Object.assign(desktop, {
        pushCheck: () => Promise.resolve(state),
        pushCheckFix: (what) => { fixes.push(what); return Promise.resolve(); },
        // the server queues one; another push comes first (its id is not the test's), then the test's, as the phone's
        // check then says; a second test at once is too soon
        pushTest: () => {
            tests++;
            if (tests > 1) return Promise.resolve({ ackIdList: [], queuedDate: Date.now(), reasonCode: "tooSoon" });
            const started = Date.now();
            setTimeout(() => { state = Object.assign({}, state, { lastPushDate: Date.now(), lastAckId: "P0", recentAcks: { P0: Date.now() } }); }, 100);
            setTimeout(() => { state = Object.assign({}, state, { lastPushDate: started + 400, lastAckId: "P2",
                recentAcks: { P0: started + 100, P1: started + 400, P2: started + 450 } }); }, 400);
            return Promise.resolve({ ackIdList: ["P1"], queuedDate: Date.now() });
        }
    });
    try {
        root.openPushCheck();
        const row = (key) => document.querySelector(".m-shell-push-row[data-check='" + key + "']");
        await until(() => row("battery"), "the check's items");
        assert.ok(row("permission").textContent.includes("Notifications are allowed"));
        assert.ok(!row("permission").querySelector("button"), "an item that is on has no fix");
        assert.ok(row("channels").textContent.includes("chat-mentions"), "the channel that is off is named");
        assert.ok(row("registered") && row("lastPush").textContent.includes("None yet"));
        row("battery").querySelector("button").click();
        // no Firebase: a UnifiedPush distributor to pick
        assert.ok(row("service").textContent.includes("Pick a UnifiedPush distributor"));
        row("service").querySelector("button").click();
        assert.deepEqual(fixes, ["battery", "distributor"]);
        // where Firebase works it stays: switching to UnifiedPush asks first
        state = Object.assign({}, state, { fcm: true });
        await root.readPushCheck();
        await until(() => row("service").textContent.includes("Firebase"), "Firebase in use");
        [...row("service").querySelectorAll("button")].find((button) => button.textContent.includes("Use UnifiedPush")).click();
        assert.deepEqual(fixes, ["battery", "distributor"], "not switched before the answer");
        const confirm = await until(() => [...document.querySelectorAll(".m-shell-push-confirm button")]
            .find((button) => button.textContent.includes("Use UnifiedPush")), "the question");
        confirm.click();
        assert.deepEqual(fixes, ["battery", "distributor", "distributor"]);
        // switched: a way back
        state = Object.assign({}, state, { distributor: "io.heckel.ntfy" });
        await root.readPushCheck();
        await until(() => row("service").textContent.includes("in place of Firebase"), "UnifiedPush in place of Firebase");
        [...row("service").querySelectorAll("button")].find((button) => button.textContent.includes("Use Firebase")).click();
        assert.deepEqual(fixes.slice(-1), ["firebase"]);
        state = Object.assign({}, state, { fcm: false, distributor: null });
        await root.readPushCheck();
        // the phone's dialer through the app: offered (and turned on in Android's settings only) to an account with a
        // number to call from; the contacts' names, asked again on a tap (plans/Comms-Messaging.md §9.1, §9.2)
        assert.equal(row("dialAccount"), null, "no calling account: no row");
        state = Object.assign({}, state, { dialAccount: "registered", contacts: false });
        await root.readPushCheck();
        await until(() => row("dialAccount") && row("contacts"), "the dialer's and the contacts' rows");
        assert.ok(row("dialAccount").textContent.includes("Calling accounts in the phone settings"));
        row("dialAccount").querySelector("button").click();
        row("contacts").querySelector("button").click();
        assert.deepEqual(fixes.slice(-2), ["callingAccounts", "contacts"]);
        state = Object.assign({}, state, { dialAccount: "on", contacts: true });
        await root.readPushCheck();
        await until(() => !row("contacts").querySelector("button"), "the contacts allowed");
        assert.ok(!row("dialAccount").textContent.includes("Calling accounts in the phone settings"), "turned on");
        const testButton = () => [...document.querySelectorAll(".m-shell-push-check button")].find((button) => button.textContent.includes("Send a test notification"));
        const result = () => (document.querySelector(".m-shell-push-result") || {}).textContent || "";
        testButton().click();
        await until(() => result().includes("Arrived in"), "the test's time");
        // its own push's time (P1, 0.4 s), not another's that came first or after
        assert.ok(/Arrived in 0\.[34] s/.test(result()), result());
        assert.ok(row("lastPush").textContent.trim().length > "Last notification received".length, "the last push shows when");
        testButton().click();
        await until(() => result().includes("try again"), "a test too soon");
    } finally {
        root.pushBox.open = false;
        delete desktop.pushCheck;
        delete desktop.pushCheckFix;
        delete desktop.pushTest;
    }
});

// plans/Comms-Messaging.md §9.1: the phone's dialer and Contacts call through the mobile app; its Dialer fills the number
// in and the user presses Call (nothing a link brings is called by itself); when several accounts can call and the
// current one cannot, the user picks which; a leading *67 is Hide my number
test("a number the phone's dialer handed the app is filled in, never placed by itself; the user picks the account; *67 hides the number", async () => {
    answers["/apps/BasicComms/Calls/Dialer/dialerData"] = () => ({ phoneActive: true, callerIdList: [{ phoneNumber: "+15550100", description: "Desk" }],
        recentList: [], hideCallerId: false });
    answers["/apps/BasicComms/Calls/Dialer/dialSearch"] = () => ({ resultList: [], number: null });
    // placed; no call view here (the call store is not loaded in this page)
    answers["/apps/BasicComms/Calls/Dialer/dialNumber"] = () => ({ callSessionId: null, callLegId: null });
    const desktop = window.moquiDesktop;
    const picked = [];
    let choice = null;
    desktop.dialChoice = () => { const now = choice; choice = null; return Promise.resolve(now); };
    desktop.dialFrom = (account) => { picked.push(account); return Promise.resolve(); };
    const placed = () => asked("/apps/BasicComms/Calls/Dialer/dialNumber", "POST");
    const field = () => document.querySelector(".m-shell-view .m-dialer input");
    const callButton = () => Array.from(document.querySelectorAll(".m-shell-view .m-dialer button")).find((el) => /Call$/.test(el.textContent.trim()));
    try {
        await open("/qapps/BasicComms/Calls/Dialer?number=%2B15557777", "dialer");
        await until(() => field() && field().value === "+15557777", "the handed number filled in");
        await until(() => root.$refs.view.loaded, "the Dialer's data");
        assert.equal(placed().length, 0, "not placed by itself");
        callButton().click();
        await until(() => placed().length === 1, "placed on the user's Call");
        assert.equal(placed()[0].params.phoneNumber, "+15557777");
        assert.equal(placed()[0].params.fromNumber, "+15550100", "from the user's number");
        assert.equal(placed()[0].params.hideCallerId, "false");
        // *67 before the number: taken off, and Hide my number set
        await open("/qapps/BasicComms/Calls/Dialer?number=%2A67%2B15558888", "dialer");
        await until(() => field() && field().value === "+15558888", "the number without *67");
        await until(() => root.$refs.view.hideCallerId, "the hide-caller-ID code applied");
        callButton().click();
        await until(() => placed().length === 2, "the hidden call placed");
        assert.equal(placed()[1].params.phoneNumber, "+15558888");
        assert.equal(placed()[1].params.hideCallerId, "true");
        // several accounts can call, the current one not: asked which; the pick goes to the host, nothing placed
        choice = { number: "+15559999", accounts: [{ id: "a2", label: "ann · Office" }, { id: "a3", label: "ann · Shop" }] };
        await open("/qapps/BasicComms/Calls/Dialer?number=%2B15559999&chooseAccount=true", "dialer");
        const shop = await until(() => Array.from(document.querySelectorAll(".q-dialog .q-radio")).find((el) => el.textContent.includes("ann · Shop")), "the accounts to pick");
        shop.click();
        await answerDialog("Continue");
        assert.deepEqual(picked, ["a3"]);
        assert.equal(placed().length, 2, "a pick places nothing");
    } finally {
        delete desktop.dialChoice;
        delete desktop.dialFrom;
        root.goHome();
    }
});

test("E2EE by default (Comms-Messaging §6.5): Chat makes the device; a dm link's unsaved chat shows the label it will have; a plain direct chat offers Start an encrypted chat, which links back; Set up this device; My devices in the shell", async () => {
    const crypto = moqui.crypto;
    const keptCrypto = { setupStatus: crypto.setupStatus, roomTrust: crypto.roomTrust, restoreBackup: crypto.restoreBackup, verifyDevice: crypto.verifyDevice,
        requestSecrets: crypto.requestSecrets, status: crypto.state.status };
    const keptChat = { openEvents: moqui.chat.openEvents, openEvent: moqui.chat.openEvent };
    const keptAnswers = Object.assign({}, answers);
    const direct = { chatRoomId: "R6", roomTypeEnumId: "CrtDirect", roomName: null, displayName: "Bob", otherUserId: "bob", otherUserIdList: ["bob"],
        lastEventSeq: 0, isMember: true, unreadCount: 0 };
    const sealedDirect = Object.assign({}, direct, { chatRoomId: "R7", encrypted: true });
    const roomList = [room, sealedRoom, direct];
    const created = [];
    answers["/apps/BasicComms/Chat/chatData"] = () => ({ me: { userId: "ann" }, prefs: {}, roomList: clone(roomList) });
    answers["/apps/BasicComms/Chat/roomView"] = (params) => {
        const shown = { R6: direct, R7: sealedDirect }[params.chatRoomId] || roomOf(params);
        return { room: clone(shown), member: { lastReadEventSeq: 6 }, access: { isMember: true, canPost: true },
            memberList: [{ userId: "ann", name: "Ann" }, { userId: "bob", name: "Bob" }], timeline: shown.chatRoomId === "R6" || shown.chatRoomId === "R7"
                ? { eventList: [], hasMoreBefore: false, hasMoreAfter: false } : timelineOf({ chatRoomId: params.chatRoomId }) };
    };
    answers["/apps/BasicComms/Chat/encryptionDefault"] = () => ({ encrypted: true });
    answers["/apps/BasicComms/Chat/createDirect"] = (params) => {
        created.push(clone(params));
        if (params.encrypted === "true") { roomList.push(sealedDirect); return { chatRoomId: "R7", created: true, encrypted: true }; }
        return { chatRoomId: "R1", created: false, encrypted: false };
    };
    answers["/apps/BasicComms/Settings/Devices/devices"] = () => ({ deviceList: [] });
    answers["/apps/BasicComms/Settings/Devices/keySettings"] = () => ({ verifiedDevicesOnly: false });
    moqui.chat.openEvents = (roomId, list) => Promise.resolve(list);
    moqui.chat.openEvent = () => Promise.resolve();
    try {
        // the device was asked for once Chat's rooms showed (in idle time), in the tests before this one
        await until(() => madeDevices.length > 0, "the device asked for");
        // a dm link to someone with no direct chat: the unsaved chat says it will be encrypted; its first message is sent by the room
        const chat = await open("/qapps/BasicComms/Chat?dm=cy", "chat");
        await until(() => chat.pendingDirect && chat.pendingDirect.encrypted === true, "the default asked");
        const pendingLabel = await until(() => document.querySelector(".m-chat-pending-e2ee"), "the unsaved chat's label");
        assert.ok(pendingLabel.textContent.includes("end-to-end encrypted"), pendingLabel.textContent);
        assert.ok(chat.pendingLabel.lock, "the lock");
        chat.pendingDirect.text = "hello cy";
        chat.sendPendingDirect();
        await until(() => host.added.find((op) => op.kind === "chat-send" && op.payload.chatRoomId === "R1" && JSON.stringify(op.payload).includes("hello cy")),
            "the first message sent by the room that was made");
        assert.deepEqual(created[0], { userIds: "cy" });
        // a plain direct chat whose person has a device too: Start an encrypted chat, then the encrypted one links back to it
        const plain = await open("/qapps/BasicComms/Chat?chatRoomId=R6", "chat");
        const vm = await until(() => plain.$refs.room && plain.$refs.room.roomId === "R6" && plain.$refs.room.state === "ready" && plain.$refs.room, "the plain direct room");
        const start = await until(() => vm.$el.querySelector(".m-chat-start-encrypted"), "Start an encrypted chat");
        assert.ok(start.textContent.includes("Start an encrypted chat"), start.textContent);
        start.click();
        const sealed = await until(() => plain.$refs.room && plain.$refs.room.roomId === "R7" && plain.$refs.room.state === "ready" && plain.$refs.room, "the encrypted direct room");
        assert.deepEqual(created[1], { userIds: "bob", encrypted: "true" });
        const earlier = await until(() => sealed.$el.querySelector(".m-chat-earlier-plain"), "the link to the earlier chat");
        assert.ok(earlier.textContent.includes("Earlier messages with Bob are in a chat that is not end-to-end encrypted"), earlier.textContent);
        assert.equal(sealed.plainSibling, "R6");
        // the picker opens the encrypted one now that there are both
        await plain.loadRooms();
        await until(() => moqui.chat.directRoomFor(plain.rooms, ["bob"], "ann") === "R7", "the encrypted one picked once the rooms are read again");
        // a new device: Set up this device in the banner's place, the message it cannot read, and the card's two ways
        let status = { deviceId: "N1", backup: "3", restored: false, verified: false, otherDevices: [{ deviceId: "O1", displayName: "Desktop", lastSeenDate: Date.now() }] };
        const restored = [];
        const verified = [];
        crypto.state.status = "ready";
        crypto.roomTrust = () => Promise.resolve({ verifiedOnly: false, users: [] });
        crypto.setupStatus = () => Promise.resolve(clone(status));
        crypto.restoreBackup = (key) => { restored.push(key); status = Object.assign({}, status, { restored: true }); return Promise.resolve({ imported: 4 }); };
        crypto.verifyDevice = (userId, deviceId) => { verified.push(userId + "/" + deviceId); return Promise.resolve("F1"); };
        sealed.loadSetup();
        const bar = await until(() => sealed.$el.querySelector(".m-chat-setup-bar"), "the set-up bar");
        assert.equal(sealed.$el.querySelector(".m-chat-e2ee-banner"), null, "in the banner's place");
        assert.equal(moqui.chat.undecryptableText(null, "Bob", sealed.setupNeeded), "Sent before this device was set up. Set up this device to read it.");
        bar.querySelector(".m-chat-setup").click();
        const card = await until(() => document.querySelector(".m-chat-device-setup"), "the card");
        card.querySelector(".m-chat-device-setup-verify").click();
        await until(() => verified.length, "Verify asked");
        assert.deepEqual(verified, ["ann/O1"]);
        const input = card.querySelector("input[name='setupRecoveryKey']");
        input.value = "EsTj 3yST y93F";
        input.dispatchEvent(new Event("input"));
        await Vue.nextTick();
        card.querySelector(".m-chat-device-setup-restore").click();
        await until(() => restored.length, "Restore asked");
        assert.deepEqual(restored, ["EsTj 3yST y93F"]);
        await until(() => document.querySelector(".m-chat-device-setup-done"), "the card says it is set up");
        await until(() => !sealed.$el.querySelector(".m-chat-setup-bar"), "the bar gone");
        sealed.setupOpen = false;
        await until(() => !card.isConnected, "the completed device setup to close");
        // verified with the other device, its backup key not here yet: still not set up, the card waits; after two minutes
        // it says the keys did not come, with Try again (the secrets asked again) and Use your recovery key
        const secretAsks = [];
        crypto.requestSecrets = () => { secretAsks.push(1); return Promise.resolve(true); };
        status = Object.assign({}, status, { restored: false, verified: true });
        sealed.loadSetup();
        await until(() => sealed.$el.querySelector(".m-chat-setup-bar"), "the bar stays while the keys have not come");
        sealed.openSetup();
        const waiting = await until(() => document.querySelector(".m-chat-device-setup-waiting"), "the card waits for the keys");
        assert.ok(waiting.textContent.includes("Getting your keys from the other device"), waiting.textContent);
        assert.equal(document.querySelector(".m-chat-device-setup-done"), null, "not set up");
        const cardVm = await until(() => sealed.$refs.setupCard, "the card's component");
        cardVm.verifiedAt = Date.now() - 121000;
        cardVm.now = Date.now();
        const slow = await until(() => document.querySelector(".m-chat-device-setup-slow"), "the keys did not come");
        assert.ok(slow.textContent.includes("did not send the keys"), slow.textContent);
        assert.ok(document.querySelector(".m-chat-device-setup-use-key"), "Use your recovery key");
        document.querySelector(".m-chat-device-setup-again").click();
        await until(() => secretAsks.length, "the secrets asked again");
        await until(() => !document.querySelector(".m-chat-device-setup-slow"), "waiting again");
        sealed.setupOpen = false;
        // My devices is the shell's own screen now
        const devices = await open("/qapps/BasicComms/Settings/Devices", "devices");
        await until(() => devices.$el && devices.$el.querySelector && devices.$el.querySelector(".m-chat-devices") || document.querySelector(".m-chat-devices"), "My devices in the shell");
        await until(() => asked("/apps/BasicComms/Settings/Devices/devices").length, "its devices asked of the server");
    } finally {
        Object.assign(crypto, { setupStatus: keptCrypto.setupStatus, roomTrust: keptCrypto.roomTrust, restoreBackup: keptCrypto.restoreBackup,
            verifyDevice: keptCrypto.verifyDevice, requestSecrets: keptCrypto.requestSecrets });
        crypto.state.status = keptCrypto.status;
        Object.assign(moqui.chat, keptChat);
        Object.keys(answers).forEach((key) => { if (!(key in keptAnswers)) delete answers[key]; });
        Object.assign(answers, keptAnswers);
        await open("/qapps/BasicComms/Chat?chatRoomId=R1", "chat");
    }
});

test("with no host, as on the web, a message's changes are asked of the server as before", async () => {
    const vm = await openRoom();
    const desktop = window.moquiDesktop;
    delete window.moquiDesktop;
    try {
        const count = host.added.length;
        const edits = asked("/apps/BasicComms/Chat/edit", "POST").length;
        await vm.saveEdit({ eventSeq: 5, event: eventOf(vm, 5) }, "<p>from the web</p>");
        assert.equal(asked("/apps/BasicComms/Chat/edit", "POST").length, edits + 1);
        vm.react({ eventSeq: 5, event: eventOf(vm, 5) }, "👀");
        await until(() => asked("/apps/BasicComms/Chat/react", "POST").some((request) => request.params.key === "👀"), "the reaction asked");
        assert.equal(moqui.chat.queueChange("chat-edit", { chatRoomId: "R1", eventSeq: 5, body: "x" }), null);
        assert.equal(host.added.length, count, "nothing queued");
        assert.ok(!vm.queues("chat-react"));
    } finally {
        window.moquiDesktop = desktop;
    }
});

test("a live location's card (plans/Location-Sharing.md §3.5): its position and age from liveLocation, moved by its share's pushes, and Live location ended at its end; the owner's says whom it was shared with", async () => {
    const vm = await openRoom();
    const thruDate = Date.now() + 3600000;
    answers["/apps/BasicComms/Chat/liveLocation"] = (params) => (Number(params.eventSeq) === 40
        ? { state: "live", owner: false, share: { shareId: "LS1", label: "Bob", thruDate, approximate: false },
            latest: { latitude: 40.7452, longitude: -111.875, accuracy: 9, ageSeconds: 120 } }
        : { state: "live", owner: true, share: { shareId: "LS2", label: "Ann", thruDate, approximate: false, sentFromDevice: false, audienceCount: 1 }, latest: null });
    // the messages carry their shares' ids, never a position
    push(message(40, "Live location: Bob, until 15:30", { location: { liveShareId: "LS1", label: "Bob", thruDate, source: "live" } }));
    push(message(41, "Live location: Ann, until 15:30", { senderUserId: "ann", senderName: "Ann", location: { liveShareId: "LS2", label: "Ann", thruDate, source: "live" } }));
    const coordinates = await until(() => document.querySelector(".m-chat-live[data-live-state='live'] .m-chat-location-coordinates"), "the live card's position");
    const card = coordinates.closest(".m-chat-live");
    assert.ok(coordinates.textContent.includes("40.7452, -111.875"), coordinates.textContent);
    assert.ok(card.querySelector(".m-chat-live-age").textContent.includes("2 min ago"), card.textContent);
    assert.ok(card.querySelector(".m-chat-location-open"), "Open in maps while it is live");
    assert.ok(!card.querySelector(".m-chat-live-stop"), "no Stop for a viewer");
    // the share's topic is followed while it is live; a push of it moves the position and makes it fresh
    const client = root.notificationClient;
    await until(() => (client.topicListeners["locationShare:LS1"] || []).length, "the share's topic followed");
    const share = (message) => socket.onmessage({ data: JSON.stringify({ topic: "locationShare:LS1", message }) });
    share({ shareId: "LS1", event: "position", latitude: 40.75, longitude: -111.88, accuracy: 5, observedDate: Date.now() });
    await until(() => card.querySelector(".m-chat-location-coordinates").textContent.includes("40.75, -111.88"), "the pushed position");
    assert.ok(card.querySelector(".m-chat-live-age").textContent.includes("just now"), card.textContent);
    // the owner's card: whom it was shared with, where it is sent from, Stop
    const own = await until(() => Array.from(document.querySelectorAll(".m-chat-live")).find((el) => el.querySelector(".m-chat-live-audience")), "the owner's card");
    assert.ok(own.textContent.includes("Shared with the members of the room when you started it"), own.textContent);
    assert.ok(own.querySelector(".m-chat-live-stop") && own.querySelector(".m-chat-live-here"), "Stop and Send from here");
    // its end: Live location ended, nothing to open, the topic let go
    share({ shareId: "LS1", event: "ended" });
    await until(() => card.getAttribute("data-live-state") === "ended", "the ended card");
    assert.ok(card.textContent.includes("Live location ended"), card.textContent);
    assert.ok(!card.querySelector(".m-chat-location-open"), "nothing to open once it ended");
    await until(() => !client.topicListeners["locationShare:LS1"], "the topic let go");
    assert.ok(vm.events.some((event) => Number(event.eventSeq) === 40));
    delete answers["/apps/BasicComms/Chat/liveLocation"];
});
})();
