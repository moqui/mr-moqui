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
