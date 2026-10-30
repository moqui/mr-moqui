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
// The services of @Title@ (service/@package@/@Class@Services.xml). Run: ./moq test @component@

spec({
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("create#Item makes an item and update#Item changes only what it is given", {
    when() {
        const { itemId } = call("@package@.@Class@Services.create#Item", { itemName: "Spec item", description: "Made by a spec" })
        call("@package@.@Class@Services.update#Item", { itemId, itemName: "Spec item, renamed" })
        return { item: ec.entity.find("@package@.@Class@Item").condition("itemId", itemId).one() }
    },
    then({ item }) {
        item.itemName == "Spec item, renamed"
        item.description == "Made by a spec"
    },
})

feature("create#Item refuses an item with no name", {
    allowMessageErrors: true,
    expect() {
        const out = ec.service.sync().name("@package@.@Class@Services.create#Item").parameters({ description: "No name" }).call()
        !out?.itemId
        ec.message.hasError()
    },
})
