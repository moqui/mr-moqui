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
// The data of @Title@ (data/10-seed/@Class@SeedData.xml, data/30-demo/@Class@DemoData.xml): the seed records are there,
// in order, and the demo record that refers to them.
// Run: ./moq test @component@

feature("the seed enumerations are loaded in order", {
    expect() {
        const kinds = ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "@Class@Kind")
            .orderBy("sequenceNum").disableAuthz().list()
        kinds.length >= 2
        kinds[0].enumId == "@ID@_KIND_STANDARD"
        kinds[1].enumId == "@ID@_KIND_SPECIAL"
    },
})

feature("the demo record of the seed's type loads after it", {
    expect() {
        // the test database has every data type; a demo record that loaded before its seed type would be missing
        ec.entity.find("moqui.basic.Enumeration").condition("enumId", "@ID@_KIND_DEMO").disableAuthz().count() == 1
    },
})
