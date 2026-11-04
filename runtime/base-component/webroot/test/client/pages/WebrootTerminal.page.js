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

const componentReady = fetch("/files/runtime/base-component/webroot/screen/webroot/apps/Terminal.qvue")
    .then(response => response.text())
    .then(source => moqui.parseVue(source))
const write = (term, text) => new Promise(resolve => term.write(text, resolve))
const painted = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))

const terminal = async dark => {
    const component = await componentReady
    const screen = document.createElement("div")
    screen.style.cssText = "width:800px;height:320px"
    document.body.appendChild(screen)
    const replies = []
    const context = {dark, $refs: {screen}, connect() {}}
    for (const [name, method] of Object.entries(component.methods)) context[name] = method.bind(context)
    // Keep the test offline; xterm's real input reply still crosses the same send boundary as the socket.
    context.connect = () => {}
    context.send = bytes => replies.push(new TextDecoder().decode(bytes))
    context.open()
    return {
        component, context, replies, screen,
        dispose() {
            context.observer.disconnect()
            context.term.dispose()
            screen.remove()
        }
    }
}

test("switching background reports its new color without resetting terminal content", async () => {
    const view = await terminal(false)
    try {
        const {component, context, replies} = view
        const term = context.term
        await write(term, "Unsubmitted field value")
        component.watch.dark.call(context, true)
        await write(term, "")
        assert.ok(replies.some(reply => reply.includes("11;rgb:1616/1818/1d1d")), "dark background reaches the TUI decoder")
        assert.equal(term.buffer.active.getLine(0).translateToString(true), "Unsubmitted field value")
        component.watch.dark.call(context, false)
        await write(term, "")
        assert.ok(replies.some(reply => reply.includes("11;rgb:ffff/ffff/ffff")), "light background reaches the TUI decoder")
        assert.equal(context.term, term)
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
        await write(view.context.term, "\u001b[38;2;110;110;110mSection heading")
        await painted()
        const foreground = () => {
            const text = [...view.screen.querySelectorAll(".xterm-rows span")].find(span => span.textContent.includes("Section heading"))
            assert.ok(text, "the terminal rendered the heading")
            return getComputedStyle(text).color.match(/[\d.]+/g).slice(0, 3).map(Number)
        }
        assert.ok(contrast(foreground(), [22, 24, 29]) >= 4.5, "dark terminal heading contrast")
        view.component.watch.dark.call(view.context, false)
        await write(view.context.term, "\r\u001b[38;2;150;150;150mSection heading")
        await painted()
        assert.ok(contrast(foreground(), [255, 255, 255]) >= 4.5, "light terminal heading contrast")
    } finally {
        view.dispose()
    }
})
