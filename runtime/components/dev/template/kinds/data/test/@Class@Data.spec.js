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
// The data of @Title@ (data/@Class@SeedData.xml): the seed records are there, in order.
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
