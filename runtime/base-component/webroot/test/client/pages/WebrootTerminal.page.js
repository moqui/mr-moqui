/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

const write = (term, text) => new Promise(resolve => term.write(text, resolve))
const painted = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))
let made = 0

// A session of the store with no socket, shown in a box of the page; what it sends is kept.
const terminal = async dark => {
    const store = moqui.terminal
    store.setDark(dark)
    const key = "spec-" + (++made)
    const replies = []
    const screen = document.createElement("div")
    screen.style.cssText = "width:800px;height:320px"
    document.body.appendChild(screen)
    // Keep the test offline; xterm's real input reply still crosses the same send boundary as the socket.
    const session = store.create(key, {kind: "apps", connect: false, send: bytes => replies.push(new TextDecoder().decode(bytes))})
    store.attach(key, screen)
    return {
        store, key, session, replies, screen,
        get term() { return session.term },
        dispose() {
            store.end(key)
            screen.remove()
        }
    }
}

test("switching background reports its new color without resetting terminal content", async () => {
    const view = await terminal(false)
    try {
        const {store, replies} = view
        const term = view.term
        await write(term, "Unsubmitted field value")
        store.setDark(true)
        await write(term, "")
        assert.ok(replies.some(reply => reply.includes("11;rgb:1616/1818/1d1d")), "dark background reaches the TUI decoder")
        assert.equal(term.buffer.active.getLine(0).translateToString(true), "Unsubmitted field value")
        store.setDark(false)
        await write(term, "")
        assert.ok(replies.some(reply => reply.includes("11;rgb:ffff/ffff/ffff")), "light background reaches the TUI decoder")
        assert.equal(store.session(view.key).term, term)
        assert.equal(term.buffer.active.getLine(0).translateToString(true), "Unsubmitted field value")
    } finally {
        view.dispose()
    }
})

const luminance = rgb => rgb.map(value => {
    const channel = value / 255
    return channel <= 0.04045 ? channel / 12.92 : Math.pow((channel + 0.055) / 1.055, 2.4)
}).reduce((sum, value, at) => sum + value * [0.2126, 0.7152, 0.0722][at], 0)
const contrast = (foreground, background) => {
    const a = luminance(foreground), b = luminance(background)
    return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)
}

test("muted terminal text remains readable on both browser backgrounds", async () => {
    const view = await terminal(true)
    try {
        await write(view.term, "\u001b[38;2;110;110;110mSection heading")
        await painted()
        const foreground = () => {
            const text = [...view.screen.querySelectorAll(".xterm-rows span")].find(span => span.textContent.includes("Section heading"))
            assert.ok(text, "the terminal rendered the heading")
            return getComputedStyle(text).color.match(/[\d.]+/g).slice(0, 3).map(Number)
        }
        assert.ok(contrast(foreground(), [22, 24, 29]) >= 4.5, "dark terminal heading contrast")
        view.store.setDark(false)
        await write(view.term, "\r\u001b[38;2;150;150;150mSection heading")
        await painted()
        assert.ok(contrast(foreground(), [255, 255, 255]) >= 4.5, "light terminal heading contrast")
    } finally {
        view.dispose()
    }
})

test("a session's terminal moves to another view with its screen and nothing reconnects", async () => {
    const view = await terminal(false)
    const other = document.createElement("div")
    other.style.cssText = "width:640px;height:240px"
    document.body.appendChild(other)
    try {
        const {store, key, session} = view
        await write(view.term, "kept across the move")
        store.detach(key, view.screen)
        assert.equal(view.screen.childElementCount, 0, "the first view gave the element up")
        store.attach(key, other)
        assert.equal(session.el.parentNode, other, "the element is in the second view")
        assert.equal(store.session(key), session, "the same session, not a new one")
        assert.equal(view.term.buffer.active.getLine(0).translateToString(true), "kept across the move")
        store.detach(key, view.screen)
        assert.equal(session.el.parentNode, other, "a view that no longer holds it takes nothing away")
    } finally {
        view.dispose()
        other.remove()
    }
})

test("a tab's layout splits, closes and moves focus as a tree of panes", async () => {
    const tree = moqui.terminal.tree
    const first = tree.pane("s1")
    let root = tree.split(first, first.id, "row", "s2")
    const second = tree.panes(root)[1]
    root = tree.split(root, second.id, "column", "s3")
    assert.deepEqual(tree.sessions(root), ["s1", "s2", "s3"])
    assert.equal(tree.neighbor(root, first.id, true).session, "s2")
    assert.equal(tree.neighbor(root, first.id, false).session, "s3", "wraps around")
    root = tree.remove(root, second.id)
    assert.deepEqual(tree.sessions(root), ["s1", "s3"], "its sibling takes the split's place")
    assert.equal(tree.remove(tree.remove(root, first.id), tree.panes(root)[1].id), null, "no panes left")
})

test("a kept session attaches with what it drew, acknowledges what it draws, and resumes after a drop", async () => {
    const store = moqui.terminal
    const sockets = []
    const openSocket = moqui.openSocket
    moqui.openSocket = url => {
        const socket = {url, readyState: 1, sent: [], send(message) { this.sent.push(message) }, close() { this.readyState = 3 }}
        sockets.push(socket)
        return socket
    }
    const screen = document.createElement("div")
    screen.style.cssText = "width:800px;height:320px"
    document.body.appendChild(screen)
    const key = "kept-" + (++made)
    try {
        store.create(key, {kind: "command", connect: false})
        store.connect(key)
        store.attach(key, screen)
        const socket = sockets[0]
        assert.ok(socket.url.endsWith("/termws?session=" + key), socket.url)
        socket.onopen()
        const attach = JSON.parse(socket.sent.find(message => typeof message === "string" && message.includes('"attach"')))
        assert.equal(attach.have, null, "a first attach has drawn nothing")
        socket.onmessage({data: JSON.stringify({type: "attached", kind: "command", from: 0, reset: true})})
        const chunk = new TextEncoder().encode("x".repeat(store.ACK_EVERY + 10)).buffer
        socket.onmessage({data: chunk})
        await write(store.session(key).term, "")
        const ack = socket.sent.map(message => typeof message === "string" ? JSON.parse(message) : null).find(message => message && message.type === "ack")
        assert.equal(ack && ack.count, store.ACK_EVERY + 10, "the page says what it drew")
        // the connection drops with no reason: the page attaches again, saying how far it got
        socket.onclose({code: 1006, reason: ""})
        assert.ok(store.state.views[key].reconnecting)
        await new Promise(resolve => setTimeout(resolve, 600))
        const again = sockets[1]
        assert.ok(again, "a second socket")
        again.onopen()
        const resume = JSON.parse(again.sent.find(message => typeof message === "string" && message.includes('"attach"')))
        assert.equal(resume.have, store.ACK_EVERY + 10)
        // taken by another window: no attach of its own, and Take it back
        again.onclose({code: 1000, reason: "opened elsewhere"})
        assert.ok(store.state.views[key].taken && store.state.views[key].closed)
    } finally {
        moqui.openSocket = openSocket
        store.end(key)
        screen.remove()
    }
})

test("the terminal's own keys pass over xterm.js, and copy and paste stay its own", () => {
    const key = (key, extra) => Object.assign({key, ctrlKey: true, shiftKey: true, altKey: false, metaKey: false}, extra)
    assert.ok(moqui.terminal.isOwnKey(key("D")))
    assert.ok(moqui.terminal.isOwnKey(key("Enter")))
    assert.ok(!moqui.terminal.isOwnKey(key("C")), "copy")
    assert.ok(!moqui.terminal.isOwnKey(key("V")), "paste")
    assert.ok(!moqui.terminal.isOwnKey(key("d", {shiftKey: false})), "Ctrl+D is the program's")
})

test("the terminal's components and the Terminal screen compile", async () => {
    const compiler = globalThis.VueCompilerDOM
    const compile = (name, template) => {
        const errors = []
        compiler.compile(template, {mode: "function", onError: err => errors.push(err.message)})
        assert.deepEqual(errors, [], name)
    }
    for (const name of ["m-terminal-view", "m-terminal-node", "m-terminal-copy"]) compile(name, moqui.qvtComponents[name].template)
    const source = await fetch("/files/runtime/base-component/webroot/screen/webroot/apps/Terminal.qvue").then(response => response.text())
    const screen = moqui.parseVue(source)
    compile("Terminal.qvue", screen.template)
    for (const id of ["TerminalNew", "TerminalSessionsToggle", "TerminalSettingsOpen", "TerminalSessions"]) assert.ok(screen.template.includes('id="' + id + '"'), id)
})

test("several lines pasted into a shell ask first, unless its program takes a bracketed paste", async () => {
    const store = moqui.terminal
    const key = "paste-" + (++made)
    const sent = []
    const screen = document.createElement("div")
    screen.style.cssText = "width:800px;height:320px"
    document.body.appendChild(screen)
    const asked = []
    const quasar = window.Quasar
    window.Quasar = { Dialog: { create(options) { asked.push(options); const chain = { onOk(fn) { chain.ok = fn; return chain }, onDismiss() { return chain } }; asked.chain = chain; return chain } } }
    try {
        store.create(key, { kind: "host", connect: false, send: bytes => sent.push(new TextDecoder().decode(bytes)) })
        store.attach(key, screen)
        store.paste(key, "one line")
        assert.equal(sent.join(""), "one line", "one line goes straight in")
        store.paste(key, "rm -rf build\nmake")
        assert.equal(asked.length, 1, "several lines ask")
        assert.ok(!sent.join("").includes("make"), "nothing pasted before the answer")
        asked.chain.ok()
        assert.ok(sent.join("").includes("rm -rf build\rmake"), sent.join(""))
        // the program asked for bracketed paste: it takes the lines as one paste, so nothing is asked
        await write(store.session(key).term, "\u001b[?2004h")
        store.paste(key, "a\nb")
        assert.equal(asked.length, 1)
        assert.ok(sent.join("").includes("\u001b[200~a\rb\u001b[201~"), JSON.stringify(sent))
        // the addons: wide characters at their width, and search
        assert.equal(store.session(key).term.unicode.activeVersion, "11")
        await write(store.session(key).term, "\r\nfind the needle here\r\n")
        assert.ok(store.find(key, "needle"), "found in the buffer")
    } finally {
        window.Quasar = quasar
        store.end(key)
        screen.remove()
    }
})

test("a floating terminal takes the session's element, stays out of Assist's reach, and docks without ending it", async () => {
    const store = moqui.terminal
    const floats = moqui.floats
    const key = "float-" + (++made)
    store.create(key, { kind: "host", connect: false, send: () => {} })
    store.startShell()
    const id = store.float(key)
    assert.ok(id, "the window opened")
    // the layer would mount the window's body; here it is made by hand
    const body = document.createElement("div")
    body.id = floats.bodyId(id)
    body.style.cssText = "width:600px;height:300px"
    document.body.appendChild(body)
    floats.bodyReady(id, true)
    await new Promise(resolve => setTimeout(resolve, 50))
    const session = store.session(key)
    try {
        assert.equal(session.el.parentNode, body, "the terminal's element is in the window")
        assert.ok(store.state.floating[key])
        assert.equal(session.el.getAttribute("data-assist-self"), "terminal", "never in Assist's reach")
        assert.ok(!floats.reach().includes(floats.bodyId(id)), "the kind says it is not in reach")
        // Close docks it closed: the session goes on, its terminal kept
        floats.close(id)
        assert.ok(!store.state.floating[key])
        assert.equal(store.session(key), session, "the same session")
        assert.equal(session.el.parentNode, null, "taken out of the window")
    } finally {
        store.end(key)
        body.remove()
    }
})
