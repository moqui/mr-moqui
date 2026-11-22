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
// The messages of a public island, for its config: the literal texts its components localize in the browser
// (t('...') in a .qvue), each in the session's locale. A public page has no signed-in user to load the locale's whole
// catalog (apps/l10nCatalog), and an island's component is a shared cached asset, so the page's config carries the few
// texts the island shows; WebrootPublic.qvt.js adds them to moqui.l10n before the island mounts. A t() of a value that is
// not a literal is not found here and stays as it is.
//   const {islandMessages} = ec.resource.script("component://webroot/script/IslandMessages.js")
//   config.messages = islandMessages(["component://mr-pass/screen/Shop/Cart.qvue"])

// t("..."), t('...') and this.t(...): the quote, then the text with its escapes, then the closing quote
const literalCall = /\bt\(\s*(["'])((?:\\.|(?!\1)[^\\\n])*)\1\s*[,)]/g
const unescape = text => text.replace(/\\(.)/g, (whole, char) => (char === "n" ? "\n" : char === "t" ? "\t" : char))

// The literal texts of a source, in their order.
const literalsOf = source => Array.from(String(source || "").matchAll(literalCall), match => unescape(match[2])).filter(Boolean)
// The literal texts of the files, in their order, once each; a file that cannot be read is named in the log and left out.
function literals(locations) {
    const found = new Set()
    for (const location of locations) {
        try {
            literalsOf(ec.resource.getLocationText(location, true)).forEach(text => found.add(text))
        } catch (error) {
            ec.logger.error("Island messages: " + location + " was not read: " + error.message)
        }
    }
    return Array.from(found)
}

// The texts that have a message in the session's locale, original to localized; empty in English.
function islandMessages(locations) {
    const locale = String(ec.user.locale || "")
    if (!locale || /^en(_|-|$)/.test(locale)) return {}
    const messages = {}
    for (const text of literals(Array.from(locations || []))) {
        const localized = ec.l10n.localize(text)
        if (localized && localized !== text) messages[text] = localized
    }
    return messages
}

;({islandMessages, literals, literalsOf})
