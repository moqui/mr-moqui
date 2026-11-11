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

// The browser terminal's store: palette changes preserve a session and keep its text readable, and a session's
// terminal moves between views with its screen.
spec({server: false})

browser.runInPage({
    scripts: ["/js/MoquiLib.js", "/libs/xterm/xterm.js", "/libs/xterm/addon-fit.js", "/js/WebrootTerminal.js"],
    styles: ["/libs/xterm/xterm.css"],
    test: "/files/runtime/base-component/webroot/test/client/pages/WebrootTerminal.page.js"
})
