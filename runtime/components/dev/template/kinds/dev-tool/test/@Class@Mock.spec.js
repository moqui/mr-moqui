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
// The services of @Title@ (service/@package@/@Class@MockServices.xml): the mock's fixed answer and the test data load;
// and its screen, rendered once.
// Run: ./moq test @component@

spec({
    setupSpec() { login("john.doe") },
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("get#Rate answers a fixed rate with no network", {
    expect() {
        const out = ec.service.sync().name("@package@.@Class@MockServices.get#Rate").parameters({ fromCurrency: "USD", toCurrency: "EUR" }).call()
        out.rate == 1.25
        ec.service.sync().name("@package@.@Class@MockServices.get#Rate").parameters({ fromCurrency: "USD", toCurrency: "USD" }).call().rate == 1
    },
})

feature("load#TestData loads the test records", {
    expect() {
        const out = ec.service.sync().name("@package@.@Class@MockServices.load#TestData").call()
        out.recordsLoaded >= 3
        ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "@Class@Test").count() == 2
    },
})

feature("the tool's screen lists the test records and tries the mock", {
    expect() {
        const str = ec.screen.makeTest().rootScreen("component://@component@/screen/@Class@.xml").renderMode("tui")
            .render("", {}, null)
        str.status == 200
        str.errorMessages.length == 0
        str.output.includes("First test record")
        str.output.includes("Try")
    },
})
