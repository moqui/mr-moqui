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
// The Rust service of @Title@ (src/lib.rs, service/@package@/@Class@Services.xml), in a binary ./moq build linked the
// crate into. Run: ./moq test @component@

spec({
    setupSpec() { login("john.doe") },
})

feature("echo#Message answers the message and the user", {
    expect() {
        const out = ec.service.sync().name("@package@.@Class@Services.echo#Message").parameters({ message: "hello" }).call()
        out.message == "hello"
        out.username == "john.doe"
    },
})
