/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
function mountMailScreen(name, params, navigation) {
    moqui.qvt.applyRenders();
    const screen = moqui.bundledComponent("BasicComms/Inbox/" + name);
    const app = Vue.createApp({
        data() { return { basePath: "/apps", linkBasePath: "/qapps", currentPathList: ["BasicComms", "Inbox", name],
            currentParameters: params, shellCounts: {}, quickCreateShow: false }; },
        computed: { currentLinkPath() { return "/qapps/" + this.currentPathList.join("/"); } },
        methods: { setUrl() {}, setParameters(parameters) { navigation.push(parameters); }, addContainer() {}, removeContainer() {} },
        render() { return Vue.h(screen, { ref: "screen" }); }
    });
    app.use(Quasar, { plugins: moqui.qvt.quasarPlugins(Quasar) });
    // The message body itself has its own iframe regression. This fixture exercises the Inbox's surrounding controls.
    for (const [name, component] of Object.entries(moqui.qvt.components))
        app.component(name, name === "m-html-view" ? { props: ["text"], render() { return Vue.h("p", this.text); } } : component);
    app.config.globalProperties.moqui = moqui;
    const el = document.createElement("div");
    document.body.appendChild(el);
    return { app, el, screen: app.mount(el).$refs.screen };
}

test("mail actions follow mailbox rights, the message's folder, server refusals and each thread's snooze time", async () => {
    const deadline = Date.now() + 3600000;
    const row = (id, until) => ({ store: "email", rowKey: "email:" + id, threadId: id, emailAccountId: "A", subject: id,
        folderKindEnumId: "EfkJunk", sortDate: Date.now(), snoozedThruDate: until, participantList: [], labelList: [] });
    const rows = [row("one", deadline), row("two", deadline + 10000)];
    const account = { emailAccountId: "A", accountName: "Shared", canSend: false, folderList: [], labelList: [] };
    const message = { store: "email", key: "email:m", emailMessageId: "m", emailAccountId: "A", folderKindEnumId: "EfkJunk", date: Date.now(),
        from: { address: "cy@example.org" }, toList: [], ccList: [], bccList: [], linkList: [], attachmentList: [], text: "Hello",
        snoozedThruDate: deadline, isFlagged: false };
    const thread = { store: "email", threadId: "three", subject: "Direct link", emailAccountId: "A", folderKindEnumId: "EfkJunk", canSend: false,
        messageList: [message], labelList: [], linkList: [] };
    const writes = [], alerts = [];
    let failed = [], hold = false, release, conversationView = "threads";
    const other = { emailAccountId: "B", accountName: "Other", canSend: false, folderList: [], labelList: [] };
    const navigation = [];
    const notices = moqui.notifyMessages;
    moqui.notifyMessages = (infos, errors) => alerts.push({ infos, errors });
    moqui.request = async (url, options) => {
        const name = String(url).split("?")[0].split("/").pop();
        let json = {};
        if (name === "inboxData") json = { accountList: [account, other], mailboxList: [], searchList: [], me: {}, totals: {}, prefs: {} };
        if (name === "threads") json = { threadList: rows.slice(), conversationView, storeList: ["email"] };
        if (name === "thread") json = { thread };
        if (name === "state" || name === "viewAction") {
            const params = options.params;
            const targets = params.targetsJson ? JSON.parse(params.targetsJson) : rows;
            writes.push({ name, targets, action: params.action, until: Number(params.snoozedThruDate) });
            json = { updatedCount: targets.length - failed.length, failedList: failed };
            if (hold) await new Promise((resolve) => { release = resolve; });
        }
        return { ok: true, status: 200, json };
    };
    const { app, el, screen: mail } = mountMailScreen("Mail", { thread: "email:three" }, navigation);
    const settle = async () => { await new Promise((resolve) => setTimeout(resolve, 0)); await Vue.nextTick(); };
    try {
        await settle();
        assert.equal(mail.thread.threadId, "three");
        assert.equal(el.querySelector('button[aria-label="Flag"]'), null, "no flag on the row or the message of read-only mail");
        assert.ok(el.querySelector('button[aria-label="Mark unread"]'));
        mail.toggleAll();
        mail.allInView = true;
        mail.keyAction("archive");
        mail.openSnooze(mail.keyRows());
        mail.openMove(mail.keyRows());
        mail.openLabels(mail.keyRows());
        assert.equal(mail.snooze.open || mail.move.open || mail.labels.open, false);
        assert.equal(writes.length, 0);
        mail.keyAction("unread");
        await settle();
        assert.equal(writes[0].name, "viewAction");
        assert.equal(writes[0].action, "unread");

        account.canSend = true;
        thread.canSend = true;
        await mail.loadData();
        mail.reloadThread();
        await settle();
        assert.ok(el.querySelector('button[aria-label="Unsnooze"]'), "a snoozed thread outside the current list can wake now");
        assert.equal(mail.threadRow().snoozedThruDate, deadline);

        failed = [{ key: "email:m", message: "Rights changed" }];
        mail.flagMessage(mail.lastMessage);
        mail.reportMessageSpam(mail.lastMessage);
        await settle();
        assert.equal(mail.lastMessage.isFlagged, false);
        assert.equal(writes[writes.length - 1].action, "notSpam", "More uses the selected message's folder");
        assert.ok(alerts.some((notice) => notice.errors === "Rights changed"));
        assert.ok(!alerts.some((notice) => (notice.infos || []).some((info) => info.message.includes("back in the Inbox"))));
        failed = [];
        mail.keyAction("spam");
        await settle();
        assert.equal(writes[writes.length - 1].action, "notSpam", "the shortcut uses the opened thread in Spam");

        mail.toggleAll();
        failed = [{ key: "email:two", message: "Mailbox changed" }];
        mail.bulk("flag");
        await settle();
        assert.equal(mail.threads[0].isFlagged, true);
        assert.ok(!mail.threads[1].isFlagged);
        assert.deepEqual(mail.selectedKeys, ["email:two"], "only the refused thread stays selected");
        failed = [];
        mail.clearSelection();
        mail.toggleAll();
        mail.bulk("unsnooze");
        await settle();
        const beforeUndo = writes.length;
        mail.undo();
        await settle();
        assert.deepEqual(writes.slice(beforeUndo).map((call) => [call.action, call.targets[0].threadId, call.until]),
            [["snooze", "one", deadline], ["snooze", "two", deadline + 10000]]);

        // Account A is writable, B is read-only. The whole Inbox cannot be changed, but the individual row in A can.
        mail.toggleAll();
        mail.allInView = true;
        assert.equal(mail.selectionCanChange, false);
        const beforeRow = writes.length;
        mail.toggleRowFlag(mail.threads[0]);
        await settle();
        assert.equal(writes.length, beforeRow + 1);
        mail.$root.currentParameters = { account: "A" };
        await settle();
        mail.toggleAll();
        mail.allInView = true;
        assert.equal(mail.selectionCanChange, true, "all-in-view is scoped to A");
        mail.bulk("flag");
        await settle();
        assert.equal(writes[writes.length - 1].name, "viewAction");

        // In message mode both rows belong to the same root. A refusal for the reply keeps only the reply selected.
        conversationView = "messages";
        rows[0].emailMessageId = "one";
        rows[1].emailMessageId = "two";
        rows[1].threadId = "one";
        rows[0].folderKindEnumId = "EfkInbox";
        thread.threadId = "one";
        thread.folderKindEnumId = "EfkInbox";
        thread.messageList = [
            Object.assign({}, message, { emailMessageId: "two", key: "email:two", folderKindEnumId: "EfkJunk" }),
            Object.assign({}, message, { emailMessageId: "one", key: "email:one", folderKindEnumId: "EfkInbox" })
        ];
        await mail.loadList(true);
        mail.openThread("email:two");
        await settle();
        failed = [{ key: "email:two", message: "Reply refused" }];
        const header = el.querySelector('button[aria-label="Not spam"]');
        assert.ok(header, "the older reply in Junk determines the header's name despite its newer Inbox sibling");
        header.click();
        mail.keyAction("spam");
        await settle();
        assert.deepEqual(writes.slice(-2).map((call) => [call.action, call.targets[0].emailMessageId]), [["notSpam", "two"], ["notSpam", "two"]]);
        rows[1].folderKindEnumId = "EfkTrash";
        thread.messageList[0].folderKindEnumId = "EfkTrash";
        await mail.loadList(true);
        await settle();
        assert.ok(el.querySelector('button[aria-label="Delete forever"]'));
        const beforeDelete = writes.length;
        let asked = 0;
        mail.confirmDeleteForever = () => { asked++; return Promise.resolve(); };
        mail.keyAction("trash");
        await settle();
        assert.equal(asked, 1, "the shortcut confirms permanent deletion of the selected reply");
        assert.deepEqual(writes.slice(beforeDelete).map((call) => [call.action, call.targets[0].emailMessageId]), [["deleteForever", "two"]]);
        mail.toggleAll();
        failed = [{ key: "email:two", message: "Reply refused" }];
        mail.bulk("archive");
        await settle();
        assert.deepEqual(writes[writes.length - 1].targets.map((target) => target.emailMessageId), ["one", "two"]);
        assert.deepEqual(mail.threads.map((item) => item.rowKey), ["email:two"]);
        assert.deepEqual(mail.selectedKeys, ["email:two"]);

        // A late refusal cannot restore an older selection. A late success cannot navigate away from a newly opened thread.
        await mail.loadList(true);
        hold = true;
        mail.bulk("archive");
        mail.clearSelection();
        mail.checkRow(mail.threads[0], true);
        release();
        await settle();
        assert.deepEqual(mail.selectedKeys, ["email:one"]);
        failed = [];
        mail.openThread("email:one");
        await settle();
        const moves = navigation.length;
        mail.bulk("archive");
        mail.clearSelection();
        mail.checkRow(mail.threads[1], true);
        mail.openThread("email:two");
        await settle();
        release();
        await settle();
        assert.deepEqual(mail.selectedKeys, ["email:two"]);
        assert.equal(navigation.length, moves, "the archive of the previous message did not move the open pane");

        // The displayed draft has different recipients from the thread it replies to or forwards.
        const draft = (id, fields) => Object.assign(row(id, null), { statusId: "ES_DRAFT", folderKindEnumId: "EfkDrafts",
            messageCount: 2, participantList: [{ name: "Prior correspondent" }] }, fields);
        rows.splice(0, rows.length, draft("empty", { recipientList: [] }), draft("bcc", { recipientList: [{ address: "blind@example.org" }] }),
            draft("old-cache", {}), draft("received", { statusId: "ES_RECEIVED", recipientList: null, fromName: "Latest sender" }));
        for (const params of [{ view: "drafts" }, { account: "A", folder: "DF" }]) {
            mail.$root.currentParameters = params;
            await settle();
            await mail.loadList(true);
            await Vue.nextTick();
            const summary = (id) => el.querySelector('[aria-label^="' + id + '. "]').getAttribute("aria-label");
            assert.equal(summary("empty"), "empty. No recipients. 2 messages");
            assert.equal(summary("bcc"), "bcc. To: blind. 2 messages");
            assert.equal(summary("old-cache"), "old-cache. Recipients unavailable. 2 messages");
            assert.equal(summary("received"), "received. Prior correspondent, Latest sender. 2 messages");
        }
    } finally {
        moqui.notifyMessages = notices;
        app.unmount();
        el.remove();
    }
});

test("Compose inserts a whole block at the caret and reads the shared blocks again on every opening", async () => {
    const long = "The shared block includes more than eighty characters so its menu preview is shorter than the text inserted here.";
    const block = { emailTextBlockId: "shared", title: "Delivery", bodyHtml: "<p>" + long + "</p><p>Next line</p>" };
    const pending = [];
    moqui.request = async (url) => {
        if (String(url).endsWith("/textBlocks")) return new Promise((resolve) => pending.push((blocks) => resolve({ json: { textBlockList: blocks } })));
        if (String(url).endsWith("/composeData")) return { json: { compose: { fromValue: "account:A", identityList: [{ value: "account:A", label: "Shared" }], kind: "new", to: [] } } };
        return { json: { emailMessageId: "Draft1" } };
    };
    // Keep the asynchronous show/hide and focus handoff, with a short animation for this regression.
    const duration = Quasar.QMenu.props.transitionDuration.default;
    Quasar.QMenu.props.transitionDuration.default = 40;
    const { app, el, screen: compose } = mountMailScreen("Compose", {}, []);
    const until = async (check, what) => {
        const end = Date.now() + 4000;
        while (!check()) {
            if (Date.now() > end) throw new Error("Waited for " + what);
            await new Promise((resolve) => setTimeout(resolve, 10));
        }
    };
    const key = (target, type, value) => target.dispatchEvent(new KeyboardEvent(type, { key: value, bubbles: true, cancelable: true }));
    const menuReady = () => compose.$refs.blockPanel && compose.$refs.blockPanel.querySelector("input") === document.activeElement && pending.length;
    const closed = () => !compose.blocksOpen && !document.querySelector(".q-menu");
    try {
        await until(() => compose.$refs.editor && compose.$refs.editor.editor, "the rich editor");
        const field = compose.$refs.editor;
        field.togglePlain();
        await Vue.nextTick();
        const area = field.$refs.plainArea;
        area.focus();
        area.value = "Before REPLACE after";
        area.dispatchEvent(new Event("input", { bubbles: true }));
        area.setSelectionRange(7, 14);
        compose.$refs.blockButton.$el.click();
        await until(menuReady, "the focused picker");
        pending.shift()([block]);
        await until(() => compose.shownBlocks.length, "the shared block");
        for (const imeKey of ["Escape", "Enter"]) {
            const compositionKey = new KeyboardEvent("keydown", { key: imeKey, isComposing: true, bubbles: true, cancelable: true });
            document.activeElement.dispatchEvent(compositionKey);
            document.activeElement.dispatchEvent(new KeyboardEvent("keyup", { key: imeKey, isComposing: true, bubbles: true }));
            await Vue.nextTick();
            assert.ok(!compositionKey.defaultPrevented, "the IME can handle " + imeKey);
            assert.ok(compose.blocksOpen, "the picker remains open while composing");
        }
        assert.equal(area.value, "Before REPLACE after");
        key(document.activeElement, "keydown", "Enter");
        await until(closed, "the picker closing after insertion");
        const plain = "Before " + long + "\n\nNext line after";
        assert.equal(area.value, plain);
        assert.equal(area.selectionStart, plain.length - 6);
        assert.equal(moqui.richText.toPlain(compose.draft.body), plain, "the stored draft uses the full text, with paragraphs");
        await until(() => document.activeElement === area, "focus returning to the plain editor");

        // The first reopening is slow. By the next opening the owner has unshared the block.
        compose.$refs.blockButton.$el.click();
        await until(menuReady, "the first refresh");
        assert.equal(compose.shownBlocks.length, 0);
        const oldReply = pending.shift();
        key(document.activeElement, "keydown", "Escape");
        await until(closed, "the canceled refresh");
        compose.$refs.blockButton.$el.click();
        await until(menuReady, "the second refresh");
        pending.shift()([]);
        await until(() => compose.blocksState === "ready", "the current empty list");
        oldReply([block]);
        await Vue.nextTick();
        assert.equal(compose.shownBlocks.length, 0, "the older authorized response cannot restore a revoked choice");
        assert.equal(area.value, plain, "unsharing leaves the previously inserted draft text intact");
        key(document.activeElement, "keydown", "Escape");
        await until(closed, "the empty picker closing");

        field.togglePlain();
        await Vue.nextTick();
        const editor = field.editor;
        const slash = () => {
            editor.commands.setContent("<p>/ suffix</p>", { emitUpdate: true });
            editor.commands.focus(2);
            key(editor.view.dom, "keyup", "/");
        };
        slash();
        await until(menuReady, "slash moving focus to search");
        pending.shift()([block]);
        await until(() => compose.shownBlocks.length, "the rich text block");
        key(document.activeElement, "keydown", "Enter");
        await until(closed, "the rich insertion");
        assert.ok(editor.getText().startsWith(long), "the rich block replaced the slash at the start: " + JSON.stringify(editor.getText()));
        assert.ok(editor.getText().includes("Next line") && editor.getText().endsWith(" suffix"), "the whole block precedes the retained suffix: " + JSON.stringify(editor.getText()));
        assert.ok(!editor.getText().includes("/"), "the accepted slash is removed: " + JSON.stringify(editor.getText()));
        await until(() => editor.view.hasFocus(), "focus returning to the rich editor");
        slash();
        await until(menuReady, "slash reopening search");
        pending.shift()([]);
        key(document.activeElement, "keydown", "Escape");
        await until(closed, "Escape returning to the editor");
        assert.equal(editor.getText(), "/ suffix");
        assert.equal(editor.state.selection.from, 2);
        await until(() => editor.view.hasFocus(), "focus returning to the rich editor");
    } finally {
        Quasar.QMenu.props.transitionDuration.default = duration;
        app.unmount();
        el.remove();
    }
});
