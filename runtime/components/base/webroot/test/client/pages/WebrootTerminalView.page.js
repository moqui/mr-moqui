/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The terminal's view (js/WebrootTerminal.js m-terminal-view and m-terminal-floats) in a real Vue application with Quasar
// and the floats' layer. Sessions have no socket: what they send is dropped.
"use strict"
const store = moqui.terminal
const floats = moqui.floats

const write = (term, text) => new Promise(resolve => term.write(text, resolve))
const settle = async () => {
    for (let i = 0; i < 4; i++) {
        await Vue.nextTick()
        await new Promise(resolve => setTimeout(resolve, 0))
    }
}
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))
const until = async check => {
    for (let i = 0; i < 100 && !check(); i++) await sleep(20)
    assert.ok(check(), "waited for " + check)
}
const menus = () => [...document.querySelectorAll(".q-menu")]

// the clipboard, kept: a write is the copy the user asked for
const copied = []
Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
        writeText: text => {
            copied.push(text)
            return Promise.resolve()
        }
    }
})

floats.viewport = () => ({w: 1200, h: 850, top: 50})
try {
    localStorage.removeItem("moqui.floats")
} catch (err) {
    /* no storage */
}
floats.load(true)
floats.measure()

store.create("view-1", {kind: "host", connect: false, send: () => {}})
const host = document.createElement("div")
host.style.cssText = "width:800px;height:320px"
document.body.appendChild(host)
const app = Vue.createApp({
    data() {
        return {shown: "view-1"}
    },
    template:
        '<div style="width:800px;height:320px"><m-terminal-view v-if="shown" :session-key="shown"></m-terminal-view></div>' +
        "<m-float-layer></m-float-layer><m-terminal-floats></m-terminal-floats>"
})
app.use(Quasar)
Object.keys(floats.components).forEach(name => app.component(name, floats.components[name]))
Object.keys(moqui.qvtComponents).forEach(name => app.component(name, moqui.qvtComponents[name]))
const vm = app.mount(host)

const touch = (el, type, x, y) => {
    const point = new Touch({identifier: 1, target: el, clientX: x, clientY: y})
    const touches = type === "touchend" ? [] : [point]
    el.dispatchEvent(
        new TouchEvent(type, {touches, targetTouches: touches, changedTouches: [point], bubbles: true, cancelable: true})
    )
}

test("a right click opens the menu, and a click on the screen does not", async () => {
    await settle()
    const session = store.session("view-1")
    session.el.click()
    await settle()
    assert.equal(menus().length, 0)
    const box = session.el.getBoundingClientRect()
    session.el.dispatchEvent(
        new MouseEvent("contextmenu", {clientX: box.left + 30, clientY: box.top + 30, bubbles: true, cancelable: true})
    )
    await settle()
    assert.equal(menus().length, 1)
    for (const type of ["keydown", "keyup"])
        document.dispatchEvent(new KeyboardEvent(type, {key: "Escape", keyCode: 27, bubbles: true}))
    await until(() => menus().length === 0)
})

test("a long press opens the menu once at the finger, with the line selected and nothing copied; Copy copies it", async () => {
    const session = store.session("view-1")
    await write(session.term, "first line\r\nthe second line\r\nthird\r\n")
    await settle()
    const box = session.el.getBoundingClientRect()
    const rowHeight = box.height / session.term.rows
    const x = box.left + 20,
        y = box.top + rowHeight * 1.5
    touch(session.el, "touchstart", x, y)
    await sleep(600)
    // the browser's own contextmenu of the hold (Android's) opens nothing more
    session.el.dispatchEvent(new MouseEvent("contextmenu", {clientX: x, clientY: y, bubbles: true, cancelable: true}))
    touch(session.el, "touchend", x, y)
    await settle()
    assert.equal(menus().length, 1, "one menu")
    assert.equal(session.term.getSelection().trim(), "the second line")
    assert.deepEqual(copied, [], "nothing copied by the press")
    const items = [...menus()[0].querySelectorAll(".q-item")].map(item => item.textContent.trim())
    assert.ok(items[0].startsWith("Copy"), items.join(", "))
    menus()[0].querySelector(".q-item").click()
    await until(() => copied.length > 0 && menus().length === 0)
    assert.deepEqual(
        copied.map(text => text.trim()),
        ["the second line"]
    )
    vm.shown = ""
    await settle()
})

test("a floating terminal's window holds the whole view, its menu and banners, out of Assist's reach; it docks without ending the session", async () => {
    const key = "float-1"
    store.create(key, {kind: "host", connect: false, send: () => {}})
    const id = store.float(key)
    assert.ok(id, "the window opened")
    await settle()
    await sleep(50)
    const body = document.getElementById(floats.bodyId(id))
    const view = body && body.querySelector("[data-float-terminal]")
    assert.ok(view, "the view is in the window's body")
    assert.ok(view.classList.contains("m-terminal-view"))
    const session = store.session(key)
    assert.equal(session.el.closest("[data-float-terminal]"), view, "the terminal's element is in the view")
    assert.equal(session.el.getAttribute("data-assist-self"), "terminal", "never in Assist's reach")
    assert.ok(!floats.reach().includes(floats.bodyId(id)), "the kind says it is not in reach")
    // the view's own banners: a closed session says so in the window
    store.state.views[key] = Object.assign(store.state.views[key] || {}, {closed: true, reason: "opened elsewhere", taken: true})
    await settle()
    assert.ok(body.querySelector("#TerminalClosed"), "the closed banner is in the window")
    floats.close(id)
    await settle()
    assert.ok(!store.state.floating[key])
    assert.equal(store.session(key), session, "the same session")
    assert.equal(session.el.isConnected, false, "taken out with the window")
    store.end(key)
})

test("a watching window keeps the session's size, sends nothing it is typed, says whose it watches, and takes the typing back", async () => {
    const sockets = []
    const openSocket = moqui.openSocket
    moqui.openSocket = url => {
        const socket = {
            url,
            readyState: 1,
            sent: [],
            send(message) {
                this.sent.push(message)
            },
            close() {
                this.readyState = 3
            }
        }
        sockets.push(socket)
        return socket
    }
    const key = "watch-1"
    try {
        store.create(key, {kind: "host", connect: false})
        store.state.sessions[key] = {
            id: key,
            kind: "host",
            title: "shared",
            shared: true,
            write: true,
            owner: "ann.lee",
            viewers: []
        }
        vm.shown = key
        await settle()
        store.connect(key, {observe: true})
        const socket = sockets[0]
        assert.ok(socket.url.includes("&observe=1"), socket.url)
        socket.onopen()
        socket.onmessage({
            data: JSON.stringify({
                type: "attached",
                kind: "host",
                from: 0,
                reset: true,
                viewer: true,
                owner: "ann.lee",
                cols: 50,
                rows: 12
            })
        })
        await settle()
        const session = store.session(key)
        assert.deepEqual([session.term.cols, session.term.rows], [50, 12], "the session's size, not the box's")
        const banner = document.querySelector("#TerminalWatching")
        assert.ok(banner && banner.textContent.includes("Watching ann.lee's session"), banner && banner.textContent)
        const before = socket.sent.length
        session.out("ls\r")
        assert.equal(socket.sent.length, before, "nothing typed is sent")
        // Take it back: a new socket that types
        banner.querySelector(".q-btn").click()
        await settle()
        assert.ok(!sockets[1].url.includes("observe"), sockets[1].url)
        sockets[1].onopen()
        sockets[1].onmessage({
            data: JSON.stringify({type: "attached", kind: "host", from: 0, reset: true, viewer: false, owner: "ann.lee"})
        })
        // another window takes it: this one watches on, told so
        sockets[1].onmessage({data: JSON.stringify({type: "role", viewer: true})})
        await settle()
        assert.ok(store.state.views[key].takenAway && session.viewer)
    } finally {
        moqui.openSocket = openSocket
        vm.shown = ""
        store.end(key)
        await settle()
    }
})

test("a recording plays at its times, seeks by feeding again from the start, and offers its download", async () => {
    const cast = [
        '{"version":2,"width":40,"height":6}',
        '[0.0,"o","first line\\r\\n"]',
        '[0.2,"r","50x8"]',
        '[0.4,"o","second line\\r\\n"]'
    ].join("\n")
    const fetching = window.fetch
    window.fetch = url =>
        String(url).includes("/apps/Terminal/recording?recordingId=rec-1")
            ? Promise.resolve(new Response(cast, {status: 200}))
            : fetching(url)
    const box = document.createElement("div")
    document.body.appendChild(box)
    const player = Vue.createApp({template: '<m-terminal-player recording-id="rec-1"></m-terminal-player>'})
    player.use(Quasar)
    Object.keys(moqui.qvtComponents).forEach(name => player.component(name, moqui.qvtComponents[name]))
    const vm = player.mount(box).$.subTree.component.proxy
    try {
        await until(() => vm.loaded)
        const text = () =>
            vm.term.buffer.active.getLine(0).translateToString(true) +
            "|" +
            vm.term.buffer.active.getLine(1).translateToString(true)
        await new Promise(resolve => vm.term.write("", resolve))
        assert.equal(text(), "first line|", "the start, before the first second")
        assert.equal(vm.duration, 0.4)
        vm.seek(0.5)
        await new Promise(resolve => vm.term.write("", resolve))
        assert.equal(text(), "first line|second line")
        assert.deepEqual([vm.term.cols, vm.term.rows], [50, 8], "its resize")
        vm.seek(0)
        vm.play()
        await until(() => !vm.playing)
        await new Promise(resolve => vm.term.write("", resolve))
        assert.equal(text(), "first line|second line", "played to its end")
        assert.ok(box.querySelector('a[href*="download=true"]'), "Download")
    } finally {
        window.fetch = fetching
        player.unmount()
        box.remove()
    }
})
