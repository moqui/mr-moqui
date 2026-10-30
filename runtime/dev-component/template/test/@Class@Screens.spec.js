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
// The screens of @Title@ (screen/@Class@.xml) render in the browser (qvt) and the terminal (tui) without errors.
// Run: ./moq test @component@

const screens = [["FindItem", {}], ["EditItem", { itemId: "@ID@_DEMO_1" }]]

spec({
    setupSpec() { login("john.doe") },
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("#name renders in #mode", {
    where: screens.flatMap(([name, parameters]) => ["qvt", "tui"].map((mode) => ({ name, parameters, mode }))),
    expect({ name, parameters, mode }) {
        const str = ec.screen.makeTest().rootScreen("component://@component@/screen/@Class@.xml").renderMode(mode)
            .render(name, parameters, null)
        str.status == 200
        str.errorMessages.length == 0
        !/Error rendering|Exception in render/.test(str.output)
    },
})

feature("the item detail shows the item", {
    expect() {
        const str = ec.screen.makeTest().rootScreen("component://@component@/screen/@Class@.xml").renderMode("tui")
            .render("EditItem", { itemId: "@ID@_DEMO_1" }, null)
        str.output.includes("First demo item")
    },
})
