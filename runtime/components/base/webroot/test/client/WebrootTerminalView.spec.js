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

// The terminal's view in the page with Vue and Quasar (plans/Terminal-Fancy2.md §4.1): a long press opens its menu once
// with the line selected and nothing copied, and a floating terminal's window holds the whole view.
spec({server: false, timeout: 60})

browser.runInPage({
    styles: ["/libs/quasar/quasar.prod.css", "/css/WebrootVue.qvt.css", "/libs/xterm/xterm.css"],
    scripts: [
        "/libs/vue/vue.global.prod.js",
        "/libs/quasar/quasar.umd.prod.js",
        "/js/MoquiLib.js",
        "/libs/xterm/xterm.js",
        "/libs/xterm/addon-fit.js",
        "/libs/xterm/addon-search.js",
        "/libs/xterm/addon-web-links.js",
        "/libs/xterm/addon-unicode11.js",
        "/js/WebrootFloats.js",
        "/js/WebrootTerminal.js"
    ],
    test: "/files/component/webroot/test/client/pages/WebrootTerminalView.page.js"
})
