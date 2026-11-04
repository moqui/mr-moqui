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
// A rule of @Title@'s screens (screen/@Class@.xml): the item detail shows the item it is given. A regression spec renders
// a screen once, for logic of its own, with all of its checks on that render; the every-screen sweep is an acceptance
// test (acceptance/@Class@ScreenSweep.spec.js).
// Run: ./moq test @component@

spec({
    setupSpec() { login("john.doe") },
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("the item detail shows the item", {
    expect() {
        const str = ec.screen.makeTest().rootScreen("component://@component@/screen/@Class@.xml").renderMode("tui")
            .render("EditItem", { itemId: "@ID@_DEMO_1" }, null)
        str.status == 200
        str.errorMessages.length == 0
        str.output.includes("First demo item")
    },
})
