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
// The REST API of @Title@ (service/@component@.rest.xml), as an ADMIN user calls it. Run: ./moq test @component@

spec({
    setupSpec() { login("john.doe") },
})

feature("POST creates an item that GET then returns", {
    when() {
        const created = screenTest("rest").render("s1/@component@/items", { itemName: "REST item" }, "post")
        const itemId = JSON.parse(created.output).itemId
        const found = screenTest("rest").render(`s1/@component@/items/${itemId}`, null, "get")
        return { created, found, item: JSON.parse(found.output) }
    },
    then({ created, found, item }) {
        created.status == 200
        found.status == 200
        item.itemName == "REST item"
    },
})

feature("GET lists the demo items", {
    expect() {
        const str = screenTest("rest").render("s1/@component@/items", null, "get")
        str.status == 200
        JSON.parse(str.output).some((item) => item.itemId == "@ID@_DEMO_1")
    },
})
