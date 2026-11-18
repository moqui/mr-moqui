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

// The first administrator's setup on a system that has accounts (the test runtime): get#InitialAdminNeeded and the
// Login screen's anonymous initialAdminNeeded transition answer needed false and nothing else (no password rules, no
// session token), and the Login screen in tui mode renders without the setup form (the NeedsSetupSection's tui text is
// in its fail-widgets). The refusal of create#InitialAdminAccount itself is framework/test/client/SecurityAuthn.spec.js.
// user-facade/login@initial-admin-enabled (off by default) does not matter once accounts exist: needed and disabled are both false, and
// the Login screen has no hint to turn it on. The empty database (needed with it on, ADMIN_ADV by joinAdvanced) is
// crates/moqui/tests/cli/skeleton.rs, an acceptance test.
// Run: ./moq test webroot --spec LoginSetup
const WEBROOT = "component://webroot/screen/webroot.xml"

/** A render as nobody, as the Login screen is reached before a login. */
function anonymousRender(path, mode, method) {
    if (ec.user.userId) ec.user.logoutUser()
    try {
        let test = ec.screen.makeTest().rootScreen(WEBROOT)
        if (mode) test = test.renderMode(mode)
        const out = test.render(path, {}, method || null)
        const text = String(out.output || "")
        return {status: out.status, text, json: out.jsonObject, token: out.header("moquiSessionToken"),
            broken: /Error rendering|Exception in render/.test(text), errors: Array.from(out.errorMessages || []).join(" | ")}
    } finally {
        ec.message.clearErrors()
    }
}

feature("with accounts, initialAdminNeeded answers needed false and hands out no token or rules", {
    when() {
        const needed = () => ec.service.sync().name("org.moqui.impl.UserServices.get#InitialAdminNeeded").call()
        const out = needed()
        return {out, on: withConf("user-facade/login@initial-admin-enabled", "true", needed),
            off: withConf("user-facade/login@initial-admin-enabled", "false", needed), res: anonymousRender("Login/initialAdminNeeded", null, "get")}
    },
    then({out, on, off, res}) {
        out.needed === false
        out.disabled === false
        on.needed === false && on.disabled === false
        off.needed === false && off.disabled === false
        out.minLength == null
        res.status == 200 && res.errors == ""
        res.json != null
        JSON.stringify(Object.keys(res.json)) == '["needed"]'
        res.json.needed === false
        !res.text.includes("sessionToken")
        !res.token
    },
})

feature("the Login screen renders in tui mode without the setup form while accounts exist", {
    when() {
        return {res: anonymousRender("Login", "tui", null)}
    },
    then({res}) {
        res.status == 200 && res.errors == ""
        !res.broken
        res.json != null
        !res.text.includes("InitialAdminForm")
        !res.text.includes("Welcome to your new system")
        !res.text.includes("initial_admin_enabled")
    },
})

// SEC-RTE-17: the Test Login button (john.doe / moqui) only on a dev or test instance or where demo_test_login is true
// (the demo component's conf), and its password check kept a minute by a digest of the stored hash.
feature("the Test Login is offered only on a dev or test instance or a demo, and its password check is cached", {
    when() {
        const offered = () => ec.service.sync().name("org.moqui.impl.UserServices.get#TestLoginOffered").call().offered
        const account = ec.entity.find("moqui.security.UserAccount").condition("userId", "EX_JOHN_DOE").disableAuthz().one()
        const matches = account != null && ec.user.passwordsMatch("moqui", account.currentPassword,
            account.passwordHashType, account.passwordSalt, account.passwordBase64 == "Y")
        const cache = ec.cache.getCache("webroot.login.test")
        cache.clear()
        const out = {matches, hasAccount: account != null}
        withProperty("demo_test_login", null, () => {
            out.production = withConf("instance@purpose", "production", offered)
            out.productionCached = cache.size()
            out.test = withConf("instance@purpose", "test", offered)
            out.dev = withConf("instance@purpose", "dev", offered)
        })
        out.demo = withProperty("demo_test_login", "true", () => withConf("instance@purpose", "production", offered))
        out.cached = account != null && cache.containsKey(ec.resource.digestText(String(account.currentPassword)))
        return out
    },
    then({matches, hasAccount, production, productionCached, test, dev, demo, cached}) {
        production === false
        productionCached == 0
        test === matches
        dev === matches
        demo === matches
        hasAccount
        cached
    },
})
