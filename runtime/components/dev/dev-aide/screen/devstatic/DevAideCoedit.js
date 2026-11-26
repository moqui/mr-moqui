/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * Co-editing a file in the Workspace on a shared dev server (plans/design-dev-aide.md F15): each open tab of a file the
 * user may write is a session on the file's room (moqui-collab's text rooms, key DevFile:<rootId>:<path>), over the
 * notification socket with the protocol of m-coedit (moqui.coedit.Provider, docs/ui/Qvt-Client.md). The room is seeded
 * from the file by the server; the tab's text is the room's Y.Text from the join on (the editor binds it with
 * y-codemirror.next: the others' carets and selections with their names, Yjs' undo of this user's own edits). A tab
 * with unsaved edits joining a room that is still the file as it read it puts its edits in. Save stays the tab's own,
 * by the hash of the file it read: a save by another member moves this tab's base to the new file (its DevSaved names
 * the hash), so the next save of the shared text is no conflict.
 *
 * A session lasts as long as one open socket: when the socket closes the sessions end (the editors keep their text) and
 * the next open starts them again with fresh documents, so a room seeded again after everyone left is never merged with
 * an old copy. A join the server refuses (not shared, not a writer of the file, too many files, too large) leaves the
 * tab to plain editing until the socket opens again. Loaded by the Workspace in shared mode only.
 *
 * The pure parts are moqui.devAide.coedit, which test/client/pages/DevAideCoedit.page.js runs in the page with a fake
 * room.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui || (root.moqui = {});
    const devAide = moqui.devAide = moqui.devAide || {};
    const coedit = devAide.coedit = devAide.coedit || {};

    // ---- pure parts ----

    // The room key of a file, and the longest the socket takes (notws/collab.rs): a longer one is not co-edited.
    coedit.keyOf = (rootId, path) => "DevFile:" + rootId + ":" + path;
    coedit.MAX_KEY = 200;
    // Text with LF line ends, as the editor and the room keep it (the file's own are written back by Save).
    coedit.lf = (text) => String(text == null ? "" : text).replace(/\r\n?/g, "\n");
    // Whether a tab may join its file's room: a writable text file that has loaded, of a root that is not hosted.
    coedit.eligible = (tab, rootId, hosted) => !!(tab && rootId && !hosted && !tab.loading && !tab.readOnly && !tab.binary
        && !tab.missing && coedit.keyOf(rootId, tab.path).length <= coedit.MAX_KEY);
    // What a tab does with the room's text when it joins: push its unsaved edits into a room that is still the file it
    // read; else take the room's text, saying when that drops unsaved edits of its own.
    coedit.joinPlan = (tab, roomText) => {
        const mine = coedit.lf(tab.text), saved = coedit.lf(tab.saved);
        if (mine === roomText) return { take: true, push: false, dropped: false };
        if (mine !== saved && roomText === saved) return { take: false, push: true, dropped: false };
        return { take: true, push: false, dropped: mine !== saved };
    };
    // The one change that turns `before` into `after` (as the editor's textChange).
    coedit.change = (before, after) => {
        let start = 0;
        const shortest = Math.min(before.length, after.length);
        while (start < shortest && before.charCodeAt(start) === after.charCodeAt(start)) start++;
        let end = 0;
        while (end < shortest - start && before.charCodeAt(before.length - 1 - end) === after.charCodeAt(after.length - 1 - end)) end++;
        return { from: start, to: before.length - end, insert: after.slice(start, after.length - end) };
    };
    // `text` written into a Y.Text as the one change from what it holds.
    coedit.writeText = (ytext, text, origin) => {
        const now = ytext.toString();
        if (now === text) return false;
        const change = coedit.change(now, text);
        ytext.doc.transact(() => {
            if (change.to > change.from) ytext.delete(change.from, change.to - change.from);
            if (change.insert) ytext.insert(change.from, change.insert);
        }, origin);
        return true;
    };
    // The names of the others in a session's status, once each (two tabs of one user are one name).
    coedit.peerNames = (status, me) => {
        const names = [];
        for (const peer of (status && status.peers) || []) {
            if (me && peer.userId === me) continue;
            const name = peer.name || peer.userId || "";
            if (name && names.indexOf(name) < 0) names.push(name);
        }
        return names;
    };

    // ---- a session: one tab on its file's room ----

    // options: client (the notification client), rootId, tab (the Workspace's reactive tab), user ({id, name, color}),
    // lib ({Yjs, YCodeMirror}), markRaw (Vue's), onRefused(tab, info) and onDropped(tab) (the tab's unsaved edits gave
    // way to the room's text), batchMs.
    class Session {
        constructor(options) {
            this.options = options;
            this.tab = options.tab;
            this.key = coedit.keyOf(options.rootId, options.tab.path);
            this.ended = false;
        }
        start() {
            const { Yjs } = this.options.lib;
            const Y = Yjs.Y;
            const doc = this.doc = new Y.Doc();
            this.ytext = doc.getText("codemirror");
            this.awareness = new Yjs.Awareness(doc);
            const user = this.options.user || {};
            const color = user.color || "#1e88e5";
            this.awareness.setLocalStateField("user", { name: user.name || user.id || "", color, colorLight: color + "33" });
            this.undoManager = new Y.UndoManager(this.ytext);
            // the Workspace's own changes of the text (a reload, Take the disk's) are this user's to undo too
            this.undoManager.trackedOrigins.add(this);
            this.onText = () => {
                const text = this.ytext.toString();
                if (this.tab.text !== text) this.tab.text = text;
            };
            this.provider = new moqui.coedit.Provider({ client: this.options.client, docKey: this.key, doc, awareness: this.awareness,
                lib: Yjs, batchMs: this.options.batchMs,
                onStatus: (status) => this.status(status),
                onAccess: (canEdit) => this.access(canEdit),
                onFallback: (info) => { this.end(); if (this.options.onRefused) this.options.onRefused(this.tab, info); } });
            this.provider.start();
            return this;
        }
        // the join answered: the tab's text and the room's made one, then the editor binds the Y.Text
        access(canEdit) {
            if (this.ended) return;
            if (!canEdit) { this.end(); if (this.options.onRefused) this.options.onRefused(this.tab, { reason: "server", code: 403 }); return; }
            if (this.joined) return;
            this.joined = true;
            const plan = coedit.joinPlan(this.tab, this.ytext.toString());
            if (plan.push) coedit.writeText(this.ytext, coedit.lf(this.tab.text), this);
            this.undoManager.stopCapturing();
            this.tab.saved = coedit.lf(this.tab.saved);
            this.ytext.observe(this.onText);
            this.onText();
            if (plan.dropped && this.options.onDropped) this.options.onDropped(this.tab);
            const markRaw = this.options.markRaw || ((value) => value);
            this.tab.collab = markRaw({ ytext: this.ytext, awareness: this.awareness, undoManager: this.undoManager, key: this.key });
        }
        status(status) {
            if (this.ended) return;
            const me = this.options.user && this.options.user.id;
            this.tab.coedit = { state: status.state, peers: coedit.peerNames(status, me) };
        }
        // the tab's text set by the Workspace (a reload, Take the disk's, a draft restored), not by the room: into the room
        fromTab(text) {
            if (this.ended || !this.joined) return;
            // its own undo step, apart from the typing around it
            this.undoManager.stopCapturing();
            if (coedit.writeText(this.ytext, coedit.lf(text), this)) this.undoManager.stopCapturing();
        }
        // leave the room (the provider sends its last batch first); the tab keeps its text and edits on alone
        end() {
            if (this.ended) return;
            this.ended = true;
            if (this.provider) this.provider.destroy();
            if (this.joined) this.ytext.unobserve(this.onText);
            this.tab.collab = null;
            this.tab.coedit = null;
            this.undoManager.destroy();
            this.awareness.destroy();
            this.doc.destroy();
        }
    }
    coedit.Session = Session;

    // ---- the Workspace's sessions ----

    // One per Workspace: sync(rootId, tabs, hosted) starts a session for each eligible tab and ends the others; the
    // sessions end with the socket and start again when it opens; a refused tab waits for the next open. options:
    // client, user, markRaw, watch(tab, fn) (Vue's $watch of the tab's text, answering its stop), onDropped, batchMs.
    class Sessions {
        constructor(options) {
            this.options = options;
            this.sessions = new Map();
            this.refused = new Set();
            this.lib = null;
            this.stateOff = options.client.onState((state) => {
                if (state === "open") { this.refused.clear(); this.resync(); }
                else this.endAll();
            });
        }
        // the libraries (moqui.codeEditor.loadCollab), once
        load() {
            if (this.lib) return Promise.resolve(this.lib);
            return moqui.codeEditor.loadCollab().then((lib) => (this.lib = lib));
        }
        sync(rootId, tabs, hosted) {
            this.last = { rootId, tabs, hosted };
            if (this.destroyed) return Promise.resolve();
            return this.load().then(() => this.resync(), (error) => {
                console.error("Co-editing could not load; the files are edited alone", error);
            });
        }
        resync() {
            if (!this.last || !this.lib || this.destroyed) return;
            const { rootId, tabs, hosted } = this.last;
            const wanted = new Map();
            if (this.options.client.isOpen()) {
                for (const tab of tabs) {
                    const key = coedit.keyOf(rootId, tab.path);
                    if (coedit.eligible(tab, rootId, hosted) && !this.refused.has(key)) wanted.set(key, tab);
                }
            }
            for (const [key, entry] of this.sessions) {
                if (wanted.get(key) !== entry.session.tab) this.endOne(key);
            }
            for (const [key, tab] of wanted) {
                if (this.sessions.has(key)) continue;
                const session = new Session({ client: this.options.client, rootId, tab, user: this.options.user, lib: this.lib,
                    markRaw: this.options.markRaw, batchMs: this.options.batchMs,
                    onDropped: this.options.onDropped,
                    onRefused: (refusedTab, info) => {
                        this.refused.add(key);
                        const entry = this.sessions.get(key);
                        if (entry && entry.session.tab === refusedTab) { if (entry.stop) entry.stop(); this.sessions.delete(key); }
                        if (info && info.code && info.code !== 403) console.info("Co-editing " + refusedTab.path + " was refused (" + info.code + "): " + (info.message || ""));
                    } });
                const entry = { session };
                this.sessions.set(key, entry);
                if (this.options.watch) entry.stop = this.options.watch(tab, (text) => session.fromTab(text));
                session.start();
            }
        }
        endOne(key) {
            const entry = this.sessions.get(key);
            if (!entry) return;
            this.sessions.delete(key);
            if (entry.stop) entry.stop();
            entry.session.end();
        }
        endAll() { for (const key of Array.from(this.sessions.keys())) this.endOne(key); }
        destroy() {
            this.destroyed = true;
            this.endAll();
            if (this.stateOff) this.stateOff();
        }
    }
    coedit.Sessions = Sessions;
    coedit.loaded = true;
})(typeof window !== "undefined" ? window : globalThis);
