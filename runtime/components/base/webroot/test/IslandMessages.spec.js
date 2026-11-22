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

// A public island's messages (script/IslandMessages.js): the literal t() texts of its .qvue, each in the session's
// locale, for its config, since a public page loads no locale catalog. A made-up locale keeps the spec off the loaded
// language packs. WebrootPublic.qvt.js adds config.messages to moqui.l10n before the island mounts.
// Run: ./moq test webroot --spec IslandMessages
const lib = ec.resource.script("component://webroot/script/IslandMessages.js")
const CHAT_JOIN = "component://basic-comms/screen/ChatJoin.qvue"
const SHARED = "{name} shared this link"

feature("an island's literal t() texts are found, escapes read and a value left out", {
    expect() {
        lib.literalsOf(
            "{{ t('Save') }} :label=\"t('Don\\'t go')\" this.t(\"Say \\\"hi\\\"\") t(name) t('A, then', x) t('') formatted('No')"
        ) == ["Save", "Don't go", 'Say "hi"', "A, then"]
    }
})

feature("an island's messages are its texts in the session's locale, none in English", {
    when() {
        const row = {original: SHARED, locale: "xx", localized: "xx shared"}
        ec.artifactExecution.disableAuthz()
        ec.entity.makeValue("moqui.basic.LocalizedMessage").setAll(row).createOrUpdate()
        const before = ec.user.locale
        try {
            ec.user.setLocale("xx")
            const xx = lib.islandMessages([CHAT_JOIN, "component://basic-comms/screen/NoSuchIsland.qvue"])
            ec.user.setLocale("en_US")
            const en = lib.islandMessages([CHAT_JOIN])
            return {xx, en, found: lib.literals([CHAT_JOIN])}
        } finally {
            ec.user.setLocale(before)
            ec.entity.find("moqui.basic.LocalizedMessage").condition({original: SHARED, locale: "xx"}).deleteAll()
            ec.artifactExecution.enableAuthz()
        }
    },
    then({xx, en, found}) {
        found.includes(SHARED)
        // only the texts with a message: the rest stay as they are in the browser
        xx[SHARED] == "xx shared"
        Object.values(xx).every(text => text === "xx shared")
        en == {}
    }
})
